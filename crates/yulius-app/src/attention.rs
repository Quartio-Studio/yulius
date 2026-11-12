//! Why a task reads `needs_attention` (SPEC-472, ADR-0306). The flag is one and its causes are
//! many: Claude Code asks in the task's terminal (a notice), or Yulius flags the task itself
//! (the gate gave up, a goal never set, a first message never sent, a full context, a session
//! that could not be ended, a secret request). The reason is written with the flag
//! (`tasks.attention`) and read only while the task still reads that flag, so whoever opens the
//! task later knows what to do. The notice in its terminal reaches only the viewers connected
//! when the flag is raised.

use serde::{Deserialize, Serialize};
use yulius_contracts::api::tasks::AttentionView;
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};

use crate::context::App;
use crate::error::AppResult;
use crate::model::TaskRecord;

/// Longest `problem`, in characters: what a notice gives of the gate's reasons, and the most
/// kept of Claude Code's words.
pub const MAX_PROBLEM_CHARS: usize = 400;

/// Who asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// Claude Code asks in the task's terminal: a permission menu, a question.
    Terminal,
    /// Yulius flagged the task itself.
    Yulius,
}

impl AttentionKind {
    /// The API name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Yulius => "yulius",
        }
    }
}

/// The reason of one flag (JSON in `tasks.attention`, where the store adds the flag's time).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionRecord {
    /// Who asks.
    pub kind: AttentionKind,
    /// What is wrong, one line.
    #[serde(default)]
    pub problem: Option<String>,
    /// What the person does.
    #[serde(default)]
    pub action: Option<String>,
    /// Where they do it (a page of this Yulius).
    #[serde(default)]
    pub link: Option<String>,
}

impl AttentionRecord {
    /// Claude Code asks in the task's terminal, in its own `words` (a notice's message).
    #[must_use]
    pub fn terminal(words: Option<&str>) -> Self {
        Self {
            kind: AttentionKind::Terminal,
            problem: words.and_then(one_line),
            action: None,
            link: None,
        }
    }

    /// Yulius flagged the task itself: what is wrong and what the person does.
    #[must_use]
    pub fn yulius(problem: &str, action: &str) -> Self {
        Self {
            kind: AttentionKind::Yulius,
            problem: one_line(problem),
            action: one_line(action),
            link: None,
        }
    }

    /// The same reason, answered on a page of this Yulius.
    #[must_use]
    pub fn on_page(mut self, link: &str) -> Self {
        self.link = Some(link.to_owned());
        self
    }

    /// The API view.
    #[must_use]
    pub fn view(&self) -> AttentionView {
        AttentionView {
            kind: self.kind.as_str().to_owned(),
            problem: self.problem.clone(),
            action: self.action.clone(),
            link: self.link.clone(),
        }
    }
}

/// `text` as one line of at most [`MAX_PROBLEM_CHARS`] characters (cut ones end in `…`): control
/// characters and runs of whitespace become one space. `None` when nothing is left.
fn one_line(text: &str) -> Option<String> {
    let spaced: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }
    if line.chars().count() <= MAX_PROBLEM_CHARS {
        return Some(line);
    }
    let mut cut: String = line.chars().take(MAX_PROBLEM_CHARS - 1).collect();
    cut.push('…');
    Some(cut)
}

impl App {
    /// Flags a running task as needing a person, for `reason`: whoever opens the task reads it
    /// while the task reads this flag. A task already flagged keeps its flag and its reason.
    ///
    /// # Errors
    /// The store's.
    pub(crate) async fn flag_attention(
        &self,
        task: &TaskId,
        reason: AttentionRecord,
    ) -> AppResult<()> {
        self.flag_needs_attention(task, Some(&reason)).await
    }

    /// Why `task` reads `needs_attention`, for its view: the reason of the flag it reads.
    pub(crate) async fn attention_view(&self, task: &TaskRecord) -> Option<AttentionView> {
        if task.status != TaskStatus::Running || task.activity != Some(Activity::NeedsAttention) {
            return None;
        }
        match self.deps.store.task_attention(&task.id).await {
            Ok(reason) => reason.map(|r| r.view()),
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "reading a flag's reason failed");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-472 ATN-001, ATN-005: a reason is one bounded line, whatever its source wrote.
    #[test]
    fn reasons_are_single_bounded_lines() {
        let asked = AttentionRecord::terminal(Some("Claude needs\r\n your\tpermission \u{1b}[1m"));
        assert_eq!(
            asked.problem.as_deref(),
            Some("Claude needs your permission [1m")
        );
        assert_eq!(asked.view().kind, "terminal");
        assert_eq!(AttentionRecord::terminal(Some(" \n ")).problem, None);
        assert_eq!(AttentionRecord::terminal(None).problem, None);

        let long = "é".repeat(MAX_PROBLEM_CHARS + 50);
        let cut = AttentionRecord::yulius(&long, "Do it.");
        let problem = cut.problem.as_deref().unwrap_or_default();
        assert_eq!(problem.chars().count(), MAX_PROBLEM_CHARS);
        assert!(problem.ends_with('…'), "{problem}");
        let exact = "x".repeat(MAX_PROBLEM_CHARS);
        assert_eq!(
            AttentionRecord::yulius(&exact, "Do it.").problem.as_deref(),
            Some(exact.as_str())
        );
    }

    /// SPEC-472 ATN-003: the view names the kind and carries the page.
    #[test]
    fn the_view_carries_the_reason_and_its_page() {
        let view = AttentionRecord::yulius("A secret waits.", "Answer it.")
            .on_page("https://yulius.test/s/app/secrets")
            .view();
        assert_eq!(
            view,
            AttentionView {
                kind: "yulius".into(),
                problem: Some("A secret waits.".into()),
                action: Some("Answer it.".into()),
                link: Some("https://yulius.test/s/app/secrets".into()),
            }
        );
    }
}
