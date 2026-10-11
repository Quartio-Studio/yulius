//! Updates of the instance from `main` of its one repository (SPEC-001 OPS-006..OPS-009, SPEC-002
//! ADR-18): commit ids, the summary of new commits, the update run's states and the rule for
//! when a live task is at a pause point.

use crate::task::{Activity, TaskStatus};

/// The repository every Yulius instance updates from ("always will be", F 2026-10-10).
pub const REPO_OWNER: &str = "Quartio-Studio";
/// Its name.
pub const REPO_NAME: &str = "yulius";
/// The production branch an update installs from.
pub const BRANCH: &str = "main";
/// Commit subjects named in a summary.
pub const SUMMARY_TITLES: usize = 3;
/// Longest summary.
pub const SUMMARY_MAX_CHARS: usize = 240;

/// Whether `text` is a full commit id: 40 lowercase hexadecimal characters.
#[must_use]
pub fn is_commit(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The first 7 characters of a commit id (the whole text when shorter).
#[must_use]
pub fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// The readable title of a commit message, or `None` for a merge or promotion commit: the first
/// line without its conventional `type(scope)!: ` prefix and trailing ` (#123)`, capitalized.
#[must_use]
pub fn commit_title(message: &str) -> Option<String> {
    let line = message.lines().next().unwrap_or_default().trim();
    let lower = line.to_ascii_lowercase();
    if line.is_empty()
        || lower.starts_with("merge pull request")
        || lower.starts_with("merge branch")
        || lower.starts_with("merge remote-tracking")
        || lower.starts_with("release:")
        || lower.starts_with("release(")
    {
        return None;
    }
    let mut text = line;
    if let Some((head, rest)) = line.split_once(": ") {
        let kind = head.trim_end_matches('!');
        let kind = kind.split_once('(').map_or(
            kind,
            |(k, scope)| {
                if scope.ends_with(')') { k } else { "" }
            },
        );
        if !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_lowercase()) && kind.len() <= 10 {
            text = rest.trim();
        }
    }
    if let Some(open) = text.rfind(" (#")
        && text.ends_with(')')
        && text[open + 3..text.len() - 1]
            .bytes()
            .all(|b| b.is_ascii_digit())
    {
        text = text[..open].trim_end();
    }
    let mut chars = text.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// "A · B · C and 4 more" from commit messages, newest first; `None` when no commit has a title
/// (only merges).
#[must_use]
pub fn summary<'a>(messages: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let titles: Vec<String> = messages.into_iter().filter_map(commit_title).collect();
    if titles.is_empty() {
        return None;
    }
    let mut text = titles
        .iter()
        .take(SUMMARY_TITLES)
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ");
    if titles.len() > SUMMARY_TITLES {
        text.push_str(&format!(" and {} more", titles.len() - SUMMARY_TITLES));
    }
    if text.chars().count() > SUMMARY_MAX_CHARS {
        text = text.chars().take(SUMMARY_MAX_CHARS - 1).collect();
        text.push('…');
    }
    Some(text)
}

/// How the running build relates to the head of `main`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// The build is the head.
    UpToDate,
    /// The build is an older commit of `main`: an update is available.
    Behind,
    /// The build has commits `main` does not (a development build ahead of it).
    Ahead,
    /// Both have commits the other lacks.
    Diverged,
    /// The build's commit is unknown or not on GitHub.
    Unknown,
}

impl Relation {
    /// Whether "Update now" is offered: behind, or a build that cannot be placed.
    #[must_use]
    pub fn offers_update(self) -> bool {
        matches!(self, Self::Behind | Self::Unknown | Self::Diverged)
    }

    /// Stable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpToDate => "up_to_date",
            Self::Behind => "behind",
            Self::Ahead => "ahead",
            Self::Diverged => "diverged",
            Self::Unknown => "unknown",
        }
    }
}

/// States of one update run. Nonterminal states have an owner, an exit and a bound (SPEC-002
/// §6.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunState {
    /// The helper fetches, verifies and builds the commit; tasks keep working.
    Building,
    /// Tasks pause at their turn boundaries (held hooks, queued starts).
    Draining,
    /// The helper stops the service, backs up, installs, starts and checks the new build.
    Installing,
    /// The paused tasks are released one by one.
    Resuming,
    /// The new build runs.
    Succeeded,
    /// Nothing was installed, or installing failed (see the error).
    Failed,
    /// The new build failed its health check; the previous binary and database are back.
    RolledBack,
    /// Cancelled before anything was installed.
    Cancelled,
}

impl RunState {
    /// Stored name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Building => "building",
            Self::Draining => "draining",
            Self::Installing => "installing",
            Self::Resuming => "resuming",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses a stored name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "building" => Self::Building,
            "draining" => Self::Draining,
            "installing" => Self::Installing,
            "resuming" => Self::Resuming,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "rolled_back" => Self::RolledBack,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    /// The phase people see: `building`, `pausing`, `restarting`, `resuming`, or the outcome.
    #[must_use]
    pub fn phase(self) -> &'static str {
        match self {
            Self::Draining => "pausing",
            Self::Installing => "restarting",
            other => other.as_str(),
        }
    }

    /// Whether the run has ended.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::RolledBack | Self::Cancelled
        )
    }

    /// Whether tasks are held at their turn boundaries and starts queue.
    #[must_use]
    pub fn holds_tasks(self) -> bool {
        matches!(self, Self::Draining | Self::Installing | Self::Resuming)
    }

    /// Whether `next` may follow `self`.
    #[must_use]
    pub fn can_become(self, next: Self) -> bool {
        use RunState::{
            Building, Cancelled, Draining, Failed, Installing, Resuming, RolledBack, Succeeded,
        };
        matches!(
            (self, next),
            (Building, Draining | Failed | Cancelled)
                | (Draining, Installing | Cancelled)
                | (Installing, Resuming)
                | (Resuming, Succeeded | Failed | RolledBack | Cancelled)
        )
    }
}

/// Whether a task with a live process is at a pause point for an update: its hook is held for
/// the update right now, it is paused for capacity, or its agent finished its turn and waits
/// (idle or for a person). A turn in progress is not, since restarting the server under it
/// could fail a Yulius tool call the agent would see.
#[must_use]
pub fn at_pause_point(status: TaskStatus, activity: Option<Activity>, held: bool) -> bool {
    held || status == TaskStatus::WaitingCapacity
        || (status == TaskStatus::Running
            && matches!(
                activity,
                Some(Activity::Waiting | Activity::Done | Activity::NeedsAttention)
            ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_ids_are_full_lowercase_hex() {
        assert!(is_commit("68b4f5f0c3a0b1d2e3f4a5b6c7d8e9f001234567"));
        assert!(!is_commit("68b4f5f"));
        assert!(!is_commit("68B4F5F0C3A0B1D2E3F4A5B6C7D8E9F001234567"));
        assert!(!is_commit("68b4f5f0c3a0b1d2e3f4a5b6c7d8e9f00123456g"));
        assert!(!is_commit("68b4f5f0c3a0b1d2e3f4a5b6c7d8e9f001234567\n"));
        assert_eq!(short("68b4f5f0c3a0b1d2e3f4a5b6c7d8e9f001234567"), "68b4f5f");
        assert_eq!(short("abc"), "abc");
    }

    #[test]
    fn titles_drop_prefixes_numbers_and_merges() {
        assert_eq!(
            commit_title("fix(ui): a solid wordmark beside the mark (#133)\n\nbody").as_deref(),
            Some("A solid wordmark beside the mark")
        );
        assert_eq!(
            commit_title("perf(browser)!: keep the stream live").as_deref(),
            Some("Keep the stream live")
        );
        assert_eq!(commit_title("feat: tasks (#12)").as_deref(), Some("Tasks"));
        assert_eq!(
            commit_title("Note: not a conventional prefix").as_deref(),
            Some("Note: not a conventional prefix")
        );
        assert_eq!(
            commit_title("Merge pull request #9 from Quartio-Studio/dev"),
            None
        );
        assert_eq!(commit_title("release: promote dev to main"), None);
        assert_eq!(commit_title("   "), None);
        assert_eq!(commit_title("fix: keep (#x)").as_deref(), Some("Keep (#x)"));
    }

    #[test]
    fn summaries_name_three_and_count_the_rest() {
        let messages = [
            "Merge pull request #140 from Quartio-Studio/dev",
            "fix(ui): wordmark (#133)",
            "perf(browser): live stream (#131)",
            "feat(files): drag and drop (#127)",
            "feat(tasks): done tasks (#128)",
            "fix(terminal): copy and paste (#126)",
        ];
        assert_eq!(
            summary(messages).as_deref(),
            Some("Wordmark · Live stream · Drag and drop and 2 more")
        );
        assert_eq!(summary(["Merge branch 'dev'"]), None);
        let long = "feat: ".to_owned() + &"x".repeat(400);
        let text = summary([long.as_str()]).unwrap_or_default();
        assert_eq!(text.chars().count(), SUMMARY_MAX_CHARS);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn run_states_round_trip_and_move_forward_only() {
        let all = [
            RunState::Building,
            RunState::Draining,
            RunState::Installing,
            RunState::Resuming,
            RunState::Succeeded,
            RunState::Failed,
            RunState::RolledBack,
            RunState::Cancelled,
        ];
        for state in all {
            assert_eq!(RunState::parse(state.as_str()), Some(state));
            for next in all {
                if state.is_terminal() {
                    assert!(!state.can_become(next), "{state:?} is terminal");
                }
            }
        }
        assert!(RunState::Building.can_become(RunState::Draining));
        assert!(!RunState::Building.can_become(RunState::Installing));
        assert!(!RunState::Installing.can_become(RunState::Cancelled));
        assert!(RunState::Resuming.can_become(RunState::RolledBack));
        assert_eq!(RunState::Draining.phase(), "pausing");
        assert_eq!(RunState::Installing.phase(), "restarting");
        assert!(RunState::Installing.holds_tasks());
        assert!(!RunState::Building.holds_tasks());
        assert_eq!(RunState::parse("paused"), None);
    }

    #[test]
    fn a_turn_in_progress_is_not_a_pause_point() {
        assert!(!at_pause_point(
            TaskStatus::Running,
            Some(Activity::Working),
            false
        ));
        assert!(!at_pause_point(TaskStatus::Running, None, false));
        assert!(!at_pause_point(TaskStatus::Starting, None, false));
        assert!(at_pause_point(
            TaskStatus::Running,
            Some(Activity::Working),
            true
        ));
        assert!(at_pause_point(
            TaskStatus::Running,
            Some(Activity::Waiting),
            false
        ));
        assert!(at_pause_point(
            TaskStatus::Running,
            Some(Activity::Done),
            false
        ));
        assert!(at_pause_point(
            TaskStatus::Running,
            Some(Activity::NeedsAttention),
            false
        ));
        assert!(at_pause_point(TaskStatus::WaitingCapacity, None, false));
    }

    #[test]
    fn relations_offer_an_update_unless_current_or_ahead() {
        assert!(Relation::Behind.offers_update());
        assert!(Relation::Unknown.offers_update());
        assert!(Relation::Diverged.offers_update());
        assert!(!Relation::UpToDate.offers_update());
        assert!(!Relation::Ahead.offers_update());
    }
}
