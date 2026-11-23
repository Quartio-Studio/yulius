//! Notifications sent to a person's own chat channel (SPEC-498, ADR-0351): which task and
//! account changes are worth a message, its plain text, the per-person rate bound that folds a
//! burst into one summary, and the chat commands of linking. Nothing here names a channel.

use crate::harness::Harness;
use crate::task::{StopReason, TaskGroup, TaskStatus};

/// Characters of a link code (TGC-011): letters and digits only, so it fits a `t.me` start link.
pub const LINK_CODE_LEN: usize = 16;
/// How long a link code is valid (TGC-011).
pub const LINK_CODE_TTL_MS: i64 = 10 * 60 * 1000;
/// Longest message text sent (TGC-023); Telegram's limit is 4096.
pub const MAX_MESSAGE_CHARS: usize = 4000;
/// Messages per person per window (TGC-024).
pub const PER_WINDOW: usize = 5;
/// The rate bound's window (TGC-024).
pub const WINDOW_MS: i64 = 60_000;
/// Lines a summary keeps; the rest are counted (TGC-024).
pub const SUMMARY_LINES: usize = 20;

/// What a notification is about: the person's event choices (TGC-020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoticeKind {
    /// A task entered "Needs you", or asked again while there.
    NeedsYou,
    /// A task is Done.
    Done,
    /// A task failed to start or stopped abnormally.
    Failed,
    /// A subscription needs to sign in again, reached its limit or reports an error.
    Account,
}

impl NoticeKind {
    /// Its wire name (a stored notice's category, SPEC-624).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs_you",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Account => "account",
        }
    }

    /// The kind named `text`, if any.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "needs_you" => Some(Self::NeedsYou),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "account" => Some(Self::Account),
            _ => None,
        }
    }
}

/// Which notifications a person wants (TGC-020): Needs you by default, nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct NotifyChoices {
    /// Tasks that need them.
    pub needs_you: bool,
    /// Tasks that are Done.
    pub done: bool,
    /// Tasks that failed or stopped abnormally.
    pub failed: bool,
    /// Subscription problems (instance administrators only see these events).
    pub account: bool,
    /// The end-of-day digest, when the instance sends one (SPEC-512 DIG-005).
    pub digest: bool,
}

impl Default for NotifyChoices {
    fn default() -> Self {
        Self {
            needs_you: true,
            done: false,
            failed: false,
            account: false,
            digest: true,
        }
    }
}

impl NotifyChoices {
    /// Whether the person wants `kind`.
    #[must_use]
    pub fn wants(&self, kind: NoticeKind) -> bool {
        match kind {
            NoticeKind::NeedsYou => self.needs_you,
            NoticeKind::Done => self.done,
            NoticeKind::Failed => self.failed,
            NoticeKind::Account => self.account,
        }
    }
}

/// The notification a task's state calls for (TGC-022): its sidebar group, or an abnormal end.
/// A person's own stop (`stopped_cancelled`) and a normal close call for none.
#[must_use]
pub fn task_notice(group: TaskGroup, status: TaskStatus) -> Option<NoticeKind> {
    match status {
        TaskStatus::Failed(_) => Some(NoticeKind::Failed),
        TaskStatus::Stopped(reason) if reason != StopReason::Cancelled => Some(NoticeKind::Failed),
        _ => match group {
            TaskGroup::NeedsYou => Some(NoticeKind::NeedsYou),
            TaskGroup::Done => Some(NoticeKind::Done),
            _ => None,
        },
    }
}

/// What makes a task's notification new (TGC-022): its kind, its status for an end, and the time
/// of its `needs_you` request, so a new request while it already needs you is told again.
#[must_use]
pub fn task_signature(kind: NoticeKind, status: TaskStatus, needs_you_at: Option<&str>) -> String {
    match kind {
        NoticeKind::NeedsYou => format!("needs_you:{}", needs_you_at.unwrap_or("")),
        NoticeKind::Done => "done".into(),
        NoticeKind::Failed => format!("failed:{}", status.as_str()),
        NoticeKind::Account => "account".into(),
    }
}

/// The words for a failed or abnormally stopped task, as the web UI's `status.js` says them
/// (`STOP_LABELS`). Matched on the status itself, so a new one has words before it compiles
/// (TGC-028); a status that is no end reads "Stopped".
#[must_use]
pub fn ended_label(status: TaskStatus) -> &'static str {
    use crate::task::FailReason;
    match status {
        TaskStatus::Stopped(reason) => match reason {
            StopReason::Cancelled => "Stopped",
            StopReason::Crashed => "Stopped (crashed)",
            StopReason::Killed => "Stopped (process ended)",
            StopReason::Lost => "Stopped (process lost)",
            StopReason::SwitchFailed => "Stopped (switch failed)",
            StopReason::SwitchTimeout => "Stopped (switch timed out)",
            StopReason::ServerLost => "Stopped (server restarted)",
        },
        TaskStatus::Failed(reason) => match reason {
            FailReason::SpawnError => "Failed to start",
            FailReason::Precondition => "Failed (precondition)",
            FailReason::NoAccount => "Failed (no Claude account)",
        },
        _ => "Stopped",
    }
}

/// What the agent or Yulius asked a person (`needs_you`, SPEC-134, SPEC-578).
#[derive(Debug, Clone, Copy)]
pub struct AskedOf<'a> {
    /// What happened (optional; shown before the action when given).
    pub happened: Option<&'a str>,
    /// What to do, or decide.
    pub action: &'a str,
    /// What is wrong because of it.
    pub problem: Option<&'a str>,
    /// The person chooses rather than does.
    pub decision: bool,
}

/// The words for a problem of a whole sandbox that a `needs_you` request names (its `cause`,
/// SPEC-494), as the notification center says them; `None` for a task's own request. Such
/// requests are told once per episode of the sandbox, not once per task (TGC-026).
#[must_use]
pub fn shared_cause(cause: &str) -> Option<&'static str> {
    match cause {
        "docker" => Some("its Docker needs you"),
        _ => None,
    }
}

/// One task's notification, before redaction.
#[derive(Debug, Clone, Copy)]
pub struct TaskNotice<'a> {
    /// Its kind.
    pub kind: NoticeKind,
    /// The task's status (for an end's words).
    pub status: TaskStatus,
    /// The task's agent: an end for want of an account names its provider's (SPEC-503).
    pub harness: Harness,
    /// The sandbox's name.
    pub sandbox: &'a str,
    /// The task's number in it.
    pub number: u32,
    /// The task's title.
    pub title: &'a str,
    /// The open `needs_you` request, if any.
    pub asked: Option<AskedOf<'a>>,
    /// The words of the sandbox-wide problem the request names ([`shared_cause`]): the message
    /// speaks of the sandbox, naming this task as one that asked.
    pub shared: Option<&'a str>,
    /// `{PUBLIC_URL}/s/{slug}/t/{n}`.
    pub link: &'a str,
}

impl TaskNotice<'_> {
    fn state(&self) -> &'static str {
        match self.kind {
            NoticeKind::NeedsYou => {
                if self.asked.is_some_and(|a| a.decision) {
                    "Needs your decision"
                } else {
                    "Needs you"
                }
            }
            NoticeKind::Done => "Done",
            // A Codex task that found no account names Codex's; the rest read as `status.js`'s.
            NoticeKind::Failed => match (self.status, self.harness) {
                (TaskStatus::Failed(crate::task::FailReason::NoAccount), Harness::Codex) => {
                    "Failed (no Codex account)"
                }
                _ => ended_label(self.status),
            },
            NoticeKind::Account => "Subscription",
        }
    }

    /// One line for a summary: "Needs you · #12 Title (sandbox)", or for a sandbox-wide
    /// problem "Needs you · sandbox: its Docker needs you".
    #[must_use]
    pub fn headline(&self) -> String {
        if let Some(shared) = self.shared {
            return format!("{} · {}: {shared}", self.state(), one_line(self.sandbox));
        }
        format!(
            "{} · #{} {} ({})",
            self.state(),
            self.number,
            one_line(self.title),
            one_line(self.sandbox)
        )
    }

    /// The message: state and sandbox, the task, what was asked, the link.
    #[must_use]
    pub fn text(&self) -> String {
        let mut lines = match self.shared {
            Some(_) => vec![
                self.headline(),
                format!(
                    "Asked by #{} {} and any other task of it that shares the problem",
                    self.number,
                    one_line(self.title)
                ),
            ],
            None => vec![
                format!("{} · {}", self.state(), one_line(self.sandbox)),
                format!("#{} {}", self.number, one_line(self.title)),
            ],
        };
        if let Some(asked) = self.asked
            && self.kind == NoticeKind::NeedsYou
        {
            if let Some(happened) = asked.happened.filter(|h| !h.trim().is_empty()) {
                lines.push(format!("What happened: {}", plain(happened)));
            }
            let verb = if asked.decision { "Decide" } else { "To do" };
            lines.push(format!("{verb}: {}", plain(asked.action)));
            if let Some(problem) = asked.problem.filter(|p| !p.trim().is_empty()) {
                lines.push(format!("Problem: {}", plain(problem)));
            }
        }
        lines.push(self.link.to_owned());
        truncate(&lines.join("\n"))
    }
}

/// The words for a subscription's problem status (`relogin`, `limited`, `error`), as the
/// notification center says them; `None` for any other status.
#[must_use]
pub fn account_problem(status: &str) -> Option<&'static str> {
    match status {
        "relogin" => Some("needs to sign in again"),
        "limited" => Some("reached its usage limit"),
        "error" => Some("reports an error"),
        _ => None,
    }
}

/// A posted notice (SPEC-624): (headline, text), before redaction: its project, its title, its
/// detail and its link.
#[must_use]
pub fn posted_text(
    project: Option<&str>,
    title: &str,
    detail: Option<&str>,
    link: Option<&str>,
) -> (String, String) {
    let headline = match project {
        Some(project) => format!("{}: {}", one_line(project), one_line(title)),
        None => one_line(title),
    };
    let mut lines = vec![headline.clone()];
    if let Some(detail) = detail.filter(|d| !d.trim().is_empty()) {
        lines.push(plain(detail));
    }
    if let Some(link) = link {
        lines.push(link.to_owned());
    }
    (headline, truncate(&lines.join("\n")))
}

/// A request for control of a task, for its owner: (headline, text), before redaction (TGC-027),
/// in the notification center's words.
#[must_use]
pub fn control_text(
    by: &str,
    number: u32,
    title: &str,
    sandbox: &str,
    link: &str,
) -> (String, String) {
    let headline = format!(
        "@{} asks for control of #{number} {}",
        one_line(by),
        one_line(title)
    );
    let text = [
        headline.clone(),
        format!("It would act on GitHub as them · {}", one_line(sandbox)),
        link.to_owned(),
    ]
    .join("\n");
    (headline, truncate(&text))
}

/// A take of a person's task, for them (SPEC-510 TKC-002): `back` when its giver took it back
/// (SPEC-490 OWN-010). (headline, text) before redaction, in the notification center's words.
#[must_use]
pub fn taken_text(
    by: &str,
    number: u32,
    title: &str,
    sandbox: &str,
    link: &str,
    back: bool,
) -> (String, String) {
    let (verb, then) = if back {
        ("took back control", "It acts on GitHub as them again")
    } else {
        (
            "took control",
            "It acts on GitHub as them now; you can take it back",
        )
    };
    let headline = format!("@{} {verb} of #{number} {}", one_line(by), one_line(title));
    let text = [
        headline.clone(),
        format!("{then} · {}", one_line(sandbox)),
        link.to_owned(),
    ]
    .join("\n");
    (headline, truncate(&text))
}

/// Backticks around code are dropped from plain text (SPEC-327).
fn plain(text: &str) -> String {
    text.replace('`', "")
}

fn one_line(text: &str) -> String {
    plain(text).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` cut to [`MAX_MESSAGE_CHARS`] characters, ending with "…" when cut.
#[must_use]
pub fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_MESSAGE_CHARS {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(MAX_MESSAGE_CHARS - 1).collect();
    out.push('…');
    out
}

/// One person's rate bound (TGC-024): at most [`PER_WINDOW`] messages per window ([`WINDOW_MS`]
/// by default); what
/// comes beyond is held and sent as one summary when the window ends.
#[derive(Debug, Clone)]
pub struct Throttle {
    window: i64,
    start: i64,
    sent: usize,
    held: Vec<String>,
    more: usize,
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new(WINDOW_MS)
    }
}

impl Throttle {
    /// A bound over windows of `window` milliseconds (at least 1).
    #[must_use]
    pub fn new(window: i64) -> Self {
        Self {
            window: window.max(1),
            start: i64::MIN / 2,
            sent: 0,
            held: Vec::new(),
            more: 0,
        }
    }

    /// Whether a notification with `headline` is sent now (`true`) or held for the summary.
    pub fn offer(&mut self, now: i64, headline: String) -> bool {
        if self.held.is_empty() && now.saturating_sub(self.start) >= self.window {
            self.start = now;
            self.sent = 0;
        }
        if self.held.is_empty() && self.sent < PER_WINDOW {
            self.sent += 1;
            return true;
        }
        if self.held.len() < SUMMARY_LINES {
            self.held.push(headline);
        } else {
            self.more += 1;
        }
        false
    }

    /// The summary of what was held, once the window ended (it counts as the next window's
    /// first message); `None` before that or when nothing was held.
    pub fn flush(&mut self, now: i64, link: &str) -> Option<String> {
        if self.held.is_empty() || now.saturating_sub(self.start) < self.window {
            return None;
        }
        let count = self.held.len() + self.more;
        let mut lines = vec![format!(
            "{count} more notification{} in the last minute:",
            if count == 1 { "" } else { "s" }
        )];
        lines.extend(self.held.drain(..).map(|line| format!("• {line}")));
        if self.more > 0 {
            lines.push(format!("• and {} more", self.more));
        }
        lines.push(link.to_owned());
        self.more = 0;
        self.start = now;
        self.sent = 1;
        Some(truncate(&lines.join("\n")))
    }

    /// Nothing held and the window over: the bound can be forgotten.
    #[must_use]
    pub fn idle(&self, now: i64) -> bool {
        self.held.is_empty() && now.saturating_sub(self.start) >= self.window
    }
}

/// A chat message as a linking command (TGC-012, TGC-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatCommand<'a> {
    /// `/start` with its argument, if any (`/start@bot code` too).
    Start(Option<&'a str>),
    /// `/stop`.
    Stop,
    /// Anything else.
    Other,
}

/// Reads a chat message's command.
#[must_use]
pub fn chat_command(text: &str) -> ChatCommand<'_> {
    let text = text.trim();
    let (head, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let name = head.split_once('@').map_or(head, |(name, _)| name);
    match name {
        "/start" => {
            let arg = rest.trim();
            ChatCommand::Start((!arg.is_empty()).then_some(arg))
        }
        "/stop" => ChatCommand::Stop,
        _ => ChatCommand::Other,
    }
}

/// What a linked chat's message asks with intake on (SPEC-512 TGI-003, TGI-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeCommand<'a> {
    /// `/status`: the person's tasks that wait for them.
    Status,
    /// `/help`, or a command this chat does not know.
    Help,
    /// `/task <project> <text>`: a task in that project (`text` may be empty when files come
    /// with it).
    Task {
        /// The project's repository name or slug.
        project: &'a str,
        /// The task's text.
        text: &'a str,
    },
    /// `/task` without a project.
    TaskWithoutProject,
    /// Anything else: a task in the default project.
    Text,
}

/// Reads a message's intake command (`/status@bot` too).
#[must_use]
pub fn intake_command(text: &str) -> IntakeCommand<'_> {
    let text = text.trim();
    if !text.starts_with('/') {
        return IntakeCommand::Text;
    }
    let (head, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let name = head.split_once('@').map_or(head, |(name, _)| name);
    match name {
        "/status" => IntakeCommand::Status,
        "/task" => {
            let rest = rest.trim();
            let (project, body) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            if project.is_empty() {
                IntakeCommand::TaskWithoutProject
            } else {
                IntakeCommand::Task {
                    project,
                    text: body.trim(),
                }
            }
        }
        _ => IntakeCommand::Help,
    }
}

/// Whether `code` has a link code's shape (TGC-011).
#[must_use]
pub fn is_link_code(code: &str) -> bool {
    code.len() == LINK_CODE_LEN && code.bytes().all(|b| b.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::FailReason;

    #[test]
    fn task_states_call_for_their_notice() {
        let running = TaskStatus::Running;
        assert_eq!(
            task_notice(TaskGroup::NeedsYou, running),
            Some(NoticeKind::NeedsYou)
        );
        assert_eq!(
            task_notice(TaskGroup::Done, running),
            Some(NoticeKind::Done)
        );
        assert_eq!(task_notice(TaskGroup::Working, running), None);
        assert_eq!(task_notice(TaskGroup::Queued, TaskStatus::Queued), None);
        let crashed = TaskStatus::Stopped(StopReason::Crashed);
        assert_eq!(
            task_notice(TaskGroup::Ended, crashed),
            Some(NoticeKind::Failed)
        );
        let failed = TaskStatus::Failed(FailReason::SpawnError);
        assert_eq!(
            task_notice(TaskGroup::Ended, failed),
            Some(NoticeKind::Failed)
        );
        // A person's own stop and a normal close are not failures.
        let cancelled = TaskStatus::Stopped(StopReason::Cancelled);
        assert_eq!(task_notice(TaskGroup::Ended, cancelled), None);
        assert_eq!(task_notice(TaskGroup::Ended, TaskStatus::Closed), None);
    }

    #[test]
    fn a_new_needs_you_request_is_a_new_signature() {
        let first = task_signature(NoticeKind::NeedsYou, TaskStatus::Running, Some("t1"));
        let again = task_signature(NoticeKind::NeedsYou, TaskStatus::Running, Some("t1"));
        let second = task_signature(NoticeKind::NeedsYou, TaskStatus::Running, Some("t2"));
        assert_eq!(first, again);
        assert_ne!(first, second);
        assert_ne!(
            task_signature(NoticeKind::Done, TaskStatus::Running, None),
            first
        );
    }

    #[test]
    fn choices_default_to_needs_you_only() {
        let choices = NotifyChoices::default();
        assert!(choices.wants(NoticeKind::NeedsYou));
        assert!(!choices.wants(NoticeKind::Done));
        assert!(!choices.wants(NoticeKind::Failed));
        assert!(!choices.wants(NoticeKind::Account));
    }

    #[test]
    fn a_needs_you_message_carries_the_request_and_the_link() {
        let notice = TaskNotice {
            kind: NoticeKind::NeedsYou,
            status: TaskStatus::Running,
            harness: Harness::ClaudeCode,
            sandbox: "Shop",
            number: 12,
            title: "Fix  the\ncheckout",
            asked: Some(AskedOf {
                happened: None,
                action: "Add the `STRIPE` key",
                problem: Some("Payments fail"),
                decision: false,
            }),
            shared: None,
            link: "https://y.example/s/shop/t/12",
        };
        assert_eq!(
            notice.text(),
            "Needs you · Shop\n#12 Fix the checkout\nTo do: Add the STRIPE key\nProblem: Payments fail\nhttps://y.example/s/shop/t/12"
        );
        assert_eq!(notice.headline(), "Needs you · #12 Fix the checkout (Shop)");
        let decide = TaskNotice {
            asked: Some(AskedOf {
                happened: None,
                action: "Pick a plan",
                problem: None,
                decision: true,
            }),
            ..notice
        };
        assert!(decide.text().starts_with("Needs your decision · Shop"));
        assert!(decide.text().contains("\nDecide: Pick a plan\n"));
    }

    #[test]
    fn an_ended_task_says_how_it_ended() {
        let notice = TaskNotice {
            kind: NoticeKind::Failed,
            status: TaskStatus::Stopped(StopReason::Crashed),
            harness: Harness::ClaudeCode,
            sandbox: "Shop",
            number: 3,
            title: "T",
            asked: None,
            shared: None,
            link: "L",
        };
        assert_eq!(notice.text(), "Stopped (crashed) · Shop\n#3 T\nL");
        // A task that found no account names its own agent's (SPEC-503 CDX-013); a Claude one
        // reads as before.
        let no_account = TaskNotice {
            status: TaskStatus::Failed(FailReason::NoAccount),
            ..notice
        };
        assert_eq!(
            no_account.headline(),
            "Failed (no Claude account) · #3 T (Shop)"
        );
        let codex = TaskNotice {
            harness: Harness::Codex,
            ..no_account
        };
        assert_eq!(codex.headline(), "Failed (no Codex account) · #3 T (Shop)");
    }

    /// TGC-028: every end has the words of `status.js`'s `STOP_LABELS`.
    #[test]
    fn every_end_has_the_web_uis_words() {
        use crate::task::FailReason;
        let ends = [
            (
                TaskStatus::Stopped(StopReason::Crashed),
                "stopped_crashed",
                "Stopped (crashed)",
            ),
            (
                TaskStatus::Stopped(StopReason::Killed),
                "stopped_killed",
                "Stopped (process ended)",
            ),
            (
                TaskStatus::Stopped(StopReason::Lost),
                "stopped_lost",
                "Stopped (process lost)",
            ),
            (
                TaskStatus::Stopped(StopReason::SwitchFailed),
                "stopped_switch_failed",
                "Stopped (switch failed)",
            ),
            (
                TaskStatus::Stopped(StopReason::SwitchTimeout),
                "stopped_switch_timeout",
                "Stopped (switch timed out)",
            ),
            (
                TaskStatus::Stopped(StopReason::ServerLost),
                "stopped_server_lost",
                "Stopped (server restarted)",
            ),
            (
                TaskStatus::Failed(FailReason::SpawnError),
                "failed_spawn_error",
                "Failed to start",
            ),
            (
                TaskStatus::Failed(FailReason::Precondition),
                "failed_precondition",
                "Failed (precondition)",
            ),
            (
                TaskStatus::Failed(FailReason::NoAccount),
                "failed_no_account",
                "Failed (no Claude account)",
            ),
        ];
        let web = include_str!("../../yulius-server/web/assets/status.js");
        for (status, name, words) in ends {
            assert_eq!(status.as_str(), name);
            assert_eq!(ended_label(status), words, "{name}");
            assert!(
                web.contains(&format!("  {name}: '{words}',")),
                "status.js: {name}"
            );
        }
        // And every end `status.js` words has the same words here (review round 1: the list
        // above is written by hand).
        let labels = web
            .lines()
            .filter_map(|line| line.trim().strip_suffix("',"))
            .filter_map(|line| line.split_once(": '"))
            .filter(|(name, _)| name.starts_with("stopped_") || name.starts_with("failed_"));
        let mut seen = 0;
        for (name, words) in labels {
            let status = TaskStatus::parse(name).unwrap_or(TaskStatus::Queued);
            assert_eq!(status.as_str(), name, "status.js names an unknown end");
            if name != "stopped_cancelled" {
                assert_eq!(ended_label(status), words, "{name}");
            }
            seen += 1;
        }
        assert_eq!(
            seen,
            ends.len() + 1,
            "status.js's ends (with a person's stop)"
        );
    }

    #[test]
    fn a_control_request_names_who_asks() {
        let taken = taken_text("bob", 7, "Fix\nit", "Shop", "L", false);
        assert_eq!(
            taken.1,
            "@bob took control of #7 Fix it\nIt acts on GitHub as them now; you can take it back · \
             Shop\nL"
        );
        let back = taken_text("bob", 7, "Fix", "Shop", "L", true);
        assert_eq!(
            back.1,
            "@bob took back control of #7 Fix\nIt acts on GitHub as them again · Shop\nL"
        );
        let (headline, text) = control_text("bob", 7, "Fix\nit", "Shop", "L");
        assert_eq!(headline, "@bob asks for control of #7 Fix it");
        assert_eq!(
            text,
            "@bob asks for control of #7 Fix it\nIt would act on GitHub as them · Shop\nL"
        );
    }

    #[test]
    fn a_sandbox_wide_problem_speaks_of_the_sandbox() {
        assert_eq!(shared_cause("docker"), Some("its Docker needs you"));
        assert_eq!(shared_cause("disk"), None);
        let notice = TaskNotice {
            kind: NoticeKind::NeedsYou,
            status: TaskStatus::Running,
            harness: Harness::ClaudeCode,
            sandbox: "Shop",
            number: 4,
            title: "Build",
            asked: Some(AskedOf {
                happened: None,
                action: "Restart Docker",
                problem: None,
                decision: false,
            }),
            shared: shared_cause("docker"),
            link: "L",
        };
        assert_eq!(notice.headline(), "Needs you · Shop: its Docker needs you");
        assert_eq!(
            notice.text(),
            "Needs you · Shop: its Docker needs you\nAsked by #4 Build and any other task of it that shares the problem\nTo do: Restart Docker\nL"
        );
    }

    #[test]
    fn a_posted_message_names_its_problem() {
        let (headline, text) =
            posted_text(None, "Team Max needs to sign in again", None, Some("L"));
        assert_eq!(headline, "Team Max needs to sign in again");
        assert_eq!(text, "Team Max needs to sign in again\nL");
        assert_eq!(account_problem("limited"), Some("reached its usage limit"));
        assert_eq!(account_problem("ok"), None);
    }

    #[test]
    fn long_text_is_cut() {
        let text = "x".repeat(MAX_MESSAGE_CHARS + 50);
        let cut = truncate(&text);
        assert_eq!(cut.chars().count(), MAX_MESSAGE_CHARS);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn a_burst_is_folded_into_one_summary() {
        let mut throttle = Throttle::default();
        let now = 1_000_000;
        let sent: Vec<bool> = (0..7)
            .map(|n| throttle.offer(now + n, format!("line {n}")))
            .collect();
        assert_eq!(sent, [true, true, true, true, true, false, false]);
        // Before the window ends nothing is flushed, and later offers are held behind it.
        assert_eq!(throttle.flush(now + 30_000, "L"), None);
        assert!(!throttle.offer(now + WINDOW_MS + 1, "line 7".into()));
        let summary = throttle.flush(now + WINDOW_MS + 2, "L");
        assert_eq!(
            summary.as_deref(),
            Some("3 more notifications in the last minute:\n• line 5\n• line 6\n• line 7\nL")
        );
        // The summary was the new window's first message: four more go out.
        let next: Vec<bool> = (0..5)
            .map(|n| throttle.offer(now + WINDOW_MS + 10 + n, String::new()))
            .collect();
        assert_eq!(next, [true, true, true, true, false]);
        assert!(!throttle.idle(now + WINDOW_MS + 20));
    }

    #[test]
    fn a_summary_keeps_its_lines_bounded() {
        let mut throttle = Throttle::default();
        for n in 0..(PER_WINDOW + SUMMARY_LINES + 3) {
            throttle.offer(0, format!("n{n}"));
        }
        let summary = throttle.flush(WINDOW_MS, "L").unwrap_or_default();
        assert!(summary.starts_with(&format!("{} more notifications", SUMMARY_LINES + 3)));
        assert!(summary.contains("• and 3 more\nL"));
        assert_eq!(summary.lines().count(), SUMMARY_LINES + 3);
        assert!(throttle.idle(2 * WINDOW_MS));
    }

    #[test]
    fn chat_commands_are_read() {
        assert_eq!(chat_command("/start abc"), ChatCommand::Start(Some("abc")));
        assert_eq!(
            chat_command(" /start@yulius_bot  abc "),
            ChatCommand::Start(Some("abc"))
        );
        assert_eq!(chat_command("/start"), ChatCommand::Start(None));
        assert_eq!(chat_command("/stop"), ChatCommand::Stop);
        assert_eq!(chat_command("/stop@yulius_bot"), ChatCommand::Stop);
        assert_eq!(chat_command("hello /stop"), ChatCommand::Other);
        assert_eq!(chat_command("/stopping"), ChatCommand::Other);
    }

    #[test]
    fn link_codes_have_one_shape() {
        assert!(is_link_code("abcDEF0123456789"));
        assert!(!is_link_code("abcDEF012345678"));
        assert!(!is_link_code("abcDEF012345678_"));
        assert!(!is_link_code("abcDEF0123456789x"));
    }

    /// SPEC-512 TGI-003, TGI-004: the intake's commands.
    #[test]
    fn intake_commands_are_read() {
        assert_eq!(intake_command("Fix the login page"), IntakeCommand::Text);
        assert_eq!(intake_command(" /status "), IntakeCommand::Status);
        assert_eq!(intake_command("/status@yulius_bot"), IntakeCommand::Status);
        assert_eq!(
            intake_command("/task website  Make the header blue\nand bold"),
            IntakeCommand::Task {
                project: "website",
                text: "Make the header blue\nand bold"
            }
        );
        assert_eq!(
            intake_command("/task website"),
            IntakeCommand::Task {
                project: "website",
                text: ""
            }
        );
        assert_eq!(intake_command("/task"), IntakeCommand::TaskWithoutProject);
        assert_eq!(intake_command("/help"), IntakeCommand::Help);
        assert_eq!(intake_command("/unknown x"), IntakeCommand::Help);
    }
}
