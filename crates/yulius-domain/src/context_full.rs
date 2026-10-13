//! What Yulius types when a task's session runs out of context (SPEC-060, #213): the focus of
//! the `/compact` that recovers it, the line that wakes the compacted session, and the handoff
//! that opens a fresh session when compacting fails.

/// Most characters of the goal quoted in a focus or a handoff.
const GOAL_CHARS: usize = 400;

/// Most characters of one step's title.
const STEP_CHARS: usize = 120;

/// Most steps named; more are counted.
const STEPS_NAMED: usize = 8;

/// What the texts say about the task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFocus {
    /// Task number (`#N`).
    pub number: u32,
    /// Task title.
    pub title: String,
    /// The task's request (its goal), quoted shortened.
    pub goal: String,
    /// The task's branch, when it has one.
    pub branch: Option<String>,
    /// The task's pull request number, when it has one.
    pub pull_request: Option<u64>,
    /// Titles of the steps not done yet, in plan order.
    pub steps_left: Vec<String>,
}

impl TaskFocus {
    /// `/compact <focus>`: one line that asks the compaction to keep what the task needs to go
    /// on (the person's own `/compact` in #213 named the goal, the pull request and the steps
    /// left).
    #[must_use]
    pub fn compact_command(&self) -> String {
        let mut keep = vec![format!(
            "task #{} \"{}\" and its goal: {}",
            self.number,
            clean(&self.title, STEP_CHARS),
            clean(&self.goal, GOAL_CHARS)
        )];
        if let Some(branch) = &self.branch {
            keep.push(format!("branch {}", clean(branch, STEP_CHARS)));
        }
        if let Some(pr) = self.pull_request {
            keep.push(format!("pull request #{pr}"));
        }
        keep.push(match self.steps() {
            Some(steps) => format!("the steps left: {steps}"),
            None => "what is left to do".to_owned(),
        });
        keep.push("what is done and what was decided".to_owned());
        format!("/compact Keep: {}.", keep.join("; "))
    }

    /// The first message of a fresh session that continues the task after its old session ran
    /// out of context and could not be compacted.
    #[must_use]
    pub fn handoff(&self) -> String {
        let mut lines = vec![
            format!(
                "Yulius: you continue task #{} \"{}\" in a fresh session. The previous session's \
                 context was full and could not be compacted, so its conversation is gone; its \
                 work is in the repository.",
                self.number,
                clean(&self.title, STEP_CHARS)
            ),
            format!("Goal: {}", clean(&self.goal, GOAL_CHARS)),
        ];
        if let Some(branch) = &self.branch {
            lines.push(format!("Branch: {}", clean(branch, STEP_CHARS)));
        }
        if let Some(pr) = self.pull_request {
            lines.push(format!("Pull request: #{pr}"));
        }
        lines.push(match self.steps() {
            Some(steps) => format!("Steps left: {steps}"),
            None => "Steps left: none recorded".to_owned(),
        });
        lines.push(
            "Pick up where it stopped: read the task's request and spec, `git log` and \
             `git status` of the branch, the steps (`steps_get`) and the delivery \
             (`delivery_status`), then carry on."
                .to_owned(),
        );
        lines.join("\n")
    }

    /// The line typed after a compaction: the compacted session is idle (no turn ran, so no
    /// Stop hook or goal check wakes it), and the prompts the full context refused are lost.
    #[must_use]
    pub fn after_compaction(&self) -> String {
        format!(
            "Yulius: your context was full, so Yulius compacted this conversation. Carry on with \
             task #{} where you left off: check your steps and the delivery first.",
            self.number
        )
    }

    /// The steps left as one `a; b; c` line (`None` when there are none).
    fn steps(&self) -> Option<String> {
        if self.steps_left.is_empty() {
            return None;
        }
        let mut named: Vec<String> = self
            .steps_left
            .iter()
            .take(STEPS_NAMED)
            .map(|s| clean(s, STEP_CHARS))
            .collect();
        let more = self.steps_left.len().saturating_sub(STEPS_NAMED);
        if more > 0 {
            named.push(format!("and {more} more"));
        }
        Some(named.join("; "))
    }
}

/// `text` on one line (control characters and runs of whitespace become one space), at most
/// `max` characters (an ellipsis marks a cut).
fn clean(text: &str, max: usize) -> String {
    let one_line = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let cut: String = one_line.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus() -> TaskFocus {
        TaskFocus {
            number: 8,
            title: "Sandbox disk upkeep".into(),
            goal: "Fix: a full sandbox disk\nkilled four sessions.\r\n\u{1b}[31mKeep caches small"
                .into(),
            branch: Some("feat/8-disk".into()),
            pull_request: Some(205),
            steps_left: vec!["Merge dev".into(), "Deliver".into()],
        }
    }

    /// CTX-003: one line, the task's goal, branch, pull request and steps left.
    #[test]
    fn the_compact_focus_is_one_line_naming_the_task() {
        let command = focus().compact_command();
        assert_eq!(
            command,
            "/compact Keep: task #8 \"Sandbox disk upkeep\" and its goal: Fix: a full sandbox disk \
             killed four sessions. [31mKeep caches small; branch feat/8-disk; pull request #205; \
             the steps left: Merge dev; Deliver; what is done and what was decided."
        );
        assert!(!command.chars().any(char::is_control));
        let bare = TaskFocus {
            branch: None,
            pull_request: None,
            steps_left: Vec::new(),
            ..focus()
        }
        .compact_command();
        assert!(bare.contains("what is left to do") && !bare.contains("pull request"));
    }

    /// A long goal and many steps stay bounded.
    #[test]
    fn long_goals_and_plans_are_bounded() {
        let long = TaskFocus {
            goal: "word ".repeat(1_000),
            steps_left: (1..=20).map(|n| format!("step {n} {}", "x".repeat(300))).collect(),
            ..focus()
        };
        let command = long.compact_command();
        assert!(command.chars().count() < 2_000, "{}", command.len());
        assert!(command.contains("and 12 more"));
        assert!(command.contains('…'));
        assert!(long.handoff().chars().count() < 2_200);
    }

    /// CTX-004: the handoff says why the session is fresh and how to pick up.
    #[test]
    fn the_handoff_says_why_and_how_to_pick_up() {
        let handoff = focus().handoff();
        assert!(handoff.starts_with("Yulius: you continue task #8 \"Sandbox disk upkeep\""));
        assert!(handoff.contains("context was full"));
        assert!(handoff.contains("\nBranch: feat/8-disk\nPull request: #205\n"));
        assert!(handoff.contains("Steps left: Merge dev; Deliver"));
        assert!(handoff.contains("`git log`"));
        assert!(focus().after_compaction().contains("task #8"));
    }
}
