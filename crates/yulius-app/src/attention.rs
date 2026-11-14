//! Why a task reads `needs_attention` (SPEC-472, ADR-0306). The flag is one and its causes are
//! many: Claude Code asks in the task's terminal (a notice), or Yulius flags the task itself
//! (the gate gave up, a goal never set, a first message never sent, a full context, a session
//! that could not be ended, a secret request). The reason is written with the flag
//! (`tasks.attention`) and read only while the task still reads that flag, so whoever opens the
//! task later knows what to do. The notice in its terminal reaches only the viewers connected
//! when the flag is raised. The reason also says whether the session's own work answers the flag
//! (STA-003), so a restart of Yulius still knows it. A flag keeps its first reason, unless the
//! flag changes owner: a secret request's flag that another matter takes over says that
//! matter's reason, and one that waits for a second request points at the request still open.

use std::sync::atomic::Ordering;

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
    /// The session's next work answers this flag (STA-003): its own question, which the person
    /// answers in the terminal with no prompt to the agent. Kept with the flag so that a restart
    /// of Yulius still knows (ATN-010); no part of the API view.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub work_answers: bool,
    /// The flag is a secret request's: the request's answer ends it (SPEC-213), until another
    /// matter takes the flag over. Kept with the flag, as the one record of whose it is that a
    /// restart of Yulius still has (ATN-011); no part of the API view.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub answer_ends: bool,
}

/// What flagging a task did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flagged {
    /// The task reads the flag from now on, with the reason given.
    Raised,
    /// The task read a secret request's flag: it is this matter's from now on, with its reason
    /// (ATN-011).
    TakenOver,
    /// The task was flagged already (or is not running): its flag and its reason stand.
    Stands,
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
            work_answers: false,
            answer_ends: false,
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
            work_answers: false,
            answer_ends: false,
        }
    }

    /// The same reason, answered on a page of this Yulius.
    #[must_use]
    pub fn on_page(mut self, link: &str) -> Self {
        self.link = Some(link.to_owned());
        self
    }

    /// The same reason, for a flag that the session's next work answers (or not).
    #[must_use]
    pub fn answered_by_work(mut self, answered: bool) -> Self {
        self.work_answers = answered;
        self
    }

    /// The same reason, for a secret request's flag: the request's answer ends it.
    #[must_use]
    pub fn ended_by_its_answer(mut self) -> Self {
        self.answer_ends = true;
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

/// Whether `c` draws nothing and changes how the line around it reads: the bidirectional marks,
/// embeddings, overrides and isolates (which reorder what follows), zero-width spaces and the
/// other invisible format characters. The zero-width joiner and non-joiner stay: they shape
/// letters and emoji and neither hide nor reorder text.
pub(crate) fn hidden_format(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{61c}'
            | '\u{180e}'
            | '\u{200b}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// `text` as one line of at most [`MAX_PROBLEM_CHARS`] characters (cut ones end in `…`): control
/// characters and runs of whitespace become one space, and invisible format characters
/// ([`hidden_format`]) are dropped: the words come from Claude Code and from agents, and are
/// shown to a person as Yulius's own. `None` when nothing is left.
fn one_line(text: &str) -> Option<String> {
    shown_line(text, MAX_PROBLEM_CHARS)
}

/// `text` as one line of at most `max` characters, as [`one_line`] makes it: what an agent,
/// Claude Code or a person wrote, shown inside Yulius's own words (a task's reason, a held
/// prompt's excerpt in a tool's answer, SPEC-487 QST-001). `None` when nothing is left.
pub(crate) fn shown_line(text: &str, max: usize) -> Option<String> {
    let spaced: String = text
        .chars()
        .filter(|c| !hidden_format(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }
    if line.chars().count() <= max {
        return Some(line);
    }
    let mut cut: String = line.chars().take(max.saturating_sub(1)).collect();
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
    ) -> AppResult<Flagged> {
        self.flag_needs_attention(task, Some(&reason)).await
    }

    /// Why `task` reads `needs_attention`, for its view: the reason of the flag it reads.
    pub(crate) async fn attention_view(&self, task: &TaskRecord) -> Option<AttentionView> {
        if task.status != TaskStatus::Running || task.activity != Some(Activity::NeedsAttention) {
            return None;
        }
        match self.deps.store.task_attention(&task.id).await {
            Ok(reason) => reason.map(|(reason, _)| reason.view()),
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "reading a flag's reason failed");
                None
            }
        }
    }

    /// A run of `task_id` is watched from now on: a flag that the session's work answers
    /// (STA-003), raised before this server started and still read by the task, is answered by
    /// that work too. A restart of Yulius forgot when the flag was raised; its reason says that
    /// work answers it, and since when the task reads it (ATN-010). Without this the task keeps
    /// reading Needs your action over a working session until its turn ends.
    pub(crate) async fn watch_answerable_flag(&self, task_id: &TaskId) {
        let runtime = self.runtime(task_id);
        if runtime.notice_attention_ms.load(Ordering::SeqCst) != 0 {
            return;
        }
        match self.deps.store.task_attention(task_id).await {
            Ok(Some((reason, flagged))) if reason.work_answers => {
                // Only over nothing: a flag raised meanwhile stored its own time.
                let _ = runtime.notice_attention_ms.compare_exchange(
                    0,
                    flagged.0.max(1),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(task = %task_id, error = %err, "reading a flag's reason failed");
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

        // Characters that change how a line is drawn without showing (review round 1): a bidi
        // override would reverse the words that follow, a zero-width space hides a break.
        let hidden = AttentionRecord::yulius(
            "The agent asks: sign in\u{202e} txet siht\u{202c} now\u{200b}!\u{feff}",
            "Open\u{2066} it\u{2069}.",
        );
        assert_eq!(
            hidden.problem.as_deref(),
            Some("The agent asks: sign in txet siht now!")
        );
        assert_eq!(hidden.action.as_deref(), Some("Open it."));

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

    /// SPEC-472 ATN-010: whether work answers the flag is kept with the reason and is no part
    /// of the view; a reason written without it says no.
    #[test]
    fn whether_work_answers_is_kept_with_the_reason() -> Result<(), serde_json::Error> {
        let plain = AttentionRecord::terminal(Some("Allow Bash?"));
        assert!(!plain.work_answers);
        assert_eq!(
            serde_json::to_string(&plain)?,
            r#"{"kind":"terminal","problem":"Allow Bash?","action":null,"link":null}"#
        );
        let answered = plain.clone().answered_by_work(true);
        let text = serde_json::to_string(&answered)?;
        assert!(text.ends_with(r#""work_answers":true}"#), "{text}");
        assert_eq!(serde_json::from_str::<AttentionRecord>(&text)?, answered);
        assert_eq!(answered.view(), plain.view(), "the view says nothing of it");
        assert!(!answered.answered_by_work(false).work_answers);

        // Whose flag it is (a secret request's) is kept the same way.
        let secret = AttentionRecord::yulius("A secret waits.", "Answer it.");
        assert!(!secret.answer_ends);
        let owned = secret.clone().ended_by_its_answer();
        let text = serde_json::to_string(&owned)?;
        assert!(text.ends_with(r#""answer_ends":true}"#), "{text}");
        assert_eq!(serde_json::from_str::<AttentionRecord>(&text)?, owned);
        assert_eq!(owned.view(), secret.view());
        Ok(())
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
