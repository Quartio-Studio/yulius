//! Notifications sent to a person's own chat channel (SPEC-444, ADR-0287): which task and
//! account changes are worth a message, its plain text, the per-person rate bound that folds a
//! burst into one summary, and the chat commands of linking. Nothing here names a channel.

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
}

impl Default for NotifyChoices {
    fn default() -> Self {
        Self {
            needs_you: true,
            done: false,
            failed: false,
            account: false,
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

/// The words for a failed or abnormally stopped task, as the web UI's `status.js` says them.
#[must_use]
pub fn ended_label(status: TaskStatus) -> &'static str {
    match status.as_str() {
        "stopped_crashed" => "Stopped (crashed)",
        "stopped_killed" => "Stopped (process ended)",
        "stopped_lost" => "Stopped (process lost)",
        "stopped_switch_failed" => "Stopped (switch failed)",
        "stopped_switch_timeout" => "Stopped (switch timed out)",
        "stopped_server_lost" => "Stopped (server restarted)",
        "failed_spawn_error" => "Failed to start",
        "failed_start_timeout" => "Failed (start timeout)",
        "failed_precondition" => "Failed (precondition)",
        "failed_no_account" => "Failed (no Claude account)",
        _ => "Stopped",
    }
}

/// What the agent asked a person (`needs_you`, SPEC-134).
#[derive(Debug, Clone, Copy)]
pub struct AskedOf<'a> {
    /// What to do, or decide.
    pub action: &'a str,
    /// What is wrong because of it.
    pub problem: Option<&'a str>,
    /// The person chooses rather than does.
    pub decision: bool,
}

/// One task's notification, before redaction.
#[derive(Debug, Clone, Copy)]
pub struct TaskNotice<'a> {
    /// Its kind.
    pub kind: NoticeKind,
    /// The task's status (for an end's words).
    pub status: TaskStatus,
    /// The sandbox's name.
    pub sandbox: &'a str,
    /// The task's number in it.
    pub number: u32,
    /// The task's title.
    pub title: &'a str,
    /// The open `needs_you` request, if any.
    pub asked: Option<AskedOf<'a>>,
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
            NoticeKind::Failed => ended_label(self.status),
            NoticeKind::Account => "Subscription",
        }
    }

    /// One line for a summary: "Needs you · #12 Title (sandbox)".
    #[must_use]
    pub fn headline(&self) -> String {
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
        let mut lines = vec![
            format!("{} · {}", self.state(), one_line(self.sandbox)),
            format!("#{} {}", self.number, one_line(self.title)),
        ];
        if let Some(asked) = self.asked
            && self.kind == NoticeKind::NeedsYou
        {
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

/// A subscription's notification: (headline, text), before redaction.
#[must_use]
pub fn account_text(
    label: &str,
    problem: &str,
    detail: Option<&str>,
    link: &str,
) -> (String, String) {
    let headline = format!("{} {problem}", one_line(label));
    let mut lines = vec![headline.clone()];
    if let Some(detail) = detail.filter(|d| !d.trim().is_empty()) {
        lines.push(plain(detail));
    }
    lines.push(link.to_owned());
    (headline, truncate(&lines.join("\n")))
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
        let failed = TaskStatus::Failed(FailReason::StartTimeout);
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
            sandbox: "Shop",
            number: 12,
            title: "Fix  the\ncheckout",
            asked: Some(AskedOf {
                action: "Add the `STRIPE` key",
                problem: Some("Payments fail"),
                decision: false,
            }),
            link: "https://y.example/s/shop/t/12",
        };
        assert_eq!(
            notice.text(),
            "Needs you · Shop\n#12 Fix the checkout\nTo do: Add the STRIPE key\nProblem: Payments fail\nhttps://y.example/s/shop/t/12"
        );
        assert_eq!(notice.headline(), "Needs you · #12 Fix the checkout (Shop)");
        let decide = TaskNotice {
            asked: Some(AskedOf {
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
            sandbox: "Shop",
            number: 3,
            title: "T",
            asked: None,
            link: "L",
        };
        assert_eq!(notice.text(), "Stopped (crashed) · Shop\n#3 T\nL");
    }

    #[test]
    fn an_account_message_names_its_problem() {
        let (headline, text) = account_text("Team Max", "needs to sign in again", None, "L");
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
}
