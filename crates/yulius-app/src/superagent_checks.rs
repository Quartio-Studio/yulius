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

    fn save_superagent_checks(&self) {
        let checks = self.superagent_checks.lock().clone();
        let path = self.checks_path();
        let tmp = path.with_extension("json.tmp");
        let written = serde_json::to_vec(&checks)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&tmp, bytes))
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
            Ok(checks) => *self.superagent_checks.lock() = checks,
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
        if name.is_empty() || name.chars().count() > 60 || name.contains(['\n', '\r']) {
            return Err(AppError::Invalid(
                "the name is 1 to 60 characters on one line".into(),
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
    /// (CHK-003): called by its watch when its conversation is idle. The next interval counts
    /// from now, typed or not reachable, so a check never piles up.
    pub(crate) async fn type_due_check(&self, runtime: &ShellRuntime) {
        if runtime.global {
            return;
        }
        let now = self.now().0;
        if !may_type_check(
            self.cockpit_has_draft(runtime.id()),
            runtime
                .last_key_ms
                .load(std::sync::atomic::Ordering::SeqCst),
            now,
        ) {
            return;
        }
        let due = {
            let mut checks = self.superagent_checks.lock();
            let Some(check) = checks.iter_mut().find(|c| {
                c.user == runtime.creator().as_str()
                    && c.sandbox == runtime.sandbox().as_str()
                    && c.next_at <= now
            }) else {
                return;
            };
            check.last_run_at = Some(now);
            check.next_at = now.saturating_add(i64::from(check.every_minutes) * 60_000);
            check.clone()
        };
        self.save_superagent_checks();
        let typed = async {
            let mut writer = runtime.writer.lock().await;
            let writer = writer
                .as_mut()
                .ok_or_else(|| AppError::Conflict("the Superagent has ended".into()))?;
            writer.input(&check_paste(&due)).await?;
            tokio::time::sleep(SUBMIT_DELAY).await;
            writer.input(b"\r").await
        }
        .await;
        match typed {
            Ok(()) => {
                tracing::info!(shell = %runtime.id(), check = %due.name, "a Superagent's check was typed")
            }
            Err(err) => {
                tracing::warn!(shell = %runtime.id(), check = %due.name, error = %err, "a Superagent's check was not typed")
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
        for (user, sandbox) in wanted {
            let (Ok(user_id), Ok(sandbox_id)) = (UserId::parse(&user), SandboxId::parse(&sandbox))
            else {
                continue;
            };
            if self.running_cockpit(&user_id, &sandbox_id).is_some() {
                continue;
            }
            let Ok(Some(person)) = self.deps.store.user(&user_id).await else {
                continue;
            };
            if person.disabled_at.is_some() {
                continue;
            }
            let ctx = AuthContext {
                user: person,
                csrf_token: None,
                session_hash: None,
                token: None,
            };
            match self.cockpit_open(&ctx, sandbox_id.as_str()).await {
                Ok(_) => started += 1,
                Err(err) => {
                    tracing::info!(user = %user, sandbox = %sandbox, error = %err, "a Superagent was not started for its check")
                }
            }
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
