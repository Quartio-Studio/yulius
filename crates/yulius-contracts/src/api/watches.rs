//! Script watches (SPEC-630, ADR-0465): a project's bash script on a schedule, run by Yulius in
//! the sandbox without an agent, whose output becomes a notice keyed `watch:<name>` for its owner.

use serde::{Deserialize, Serialize};

/// A watch as people and the Superagent write it (WCH-001). `name` is given only on creation: it
/// is the key of the watch's notices and never changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchInput {
    /// 1 to 40 of `a-z`, `0-9` and `-`, unique in the project; refused on an edit when it differs.
    #[serde(default)]
    pub name: Option<String>,
    /// The bash script (at most 16 KiB).
    pub script: String,
    /// An interval: `15m`, `2h`, `1d` (1 minute to 7 days); exactly one of `every` and `cron`.
    #[serde(default)]
    pub every: Option<String>,
    /// A five-field cron expression.
    #[serde(default)]
    pub cron: Option<String>,
    /// The cron expression's UTC offset in minutes.
    #[serde(default)]
    pub utc_offset_minutes: i32,
    /// Its notices' expiry: `never` or a duration (`2h`, `3d`); none: a day.
    #[serde(default)]
    pub expiry: Option<String>,
    /// A run's timeout in seconds (1 to 3600); none: 300.
    #[serde(default)]
    pub timeout_seconds: Option<u32>,
    /// Agent secret names its runs get through the egress proxy (at most 20).
    #[serde(default)]
    pub secrets: Vec<String>,
}

/// One watch with its state (WCH-011).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchView {
    /// Its id.
    pub id: String,
    /// Its project.
    pub sandbox_id: String,
    /// Its name (its notices are keyed `watch:<name>`).
    pub name: String,
    /// The bash script.
    pub script: String,
    /// The interval as `15m`, if it runs on one.
    pub every: Option<String>,
    /// The cron expression, if it runs on one.
    pub cron: Option<String>,
    /// The cron expression's UTC offset in minutes.
    pub utc_offset_minutes: i32,
    /// Its notices' expiry as given.
    pub expiry: Option<String>,
    /// A run's timeout in seconds.
    pub timeout_seconds: u32,
    /// The agent secrets it names.
    pub secrets: Vec<String>,
    /// Its owner's user id.
    pub owner_id: String,
    /// Its owner's login.
    pub owner: String,
    /// `ui` or `agent`.
    pub created_via: String,
    /// When it was created.
    pub created_at: String,
    /// `active`, `running`, `paused` or `ended`.
    pub state: String,
    /// Why it ended (exit 3).
    pub ended_reason: Option<String>,
    /// When it runs next, if it is active.
    pub next_run_at: Option<String>,
    /// When its last run started.
    pub last_started_at: Option<String>,
    /// When its last run ended.
    pub last_ended_at: Option<String>,
    /// Its last run's exit code.
    pub last_exit: Option<i32>,
    /// Why its last run failed.
    pub last_failure: Option<String>,
    /// What its last run printed (redacted, cut).
    pub last_output: Option<String>,
    /// The tail of its last run's error output (redacted).
    pub last_errors: Option<String>,
    /// Failures in a row.
    pub failures: u32,
    /// Runs so far.
    pub runs: u32,
    /// Posts so far.
    pub posts: u32,
    /// Posts not stored yet (they are retried).
    pub pending_posts: u32,
    /// The caller may edit, run, pause and resume it (its owner).
    pub can_edit: bool,
    /// The caller may remove it (its owner or a project administrator).
    pub can_remove: bool,
}
