//! Moving idle tasks off a nearly used-up subscription before it runs out (SPEC-629, ADR-0461).
//!
//! Every [`REBALANCE_EVERY`] a pass ([`App::rebalance_now`]) looks at the running tasks whose
//! live run sits on a subscription past its harness's rebalance threshold and moves each idle one
//! to the best subscription below it (`rebalance::idle_move`), with nothing typed and a claim of
//! the idle state it read (`App::switch_from`, `SwitchCondition::Idle`), only when a restart
//! loses nothing (`agent_cli::kept_idle`) and opens no turn; a goal session is parked instead.
//! A task mid-turn, switching or paused is not looked at; it is looked at again in the next pass.
//!
//! A person's choice of a subscription pins the task to it (`App::pin_task`): the rebalance
//! never moves a task running on its pin, and a parked task resumes on it while it can take the
//! task (`App::pinned_start`).

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use yulius_domain::agent_cli::{IdleFacts, kept_idle};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::notice::{NoticeKey, NoticeTone};
use yulius_domain::notify::NoticeKind;
use yulius_domain::rebalance::{Fullest, IdleMove, idle_move};
use yulius_domain::rotation::{self, Decision, Trigger};
use yulius_domain::task::TaskStatus;

use crate::account_gate::account_kind;
use crate::context::App;
use crate::error::AppError;
use crate::model::{TaskMode, TaskRecord};
use crate::notices::NewNotice;
use crate::parking::ParkRule;
use crate::supervisor::{RunStart, SwitchCondition};

/// How often the rebalance pass runs (SPEC-629 RBL-003): as often as the background usage round.
pub const REBALANCE_EVERY: Duration = Duration::from_secs(2 * 60);
/// The first pass after a start: once the tasks that went idle while Yulius was down are woken
/// (SPEC-358 MSB-007), as for parking.
pub const REBALANCE_FIRST: Duration = Duration::from_secs(3 * 60);
const _: () = assert!(REBALANCE_FIRST.as_secs() > crate::outage::WAKE_GRACE.as_secs());
/// The key prefix of a task's rebalance notice (SPEC-624): `rebalance:<task id>`.
pub const NOTICE_KEY: &str = "rebalance:";
/// How long a pass waits for a task's transcript to be read before it looks at its queue.
const LINES_READ_WAIT: Duration = Duration::from_secs(5);

/// How long an idle session whose restart would start a turn waits before the pass parks it;
/// never longer than the instance's own grace period (`YULIUS_PARK_AFTER_MINUTES`).
const PARK_QUIET_MS: u64 = 60_000;

/// What one rebalance pass did (SPEC-629 RBL-003, RBL-007).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RebalancePass {
    /// Idle tasks moved to a subscription below the threshold.
    pub moved: u32,
    /// Idle sessions whose restart would start a turn, parked: they resume off the subscription.
    pub parked: u32,
    /// Idle tasks that stay because no other subscription is below the threshold.
    pub no_room: u32,
    /// Idle tasks kept where they are for now: a restart would lose something or start a turn,
    /// a turn began while the pass looked, or the move was refused.
    pub kept: u32,
    /// Tasks on a subscription past the threshold that their person pinned there.
    pub pinned: u32,
}

/// What the pass did with one task.
enum Looked {
    Nothing,
    Moved,
    Parked,
    NoRoom,
    Kept,
    Pinned,
}

/// The switch reason of a rebalance move: `rebalance: <subscription> <how full it is>`, as the
/// task's info lists it. It names no reason a switch acts on (`manual`, `handover`, …).
fn move_reason(label: &str, said: &str) -> String {
    format!("rebalance: {label} {said}")
}

/// Logs why the pass keeps an idle task where it is (SPEC-629 RBL-008).
fn kept(task: &TaskId, why: &str) -> Looked {
    tracing::debug!(task = %task, why, "rebalance: the idle task stays for now");
    Looked::Kept
}

impl App {
    /// Starts the rebalance pass: the first after [`REBALANCE_FIRST`], then every
    /// [`REBALANCE_EVERY`] (SPEC-629 RBL-003).
    pub fn start_rebalance_loop(self: &Arc<Self>) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(REBALANCE_FIRST).await;
            let mut every = tokio::time::interval(REBALANCE_EVERY);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                let pass = app.rebalance_now().await;
                if pass.moved + pass.parked > 0 {
                    tracing::info!(?pass, "rebalance pass");
                } else if pass != RebalancePass::default() {
                    tracing::debug!(?pass, "rebalance pass");
                }
            }
        });
    }

    /// One rebalance pass over the running tasks, one move after the other (SPEC-629 RBL-003).
    /// A subscription a move to failed takes no further move in the same pass. Public for
    /// tests, which run it instead of waiting for the clock.
    pub async fn rebalance_now(self: &Arc<Self>) -> RebalancePass {
        let mut pass = RebalancePass::default();
        // An instance update holds every run: nothing is restarted under it.
        if self.update_holds_starts() {
            return pass;
        }
        let Ok(running) = self
            .deps
            .store
            .tasks_with_status(&[TaskStatus::Running])
            .await
        else {
            return pass;
        };
        let mut failed = HashSet::new();
        for task in running {
            match self.rebalance_task(&task, &mut failed).await {
                Looked::Nothing => {}
                Looked::Moved => pass.moved += 1,
                Looked::Parked => pass.parked += 1,
                Looked::NoRoom => pass.no_room += 1,
                Looked::Kept => pass.kept += 1,
                Looked::Pinned => pass.pinned += 1,
            }
        }
        pass
    }

    async fn rebalance_task(
        self: &Arc<Self>,
        task: &TaskRecord,
        failed: &mut HashSet<AccountId>,
    ) -> Looked {
        // Cheap facts first: most running tasks work. Mid-turn (working, or a permission prompt
        // that needs attention) is never moved.
        if !task.activity.is_some_and(|a| a.is_idle()) {
            return Looked::Nothing;
        }
        let store = &self.deps.store;
        let Ok(Some(run)) = store.live_attempt(&task.id).await else {
            return Looked::Nothing;
        };
        let Some(current) = run.account_id.clone() else {
            return Looked::Nothing;
        };
        let Ok(input) = self.rotation_input(task, Some(current.clone())).await else {
            return Looked::Nothing;
        };
        // At the pool's own threshold the rebalance is off: the turn boundary and the limit
        // move tasks, as before (SPEC-629 RBL-001).
        if input.rebalance >= rotation::THRESHOLD {
            return Looked::Nothing;
        }
        // A pin holds while the task runs on it; one that names another subscription ended
        // when the task left it (SPEC-629 RBL-005), whichever way it was placed there.
        let pinned = match store.task_pin(&task.id).await {
            Ok(Some(pin)) if pin == current => true,
            Ok(Some(_)) => {
                self.pin_task(&task.id, None).await;
                false
            }
            _ => false,
        };
        let decision = idle_move(&input);
        if decision == IdleMove::Stay {
            // Below the threshold again: an episode without room ended, and its notice with it.
            if self.end_no_room(&task.id) {
                self.untell_no_room(task).await;
            }
            return Looked::Nothing;
        }
        if pinned {
            return Looked::Pinned;
        }
        let label = self.account_label(&current).await;
        let (account, from) = match decision {
            IdleMove::Move { account, from } => (account, from),
            IdleMove::NoRoom { from } => {
                self.tell_no_room(task, &current, &label, &from).await;
                return Looked::NoRoom;
            }
            IdleMove::Stay => return Looked::Nothing,
        };
        if failed.contains(&account) {
            return kept(&task.id, "a move to that subscription failed in this pass");
        }
        if let Some(why) = self.idle_kept_by(task).await {
            return kept(&task.id, why);
        }
        // A restart that would start a turn (a goal typed again, a workflow or a reviewer
        // resumed) is no move: the session is parked instead when parking may take it, and its
        // resume is placed off this subscription (RBL-004).
        if let Some(why) = self.restart_starts_a_turn(task).await {
            if self.config.park_after_ms == 0 {
                return kept(&task.id, why);
            }
            let rule = ParkRule::Rebalance(PARK_QUIET_MS.min(self.config.park_after_ms));
            // Local facts first: the park itself reads the task's pull requests on GitHub.
            if self.parkable_now(task, rule).await.is_none() {
                return kept(&task.id, why);
            }
            let because = format!(
                "{label} {}: it continues on a subscription with room",
                from.said()
            );
            return match self.park_task_because(&task.id, rule, Some(&because)).await {
                Ok(true) => {
                    self.end_no_room(&task.id);
                    self.tell_parked(task, &label, &from).await;
                    Looked::Parked
                }
                Ok(false) => kept(&task.id, why),
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "rebalance: a park failed");
                    Looked::Kept
                }
            };
        }
        // Still idle on the same run: no prompt arrived while this looked. The switch claims
        // exactly this state, so one that arrives later wins and the move is a conflict.
        let Ok(Some(now)) = store.task(&task.id).await else {
            return Looked::Nothing;
        };
        let Some(activity) = now
            .activity
            .filter(|a| a.is_idle() && now.status == TaskStatus::Running)
        else {
            return kept(&task.id, "a turn began while the pass looked");
        };
        if !matches!(store.live_attempt(&task.id).await, Ok(Some(live)) if live.id == run.id) {
            return kept(&task.id, "its run changed while the pass looked");
        }
        let reason = move_reason(&label, &from.said());
        let condition = SwitchCondition::Idle {
            from: (activity, now.updated_at),
            attempt: run.id.clone(),
        };
        let start = RunStart::Resume { prompt: None };
        let moved = self
            .switch_from(&task.id, Some(account.clone()), &reason, start, condition)
            .await;
        match moved {
            Ok(()) => {}
            // A turn began first: this task stays, the subscription stays a target. A switch
            // whose old run did not end in time is a conflict too, and stops the task.
            Err(AppError::Conflict(why)) if matches!(store.task(&task.id).await, Ok(Some(t)) if t.status == TaskStatus::Running) =>
            {
                return kept(&task.id, &why);
            }
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "rebalance: the move was refused");
                failed.insert(account);
                return Looked::Kept;
            }
        }
        // Told once it runs there: a launch that found no capacity paused the task instead.
        let running = matches!(
            store.task(&task.id).await,
            Ok(Some(t)) if t.status == TaskStatus::Running
        );
        let on_target = matches!(
            store.live_attempt(&task.id).await,
            Ok(Some(live)) if live.account_id.as_ref() == Some(&account)
        );
        if !(running && on_target) {
            failed.insert(account);
            return kept(&task.id, "the move did not leave it running on the target");
        }
        self.end_no_room(&task.id);
        let to = self.account_label(&account).await;
        self.tell_moved(task, (&label, &to), &from).await;
        Looked::Moved
    }

    /// What keeps an idle task's session from a restart now (`agent_cli::kept_idle`), its
    /// person's own goal in a prompt-mode session, a usage limit it waits at, or a first message
    /// that never arrived; `None` when a restart loses nothing.
    async fn idle_kept_by(&self, task: &TaskRecord) -> Option<&'static str> {
        if self.keeps_own_goal(task) {
            return Some("its person's own goal is open in it");
        }
        let id = &task.id;
        if self.runtime(id).at_limit.load(Ordering::SeqCst) {
            return Some("it waits at a usage limit, which moves it");
        }
        if self.held_opening(id).await {
            return Some("its first message has not arrived yet");
        }
        // Its last lines are read first: they say what waits in Claude Code's queue.
        if tokio::time::timeout(LINES_READ_WAIT, self.await_transcript_read(id))
            .await
            .is_err()
        {
            return Some("its lines were not read yet");
        }
        let facts = IdleFacts {
            typed_ms_ago: self.typed_ms_ago(id),
            draft: self.has_draft(id),
            switch_refused: self.switch_cooling(id) || self.claude_move_cooling(id),
            paused: self.is_paused_by_person(id).await,
            update_holds: self.update_holds_starts(),
            background: self.background_or_wait_now(id).await,
            queued: self.claude_queue_holds(id),
            prompt_unread: self.prompt_just_went_on(id),
        };
        kept_idle(&facts)
    }

    /// Why a restart of `task`'s idle session would open with a turn, as `switch_from` decides
    /// an idle rest: a goal session of Claude Code or Antigravity gets its goal typed again, and
    /// a session with an unfinished workflow or a cut-off reviewer is told to resume it; `None`
    /// when it opens at its prompt with nothing typed.
    async fn restart_starts_a_turn(&self, task: &TaskRecord) -> Option<&'static str> {
        if task.mode != TaskMode::Prompt && task.harness != Harness::Codex {
            return Some("a restart would type its goal again");
        }
        let runtime = self.runtime(&task.id);
        if !runtime.with_workflows(|workflows| workflows.runs().is_empty()) {
            return Some("a restart would resume its workflow with a turn");
        }
        if runtime.cut_off_reviewer.lock().await.is_some() {
            return Some("a restart would resume its cut-off reviewer with a turn");
        }
        None
    }

    /// Records a person's choice for `task`: pinned to `account`, or to none when they let the
    /// pool choose (SPEC-629 RBL-005). A failed write is logged; the move itself stands.
    pub(crate) async fn pin_task(&self, task: &TaskId, account: Option<&AccountId>) {
        if let Err(err) = self.deps.store.set_task_pin(task, account).await {
            tracing::warn!(task = %task, error = %err, "the task's pin could not be written");
        }
    }

    /// The subscription a parked `task` resumes on: its pin while the pin can take it (a
    /// person's choice is honoured below 100 %, ROT-007), else `None`, the pool's placement
    /// (SPEC-629 RBL-004, RBL-005).
    pub(crate) async fn pinned_start(self: &Arc<Self>, task: &TaskRecord) -> Option<AccountId> {
        let pin = self.deps.store.task_pin(&task.id).await.ok().flatten()?;
        let input = self.rotation_input(task, None).await.ok()?;
        let asked = Trigger::Manual {
            account: pin.clone(),
        };
        match rotation::decide(&input, asked).0 {
            Decision::Switch { .. } => Some(pin),
            Decision::Stay | Decision::Wait { .. } => None,
        }
    }

    async fn account_label(&self, account: &AccountId) -> String {
        match self.deps.store.account(account).await {
            Ok(Some(record)) => record.label,
            _ => account.as_str().to_owned(),
        }
    }

    /// Says a rebalance move once (SPEC-629 RBL-006): one line in the task's terminal and one
    /// notice for its person.
    async fn tell_moved(&self, task: &TaskRecord, (from, to): (&str, &str), how: &Fullest) {
        let said = how.said();
        self.runtime(&task.id).hub.notice(format!(
            "↻ {from} {said}: this idle session continues on {to}."
        ));
        let title = format!("#{} {} moved to {to}", task.number, task.title);
        self.tell_person(task, NoticeTone::Info, title, format!("{from} {said}."))
            .await;
    }

    /// Says once per episode that an idle task stays on a subscription past the threshold
    /// because no other has room (SPEC-629 RBL-007). The episode is remembered in this process:
    /// a restart of Yulius may tell it once more.
    async fn tell_no_room(
        &self,
        task: &TaskRecord,
        account: &AccountId,
        label: &str,
        how: &Fullest,
    ) {
        {
            let runtime = self.runtime(&task.id);
            let Ok(mut told) = runtime.rebalance_no_room.lock() else {
                return;
            };
            if told.as_ref().is_some_and(|(on, _)| on == account) {
                return;
            }
            *told = Some((account.clone(), self.now()));
        }
        let said = how.said();
        let kind = account_kind(task.harness.provider());
        self.runtime(&task.id).hub.notice(format!(
            "⏸ {label} {said} and no other {kind} subscription has room: this task stays on it \
             and moves when one has."
        ));
        let title = format!("#{} {} stays on {label}", task.number, task.title);
        let detail = format!("{label} {said} and no other {kind} subscription has room.");
        self.tell_person(task, NoticeTone::Warn, title, detail)
            .await;
    }

    /// Says once that an idle session whose restart would start a turn was parked off a
    /// subscription past the threshold: it continues on one with room when it is written to next
    /// (SPEC-629 RBL-004, RBL-006).
    async fn tell_parked(&self, task: &TaskRecord, label: &str, how: &Fullest) {
        let said = how.said();
        self.runtime(&task.id).hub.notice(format!(
            "⏸ {label} {said}: this idle session is parked and continues on a subscription \
             with room when it is written to next."
        ));
        let title = format!("#{} {} parked off {label}", task.number, task.title);
        let detail = format!(
            "{label} {said}: it continues on a subscription with room when it is written to next."
        );
        self.tell_person(task, NoticeTone::Info, title, detail)
            .await;
    }

    /// Posts the task's rebalance notice to its person (SPEC-624): keyed `rebalance:<task id>`,
    /// so its next move, park or wait replaces it, with the default expiry of a report (1 day).
    async fn tell_person(
        &self,
        task: &TaskRecord,
        tone: NoticeTone,
        title: String,
        detail: String,
    ) {
        let Some(key) = notice_key(task) else {
            return;
        };
        let link = match self.deps.store.sandbox(&task.sandbox_id).await {
            Ok(Some(sandbox)) => Some(format!("/s/{}/t/{}", sandbox.slug, task.number)),
            _ => None,
        };
        let mut notice = NewNotice::new(
            Some(task.sandbox_id.clone()),
            key,
            NoticeKind::Account,
            tone,
            title,
        );
        notice.to = Some(task.owner.clone());
        notice.detail = Some(detail);
        notice.link = link;
        if let Err(err) = self.post_notice(notice).await {
            tracing::warn!(task = %task.id, error = %err, "rebalance: the notice was not posted");
        }
    }

    /// An episode without room ended: the task moved, or its subscription is below the
    /// threshold again. Whether one was open.
    fn end_no_room(&self, task: &TaskId) -> bool {
        self.existing_runtime(task)
            .and_then(|runtime| runtime.rebalance_no_room.lock().ok()?.take())
            .is_some()
    }

    /// Removes the task's "stays on" notice once its subscription has room again: no longer
    /// true, it would stand for a day (SPEC-629 RBL-007).
    async fn untell_no_room(&self, task: &TaskRecord) {
        let Some(key) = notice_key(task) else {
            return;
        };
        let removed = self
            .remove_notice(Some(&task.sandbox_id), Some(&task.owner), &key)
            .await;
        if let Err(err) = removed {
            tracing::warn!(task = %task.id, error = %err, "rebalance: the notice was not removed");
        }
    }
}

/// The key of `task`'s rebalance notice; `None` (logged) for a task id the key grammar refuses.
fn notice_key(task: &TaskRecord) -> Option<NoticeKey> {
    NoticeKey::parse(&format!("{NOTICE_KEY}{}", task.id))
        .inspect_err(
            |err| tracing::warn!(task = %task.id, error = %err, "rebalance: no notice key"),
        )
        .ok()
}
