//! Task step manager (SPEC-001 STP-001..004; SPEC-002 §5 `steps` and `step_events`).
//!
//! Each task has its own harness-independent step list: the plan's subtasks, their status,
//! the current step and what remains. Every change is returned as a [`StepChange`] carrying
//! time, actor and note so the caller can persist an audit trail (STP-002). The list stays in
//! sync with the task spec's Plan checklist, which is the durable source in the repository
//! (STP-004): [`StepList::sync_from_plan`] applies the checklist and
//! [`render_plan_checklist`] renders the list back as one.
//!
//! Transition rules: any actor may move a step between pending, in progress and blocked, or
//! resolve it (done, skipped); only a user may change a resolved step (for example reopen
//! done → pending). A blocked step needs a note saying why.

use std::fmt;

use crate::ids::{StepId, TaskId, UserId};
use crate::spec_format::PlanItem;
use crate::time::Timestamp;

/// Most steps per task.
pub const MAX_STEPS: usize = 100;
/// Longest step title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// Longest note, in characters.
pub const MAX_NOTE_CHARS: usize = 2_000;

/// Status of one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepStatus {
    /// Not started.
    Pending,
    /// Being worked on.
    InProgress,
    /// Finished.
    Done,
    /// Deliberately not done.
    Skipped,
    /// Cannot proceed; the note says why.
    Blocked,
}

impl StepStatus {
    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Skipped => "skipped",
            Self::Blocked => "blocked",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "done" => Some(Self::Done),
            "skipped" => Some(Self::Skipped),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }

    /// Done or skipped: nothing remains to do.
    #[must_use]
    pub fn is_resolved(self) -> bool {
        matches!(self, Self::Done | Self::Skipped)
    }
}

impl fmt::Display for StepStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who changed a step.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Actor {
    /// The task's agent, through the Yulius tools.
    Task(TaskId),
    /// A person.
    User(UserId),
    /// Yulius itself (for example syncing from the spec).
    System,
}

impl Actor {
    /// Persisted form: `task:<id>`, `user:<id>` or `system`.
    #[must_use]
    pub fn encode(&self) -> String {
        match self {
            Self::Task(id) => format!("task:{id}"),
            Self::User(id) => format!("user:{id}"),
            Self::System => "system".to_owned(),
        }
    }

    /// Parses [`Actor::encode`] output.
    #[must_use]
    pub fn decode(text: &str) -> Option<Self> {
        if text == "system" {
            return Some(Self::System);
        }
        let (kind, id) = text.split_once(':')?;
        match kind {
            "task" => TaskId::parse(id).ok().map(Self::Task),
            "user" => UserId::parse(id).ok().map(Self::User),
            _ => None,
        }
    }
}

/// One step of a task's plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Identifier (injected by the caller).
    pub id: StepId,
    /// 1-based position.
    pub ordinal: u32,
    /// Single-line title.
    pub title: String,
    /// Status.
    pub status: StepStatus,
    /// Latest note, if any.
    pub note: Option<String>,
}

/// One audited change. `from: None` means the step was added; `to: None` means it was
/// removed from the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepChange {
    /// The step.
    pub step: StepId,
    /// Its title at the time of the change.
    pub title: String,
    /// When.
    pub at: Timestamp,
    /// Who.
    pub actor: Actor,
    /// Previous status (`None` = added).
    pub from: Option<StepStatus>,
    /// New status (`None` = removed).
    pub to: Option<StepStatus>,
    /// Note supplied with the change.
    pub note: Option<String>,
}

/// A rejected step operation. Nothing is changed when an error is returned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StepError {
    /// A blank title.
    #[error("step titles must not be empty")]
    EmptyTitle,
    /// Longer than [`MAX_TITLE_CHARS`].
    #[error("step titles must be at most {MAX_TITLE_CHARS} characters")]
    TitleTooLong,
    /// More than [`MAX_STEPS`].
    #[error("a task may have at most {MAX_STEPS} steps")]
    TooManySteps,
    /// Longer than [`MAX_NOTE_CHARS`].
    #[error("step notes must be at most {MAX_NOTE_CHARS} characters")]
    NoteTooLong,
    /// The id generator produced an id already in use.
    #[error("step id '{0}' is already in use")]
    DuplicateId(StepId),
    /// The id generator ran out.
    #[error("no step id available")]
    IdsExhausted,
    /// Unknown step id.
    #[error("unknown step '{0}'")]
    UnknownStep(StepId),
    /// Only a user may change a done or skipped step.
    #[error("only a user can change a {from} step to {to}")]
    ReopenRequiresUser {
        /// Current status.
        from: StepStatus,
        /// Requested status.
        to: StepStatus,
    },
    /// Blocking needs a reason.
    #[error("a blocked step needs a note explaining what blocks it")]
    BlockedNeedsNote,
    /// Persisted steps with duplicate ids or ordinals.
    #[error("persisted steps have duplicate ids or ordinals")]
    InconsistentSteps,
}

fn normalize_title(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

fn clip_chars(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        text
    } else {
        text.chars().take(max).collect()
    }
}

/// Whether `actor` may move a step from `from` to `to`.
///
/// # Errors
/// [`StepError::ReopenRequiresUser`] when a resolved step would change by a non-user.
pub fn check_transition(from: StepStatus, to: StepStatus, actor: &Actor) -> Result<(), StepError> {
    if from == to || !from.is_resolved() || matches!(actor, Actor::User(_)) {
        Ok(())
    } else {
        Err(StepError::ReopenRequiresUser { from, to })
    }
}

/// A task's ordered steps.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StepList {
    steps: Vec<Step>,
}

impl StepList {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restores persisted steps, ordered by ordinal.
    ///
    /// # Errors
    /// [`StepError::InconsistentSteps`] for duplicate ids or ordinals,
    /// [`StepError::TooManySteps`] above [`MAX_STEPS`].
    pub fn from_steps(mut steps: Vec<Step>) -> Result<Self, StepError> {
        if steps.len() > MAX_STEPS {
            return Err(StepError::TooManySteps);
        }
        steps.sort_by_key(|step| step.ordinal);
        for (index, step) in steps.iter().enumerate() {
            let earlier = &steps[..index];
            if earlier
                .iter()
                .any(|other| other.id == step.id || other.ordinal == step.ordinal)
            {
                return Err(StepError::InconsistentSteps);
            }
        }
        Ok(Self { steps })
    }

    /// Steps in order.
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// The step with `id`.
    #[must_use]
    pub fn get(&self, id: &StepId) -> Option<&Step> {
        self.steps.iter().find(|step| step.id == *id)
    }

    /// The first in-progress step, else the first pending one.
    #[must_use]
    pub fn current(&self) -> Option<&Step> {
        self.steps
            .iter()
            .find(|step| step.status == StepStatus::InProgress)
            .or_else(|| {
                self.steps
                    .iter()
                    .find(|step| step.status == StepStatus::Pending)
            })
    }

    /// `(done, total)` where skipped steps count toward neither.
    #[must_use]
    pub fn progress(&self) -> (usize, usize) {
        let done = self
            .steps
            .iter()
            .filter(|step| step.status == StepStatus::Done)
            .count();
        let skipped = self
            .steps
            .iter()
            .filter(|step| step.status == StepStatus::Skipped)
            .count();
        (done, self.steps.len() - skipped)
    }

    /// Steps neither done nor skipped (for delivery gates).
    #[must_use]
    pub fn open_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| !step.status.is_resolved())
            .count()
    }

    /// Replaces the plan with `titles` (whitespace collapsed). Steps whose title equals an
    /// existing step's keep that step's id, status and note (duplicates match in order); new
    /// titles become pending steps with ids drawn from `ids`; unmatched old steps are removed.
    /// Returns additions in plan order, then removals.
    ///
    /// # Errors
    /// [`StepError::EmptyTitle`], [`StepError::TitleTooLong`], [`StepError::TooManySteps`],
    /// [`StepError::DuplicateId`] or [`StepError::IdsExhausted`]; the list is unchanged.
    pub fn set_all<S: AsRef<str>>(
        &mut self,
        titles: &[S],
        ids: &mut impl Iterator<Item = StepId>,
        actor: &Actor,
        at: Timestamp,
    ) -> Result<Vec<StepChange>, StepError> {
        if titles.len() > MAX_STEPS {
            return Err(StepError::TooManySteps);
        }
        let mut normalized = Vec::with_capacity(titles.len());
        for title in titles {
            let title = normalize_title(title.as_ref());
            if title.is_empty() {
                return Err(StepError::EmptyTitle);
            }
            if title.chars().count() > MAX_TITLE_CHARS {
                return Err(StepError::TitleTooLong);
            }
            normalized.push(title);
        }
        self.replace(normalized, ids, actor, at)
    }

    fn replace(
        &mut self,
        titles: Vec<String>,
        ids: &mut impl Iterator<Item = StepId>,
        actor: &Actor,
        at: Timestamp,
    ) -> Result<Vec<StepChange>, StepError> {
        let mut remaining: Vec<Option<&Step>> = self.steps.iter().map(Some).collect();
        let mut next: Vec<Step> = Vec::with_capacity(titles.len());
        let mut changes = Vec::new();
        for (ordinal, title) in (1_u32..).zip(titles) {
            let reused = remaining
                .iter_mut()
                .find(|slot| slot.is_some_and(|step| step.title == title))
                .and_then(Option::take);
            match reused {
                Some(step) => next.push(Step {
                    ordinal,
                    ..step.clone()
                }),
                None => {
                    let id = ids.next().ok_or(StepError::IdsExhausted)?;
                    let taken = self.steps.iter().chain(&next).any(|step| step.id == id);
                    if taken {
                        return Err(StepError::DuplicateId(id));
                    }
                    changes.push(StepChange {
                        step: id.clone(),
                        title: title.clone(),
                        at,
                        actor: actor.clone(),
                        from: None,
                        to: Some(StepStatus::Pending),
                        note: None,
                    });
                    next.push(Step {
                        id,
                        ordinal,
                        title,
                        status: StepStatus::Pending,
                        note: None,
                    });
                }
            }
        }
        for removed in remaining.into_iter().flatten() {
            changes.push(StepChange {
                step: removed.id.clone(),
                title: removed.title.clone(),
                at,
                actor: actor.clone(),
                from: Some(removed.status),
                to: None,
                note: None,
            });
        }
        self.steps = next;
        Ok(changes)
    }

    /// Sets a step's status. `note: Some("")` clears the note, `None` keeps it.
    ///
    /// # Errors
    /// [`StepError::UnknownStep`], [`StepError::NoteTooLong`],
    /// [`StepError::ReopenRequiresUser`] or [`StepError::BlockedNeedsNote`]; the list is
    /// unchanged.
    pub fn update(
        &mut self,
        id: &StepId,
        status: StepStatus,
        note: Option<&str>,
        actor: &Actor,
        at: Timestamp,
    ) -> Result<StepChange, StepError> {
        let step = self
            .steps
            .iter_mut()
            .find(|step| step.id == *id)
            .ok_or_else(|| StepError::UnknownStep(id.clone()))?;
        let note = match note.map(str::trim) {
            Some(text) if text.chars().count() > MAX_NOTE_CHARS => {
                return Err(StepError::NoteTooLong);
            }
            Some("") => Some(None),
            Some(text) => Some(Some(text.to_owned())),
            None => None,
        };
        check_transition(step.status, status, actor)?;
        let resulting_note = note.clone().unwrap_or_else(|| step.note.clone());
        if status == StepStatus::Blocked && resulting_note.is_none() {
            return Err(StepError::BlockedNeedsNote);
        }
        let change = StepChange {
            step: step.id.clone(),
            title: step.title.clone(),
            at,
            actor: actor.clone(),
            from: Some(step.status),
            to: Some(status),
            note: note.flatten(),
        };
        step.status = status;
        step.note = resulting_note;
        Ok(change)
    }

    /// Reconciles the list with the spec's Plan checklist (STP-004), as [`Actor::System`].
    /// When the item titles differ from the step titles the plan is replaced as in
    /// [`StepList::set_all`] (long titles are clipped, empty items ignored); then every
    /// checked item marks its step done. Unchecked items never reopen a step: reopening is a
    /// user decision. Syncing an unchanged checklist again returns no changes.
    ///
    /// # Errors
    /// [`StepError::TooManySteps`], [`StepError::DuplicateId`] or
    /// [`StepError::IdsExhausted`]; the list is unchanged.
    pub fn sync_from_plan(
        &mut self,
        items: &[PlanItem],
        ids: &mut impl Iterator<Item = StepId>,
        at: Timestamp,
    ) -> Result<Vec<StepChange>, StepError> {
        let wanted: Vec<(String, bool)> = items
            .iter()
            .map(|item| {
                (
                    clip_chars(normalize_title(&item.text), MAX_TITLE_CHARS),
                    item.checked,
                )
            })
            .filter(|(title, _)| !title.is_empty())
            .collect();
        if wanted.len() > MAX_STEPS {
            return Err(StepError::TooManySteps);
        }
        let same = wanted.len() == self.steps.len()
            && wanted
                .iter()
                .zip(&self.steps)
                .all(|((title, _), step)| *title == step.title);
        let mut changes = if same {
            Vec::new()
        } else {
            let titles = wanted.iter().map(|(title, _)| title.clone()).collect();
            self.replace(titles, ids, &Actor::System, at)?
        };
        for (step, (_, checked)) in self.steps.iter_mut().zip(&wanted) {
            if *checked && !step.status.is_resolved() {
                changes.push(StepChange {
                    step: step.id.clone(),
                    title: step.title.clone(),
                    at,
                    actor: Actor::System,
                    from: Some(step.status),
                    to: Some(StepStatus::Done),
                    note: None,
                });
                step.status = StepStatus::Done;
            }
        }
        Ok(changes)
    }
}

/// The list as a Markdown checklist for the spec's Plan section: done and skipped steps are
/// checked. Parsing the result with the spec validator and syncing it back changes nothing.
#[must_use]
pub fn render_plan_checklist(list: &StepList) -> String {
    list.steps
        .iter()
        .map(|step| {
            let mark = if step.status.is_resolved() { 'x' } else { ' ' };
            format!("- [{mark}] {}\n", step.title)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec_format::validate_spec;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn ids(prefix: &str, count: usize) -> Result<std::vec::IntoIter<StepId>, crate::ids::IdError> {
        (1..=count)
            .map(|n| StepId::parse(&format!("{prefix}{n}")))
            .collect::<Result<Vec<_>, _>>()
            .map(Vec::into_iter)
    }

    fn agent() -> Result<Actor, crate::ids::IdError> {
        Ok(Actor::Task(TaskId::parse("task-1")?))
    }

    fn user() -> Result<Actor, crate::ids::IdError> {
        Ok(Actor::User(UserId::parse("octocat")?))
    }

    fn statuses(list: &StepList) -> Vec<(&str, StepStatus)> {
        list.steps()
            .iter()
            .map(|step| (step.title.as_str(), step.status))
            .collect()
    }

    fn id(text: &str) -> Result<StepId, crate::ids::IdError> {
        StepId::parse(text)
    }

    #[test]
    fn set_all_creates_and_audits_steps() -> TestResult {
        let mut list = StepList::new();
        let mut pool = ids("s", 10)?;
        let changes = list.set_all(
            &["Write spec", "  Implement\n API ", "Test"],
            &mut pool,
            &agent()?,
            Timestamp(5),
        )?;
        assert_eq!(statuses(&list)[1], ("Implement API", StepStatus::Pending));
        assert_eq!(list.steps()[2].ordinal, 3);
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].from, None);
        assert_eq!(changes[0].to, Some(StepStatus::Pending));
        assert_eq!(changes[0].actor, agent()?);
        assert_eq!(changes[0].at, Timestamp(5));
        assert_eq!(list.current().map(|s| s.title.as_str()), Some("Write spec"));
        assert_eq!(list.progress(), (0, 3));
        Ok(())
    }

    #[test]
    fn set_all_preserves_identical_titles_and_reports_removals() -> TestResult {
        let mut list = StepList::new();
        let mut pool = ids("s", 20)?;
        list.set_all(
            &["A", "B", "Test", "Test"],
            &mut pool,
            &agent()?,
            Timestamp(1),
        )?;
        list.update(
            &id("s1")?,
            StepStatus::Done,
            Some("ok"),
            &agent()?,
            Timestamp(2),
        )?;
        list.update(
            &id("s3")?,
            StepStatus::InProgress,
            None,
            &agent()?,
            Timestamp(2),
        )?;
        let changes = list.set_all(&["Test", "A", "C"], &mut pool, &agent()?, Timestamp(3))?;
        assert_eq!(
            statuses(&list),
            vec![
                ("Test", StepStatus::InProgress),
                ("A", StepStatus::Done),
                ("C", StepStatus::Pending)
            ]
        );
        assert_eq!(
            list.steps()[0].id,
            id("s3")?,
            "first duplicate matches first"
        );
        assert_eq!(list.steps()[1].note.as_deref(), Some("ok"));
        let summary: Vec<(String, Option<StepStatus>, Option<StepStatus>)> = changes
            .iter()
            .map(|c| (c.title.clone(), c.from, c.to))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("C".to_owned(), None, Some(StepStatus::Pending)),
                ("B".to_owned(), Some(StepStatus::Pending), None),
                ("Test".to_owned(), Some(StepStatus::Pending), None),
            ]
        );
        Ok(())
    }

    #[test]
    fn set_all_rejects_bad_input_without_changes() -> TestResult {
        let mut list = StepList::new();
        let mut pool = ids("s", 5)?;
        list.set_all(&["A"], &mut pool, &agent()?, Timestamp(1))?;
        let before = list.clone();
        assert_eq!(
            list.set_all(&["A", " "], &mut pool, &agent()?, Timestamp(2)),
            Err(StepError::EmptyTitle)
        );
        assert_eq!(
            list.set_all(&["x".repeat(201)], &mut pool, &agent()?, Timestamp(2)),
            Err(StepError::TitleTooLong)
        );
        let many: Vec<String> = (0..101).map(|n| format!("step {n}")).collect();
        assert_eq!(
            list.set_all(&many, &mut pool, &agent()?, Timestamp(2)),
            Err(StepError::TooManySteps)
        );
        let mut reused = ids("s", 1)?;
        assert_eq!(
            list.set_all(&["A", "B"], &mut reused, &agent()?, Timestamp(2)),
            Err(StepError::DuplicateId(id("s1")?))
        );
        let mut empty = ids("z", 0)?;
        assert_eq!(
            list.set_all(&["New"], &mut empty, &agent()?, Timestamp(2)),
            Err(StepError::IdsExhausted)
        );
        assert_eq!(list, before);
        Ok(())
    }

    #[test]
    fn transitions_and_notes() -> TestResult {
        let mut list = StepList::new();
        let mut pool = ids("s", 5)?;
        list.set_all(&["A", "B"], &mut pool, &agent()?, Timestamp(1))?;
        let a = id("s1")?;
        let change = list.update(
            &a,
            StepStatus::InProgress,
            Some("starting"),
            &agent()?,
            Timestamp(2),
        )?;
        assert_eq!(
            (change.from, change.to),
            (Some(StepStatus::Pending), Some(StepStatus::InProgress))
        );
        assert_eq!(change.note.as_deref(), Some("starting"));
        list.update(&a, StepStatus::Done, None, &agent()?, Timestamp(3))?;
        assert_eq!(
            list.get(&a).and_then(|s| s.note.as_deref()),
            Some("starting")
        );
        // Reopening is a user decision.
        assert_eq!(
            list.update(&a, StepStatus::Pending, None, &agent()?, Timestamp(4)),
            Err(StepError::ReopenRequiresUser {
                from: StepStatus::Done,
                to: StepStatus::Pending
            })
        );
        assert!(
            list.update(&a, StepStatus::Pending, None, &Actor::System, Timestamp(4))
                .is_err()
        );
        // A note on an unchanged status is allowed for anyone.
        list.update(
            &a,
            StepStatus::Done,
            Some("verified"),
            &agent()?,
            Timestamp(4),
        )?;
        list.update(&a, StepStatus::Pending, Some(""), &user()?, Timestamp(5))?;
        assert_eq!(
            list.get(&a).map(|s| (s.status, s.note.clone())),
            Some((StepStatus::Pending, None))
        );
        // Blocking needs a reason.
        let b = id("s2")?;
        assert_eq!(
            list.update(&b, StepStatus::Blocked, None, &agent()?, Timestamp(6)),
            Err(StepError::BlockedNeedsNote)
        );
        list.update(
            &b,
            StepStatus::Blocked,
            Some("waiting for API key"),
            &agent()?,
            Timestamp(6),
        )?;
        list.update(&b, StepStatus::Skipped, None, &agent()?, Timestamp(7))?;
        assert!(
            list.update(&b, StepStatus::InProgress, None, &agent()?, Timestamp(8))
                .is_err()
        );
        assert_eq!(
            list.update(
                &id("nope")?,
                StepStatus::Done,
                None,
                &agent()?,
                Timestamp(8)
            ),
            Err(StepError::UnknownStep(id("nope")?))
        );
        assert_eq!(
            list.update(
                &a,
                StepStatus::Done,
                Some(&"n".repeat(2_001)),
                &agent()?,
                Timestamp(8)
            ),
            Err(StepError::NoteTooLong)
        );
        Ok(())
    }

    #[test]
    fn current_progress_and_open_count() -> TestResult {
        let mut list = StepList::new();
        let mut pool = ids("s", 5)?;
        list.set_all(&["A", "B", "C", "D"], &mut pool, &agent()?, Timestamp(1))?;
        list.update(&id("s1")?, StepStatus::Done, None, &agent()?, Timestamp(2))?;
        list.update(
            &id("s2")?,
            StepStatus::Skipped,
            None,
            &agent()?,
            Timestamp(2),
        )?;
        list.update(
            &id("s4")?,
            StepStatus::InProgress,
            None,
            &agent()?,
            Timestamp(2),
        )?;
        assert_eq!(list.current().map(|s| s.title.as_str()), Some("D"));
        assert_eq!(list.progress(), (1, 3));
        assert_eq!(list.open_count(), 2);
        list.update(
            &id("s4")?,
            StepStatus::Blocked,
            Some("x"),
            &agent()?,
            Timestamp(3),
        )?;
        assert_eq!(list.current().map(|s| s.title.as_str()), Some("C"));
        Ok(())
    }

    #[test]
    fn sync_from_plan_follows_the_spec() -> TestResult {
        let spec = |plan: &str| {
            format!("---\nyulius-task: task-1\nstatus: in-progress\n---\n## Plan\n\n{plan}\n")
        };
        let mut list = StepList::new();
        let mut pool = ids("s", 20)?;
        let first = validate_spec(&spec(
            "- [x] Write spec\n- [ ] Implement\n  - [ ] Validate input\n- [ ]",
        ));
        let changes = list.sync_from_plan(&first.plan, &mut pool, Timestamp(1))?;
        assert_eq!(
            statuses(&list),
            vec![
                ("Write spec", StepStatus::Done),
                ("Implement", StepStatus::Pending),
                ("Validate input", StepStatus::Pending)
            ]
        );
        assert_eq!(changes.len(), 4, "three additions and one completion");
        assert!(changes.iter().all(|c| c.actor == Actor::System));
        // Idempotent.
        assert!(
            list.sync_from_plan(&first.plan, &mut pool, Timestamp(2))?
                .is_empty()
        );
        // Unchecking never reopens; checking completes; edits add and remove.
        let edited = validate_spec(&spec("- [ ] Write spec\n- [x] Implement\n- [ ] Document"));
        let changes = list.sync_from_plan(&edited.plan, &mut pool, Timestamp(3))?;
        assert_eq!(
            statuses(&list),
            vec![
                ("Write spec", StepStatus::Done),
                ("Implement", StepStatus::Done),
                ("Document", StepStatus::Pending)
            ]
        );
        let kinds: Vec<(&str, Option<StepStatus>, Option<StepStatus>)> = changes
            .iter()
            .map(|c| (c.title.as_str(), c.from, c.to))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("Document", None, Some(StepStatus::Pending)),
                ("Validate input", Some(StepStatus::Pending), None),
                (
                    "Implement",
                    Some(StepStatus::Pending),
                    Some(StepStatus::Done)
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn rendered_checklist_round_trips() -> TestResult {
        let mut list = StepList::new();
        let mut pool = ids("s", 10)?;
        list.set_all(
            &[
                "Write spec",
                "Implement [x] parser",
                "Skip me",
                "Blocked one",
            ],
            &mut pool,
            &agent()?,
            Timestamp(1),
        )?;
        list.update(&id("s1")?, StepStatus::Done, None, &agent()?, Timestamp(2))?;
        list.update(
            &id("s3")?,
            StepStatus::Skipped,
            None,
            &agent()?,
            Timestamp(2),
        )?;
        list.update(
            &id("s4")?,
            StepStatus::Blocked,
            Some("why"),
            &agent()?,
            Timestamp(2),
        )?;
        let rendered = render_plan_checklist(&list);
        assert_eq!(
            rendered,
            "- [x] Write spec\n- [ ] Implement [x] parser\n- [x] Skip me\n- [ ] Blocked one\n"
        );
        let report = validate_spec(&format!(
            "---\nyulius-task: t\nstatus: draft\n---\n## Plan\n\n{rendered}"
        ));
        let before = list.clone();
        assert!(
            list.sync_from_plan(&report.plan, &mut pool, Timestamp(3))?
                .is_empty()
        );
        assert_eq!(list, before);
        Ok(())
    }

    #[test]
    fn restore_and_actor_encoding() -> TestResult {
        let step = |id_text: &str, ordinal| -> Result<Step, crate::ids::IdError> {
            Ok(Step {
                id: id(id_text)?,
                ordinal,
                title: id_text.to_owned(),
                status: StepStatus::Pending,
                note: None,
            })
        };
        let list = StepList::from_steps(vec![step("b", 2)?, step("a", 1)?])?;
        assert_eq!(list.steps()[0].title, "a");
        assert_eq!(
            StepList::from_steps(vec![step("a", 1)?, step("a", 2)?]),
            Err(StepError::InconsistentSteps)
        );
        assert_eq!(
            StepList::from_steps(vec![step("a", 1)?, step("b", 1)?]),
            Err(StepError::InconsistentSteps)
        );
        for actor in [agent()?, user()?, Actor::System] {
            assert_eq!(Actor::decode(&actor.encode()), Some(actor));
        }
        assert_eq!(Actor::decode("robot:x"), None);
        assert_eq!(Actor::decode("task:../x"), None);
        for status in [
            StepStatus::Pending,
            StepStatus::InProgress,
            StepStatus::Done,
            StepStatus::Skipped,
            StepStatus::Blocked,
        ] {
            assert_eq!(StepStatus::parse(status.as_str()), Some(status));
        }
        Ok(())
    }
}
