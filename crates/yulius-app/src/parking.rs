//! Parked sessions (SPEC-512 part 1, #1412, ADR-0367): a task whose turn ended and that waits for
//! a person gives its idle session up after a grace period and resumes in that session when
//! someone writes to it.
//!
//! Contract:
//! - only a `running` task whose agent is idle (`waiting` or `done`), which the list shows under
//!   Needs you or Done and in which nothing goes on by itself or waits on a keyboard, is parked
//!   ([`ParkFacts::parkable`]); the grace period runs from the last change of its status or
//!   activity (`state_at`); 0 turns parking off;
//! - the decision is taken again under the launch lock (`App::park_task_now`), so a start, a
//!   turn or a message that came meanwhile wins; the run's end makes the task `parked`;
//! - a parked task holds no session: `App::sessions_in_use` counts live runs only;
//! - every park is audited `task.parked` by `yulius`, without the task's words.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::json;
use yulius_domain::ids::SandboxId;
use yulius_domain::task::{Activity, TaskGroup, TaskStatus};

use crate::context::App;
use crate::model::{AttemptRecord, TaskOrigin, TaskRecord};

/// The grace period an idle session keeps before it is parked unless the unit says otherwise
/// (`YULIUS_PARK_AFTER_MINUTES`): 30 minutes.
pub const DEFAULT_PARK_AFTER_MS: u64 = 30 * 60 * 1000;

/// A sandbox's pending requests for a person: all of them (`needs_you` too), which decide the
/// list's group, and those in its browser, which keep a session (the agent goes on after them).
#[derive(Debug, Clone, Default)]
struct Requests {
    all: HashSet<String>,
    browser: HashSet<String>,
}

/// Who parks a session, in the audit.
pub const PARKED_BY: &str = "yulius";

/// Whose rule a park follows: part 1's grace period, or the digest's threshold (SPEC-512
/// DIG-003), which parks only a task whose agent waits for its turn and that asks nobody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkRule {
    /// The unit's grace period (PRK-002).
    Grace,
    /// The digest's idle threshold, in milliseconds.
    Digest(u64),
}

/// What decides whether a running task's session is parked now (SPEC-512 PRK-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParkFacts {
    /// The grace period; 0: parking is off.
    pub park_after_ms: u64,
    /// Since when its status and activity are what they are.
    pub quiet_ms: i64,
    /// Its status.
    pub status: TaskStatus,
    /// Its agent's activity.
    pub activity: Option<Activity>,
    /// The list's group.
    pub group: TaskGroup,
    /// Its plan, spec or goal still needs the session.
    pub unfinished: bool,
    /// A turn of its session is known to run.
    pub turn: bool,
    /// Background shells or subagents it left, or dynamic workflows not ended: it wakes by
    /// itself.
    pub background: bool,
    /// It waits in `yulius wait`, has a merge armed, or Yulius has news of its pull request due.
    pub yulius: bool,
    /// A prompt typed into it is held for a person's decision (TSK-013).
    pub held_prompt: bool,
    /// It asked a person to act in the sandbox browser.
    pub browser_request: bool,
    /// A person or an update paused it.
    pub paused: bool,
    /// A person typed into its terminal and sent nothing, or typed within the grace period.
    pub keyboard: bool,
    /// The onboarding task of its sandbox, whose run's end decides the onboarding.
    pub onboarding: bool,
}

impl ParkFacts {
    /// Whether the session is parked now.
    #[must_use]
    pub fn parkable(&self) -> bool {
        self.park_after_ms > 0
            && i64::try_from(self.park_after_ms).is_ok_and(|after| self.quiet_ms >= after)
            && self.status == TaskStatus::Running
            && self.activity.is_some_and(Activity::is_idle)
            && matches!(self.group, TaskGroup::NeedsYou | TaskGroup::Done)
            && !self.unfinished
            && !self.turn
            && !self.background
            && !self.yulius
            && !self.held_prompt
            && !self.browser_request
            && !self.paused
            && !self.keyboard
            && !self.onboarding
    }

    /// Whether the digest parks the session (DIG-003): as [`ParkFacts::parkable`] with its own
    /// threshold, and only while the agent waits for its turn (not Done) and asks nobody.
    #[must_use]
    pub fn digest_parkable(&self, asks: bool) -> bool {
        self.parkable() && self.activity == Some(Activity::Waiting) && !asks
    }
}

impl App {
    /// Parks every running task whose session [`ParkFacts::parkable`] says may go (SPEC-512
    /// PRK-002); the background tick calls it every minute. Returns how many parks began.
    pub async fn park_idle_sessions_now(self: &Arc<Self>) -> usize {
        if self.config.park_after_ms == 0 {
            return 0;
        }
        let Ok(running) = self
            .deps
            .store
            .tasks_with_status(&[TaskStatus::Running])
            .await
        else {
            return 0;
        };
        let mut requests: HashMap<SandboxId, Requests> = HashMap::new();
        let mut parked = 0;
        for task in running {
            // Cheap facts first: most running tasks work.
            if !task.activity.is_some_and(Activity::is_idle) {
                continue;
            }
            if !requests.contains_key(&task.sandbox_id) {
                let read = self.park_requests(&task.sandbox_id).await;
                requests.insert(task.sandbox_id.clone(), read);
            }
            let requested = requests.get(&task.sandbox_id).cloned().unwrap_or_default();
            if !self
                .park_facts(&task, &requested, self.config.park_after_ms)
                .await
                .parkable()
            {
                continue;
            }
            match self.park_task_now(&task.id, ParkRule::Grace).await {
                Ok(true) => parked += 1,
                Ok(false) => {}
                Err(err) => tracing::warn!(task = %task.id, error = %err, "a park failed"),
            }
        }
        parked
    }

    /// [`ParkFacts::parkable`] for `task` as stored now: what [`App::park_task_now`] asks again
    /// under the launch lock.
    pub(crate) async fn parkable_now(&self, task: &TaskRecord, rule: ParkRule) -> Option<i64> {
        let requested = self.park_requests(&task.sandbox_id).await;
        let threshold = match rule {
            ParkRule::Grace => self.config.park_after_ms,
            ParkRule::Digest(ms) => ms,
        };
        let facts = self.park_facts(task, &requested, threshold).await;
        let park = match rule {
            ParkRule::Grace => facts.parkable(),
            ParkRule::Digest(_) => facts.digest_parkable(
                self.deps
                    .store
                    .task_needs_you(&task.id)
                    .await
                    .map_or(true, |asked| asked.is_some()),
            ),
        };
        park.then_some(facts.quiet_ms)
    }

    async fn park_requests(&self, sandbox: &SandboxId) -> Requests {
        Requests {
            all: self.browser_requests(sandbox).await,
            browser: self.browser_tab_requests(sandbox).await,
        }
    }

    async fn park_facts(
        &self,
        task: &TaskRecord,
        requested: &Requests,
        park_after_ms: u64,
    ) -> ParkFacts {
        let now = self.now().0;
        let quiet_ms = match self.deps.store.task_times(&task.id).await {
            Ok(Some(times)) => now.saturating_sub(times.state_at.0),
            _ => 0,
        };
        let runtime = self.existing_runtime(&task.id);
        let keyboard = runtime.as_ref().is_some_and(|runtime| {
            let typed = runtime.typed_at_ms.load(Ordering::SeqCst);
            let emptied = runtime.empty_time_for_typing();
            yulius_domain::intake::box_held(now, typed, emptied).is_some()
                || (typed > 0
                    && i64::try_from(park_after_ms)
                        .is_ok_and(|after| now.saturating_sub(typed) < after))
        });
        let background = match &runtime {
            // A session Yulius does not hold is nothing it can end.
            None => true,
            // Shells not reported (a session resumed with nothing typed has run no turn) count
            // as none, as the list counts them.
            Some(runtime) => {
                self.background_shells_of(&task.id)
                    .is_some_and(|count| count > 0)
                    || !runtime.with_workflows(|workflows| workflows.runs().is_empty())
            }
        };
        let mut facts = self.task_attention_facts(task, &requested.all).await.ok();
        if let Some(facts) = &mut facts {
            // The supervisor verifies the current head before the launch lock.
            // Local eligibility must not depend on a previous overview.
            facts.pull = None;
        }
        let group = facts
            .as_ref()
            .map_or(TaskGroup::Working, |facts| facts.group());
        ParkFacts {
            park_after_ms,
            quiet_ms,
            status: task.status,
            activity: task.activity,
            group,
            unfinished: facts.is_none_or(|facts| {
                facts.plan_open || facts.spec_unfinished || facts.goal_unfinished
            }),
            turn: runtime
                .as_ref()
                .is_some_and(|runtime| runtime.turn_in_progress()),
            background,
            yulius: self.agent_waits_on_yulius(&task.id)
                || self.merge_is_armed(&task.id)
                || self.pr_news_due(task).await,
            held_prompt: self
                .deps
                .store
                .pending_hold(&task.id)
                .await
                .map_or(true, |hold| hold.is_some()),
            browser_request: requested.browser.contains(task.id.as_str()),
            paused: self.is_paused(task).await,
            keyboard,
            onboarding: task.origin == TaskOrigin::Onboarding,
        }
    }
}

const PARKED_SCREEN: &str = "parked-screen";
const PARKING_SCREEN: &str = "parked-screen.pending";

/// Append one parking notice after the last occupied row of the captured screen.
fn with_park_notice(cols: u16, rows: u16, bytes: &[u8], quiet_ms: i64) -> (u16, u16, Vec<u8>) {
    let mut parser = vt100::Parser::new(rows.max(1), cols.max(1), 0);
    parser.process(bytes);
    let last = parser
        .screen()
        .rows(0, cols)
        .enumerate()
        .filter(|(_, line)| !line.trim_end().is_empty())
        .map(|(row, _)| row)
        .last();
    let elapsed = yulius_domain::ci::format_duration(quiet_ms);
    let notice = format!(
        "Yulius parked this session after {elapsed} idle since its turn ended. Send a message or choose Resume to continue."
    );
    let next = last.map_or(1, |row| row + 2);
    // A full TUI needs another row. Keep the first row as well as the last
    // when the notice wraps on a narrow terminal.
    let needed = next - 1 + notice.len().div_ceil(usize::from(cols.max(1)));
    let height = rows.max(u16::try_from(needed).unwrap_or(u16::MAX));
    parser.screen_mut().set_size(height, cols.max(1));
    parser.process(format!("\x1b[{next};1H\x1b[0m{notice}").as_bytes());
    (cols, height, parser.screen().contents_formatted())
}

pub(crate) fn save_parked_screen(
    hub: &crate::hub::Hub,
    dir: &Path,
    quiet_ms: i64,
) -> std::io::Result<()> {
    let (bytes, (cols, rows)) = hub.formatted_screen();
    let (cols, rows, screen) = with_park_notice(cols, rows, &bytes, quiet_ms);
    let mut record = format!("{cols} {rows}\n").into_bytes();
    record.extend(screen);
    crate::supervisor::write_private_file(&dir.join(PARKING_SCREEN), &record)
}

/// Commit the pre-stop capture only once the task has actually transitioned to Parked.
pub(crate) fn publish_parked_screen(dir: &Path) -> Option<(u16, u16, Vec<u8>)> {
    let candidate = dir.join(PARKING_SCREEN);
    let committed = dir.join(PARKED_SCREEN);
    if let Err(err) = std::fs::rename(&candidate, &committed) {
        tracing::warn!(error = %err, "could not publish parked screen");
        return crate::supervisor::read_saved_screen(&candidate);
    }
    crate::supervisor::read_saved_screen(&committed)
}

pub(crate) struct EndedScreen {
    pub(crate) grid: (u16, u16, Vec<u8>),
    pub(crate) parked: bool,
}

/// A resume whose account vanished before any process received its prompt.
pub(crate) fn removed_before_spawn(run: &AttemptRecord) -> bool {
    run.launch_kind == crate::model::LaunchKind::Resume
        && run.exit_reason.as_deref() == Some("account_removed")
}

impl App {
    /// A recorded resume may still be spawning. Until its first host snapshot, only
    /// a committed park from the preceding attempt may be displayed in its place.
    pub(crate) async fn visible_ended_screen(
        &self,
        task: &TaskRecord,
        attempts: &[AttemptRecord],
    ) -> Option<EndedScreen> {
        let (latest, prior) = attempts.split_last()?;
        let retrying = matches!(
            task.status,
            TaskStatus::Starting
                | TaskStatus::WaitingCapacity
                | TaskStatus::Queued
                | TaskStatus::Parked
        ) && removed_before_spawn(latest);
        if latest.state.is_terminal() && !retrying {
            return self.ended_screen(latest, task).await;
        }
        if latest.launch_kind != crate::model::LaunchKind::Resume {
            return None;
        }
        let previous = prior.iter().rev().find(|run| !removed_before_spawn(run))?;
        self.ended_screen(previous, task)
            .await
            .filter(|screen| screen.parked)
    }

    /// Restore the capture for a committed park, or the ordinary final screen.
    pub(crate) async fn ended_screen(
        &self,
        run: &AttemptRecord,
        task: &TaskRecord,
    ) -> Option<EndedScreen> {
        let dir = Path::new(&run.run_dir);
        let ordinary = || {
            crate::supervisor::read_final_screen(dir).map(|grid| EndedScreen {
                grid,
                parked: false,
            })
        };
        // Read the pending name first: a concurrent rename then leaves either these
        // bytes or the committed name visible to this reader.
        let pending = crate::supervisor::read_saved_screen(&dir.join(PARKING_SCREEN));
        crate::race::reached(crate::race::Point::ParkScreenRead, dir);
        if let Some(saved) = crate::supervisor::read_saved_screen(&dir.join(PARKED_SCREEN)) {
            return Some(EndedScreen {
                grid: saved,
                parked: true,
            });
        }
        let Some(pending) = pending else {
            return ordinary();
        };
        // A restart between the state commit and the rename. A stop that won the
        // transition keeps its ordinary screen, including after a queued resume.
        if task.status == TaskStatus::Parked
            || self
                .deps
                .store
                .subject_spans(
                    &task.sandbox_id,
                    yulius_domain::stages::SubjectKind::Task,
                    task.id.as_str(),
                    32,
                )
                .await
                .is_ok_and(|spans| {
                    spans.iter().any(|span| {
                        span.kind == "state"
                            && span.ended_at == run.ended_at
                            && span.outcome.as_deref() == Some("parked")
                    })
                })
        {
            return Some(EndedScreen {
                grid: pending,
                parked: true,
            });
        }
        ordinary()
    }
}

/// The file in a parked run's directory that says its goal was left unfinished (SPEC-429): the
/// session's runtime that knew it does not outlive the server, and a parked task has no run
/// whose transcript would tell it again.
pub(crate) const GOAL_UNFINISHED_MARK: &str = "parked-goal-unfinished";

impl App {
    /// Whether Claude Code last recorded the task's goal as unfinished (SPEC-429 ANS-002): for a
    /// parked task what its parked run left ([`GOAL_UNFINISHED_MARK`]), else its live run's.
    pub(crate) async fn goal_unfinished_of(&self, task: &TaskRecord) -> bool {
        if task.status != TaskStatus::Parked {
            return self.goal_unfinished(&task.id);
        }
        match self.deps.store.attempts(&task.id).await {
            Ok(runs) => runs
                .iter()
                .rev()
                .find(|run| !removed_before_spawn(run))
                .is_some_and(|run| {
                    let dir = std::path::Path::new(&run.run_dir);
                    match std::fs::symlink_metadata(dir.join(GOAL_UNFINISHED_MARK)) {
                        Ok(_) => true,
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                            !std::fs::metadata(dir).is_ok_and(|meta| meta.is_dir())
                        }
                        Err(_) => true,
                    }
                }),
            Err(_) => true,
        }
    }
}

/// Audits the park of `task` (`task.parked`, by [`PARKED_BY`]); a failed write is logged.
pub(crate) async fn audit_parked(app: &App, task: &TaskRecord) {
    let detail = json!({"number": task.number, "activity": task.activity.map(Activity::as_str)});
    if let Err(err) = app
        .deps
        .store
        .audit(
            app.now(),
            PARKED_BY,
            "task.parked",
            Some(task.id.as_str()),
            Some(&detail.to_string()),
        )
        .await
    {
        tracing::warn!(task = %task.id, error = %err, "a park was not audited");
    }
}

/// What a queued task waits for (SPEC-512 PRK-007), when it is not a supertask's hold (the
/// task says that itself, `held_by`): an update, or a free session while `in_use` of `limit` are
/// taken; `None` when it starts at the queue's next pass.
#[must_use]
pub fn queue_reason(update: bool, held: bool, in_use: u32, limit: u32) -> Option<String> {
    if held {
        None
    } else if update {
        Some("Waiting for the Yulius update to finish".into())
    } else if in_use >= limit {
        Some(format!(
            "Waiting for a free session: {in_use} of {limit} in use"
        ))
    } else {
        Some("Starting: a session is free".into())
    }
}

impl App {
    /// [`queue_reason`] for `task` now: `None` unless it is queued.
    pub(crate) async fn queue_reason_of(
        &self,
        task: &TaskRecord,
        held_by: &[u32],
    ) -> Option<String> {
        if task.status != TaskStatus::Queued {
            return None;
        }
        crate::race::reached(
            crate::race::Point::QueuedRowCapacity,
            task.id.as_str().as_ref(),
        );
        let in_use = self.sessions_in_use().await.unwrap_or(u32::MAX);
        queue_reason(
            self.update_holds_starts(),
            !held_by.is_empty(),
            in_use,
            self.max_live_sessions(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PRK-007: a queued task says what it waits for; a supertask's hold says it itself.
    #[test]
    fn a_queued_task_says_what_it_waits_for() {
        assert_eq!(
            queue_reason(false, false, 8, 8).as_deref(),
            Some("Waiting for a free session: 8 of 8 in use")
        );
        assert_eq!(
            queue_reason(false, false, 9, 8).as_deref(),
            Some("Waiting for a free session: 9 of 8 in use")
        );
        assert_eq!(
            queue_reason(true, false, 1, 8).as_deref(),
            Some("Waiting for the Yulius update to finish")
        );
        assert_eq!(queue_reason(true, true, 8, 8), None);
        assert_eq!(
            queue_reason(false, false, 2, 8).as_deref(),
            Some("Starting: a session is free")
        );
    }

    #[test]
    fn parking_preserves_every_occupied_row_and_adds_one_timed_notice() {
        let (cols, rows, bytes) = with_park_notice(72, 2, b"first row\r\nlast row", 1_800_000);
        let hub = crate::hub::Hub::new(cols, rows);
        hub.reset(1, &bytes, cols, rows);
        let shown = hub.screen_text();
        assert!(shown.starts_with("first row\nlast row\n"), "{shown}");
        assert_eq!(shown.matches("Yulius parked").count(), 1);
        assert!(shown.contains("30 min 0 s idle since its turn ended"));
        assert!(
            shown
                .trim_end()
                .ends_with("Send a message or choose Resume to continue.")
        );
    }

    fn idle() -> ParkFacts {
        ParkFacts {
            park_after_ms: 60_000,
            quiet_ms: 60_000,
            status: TaskStatus::Running,
            activity: Some(Activity::Waiting),
            group: TaskGroup::NeedsYou,
            unfinished: false,
            turn: false,
            background: false,
            yulius: false,
            held_prompt: false,
            browser_request: false,
            paused: false,
            keyboard: false,
            onboarding: false,
        }
    }

    /// PRK-002: an idle task that waits for a person, or is Done, parks once the grace period is
    /// over.
    #[test]
    fn an_idle_task_that_waits_for_a_person_parks_after_the_grace_period() {
        assert!(idle().parkable());
        assert!(
            ParkFacts {
                activity: Some(Activity::Done),
                group: TaskGroup::Done,
                ..idle()
            }
            .parkable()
        );
        assert!(
            !ParkFacts {
                quiet_ms: 59_999,
                ..idle()
            }
            .parkable()
        );
        assert!(
            !ParkFacts {
                park_after_ms: 0,
                quiet_ms: i64::MAX,
                ..idle()
            }
            .parkable(),
            "0 turns parking off"
        );
    }

    /// PRK-002: whatever goes on by itself, asks in its terminal or waits on a keyboard keeps
    /// the session.
    #[test]
    fn what_goes_on_or_waits_on_a_keyboard_keeps_the_session() {
        let kept = [
            ParkFacts {
                activity: Some(Activity::Working),
                ..idle()
            },
            ParkFacts {
                activity: Some(Activity::NeedsAttention),
                ..idle()
            },
            ParkFacts {
                activity: None,
                ..idle()
            },
            ParkFacts {
                group: TaskGroup::Working,
                ..idle()
            },
            ParkFacts {
                status: TaskStatus::WaitingCapacity,
                ..idle()
            },
            ParkFacts {
                status: TaskStatus::Switching,
                ..idle()
            },
            ParkFacts {
                turn: true,
                ..idle()
            },
            ParkFacts {
                background: true,
                ..idle()
            },
            ParkFacts {
                yulius: true,
                ..idle()
            },
            ParkFacts {
                held_prompt: true,
                ..idle()
            },
            ParkFacts {
                browser_request: true,
                ..idle()
            },
            ParkFacts {
                paused: true,
                ..idle()
            },
            ParkFacts {
                keyboard: true,
                ..idle()
            },
            ParkFacts {
                onboarding: true,
                ..idle()
            },
        ];
        for facts in kept {
            assert!(!facts.parkable(), "{facts:?}");
        }
    }

    /// DIG-003: the digest parks a quiet task whose agent waits for its turn and asks nobody;
    /// never a Done task, one that asks, one quiet for less than its threshold, nor one any
    /// guard of PRK-002 keeps.
    #[test]
    fn the_digest_parks_only_what_waits_for_nobody() {
        assert!(idle().digest_parkable(false));
        assert!(
            !idle().digest_parkable(true),
            "a task that asks is never parked"
        );
        let done = ParkFacts {
            activity: Some(Activity::Done),
            group: TaskGroup::Done,
            ..idle()
        };
        assert!(
            done.parkable() && !done.digest_parkable(false),
            "Done is never parked"
        );
        let young = ParkFacts {
            quiet_ms: 59_999,
            ..idle()
        };
        assert!(!young.digest_parkable(false));
        for kept in [
            ParkFacts {
                background: true,
                ..idle()
            },
            ParkFacts {
                keyboard: true,
                ..idle()
            },
            ParkFacts {
                onboarding: true,
                ..idle()
            },
        ] {
            assert!(!kept.digest_parkable(false), "{kept:?}");
        }
    }
}
