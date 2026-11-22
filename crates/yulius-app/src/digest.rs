//! The end-of-day digest (SPEC-512 part 3, #1413, ADR-0380): once a day, at a time an instance
//! administrator sets (off by default), each person gets what waits for them, what was parked
//! since the last digest with what its last message left, and what still works, in the browser
//! and in their Telegram chat. With an idle threshold set, the run first parks the quiet tasks
//! that wait for nobody.
//!
//! - DIG-001: [`App::set_digest_setting`], stored in `instance.digest`;
//! - DIG-002: [`App::digest_tick`], once per day by [`run_due`] and a conditional write;
//! - DIG-003: [`crate::parking::ParkRule::Digest`];
//! - DIG-004, DIG-006: [`App::send_digests_now`], texts redacted per sandbox;
//! - DIG-005: stored per person, published to them, sent to their linked chat.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use yulius_contracts::api::digest::{DigestItem, DigestSetting, DigestView};
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, TaskGroup, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::TaskRecord;
use crate::parking::ParkRule;
use crate::ports::store_ext::StoredDigest;
use crate::transcripts;

/// The longest idle threshold, in hours: 30 days.
pub const MAX_PARK_IDLE_HOURS: u32 = 720;
/// How much of the last message a parked task's line keeps (DIG-004).
pub const LAST_WORDS: usize = 300;
/// How many working tasks a digest names.
const WORKING_NAMED: usize = 10;
/// How long a run waits for the sessions it parks to end before it lists them.
const PARKS_SETTLE_MS: u64 = 60_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// Minutes after midnight of an `HH:MM` time, `None` when it is not one.
#[must_use]
pub fn minute_of(at: &str) -> Option<i64> {
    let (h, m) = at.trim().split_once(':')?;
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if h.is_empty() || h.len() > 2 || m.len() != 2 || !digits(h) || !digits(m) {
        return None;
    }
    let (h, m) = (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

/// Whether the day's digest is due at `now_ms` (DIG-002): it is on, today's time (UTC) has
/// passed, it has not run or been saved since (`ran_at`), and no run was claimed today
/// (`sent_at`, written only by a run's claim), so moving the time later the same day sends no
/// second one; a run whose work ends after midnight does not cost the next day's.
#[must_use]
pub fn run_due(
    setting: &DigestSetting,
    now_ms: i64,
    ran_at: Option<i64>,
    sent_at: Option<i64>,
) -> bool {
    let Some(minute) = minute_of(&setting.at).filter(|_| setting.enabled) else {
        return false;
    };
    let day = now_ms.div_euclid(DAY_MS) * DAY_MS;
    let today_at = day + minute * 60_000;
    now_ms >= today_at
        && ran_at.is_none_or(|ran| ran < today_at)
        && sent_at.is_none_or(|sent| sent < day)
}

/// The end of `text` in at most `max` characters, from a word's start, `…` before it when cut.
#[must_use]
pub fn last_words(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = text.chars().count();
    if count <= max {
        return text;
    }
    let tail: String = text.chars().skip(count - (max - 1)).collect();
    let from_word = tail
        .split_once(' ')
        .map_or(tail.as_str(), |(_, rest)| rest)
        .to_owned();
    format!("…{from_word}")
}

/// The digest as one Telegram message (DIG-005), within its bound.
#[must_use]
pub fn digest_text(digest: &DigestView) -> String {
    let mut out = String::from("Your Yulius digest");
    let line = |item: &DigestItem| {
        let mut s = format!("\n• {} #{} {}", item.project, item.number, item.title);
        if let Some(detail) = &item.detail {
            s.push_str(": ");
            s.push_str(detail);
        }
        s.push_str("\n  ");
        s.push_str(&item.link);
        s
    };
    if !digest.waiting.is_empty() {
        out.push_str(&format!("\n\nWaiting for you ({}):", digest.waiting.len()));
        digest.waiting.iter().for_each(|i| out.push_str(&line(i)));
    }
    if !digest.parked.is_empty() {
        out.push_str(&format!("\n\nParked ({}):", digest.parked.len()));
        digest.parked.iter().for_each(|i| out.push_str(&line(i)));
    }
    if digest.working_count > 0 {
        out.push_str(&format!("\n\nStill working: {}", digest.working_count));
        for item in &digest.working {
            out.push_str(&format!(
                "\n• {} #{} {}",
                item.project, item.number, item.title
            ));
        }
    }
    yulius_domain::notify::truncate(&out)
}

/// What a run did: how many people got a digest and how many sessions it began to park.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DigestRun {
    /// People who got a digest.
    pub sent: usize,
    /// Parks it began (DIG-003).
    pub parked: usize,
}

/// A digest's section.
#[derive(Debug, Clone, Copy)]
enum Section {
    Waiting,
    Parked,
    Working,
}

/// One person's digest while it is built.
#[derive(Default)]
struct Draft {
    waiting: Vec<DigestItem>,
    parked: Vec<DigestItem>,
    working: Vec<DigestItem>,
}

impl App {
    /// `GET /api/admin/digest` (DIG-001).
    ///
    /// # Errors
    /// `Forbidden` for anyone but an instance administrator; store failures.
    pub async fn digest_setting(&self, ctx: &AuthContext) -> AppResult<DigestSetting> {
        self.require_instance_admin(ctx)?;
        Ok(self.stored_digest_setting().await?.0)
    }

    async fn stored_digest_setting(&self) -> AppResult<(DigestSetting, Option<Timestamp>)> {
        let (doc, ran) = self.deps.store.digest_setting().await?;
        let setting = doc
            .and_then(|doc| serde_json::from_str(&doc).ok())
            .unwrap_or_default();
        Ok((setting, ran))
    }

    /// `PUT /api/admin/digest` (DIG-001): saved, audited `instance.digest.set`; the first digest
    /// after it is the next time it names.
    ///
    /// # Errors
    /// `Forbidden` for anyone but an instance administrator, `Invalid` for a time that is not
    /// `HH:MM` or a threshold out of 1 to 720 hours; store failures.
    pub async fn set_digest_setting(
        &self,
        ctx: &AuthContext,
        req: &DigestSetting,
    ) -> AppResult<DigestSetting> {
        self.require_instance_admin(ctx)?;
        let minute = minute_of(&req.at)
            .ok_or_else(|| AppError::Invalid("the time is HH:MM, in UTC".into()))?;
        if req
            .park_idle_hours
            .is_some_and(|h| !(1..=MAX_PARK_IDLE_HOURS).contains(&h))
        {
            return Err(AppError::Invalid(format!(
                "the idle threshold is 1 to {MAX_PARK_IDLE_HOURS} hours, or none"
            )));
        }
        let setting = DigestSetting {
            enabled: req.enabled,
            at: format!("{:02}:{:02}", minute / 60, minute % 60),
            park_idle_hours: req.park_idle_hours,
        };
        let doc = serde_json::to_string(&setting).map_err(AppError::internal)?;
        let now = self.now();
        self.deps.store.set_digest_setting(&doc, now).await?;
        let login = self.login_of(&ctx.user.id).await;
        self.deps
            .store
            .audit(
                now,
                &login,
                "instance.digest.set",
                Some("digest"),
                Some(&doc),
            )
            .await?;
        Ok(setting)
    }

    /// `GET /api/me/digest` (DIG-005): the person's latest digest.
    ///
    /// # Errors
    /// Store failures.
    pub async fn my_digest(&self, ctx: &AuthContext) -> AppResult<Option<DigestView>> {
        Ok(self
            .deps
            .store
            .latest_digest(&ctx.user.id)
            .await?
            .and_then(|stored| serde_json::from_str(&stored.body).ok()))
    }

    /// The background tick's look (DIG-002): runs the day's digest when it is due and this tick
    /// took the run.
    pub async fn digest_tick(self: &Arc<Self>) -> DigestRun {
        let store = &self.deps.store;
        let (Ok((setting, ran)), Ok(sent)) = (
            self.stored_digest_setting().await,
            store.digest_sent_at().await,
        ) else {
            return DigestRun::default();
        };
        let now = self.now();
        if !run_due(&setting, now.0, ran.map(|r| r.0), sent.map(|at| at.0)) {
            return DigestRun::default();
        }
        match store.take_digest_run(ran, now).await {
            Ok(true) => {}
            Ok(false) => return DigestRun::default(),
            Err(err) => {
                tracing::warn!(error = %err, "the digest's run could not be taken");
                return DigestRun::default();
            }
        }
        let park_after_ms = setting.park_idle_hours.map(|h| u64::from(h) * 3_600_000);
        self.send_digests_now(park_after_ms).await
    }

    /// Parks what waits for nobody quiet for `park_after_ms` (DIG-003), waits for those sessions
    /// to end, then builds, keeps, publishes and sends each person's digest (DIG-004 to
    /// DIG-006), listing as parked what was parked since the last digest's listing (24 hours
    /// before the first), and records where this listing ended.
    pub async fn send_digests_now(self: &Arc<Self>, park_after_ms: Option<u64>) -> DigestRun {
        let store = &self.deps.store;
        let parked_now = match park_after_ms {
            Some(after_ms) => self.digest_park(after_ms).await,
            None => HashSet::new(),
        };
        self.parks_settled(&parked_now).await;
        let since_ms = match store.digest_listed_at().await {
            Ok(listed) => listed.map_or(self.now().0 - DAY_MS, |at| at.0),
            Err(err) => {
                tracing::warn!(error = %err, "the digest's last listing could not be read");
                return DigestRun {
                    sent: 0,
                    parked: parked_now.len(),
                };
            }
        };
        let listed_at = self.now();
        let drafts = match self.digest_drafts(since_ms, listed_at.0, &parked_now).await {
            Ok(drafts) => drafts,
            Err(err) => {
                tracing::warn!(error = %err, "the digest could not be built");
                return DigestRun {
                    sent: 0,
                    parked: parked_now.len(),
                };
            }
        };
        if let Err(err) = store.set_digest_listed_at(listed_at).await {
            tracing::warn!(error = %err, "the digest's listing could not be recorded");
        }
        let mut sent = 0;
        for (user, draft) in drafts {
            if draft.waiting.is_empty() && draft.parked.is_empty() && draft.working.is_empty() {
                continue;
            }
            match self.deliver_digest(&user, draft).await {
                Ok(()) => sent += 1,
                Err(err) => tracing::warn!(user = %user, error = %err, "a digest failed"),
            }
        }
        tracing::info!(sent, parked = parked_now.len(), "the end-of-day digest ran");
        DigestRun {
            sent,
            parked: parked_now.len(),
        }
    }

    /// Waits, at most [`PARKS_SETTLE_MS`], until the sessions this run parks have ended, so
    /// they are listed now and their `state_at` falls before this listing's end, never in the
    /// next digest's window (review round 1).
    async fn parks_settled(&self, parked: &HashSet<TaskId>) {
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_millis(PARKS_SETTLE_MS);
        let mut left: Vec<&TaskId> = parked.iter().collect();
        while !left.is_empty() && tokio::time::Instant::now() < deadline {
            let mut still = Vec::new();
            for id in left {
                let settled = match self.deps.store.task(id).await {
                    // Stopping is still on its way: the end settles the task as parked.
                    Ok(Some(task)) => !task.status.is_live(),
                    _ => true,
                };
                if !settled {
                    still.push(id);
                }
            }
            left = still;
            if !left.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }

    /// DIG-003: the running tasks the digest parks, the cheap facts first.
    async fn digest_park(self: &Arc<Self>, after_ms: u64) -> HashSet<TaskId> {
        let mut parked = HashSet::new();
        let Ok(running) = self
            .deps
            .store
            .tasks_with_status(&[TaskStatus::Running])
            .await
        else {
            return parked;
        };
        let now = self.now().0;
        for task in running {
            if task.activity != Some(Activity::Waiting) {
                continue;
            }
            let quiet = matches!(
                self.deps.store.task_times(&task.id).await,
                Ok(Some(times)) if i64::try_from(after_ms)
                    .is_ok_and(|after| now.saturating_sub(times.state_at.0) >= after)
            );
            if !quiet {
                continue;
            }
            match self
                .park_task_now(&task.id, ParkRule::Digest(after_ms))
                .await
            {
                Ok(true) => {
                    parked.insert(task.id);
                }
                Ok(false) => {}
                Err(err) => tracing::warn!(task = %task.id, error = %err, "a digest park failed"),
            }
        }
        parked
    }

    /// Each person's sections (DIG-004), redacted (DIG-006), for the people who can see each
    /// task's project; a task that cannot be read is left out, not the whole digest.
    async fn digest_drafts(
        &self,
        since_ms: i64,
        until_ms: i64,
        parked_now: &HashSet<TaskId>,
    ) -> AppResult<HashMap<UserId, Draft>> {
        let store = &self.deps.store;
        let mut people: HashMap<String, UserId> = HashMap::new();
        let mut enabled: HashMap<UserId, crate::model::UserRecord> = HashMap::new();
        for user in store.users().await? {
            if user.disabled_at.is_none() {
                if let Some(login) = &user.github_login {
                    people.insert(login.to_lowercase(), user.id.clone());
                }
                enabled.insert(user.id.clone(), user);
            }
        }
        let mut sees: HashMap<(UserId, SandboxId), bool> = HashMap::new();
        let mut requests: HashMap<SandboxId, HashSet<String>> = HashMap::new();
        let mut drafts: HashMap<UserId, Draft> = HashMap::new();
        let tasks = store
            .tasks_with_status(&[
                TaskStatus::Running,
                TaskStatus::Parked,
                TaskStatus::Starting,
                TaskStatus::Switching,
                TaskStatus::WaitingCapacity,
            ])
            .await?;
        for task in tasks {
            if task.archived_at.is_some() {
                continue;
            }
            let placed = self
                .digest_place(
                    &task,
                    (since_ms, until_ms),
                    parked_now,
                    &people,
                    &mut requests,
                )
                .await;
            let (section, item, to) = match placed {
                Ok(Some(placed)) => placed,
                Ok(None) => continue,
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "a task was left out of the digest");
                    continue;
                }
            };
            for user in to {
                let Some(person) = enabled.get(&user) else {
                    continue;
                };
                let key = (user.clone(), task.sandbox_id.clone());
                let visible = match sees.get(&key) {
                    Some(visible) => *visible,
                    None => {
                        let visible = self.can_view(person, &task.sandbox_id).await;
                        sees.insert(key, visible);
                        visible
                    }
                };
                if !visible {
                    continue;
                }
                let draft = drafts.entry(user).or_default();
                match section {
                    Section::Waiting => draft.waiting.push(item.clone()),
                    Section::Parked => draft.parked.push(item.clone()),
                    Section::Working => draft.working.push(item.clone()),
                }
            }
        }
        Ok(drafts)
    }

    /// Where `task` goes in a digest and to whom (before the visibility check), or nowhere.
    async fn digest_place(
        &self,
        task: &TaskRecord,
        (since_ms, until_ms): (i64, i64),
        parked_now: &HashSet<TaskId>,
        people: &HashMap<String, UserId>,
        requests: &mut HashMap<SandboxId, HashSet<String>>,
    ) -> AppResult<Option<(Section, DigestItem, Vec<UserId>)>> {
        let store = &self.deps.store;
        let asked = store.task_needs_you(&task.id).await?;
        if asked.is_some() || task.activity == Some(Activity::NeedsAttention) {
            let detail = asked.as_ref().map_or_else(
                || "a question in its terminal".to_owned(),
                |a| a.action.clone(),
            );
            let mut to = vec![task.owner.clone()];
            if let Some(asked) = &asked {
                to.extend(
                    asked
                        .named
                        .who
                        .iter()
                        .filter(|login| !asked.named.withdrawn.contains(login))
                        .filter_map(|login| people.get(&login.to_lowercase()).cloned()),
                );
            }
            to.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
            to.dedup();
            let item = self.digest_item(task, Some(detail)).await?;
            return Ok(Some((Section::Waiting, item, to)));
        }
        if !requests.contains_key(&task.sandbox_id) {
            let read = self.browser_requests(&task.sandbox_id).await;
            requests.insert(task.sandbox_id.clone(), read);
        }
        let requested = requests.get(&task.sandbox_id).cloned().unwrap_or_default();
        let parked_since = task.status == TaskStatus::Parked
            && store
                .task_times(&task.id)
                .await?
                .is_some_and(|times| (since_ms + 1..=until_ms).contains(&times.state_at.0));
        if parked_now.contains(&task.id) || parked_since {
            let ended = if self.task_group(task, &requested).await? == TaskGroup::Done {
                "Finished"
            } else {
                "Waits for your turn"
            };
            let detail = match self.last_agent_words(task).await {
                Some(words) => format!("{ended}. Last message: {words}"),
                None => ended.to_owned(),
            };
            let item = self.digest_item(task, Some(detail)).await?;
            return Ok(Some((Section::Parked, item, vec![task.owner.clone()])));
        }
        let working = match task.status {
            TaskStatus::Running | TaskStatus::WaitingCapacity => {
                self.task_group(task, &requested).await? == TaskGroup::Working
            }
            TaskStatus::Starting | TaskStatus::Switching => true,
            _ => false,
        };
        if !working {
            return Ok(None);
        }
        let item = self.digest_item(task, None).await?;
        Ok(Some((Section::Working, item, vec![task.owner.clone()])))
    }

    async fn digest_item(
        &self,
        task: &TaskRecord,
        detail: Option<String>,
    ) -> AppResult<DigestItem> {
        let sandbox = self
            .deps
            .store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let title = self.redact_for_sandbox(&task.sandbox_id, &task.title).await;
        let detail = match detail {
            Some(text) => Some(self.redact_for_sandbox(&task.sandbox_id, &text).await),
            None => None,
        };
        Ok(DigestItem {
            project: sandbox.repo_name,
            number: task.number,
            title,
            detail,
            link: format!(
                "{}/s/{}/t/{}",
                self.config.public_url.trim_end_matches('/'),
                sandbox.slug,
                task.number
            ),
        })
    }

    /// The end of the agent's last message in its transcript (DIG-004), redacted as the
    /// transcript is; `None` when there is none to read.
    pub(crate) async fn last_agent_words(&self, task: &TaskRecord) -> Option<String> {
        let entry = self
            .deps
            .store
            .provider_sessions(&task.id)
            .await
            .ok()?
            .pop()?;
        let path = self.task_transcript_path(task, &entry);
        let anchor = self.sandbox_dir(&task.sandbox_id);
        let page = tokio::task::spawn_blocking(move || {
            let file = transcripts::open(&anchor, &path).ok()?;
            transcripts::read_tail(file, 20, transcripts::MAX_PAGE_CHARS).ok()
        })
        .await
        .ok()??;
        let text = page.entries.iter().rev().find_map(|entry| {
            if entry.role != "assistant" || entry.sidechain {
                return None;
            }
            let text: Vec<&str> = entry
                .blocks
                .iter()
                .filter_map(|block| match block {
                    yulius_contracts::api::sessions::TranscriptBlock::Text { text, .. } => {
                        Some(text.as_str())
                    }
                    _ => None,
                })
                .collect();
            (!text.is_empty()).then(|| text.join("\n"))
        })?;
        let redacted = self.redact_for_sandbox(&task.sandbox_id, &text).await;
        Some(last_words(&redacted, LAST_WORDS))
    }

    /// Keeps, publishes and sends one person's digest (DIG-005).
    async fn deliver_digest(&self, user: &UserId, mut draft: Draft) -> AppResult<()> {
        let now = self.now();
        let id: String = self
            .deps
            .crypto
            .random_bytes()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        let working_count = u32::try_from(draft.working.len()).unwrap_or(u32::MAX);
        draft.working.truncate(WORKING_NAMED);
        let view = DigestView {
            id: format!("dg_{id}"),
            created_at: rfc3339(now),
            waiting: draft.waiting,
            parked: draft.parked,
            working_count,
            working: draft.working,
        };
        let body = serde_json::to_string(&view).map_err(AppError::internal)?;
        self.deps
            .store
            .put_digest(
                user,
                &StoredDigest {
                    id: view.id.clone(),
                    created_at: now,
                    body,
                },
            )
            .await?;
        let text = digest_text(&view);
        self.events.publish_to_user(
            user.clone(),
            UiEvent::DigestReady {
                digest: Box::new(view),
            },
        );
        if self.channel_services().is_ok()
            && let Some(link) = self.deps.store.chat_link(user).await?
            && link.choices.digest
        {
            self.enqueue(link.chat, text);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY0: i64 = 1_795_132_800_000; // 2026-11-19T00:00:00Z

    fn on(at: &str) -> DigestSetting {
        DigestSetting {
            enabled: true,
            at: at.into(),
            park_idle_hours: None,
        }
    }

    /// DIG-001: a time is `HH:MM`.
    #[test]
    fn a_time_is_hours_and_minutes() {
        assert_eq!(minute_of("18:00"), Some(1080));
        assert_eq!(minute_of("7:05"), Some(425));
        assert_eq!(minute_of("00:00"), Some(0));
        for bad in [
            "24:00", "12:60", "12", "12:5", "ab:cd", "", "-1:00", "123:00",
        ] {
            assert_eq!(minute_of(bad), None, "{bad}");
        }
    }

    /// DIG-002: due once today's time passed and it has not run since; never while off.
    #[test]
    fn the_digest_is_due_once_a_day_after_its_time() {
        let at = DAY0 + 18 * HOUR_MS;
        assert!(
            !run_due(&on("18:00"), at - 1, Some(DAY0 - DAY_MS), None),
            "before its time"
        );
        assert!(
            run_due(&on("18:00"), at, Some(DAY0 - DAY_MS), None),
            "at its time"
        );
        assert!(
            run_due(&on("18:00"), at + 5 * HOUR_MS, Some(at - 1), None),
            "a server back later"
        );
        assert!(
            !run_due(&on("18:00"), at + 60_000, Some(at), None),
            "ran already"
        );
        assert!(
            !run_due(&on("18:00"), at + HOUR_MS, Some(at + 30 * 60_000), None),
            "turned on after its time: the first is tomorrow"
        );
        assert!(
            run_due(&on("18:00"), at + DAY_MS, Some(at + 30 * 60_000), None),
            "tomorrow"
        );
        let off = DigestSetting {
            enabled: false,
            ..on("18:00")
        };
        assert!(!run_due(&off, at, None, None));
        assert!(!DigestSetting::default().enabled, "off by default");
        // Sent at 08:00, then moved to 18:00 the same day (a save writes the run's claim): no
        // second digest today, one tomorrow (review round 1).
        let morning = DAY0 + 8 * HOUR_MS;
        let saved = DAY0 + 9 * HOUR_MS;
        assert!(!run_due(&on("18:00"), at, Some(saved), Some(morning)));
        assert!(run_due(
            &on("18:00"),
            at + DAY_MS,
            Some(saved),
            Some(morning)
        ));
        // A run claimed at 23:59:30 (its listing may end after midnight): the next day's
        // digest is still due, since the same-day rule reads the claim (review round 2).
        let claimed = DAY0 + DAY_MS - 30_000;
        assert!(run_due(
            &on("23:59"),
            claimed + DAY_MS,
            Some(claimed),
            Some(claimed)
        ));
    }

    /// DIG-004: a long last message keeps its end, from a word's start.
    #[test]
    fn a_last_message_keeps_its_end() {
        assert_eq!(
            last_words("Promote dev to main.", 300),
            "Promote dev to main."
        );
        let long = format!("{} Please promote dev to main.", "word ".repeat(100));
        let cut = last_words(&long, 30);
        assert!(
            cut.starts_with('…') && cut.ends_with("Please promote dev to main."),
            "{cut}"
        );
        assert!(cut.chars().count() <= 30, "{cut}");
        assert_eq!(last_words("a\n\n  b", 10), "a b");
    }

    /// DIG-004, DIG-005: the message lists the sections in order and stays within its bound.
    #[test]
    fn the_message_lists_what_waits_first() {
        let item = |n: u32, detail: Option<&str>| DigestItem {
            project: "acme/app".into(),
            number: n,
            title: format!("Task {n}"),
            detail: detail.map(str::to_owned),
            link: format!("https://y.test/s/app/t/{n}"),
        };
        let view = DigestView {
            id: "dg_1".into(),
            created_at: "2026-11-19T18:00:00Z".into(),
            waiting: vec![item(1, Some("Choose the colour"))],
            parked: vec![item(2, Some("Finished. Last message: promote dev to main"))],
            working_count: 12,
            working: (3..13).map(|n| item(n, None)).collect(),
        };
        let text = digest_text(&view);
        let waiting = text.find("Waiting for you (1):").unwrap_or(usize::MAX);
        let parked = text.find("Parked (1):").unwrap_or(usize::MAX);
        let working = text.find("Still working: 12").unwrap_or(usize::MAX);
        assert!(waiting < parked && parked < working, "{text}");
        assert!(text.contains("acme/app #1 Task 1: Choose the colour\n  https://y.test/s/app/t/1"));
        let huge = DigestView {
            waiting: (0..200).map(|n| item(n, Some(&"x".repeat(100)))).collect(),
            ..view
        };
        assert!(digest_text(&huge).chars().count() <= yulius_domain::notify::MAX_MESSAGE_CHARS);
    }
}
