//! Delivery discipline at run time (SPEC-001 STP-006..008, DSC-013/014; SPEC-002 ADR-19):
//! steps that measure real progress, and specs only where they matter.
//!
//! The rules themselves are pure functions in `yulius_domain::steps` and
//! `yulius_domain::spec_format::SpecPolicy`; this module feeds them the stored steps, their
//! timing (from the step history), the task's workflow and the branch's changed paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use yulius_contracts::api::tasks::StepProgress;
use yulius_domain::ids::TaskId;
use yulius_domain::spec_format::SpecPolicy;
use yulius_domain::steps::{self as rules, Nudge, StepState, StepStatus};

use crate::context::App;
use crate::error::AppResult;
use crate::model::{StepRecord, StepTiming, TaskMode, TaskRecord};

/// A stored step as the rules see it.
#[must_use]
pub fn step_state(record: &StepRecord, timing: Option<&StepTiming>) -> StepState {
    StepState {
        title: record.title.clone(),
        status: StepStatus::parse(&record.status).unwrap_or(StepStatus::Pending),
        effort: record.effort.clamp(1, rules::MAX_EFFORT),
        started_at: timing.and_then(|t| t.started_at),
        finished_at: timing.and_then(|t| t.finished_at),
        note: record.note.clone(),
    }
}

/// Stored steps as the rules see them, in order.
#[must_use]
pub fn step_states(records: &[StepRecord], timings: &[StepTiming]) -> Vec<StepState> {
    let by_id: HashMap<&str, &StepTiming> =
        timings.iter().map(|t| (t.step_id.as_str(), t)).collect();
    records
        .iter()
        .map(|r| step_state(r, by_id.get(r.id.as_str()).copied()))
        .collect()
}

/// The API's progress summary: effort-weighted `percent` and `active_percent` (in progress),
/// finished (done or skipped) and all steps, and the current step.
#[must_use]
pub fn progress_view(records: &[StepRecord]) -> StepProgress {
    let states: Vec<StepState> = records.iter().map(|r| step_state(r, None)).collect();
    let p = rules::progress(&states);
    StepProgress {
        done: u32::try_from(p.done).unwrap_or(u32::MAX),
        total: u32::try_from(p.total).unwrap_or(u32::MAX),
        current: rules::current(&states).map(|i| records[i].title.clone()),
        percent: p.percent(),
        active_percent: p.active_percent(),
    }
}

/// The stored step whose title matches `title` (whitespace and case insensitive).
#[must_use]
pub fn step_by_title<'a>(records: &'a [StepRecord], title: &str) -> Option<&'a StepRecord> {
    let wanted = rules::normalize_title(title).to_lowercase();
    records
        .iter()
        .find(|r| rules::normalize_title(&r.title).to_lowercase() == wanted)
}

/// The nudge given once at a turn end, if any: work on a changed branch with no step ever
/// started, or (for tasks working toward a goal) a step open far too long.
#[must_use]
pub fn turn_end_nudge(
    states: &[StepState],
    branch_changed: bool,
    goal_mode: bool,
    now: yulius_domain::time::Timestamp,
) -> Option<(String, String)> {
    if branch_changed && rules::untouched(states) {
        let next = states
            .iter()
            .find(|s| s.status == StepStatus::Pending)
            .map_or_else(String::new, |s| format!(" (\"{}\")", s.title));
        return Some((
            "untracked".to_owned(),
            format!(
                "Yulius: you changed the repository but no step of your plan is in progress or \
                 done. Mark the step you are working on in_progress{next} with `step_update`, and \
                 each step done as soon as it is finished, so progress shows the work as it \
                 happens. If the plan no longer fits the work, replace it with `steps_set`."
            ),
        ));
    }
    if !goal_mode {
        return None;
    }
    rules::pace_nudges(states, now)
        .into_iter()
        .find(|n| matches!(n, Nudge::Stale { .. }))
        .map(|n| (n.key(), format!("Yulius: {n}")))
}

impl App {
    /// A task's stored steps and their states for the rules (with timing from the history).
    ///
    /// # Errors
    /// Store failures.
    pub async fn step_states_of(
        &self,
        task: &TaskId,
    ) -> AppResult<(Vec<StepRecord>, Vec<StepState>)> {
        let records = self.deps.store.steps(task).await?;
        let timings = self.deps.store.step_timings(task).await?;
        let states = step_states(&records, &timings);
        Ok((records, states))
    }

    /// The task's spec policy from its workflow (DSC-013): `Required` or `NotRequired` by the
    /// workflow's spec gate, `ByChange` without a workflow (or when it cannot be read).
    pub async fn spec_policy(&self, task: &TaskRecord) -> SpecPolicy {
        // The preparation task changes documentation, configuration and CI only (DSC-010): a
        // chore that needs no spec, so Yulius opens its pull request once its plan is done.
        if task.origin == crate::model::TaskOrigin::Onboarding {
            return SpecPolicy::NotRequired;
        }
        match self.task_workflow(task).await {
            Ok(workflow) => SpecPolicy::from_workflow(workflow.map(|w| w.require_spec)),
            Err(err) => {
                tracing::warn!(error = %err, task = %task.id, "workflow unreadable; spec policy by change");
                SpecPolicy::ByChange
            }
        }
    }

    fn integration_base(&self, sandbox: &crate::model::SandboxRecord) -> String {
        format!(
            "origin/{}",
            crate::delivery::delivery_profile(sandbox).integration_branch
        )
    }

    /// Whether the task's branch has uncommitted work or commits ahead of the integration
    /// branch.
    ///
    /// # Errors
    /// Store failures (Git failures count as no change).
    pub async fn branch_changed(&self, task: &TaskRecord) -> AppResult<bool> {
        let Some(worktree) = task.worktree_path.as_deref().map(PathBuf::from) else {
            return Ok(false);
        };
        if self.deps.git.has_changes(&worktree).await.unwrap_or(false) {
            return Ok(true);
        }
        let Some(sandbox) = self.deps.store.sandbox(&task.sandbox_id).await? else {
            return Ok(false);
        };
        let base = self.integration_base(&sandbox);
        Ok(self
            .deps
            .git
            .ahead_behind(&worktree, &base, "HEAD")
            .await
            .is_ok_and(|(ahead, _)| ahead > 0))
    }

    /// Whether the task needs a valid spec now (DSC-013): by its workflow, or, without one,
    /// when its branch changes anything other than documentation.
    ///
    /// # Errors
    /// Store failures (an unreadable diff counts as a code change).
    pub async fn spec_required(&self, task: &TaskRecord) -> AppResult<bool> {
        let policy = self.spec_policy(task).await;
        if policy != SpecPolicy::ByChange {
            return Ok(policy.requires_spec([], true));
        }
        let Some(worktree) = task.worktree_path.as_deref() else {
            return Ok(false);
        };
        let Some(sandbox) = self.deps.store.sandbox(&task.sandbox_id).await? else {
            return Ok(true);
        };
        let base = self.integration_base(&sandbox);
        match self
            .deps
            .git
            .review_changes(Path::new(worktree), &base)
            .await
        {
            Ok(changes) => Ok(policy.requires_spec(
                changes
                    .files
                    .iter()
                    .flat_map(|f| std::iter::once(f.path.as_str()).chain(f.old_path.as_deref())),
                !changes.truncated,
            )),
            Err(err) => {
                tracing::warn!(error = %err, task = %task.id, "changed paths unreadable; spec required");
                Ok(true)
            }
        }
    }

    /// The step nudges for a task now (plan shape and pace).
    pub(crate) async fn step_nudges(&self, task: &TaskRecord, states: &[StepState]) -> Vec<String> {
        let coarse_ok = self.spec_policy(task).await == SpecPolicy::NotRequired;
        rules::nudges(states, self.now(), coarse_ok)
            .into_iter()
            .map(|n| n.to_string())
            .collect()
    }

    /// The one-shot nudge for this turn end, marked as given (`None` when there is none or it
    /// was given before).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn take_turn_end_nudge(&self, task: &TaskRecord) -> AppResult<Option<String>> {
        let (_, states) = self.step_states_of(&task.id).await?;
        if states.is_empty() {
            return Ok(None);
        }
        let changed = if rules::untouched(&states) {
            self.branch_changed(task).await?
        } else {
            false
        };
        let Some((key, text)) =
            turn_end_nudge(&states, changed, task.mode == TaskMode::Goal, self.now())
        else {
            return Ok(None);
        };
        let runtime = self.runtime(&task.id);
        let Ok(mut given) = runtime.nudged.lock() else {
            return Ok(None);
        };
        Ok(given.insert(key).then_some(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use yulius_domain::ids::StepId;
    use yulius_domain::time::Timestamp;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn record(
        id: &str,
        title: &str,
        status: &str,
        effort: u8,
    ) -> Result<StepRecord, Box<dyn std::error::Error>> {
        Ok(StepRecord {
            id: StepId::parse(id)?,
            task_id: TaskId::parse("t1")?,
            ordinal: 1,
            title: title.to_owned(),
            status: status.to_owned(),
            note: None,
            updated_by: None,
            updated_at: None,
            effort,
        })
    }

    #[test]
    fn progress_is_weighted_and_skipped_counts_as_finished() -> TestResult {
        let records = vec![
            record("a", "Parser", "done", 1)?,
            record("b", "Store", "skipped", 1)?,
            record("c", "API", "in_progress", 2)?,
        ];
        let view = progress_view(&records);
        assert_eq!((view.done, view.total, view.percent), (2, 3, 50));
        // The step in progress carries 2 of 4 effort units (#150).
        assert_eq!(view.active_percent, 50);
        assert_eq!(view.current.as_deref(), Some("API"));
        assert_eq!(progress_view(&[]).percent, 0);
        assert_eq!(progress_view(&[]).active_percent, 0);
        assert_eq!(
            step_by_title(&records, "  store ").map(|r| r.id.as_str()),
            Some("b")
        );
        Ok(())
    }

    #[test]
    fn timings_join_by_step_id() -> TestResult {
        let records = vec![
            record("a", "Parser", "in_progress", 1)?,
            record("b", "Store", "pending", 9)?,
        ];
        let timings = vec![StepTiming {
            step_id: StepId::parse("a")?,
            started_at: Some(Timestamp(5)),
            finished_at: None,
        }];
        let states = step_states(&records, &timings);
        assert_eq!(states[0].started_at, Some(Timestamp(5)));
        assert_eq!(states[1].started_at, None);
        assert_eq!(
            states[1].effort,
            rules::MAX_EFFORT,
            "stored effort is clamped"
        );
        Ok(())
    }

    /// The default workflows (catalog format of #136) carry the delivery discipline of #139:
    /// features, fixes and the infra bootstrap need a spec; research, docs and chores do not;
    /// their seed steps are slices, not phases; every skill they name is bundled; and their
    /// instructions fit the briefing's 3 000-character room.
    #[test]
    fn default_workflows_follow_the_discipline() -> TestResult {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../catalog/workflows");
        let bundled: Vec<&str> = crate::skills::pack::PACKS
            .iter()
            .flat_map(|p| p.skills.iter().copied())
            .collect();
        let expected = [
            ("feature", "required"),
            ("fix", "required"),
            ("infra-bootstrap", "required"),
            ("research", "optional"),
            ("docs", "optional"),
            ("chore", "optional"),
        ];
        for (id, spec) in expected {
            let text = std::fs::read_to_string(dir.join(format!("{id}.md")))?;
            let (front, body) = text
                .strip_prefix("---\n")
                .and_then(|rest| rest.split_once("\n---\n"))
                .ok_or(id)?;
            let field = |key: &str| {
                front
                    .lines()
                    .find_map(|l| l.strip_prefix(&format!("{key}: ")))
                    .map(str::trim)
            };
            for key in ["name", "description", "when_to_use"] {
                let value = field(key).ok_or(format!("{id}: {key}"))?;
                assert!(
                    !value.is_empty() && value.chars().count() <= 300,
                    "{id}: {key}"
                );
            }
            assert_eq!(field("spec"), Some(spec), "{id}");
            let (steps, instructions) = body
                .split_once("## Steps\n")
                .and_then(|(_, rest)| rest.split_once("## Instructions\n"))
                .ok_or(format!("{id}: sections"))?;
            let titles: Vec<&str> = steps
                .lines()
                .filter_map(|l| l.split_once(". ").map(|(_, t)| t.trim()))
                .filter(|t| !t.is_empty())
                .collect();
            assert!(
                (1..=rules::MAX_STEPS).contains(&titles.len()),
                "{id}: {titles:?}"
            );
            for title in &titles {
                assert!(!rules::is_phase_title(title), "{id}: {title} is a phase");
                assert!(title.chars().count() <= rules::MAX_TITLE_CHARS);
            }
            let plan: Vec<(&str, u8)> = titles.iter().map(|t| (*t, 1)).collect();
            assert!(
                rules::plan_nudges(&plan, spec == "optional").is_empty(),
                "{id}: its seed plan is nudged"
            );
            assert!(
                instructions.chars().count() <= 2_500,
                "{id}: instructions too long"
            );
            for word in instructions.split('`').skip(1).step_by(2) {
                let looks_like_skill = word.contains('-')
                    && word.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                    && !word.starts_with("ci-");
                if looks_like_skill {
                    assert!(bundled.contains(&word), "{id} names unknown skill {word}");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn turn_end_nudges_untracked_work_and_stale_steps_in_goal_mode() {
        let min = 60_000;
        let pending = vec![StepState::pending("Parser")];
        let (key, text) = turn_end_nudge(&pending, true, false, Timestamp(0)).unwrap_or_default();
        assert_eq!(key, "untracked");
        assert!(text.contains("in_progress (\"Parser\")"), "{text}");
        assert!(turn_end_nudge(&pending, false, true, Timestamp(0)).is_none());
        let stale = vec![StepState {
            status: StepStatus::InProgress,
            started_at: Some(Timestamp(0)),
            ..StepState::pending("Parser")
        }];
        let late = Timestamp(90 * min);
        assert!(
            turn_end_nudge(&stale, true, false, late).is_none(),
            "prompt mode: no stale nudge"
        );
        let (key, text) = turn_end_nudge(&stale, true, true, late).unwrap_or_default();
        assert_eq!(key, "stale:Parser");
        assert!(text.starts_with("Yulius: \"Parser\" has been in progress for 90 min"));
    }
}
