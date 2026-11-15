//! The Superagent's scheduled checks (SPEC-487 slice 9b, CHK-001 to CHK-005; wall 8): standing
//! jobs ("keep monitoring", a disk guard) held by Yulius rather than by its session, whose own
//! schedulers end with every restart.
//!
//! Contract:
//! - a check is its Superagent's (person and sandbox): a name, an interval of 5 minutes to a day
//!   and a prompt, at most [`MAX_CHECKS`] each, kept in [`CHECKS_FILE`] beside the database and
//!   read back at a start;
//! - when one is due and its Superagent is idle (no turn, no draft, nobody typed in the last
//!   minute), Yulius types its prompt into the session, labelled as a check of Yulius's, and
//!   counts the next interval from then; one at a time;
//! - when one is due and its Superagent is not running, Yulius starts it as opening its panel
//!   would, with its person's rights at that moment.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::{Channel, ProjectCaller};
use crate::error::{AppError, AppResult};
use crate::shells::ShellRuntime;

/// The file beside the database that keeps the checks.
pub const CHECKS_FILE: &str = "superagent-checks.json";
/// The most checks a Superagent has.
pub const MAX_CHECKS: usize = 10;
/// The shortest interval, in minutes.
pub const MIN_EVERY_MINUTES: u32 = 5;
/// The longest interval, in minutes (a day).
pub const MAX_EVERY_MINUTES: u32 = 24 * 60;
/// The longest prompt, in bytes.
pub const MAX_CHECK_PROMPT_BYTES: usize = 2048;
/// How long after its person's last keystroke a Superagent takes no check.
pub(crate) const QUIET_AFTER_TYPING_MS: i64 = 60_000;
/// Between the paste and the Enter that submits it (as a task's message).
const SUBMIT_DELAY: Duration = Duration::from_millis(300);
/// How long after its start a Superagent takes no check: Claude Code's prompt box is not up yet
/// (review of slice 9b, F2).
pub(crate) const CHECK_START_GRACE_MS: i64 = 20_000;
/// The least time between two checks typed into one Superagent: its transcript shows the first
/// turn only after a moment (review of slice 9b, F5).
pub(crate) const CHECK_GAP_MS: i64 = 30_000;
/// When a check could not be typed, it is tried again after this long (F3).
const CHECK_RETRY_MS: i64 = 60_000;
/// The longest wait between two starts tried for a Superagent that would not start (F6).
const START_BACKOFF_MAX_MS: i64 = 60 * 60_000;

/// One check, as kept.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Check {
    user: String,
    sandbox: String,
    name: String,
    every_minutes: u32,
    prompt: String,
    created_at: i64,
    next_at: i64,
    #[serde(default)]
    last_run_at: Option<i64>,
}

/// Every Superagent's checks.
#[derive(Default)]
pub struct SuperagentChecks {
    checks: Mutex<Vec<Check>>,
    /// Serializes the writes of [`CHECKS_FILE`] (review of slice 9b, F1).
    file: Mutex<()>,
    /// Per person and sandbox, after a start that failed: when to try again, and how many failed.
    backoff: Mutex<HashMap<(String, String), (i64, u32)>>,
}

impl SuperagentChecks {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Check>> {
        self.checks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// What `superagent_check_set` takes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    name: String,
    every_minutes: u32,
    prompt: String,
}

/// What `superagent_check_delete` takes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteArgs {
    name: String,
}

fn parse<T: serde::de::DeserializeOwned>(args: &Value) -> AppResult<T> {
    serde_json::from_value(args.clone())
        .map_err(|err| AppError::Invalid(format!("invalid arguments: {err}")))
}

fn superagent_only(caller: &ProjectCaller) -> AppResult<()> {
    if caller.channel != Channel::Superagent || caller.task.is_some() {
        return Err(AppError::Forbidden("only a Superagent has checks".into()));
    }
    Ok(())
}

/// Whether a due check may be typed into a Superagent now: nobody holds its prompt box (a
/// draft, or a keystroke in the last minute).
pub(crate) fn may_type_check(draft: bool, last_key_ms: i64, now_ms: i64) -> bool {
    !draft && (last_key_ms == 0 || now_ms.saturating_sub(last_key_ms) >= QUIET_AFTER_TYPING_MS)
}

/// Whether a kept check is one `superagent_check_set` could have made (a hand-edited file is not
/// trusted; review of slice 9b, F8).
fn sound(check: &Check) -> bool {
    (MIN_EVERY_MINUTES..=MAX_EVERY_MINUTES).contains(&check.every_minutes)
        && !check.prompt.trim().is_empty()
        && check.prompt.len() <= MAX_CHECK_PROMPT_BYTES
        && !check.name.is_empty()
        && !check.name.chars().any(char::is_control)
}

fn every_ms(check: &Check) -> i64 {
    i64::from(check.every_minutes) * 60_000
}

/// What is typed for a check: one bracketed paste of its label and prompt.
fn check_paste(check: &Check) -> Vec<u8> {
    format!(
        "\x1b[200~Yulius check \"{}\" (every {} minutes, set by you with superagent_check_set):\n\n{}\x1b[201~",
        check.name, check.every_minutes, check.prompt
    )
    .into_bytes()
}

fn view(check: &Check) -> Value {
    json!({
        "name": check.name,
        "every_minutes": check.every_minutes,
        "prompt": check.prompt,
        "next_at": rfc3339(Timestamp(check.next_at)),
        "last_run_at": check.last_run_at.map(|at| rfc3339(Timestamp(at))),
    })
}

impl App {
    fn checks_path(&self) -> std::path::PathBuf {
        self.config.data_dir.join(CHECKS_FILE)
    }

    /// Writes [`CHECKS_FILE`] whole (owner-only: it holds prompts) and renames it into place,
    /// one writer at a time, from the checks as they are then (review of slice 9b, F1, F8).
    fn save_superagent_checks(&self) {
        let _writing = self
            .superagent_checks
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let checks = self.superagent_checks.lock().clone();
        let path = self.checks_path();
        let tmp = path.with_extension("json.tmp");
        let written = serde_json::to_vec(&checks)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                use std::io::Write as _;
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create(true).truncate(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt as _;
                    options.mode(0o600);
                }
                options.open(&tmp)?.write_all(&bytes)
            })
            .and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(err) = written {
            tracing::warn!(error = %err, "the Superagents' checks were not saved");
        }
    }

    /// At a start, before the server takes calls: reads the checks back (CHK-002). A file that
    /// cannot be read is kept as `.bad`.
    pub fn restore_superagent_checks(&self) {
        let path = self.checks_path();
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        match serde_json::from_slice::<Vec<Check>>(&bytes) {
            Ok(checks) => {
                let now = self.now().0;
                *self.superagent_checks.lock() = checks
                    .into_iter()
                    .filter(sound)
                    .map(|mut check| {
                        // A clock set back does not push a check past one interval (F7).
                        check.next_at = check.next_at.min(now.saturating_add(every_ms(&check)));
                        check
                    })
                    .collect();
            }
            Err(err) => {
                let kept = path.with_extension("json.bad");
                tracing::warn!(error = %err, kept = %kept.display(), "the Superagents' checks could not be read");
                if let Err(err) = std::fs::rename(&path, &kept) {
                    tracing::warn!(error = %err, "the unreadable checks file was not kept");
                }
            }
        }
    }

    async fn audit_check(&self, caller: &ProjectCaller, action: &str, name: &str) {
        let detail = crate::manage::tool_detail(caller, &json!({"action": action, "check": name}));
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &caller.actor,
                "superagent.check",
                Some(caller.sandbox.as_str()),
                Some(&detail),
            )
            .await
        {
            tracing::warn!(error = %err, "a Superagent's check was not audited");
        }
    }

    /// `superagent_checks` (CHK-001): the Superagent's checks, with when each runs next and ran
    /// last.
    ///
    /// # Errors
    /// `Forbidden` for anyone but a Superagent.
    pub fn tool_superagent_checks(&self, caller: &ProjectCaller) -> AppResult<Value> {
        superagent_only(caller)?;
        let checks: Vec<Value> = self
            .superagent_checks
            .lock()
            .iter()
            .filter(|c| c.user == caller.user.as_str() && c.sandbox == caller.sandbox.as_str())
            .map(view)
            .collect();
        Ok(json!({"checks": checks}))
    }

    /// `superagent_check_set` (CHK-001): creates or replaces the check of that name; it runs
    /// first one interval from now. Audited without its prompt.
    ///
    /// # Errors
    /// `Forbidden` for anyone but a Superagent; `Invalid` for a name, interval or prompt out of
    /// bounds, invisible characters or unknown arguments; `Capacity` past [`MAX_CHECKS`].
    pub async fn tool_superagent_check_set(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        superagent_only(caller)?;
        let asked: SetArgs = parse(args)?;
        crate::cross_sandbox::check_visible(args)?;
        let name = asked.name.trim().to_owned();
        if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
            return Err(AppError::Invalid(
                "the name is 1 to 60 characters on one line, without control characters".into(),
            ));
        }
        if !(MIN_EVERY_MINUTES..=MAX_EVERY_MINUTES).contains(&asked.every_minutes) {
            return Err(AppError::Invalid(format!(
                "every_minutes is {MIN_EVERY_MINUTES} to {MAX_EVERY_MINUTES}"
            )));
        }
        let prompt = asked.prompt.trim().to_owned();
        if prompt.is_empty() || prompt.len() > MAX_CHECK_PROMPT_BYTES {
            return Err(AppError::Invalid(format!(
                "the prompt is 1 to {MAX_CHECK_PROMPT_BYTES} bytes"
            )));
        }
        let now = self.now().0;
        let every_ms = i64::from(asked.every_minutes) * 60_000;
        {
            let mut checks = self.superagent_checks.lock();
            let mine =
                |c: &Check| c.user == caller.user.as_str() && c.sandbox == caller.sandbox.as_str();
            if let Some(existing) = checks.iter_mut().find(|c| mine(c) && c.name == name) {
                existing.every_minutes = asked.every_minutes;
                existing.prompt = prompt;
                existing.next_at = now.saturating_add(every_ms);
            } else {
                if checks.iter().filter(|c| mine(c)).count() >= MAX_CHECKS {
                    return Err(AppError::Capacity(format!(
                        "a Superagent has at most {MAX_CHECKS} checks: delete one first"
                    )));
                }
                checks.push(Check {
                    user: caller.user.as_str().to_owned(),
                    sandbox: caller.sandbox.as_str().to_owned(),
                    name: name.clone(),
                    every_minutes: asked.every_minutes,
                    prompt,
                    created_at: now,
                    next_at: now.saturating_add(every_ms),
                    last_run_at: None,
                });
            }
        }
        self.save_superagent_checks();
        self.audit_check(caller, "set", &name).await;
        Ok(json!({
            "check": name,
            "next_at": rfc3339(Timestamp(now.saturating_add(every_ms))),
            "note": "Kept by Yulius: it is typed into this session when due and the session is idle, also after a restart; a Superagent that is not running is started for it.",
        }))
    }

    /// `superagent_check_delete` (CHK-001). Audited.
    ///
    /// # Errors
    /// `Forbidden` for anyone but a Superagent; `NotFound` for a name it has no check of.
    pub async fn tool_superagent_check_delete(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        superagent_only(caller)?;
        let asked: DeleteArgs = parse(args)?;
        let name = asked.name.trim();
        {
            let mut checks = self.superagent_checks.lock();
            let before = checks.len();
            checks.retain(|c| {
                !(c.user == caller.user.as_str()
                    && c.sandbox == caller.sandbox.as_str()
                    && c.name == name)
            });
            if checks.len() == before {
                return Err(AppError::NotFound);
            }
        }
        self.save_superagent_checks();
        self.audit_check(caller, "delete", name).await;
        Ok(json!({"deleted": name}))
    }

    /// Types the Superagent's first due check into its session, if it may take one now
    /// (CHK-003): called by its watch when its conversation is idle. Not in the first seconds
    /// after its start (its prompt box is not up yet), nor within [`CHECK_GAP_MS`] of the last
    /// check typed, nor while its person holds its prompt box (checked again with the terminal
    /// held, so no keystroke comes between). The next interval counts from now; a check that
    /// could not be typed is tried again a minute later.
    pub(crate) async fn type_due_check(&self, runtime: &ShellRuntime) {
        use std::sync::atomic::Ordering;
        if runtime.global {
            return;
        }
        let now = self.now().0;
        if now.saturating_sub(runtime.started_ms()) < CHECK_START_GRACE_MS
            || now.saturating_sub(runtime.check_typed_ms.load(Ordering::SeqCst)) < CHECK_GAP_MS
        {
            return;
        }
        let quiet = || {
            may_type_check(
                self.cockpit_has_draft(runtime.id()),
                runtime.last_key_ms.load(Ordering::SeqCst),
                self.now().0,
            )
        };
        if !quiet() {
            return;
        }
        let mine = |c: &Check| {
            c.user == runtime.creator().as_str() && c.sandbox == runtime.sandbox().as_str()
        };
        let is_due = {
            let mut checks = self.superagent_checks.lock();
            for check in checks.iter_mut().filter(|c| mine(c)) {
                // A clock set back does not push a check past one interval (F7).
                check.next_at = check.next_at.min(now.saturating_add(every_ms(check)));
            }
            checks.iter().any(|c| mine(c) && c.next_at <= now)
        };
        if !is_due {
            return;
        }
        let mut writer = runtime.writer.lock().await;
        if !quiet() {
            return;
        }
        let (due, before) = {
            let mut checks = self.superagent_checks.lock();
            let Some(check) = checks.iter_mut().find(|c| mine(c) && c.next_at <= now) else {
                return;
            };
            let before = check.last_run_at;
            check.last_run_at = Some(now);
            check.next_at = now.saturating_add(every_ms(check));
            (check.clone(), before)
        };
        self.save_superagent_checks();
        runtime.check_typed_ms.store(now, Ordering::SeqCst);
        let typed = async {
            let writer = writer
                .as_mut()
                .ok_or_else(|| AppError::Conflict("the Superagent has ended".into()))?;
            writer.input(&check_paste(&due)).await?;
            tokio::time::sleep(SUBMIT_DELAY).await;
            writer.input(b"\r").await
        }
        .await;
        drop(writer);
        match typed {
            Ok(()) => {
                tracing::info!(shell = %runtime.id(), check = %due.name, "a Superagent's check was typed")
            }
            Err(err) => {
                tracing::warn!(shell = %runtime.id(), check = %due.name, error = %err, "a Superagent's check was not typed: tried again in a minute");
                {
                    let mut checks = self.superagent_checks.lock();
                    if let Some(check) = checks.iter_mut().find(|c| mine(c) && c.name == due.name) {
                        check.last_run_at = before;
                        check.next_at = now.saturating_add(CHECK_RETRY_MS);
                    }
                }
                self.save_superagent_checks();
            }
        }
    }

    /// Starts the Superagent of each due check that is not running (CHK-004), as its panel
    /// would, with its person's rights now; returns how many were started. Called every
    /// minute; public for tests.
    pub async fn start_superagents_for_due_checks(self: &Arc<Self>) -> usize {
        let now = self.now().0;
        let mut wanted: Vec<(String, String)> = self
            .superagent_checks
            .lock()
            .iter()
            .filter(|c| c.next_at <= now)
            .map(|c| (c.user.clone(), c.sandbox.clone()))
            .collect();
        wanted.sort();
        wanted.dedup();
        let mut started = 0;
        let mut gone: Vec<(String, Option<String>)> = Vec::new();
        for (user, sandbox) in wanted {
            let key = (user.clone(), sandbox.clone());
            let waiting = self
                .superagent_checks
                .backoff
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&key)
                .copied();
            if waiting.is_some_and(|(next_try, _)| next_try > now) {
                continue;
            }
            let (Ok(user_id), Ok(sandbox_id)) = (UserId::parse(&user), SandboxId::parse(&sandbox))
            else {
                gone.push((user, Some(sandbox)));
                continue;
            };
            if self.running_cockpit(&user_id, &sandbox_id).is_some() {
                continue;
            }
            // A person or a sandbox that is gone takes its checks with it (F6).
            let person = match self.deps.store.user(&user_id).await {
                Ok(Some(person)) => person,
                Ok(None) => {
                    gone.push((user, None));
                    continue;
                }
                Err(_) => continue,
            };
            match self.deps.store.sandbox(&sandbox_id).await {
                Ok(Some(record)) if record.removed_at.is_none() => {}
                Ok(_) => {
                    gone.push((user, Some(sandbox)));
                    continue;
                }
                Err(_) => continue,
            }
            if person.disabled_at.is_some() {
                continue;
            }
            let ctx = AuthContext {
                user: person,
                csrf_token: None,
                session_hash: None,
                token: None,
            };
            let backoff = || {
                let mut map = self
                    .superagent_checks
                    .backoff
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let failed = map
                    .get(&key)
                    .map_or(0, |(_, failed)| *failed)
                    .saturating_add(1);
                let wait = 60_000_i64
                    .saturating_mul(1_i64 << failed.min(6))
                    .min(START_BACKOFF_MAX_MS);
                map.insert(key.clone(), (now.saturating_add(wait), failed));
                failed
            };
            match self.cockpit_open(&ctx, sandbox_id.as_str()).await {
                Ok(_) => {
                    self.superagent_checks
                        .backoff
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&key);
                    started += 1;
                }
                Err(err) => {
                    if backoff() == 1 {
                        tracing::info!(user = %user, sandbox = %sandbox, error = %err, "a Superagent was not started for its check: tried again later");
                    } else {
                        tracing::debug!(user = %user, sandbox = %sandbox, error = %err, "a Superagent was again not started for its check");
                    }
                }
            }
        }
        if !gone.is_empty() {
            self.superagent_checks.lock().retain(|c| {
                !gone.iter().any(|(user, sandbox)| {
                    c.user == *user && sandbox.as_ref().is_none_or(|s| c.sandbox == *s)
                })
            });
            self.save_superagent_checks();
        }
        started
    }
}

#[cfg(test)]
mod tests {
    use super::{QUIET_AFTER_TYPING_MS, may_type_check};

    /// SPEC-487 CHK-003: a check is typed only into a prompt box nobody holds: no draft, and no
    /// keystroke in the last minute.
    #[test]
    fn a_check_waits_for_a_quiet_prompt_box() {
        assert!(may_type_check(false, 0, 10));
        assert!(!may_type_check(true, 0, 10), "a draft");
        assert!(
            !may_type_check(false, 100, 100 + QUIET_AFTER_TYPING_MS - 1),
            "typing now"
        );
        assert!(may_type_check(false, 100, 100 + QUIET_AFTER_TYPING_MS));
    }
}
