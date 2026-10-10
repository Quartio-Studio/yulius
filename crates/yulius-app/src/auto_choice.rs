//! "Auto" mode and workflow for new tasks (SPEC-001 TSK-015; SPEC-002 ADR-16, item 5).
//!
//! When a new task's mode or workflow is `auto`, Jev answers one Choice question for each (both
//! at once, within one deadline) and the domain's fixed rules turn the answers, or their absence,
//! into the task's mode and workflow. Without Jev, or when it fails or is unsure, the defaults
//! apply: Goal, and the workflow named "Feature" — none for a prompt-mode task or when there is
//! none. The decision is stored with the task, audited, and shown in the task's summary.

use std::fmt::Write as _;
use std::time::Duration;

use yulius_contracts::api::tasks::AutoChoice;
use yulius_domain::auto_choice::{
    self as rules, ChosenBy, MODE_QUESTION, ModeChoice, WorkflowCandidate,
};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::intake::{self as policy, ChoiceAnswer, ChoiceOption, ChoiceQuestion};
use yulius_domain::time::Timestamp;

use crate::context::App;
use crate::error::AppResult;
use crate::model::TaskMode;
use crate::workflows::WorkflowRecord;

/// How long the Auto questions may take (they are asked at once, before the task exists).
pub const AUTO_CHOICE_DEADLINE: Duration = Duration::from_millis(2_500);

/// How Auto chose a task's mode and workflow: one row per task created with anything on Auto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoChoiceRecord {
    /// The task.
    pub task_id: TaskId,
    /// How the mode was chosen; `None` when the person picked it.
    pub mode_by: Option<ChosenBy>,
    /// How the workflow was chosen; `None` when the person picked it.
    pub workflow_by: Option<ChosenBy>,
    /// The task's workflow (its name) when it was created; `None` without one.
    pub workflow_name: Option<String>,
    /// When.
    pub created_at: Timestamp,
}

/// A new task's workflow as the request asks for it.
#[derive(Debug, Clone)]
pub enum WorkflowPick {
    /// No workflow.
    None,
    /// This workflow (already checked to be usable in the sandbox).
    Given(Box<WorkflowRecord>),
    /// Jev chooses (one of the sandbox's workflows, or none).
    Auto,
}

/// A new task's resolved mode and workflow.
#[derive(Debug, Clone)]
pub struct AutoOutcome {
    /// The mode.
    pub mode: TaskMode,
    /// The workflow.
    pub workflow: Option<WorkflowRecord>,
    /// How the mode was chosen; `None` when the person picked it.
    pub mode_by: Option<ChosenBy>,
    /// How the workflow was chosen; `None` when the person picked it.
    pub workflow_by: Option<ChosenBy>,
    /// For the audit log: what was chosen and how sure Jev was.
    pub detail: String,
}

impl AutoOutcome {
    /// Whether the mode or the workflow was on Auto.
    #[must_use]
    pub fn is_auto(&self) -> bool {
        self.mode_by.is_some() || self.workflow_by.is_some()
    }

    /// The row to store for task `task` (when anything was on Auto).
    #[must_use]
    pub fn record(&self, task: &TaskId, at: Timestamp) -> Option<AutoChoiceRecord> {
        self.is_auto().then(|| AutoChoiceRecord {
            task_id: task.clone(),
            mode_by: self.mode_by,
            workflow_by: self.workflow_by,
            workflow_name: self.workflow.as_ref().map(|w| w.name.clone()),
            created_at: at,
        })
    }
}

fn task_mode(mode: ModeChoice) -> TaskMode {
    match mode {
        ModeChoice::Goal => TaskMode::Goal,
        ModeChoice::Prompt => TaskMode::Prompt,
    }
}

fn mode_choice(mode: TaskMode) -> ModeChoice {
    match mode {
        TaskMode::Goal => ModeChoice::Goal,
        TaskMode::Prompt => ModeChoice::Prompt,
    }
}

fn percent(answer: Option<&ChoiceAnswer>, key: &str) -> String {
    // 0..=100 after the clamp, so the cast is exact.
    answer.map_or_else(String::new, |a| {
        format!(
            " {}%",
            (a.probability(key) * 100.0).round().clamp(0.0, 100.0) as u32
        )
    })
}

impl App {
    /// Asks Jev one question within the Auto deadline; every failure is "no answer".
    async fn auto_answer(
        &self,
        state: &str,
        question: &ChoiceQuestion<'_>,
    ) -> Option<ChoiceAnswer> {
        let (jev, _) = self.jev()?;
        match jev.evaluate(state, question, AUTO_CHOICE_DEADLINE).await {
            Ok(answer) => Some(answer),
            Err(failure) => {
                tracing::info!(%failure, question = question.id, "auto choice without Jev's answer; the default applies");
                None
            }
        }
    }

    /// Resolves a new task's mode (`None` asks for Auto) and workflow in `sandbox` for `prompt`
    /// (TSK-015). Without anything on Auto nothing is asked.
    ///
    /// # Errors
    /// Store failures (reading the sandbox's workflows).
    pub async fn auto_choose(
        &self,
        sandbox: &SandboxId,
        prompt: &str,
        mode: Option<TaskMode>,
        workflow: WorkflowPick,
    ) -> AppResult<AutoOutcome> {
        let fixed_workflow = match workflow {
            WorkflowPick::Auto => None,
            WorkflowPick::None => Some(None),
            WorkflowPick::Given(record) => Some(Some(*record)),
        };
        if let (Some(mode), Some(workflow)) = (mode, fixed_workflow.clone()) {
            return Ok(AutoOutcome {
                mode,
                workflow,
                mode_by: None,
                workflow_by: None,
                detail: String::new(),
            });
        }
        let workflows = if fixed_workflow.is_none() {
            self.workflows_for(sandbox).await?
        } else {
            Vec::new()
        };
        let candidates: Vec<WorkflowCandidate<'_>> = workflows
            .iter()
            .map(|w| WorkflowCandidate {
                name: &w.name,
                description: &w.description,
                steps: &w.steps,
                require_spec: w.require_spec,
                own: w.sandbox_id.is_some(),
            })
            .collect();
        let texts = rules::workflow_options(&candidates);
        let options: Vec<ChoiceOption<'_>> = texts
            .iter()
            .map(|(key, description)| ChoiceOption { key, description })
            .collect();
        let workflow_question = rules::workflow_question(&options);
        let state = policy::split_state(prompt);
        // Both questions at once: the composer waits for the slower one, never for both in turn.
        let (mode_answer, workflow_answer) = tokio::join!(
            async {
                match mode {
                    None => self.auto_answer(&state, &MODE_QUESTION).await,
                    Some(_) => None,
                }
            },
            async {
                // A question needs two options: with no workflow at all there is nothing to ask.
                if fixed_workflow.is_none() && options.len() >= 2 {
                    self.auto_answer(&state, &workflow_question).await
                } else {
                    None
                }
            },
        );

        let mut detail = String::new();
        let (mode, mode_by) = match mode {
            Some(mode) => (mode, None),
            None => {
                let (chosen, by) = rules::mode_choice(mode_answer.as_ref());
                let _ = write!(
                    detail,
                    "mode {} ({}{})",
                    chosen.as_str(),
                    by.as_str(),
                    percent(mode_answer.as_ref(), chosen.as_str())
                );
                (task_mode(chosen), Some(by))
            }
        };
        let (workflow, workflow_by) = match fixed_workflow {
            Some(workflow) => (workflow, None),
            None => {
                let (index, by) = rules::workflow_choice(
                    workflow_answer.as_ref(),
                    &candidates,
                    mode_choice(mode),
                );
                let key = index.map_or_else(
                    || rules::NO_WORKFLOW_KEY.to_owned(),
                    |i| format!("w{}", i + 1),
                );
                let chosen = index.and_then(|i| workflows.get(i)).cloned();
                if !detail.is_empty() {
                    detail.push_str(", ");
                }
                let _ = write!(
                    detail,
                    "workflow {} ({}{})",
                    chosen.as_ref().map_or("none", |w| w.name.as_str()),
                    by.as_str(),
                    percent(workflow_answer.as_ref(), &key)
                );
                (chosen, Some(by))
            }
        };
        Ok(AutoOutcome {
            mode,
            workflow,
            mode_by,
            workflow_by,
            detail,
        })
    }

    /// Stores and audits how Auto chose a new task's mode and workflow.
    ///
    /// # Errors
    /// Store failures.
    pub async fn record_auto_choice(
        &self,
        task: &TaskId,
        actor: &str,
        outcome: &AutoOutcome,
    ) -> AppResult<()> {
        let now = self.now();
        let Some(record) = outcome.record(task, now) else {
            return Ok(());
        };
        let store = &self.deps.store;
        store.insert_auto_choice(&record).await?;
        tracing::info!(task = %task, choice = %outcome.detail, "auto choice");
        store
            .audit(
                now,
                actor,
                "task.auto_choice",
                Some(task.as_str()),
                Some(&outcome.detail),
            )
            .await
    }

    /// The task's Auto choice for its summary (`None` without one or when it cannot be read —
    /// the failure is logged).
    pub async fn auto_choice_view(&self, task: &TaskId) -> Option<AutoChoice> {
        match self.deps.store.auto_choice(task).await {
            Ok(record) => record.map(|r| AutoChoice {
                mode: r.mode_by.map(|by| by.as_str().to_owned()),
                workflow: r.workflow_by.map(|by| by.as_str().to_owned()),
                workflow_name: r.workflow_name,
            }),
            Err(err) => {
                tracing::warn!(error = %err, task = %task, "auto choice unavailable");
                None
            }
        }
    }
}
