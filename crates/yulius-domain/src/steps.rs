//! Task steps: the rules that keep progress honest (SPEC-001 STP-001..008; SPEC-002 §5, §7.4,
//! ADR-18).
//!
//! A task's plan is a short list of steps of **similar effort**, so that "9 of 10 done" means
//! about 90 %, and the steps are finished **one after another** as the work happens, so the
//! progress bar moves steadily instead of jumping from 0 % to 100 % at the end.
//!
//! The rules are deliberately few:
//!
//! - **Plan bounds** ([`validate_plan`]): 1 to [`MAX_STEPS`] unique, non-empty titles. A step
//!   may declare an effort of 1 to [`MAX_EFFORT`] (default 1) when it cannot be split; progress
//!   is weighted by effort ([`progress`]).
//! - **Steady completion** ([`check_update`]): at most [`MAX_IN_PROGRESS`] steps in progress at
//!   once; an agent never reopens a finished step (it adds a follow-up step instead, so
//!   progress never runs backwards silently); blocking or skipping needs a reason.
//! - **Nudges** ([`nudges`]): never blocking, always actionable: a plan that is too coarse or
//!   too fine, titles that name a phase instead of a slice of work ("Implement", "Test"), no
//!   step started, two steps open at once, a step open far longer than finished ones took, and
//!   progress that arrived in a burst.
//!
//! Skipped steps count as done for progress and delivery (STP-005). The functions are pure:
//! callers pass the steps, the actor and the time.

use std::fmt;

use crate::time::Timestamp;

/// Most steps in one plan. More than this is no longer a plan of similar slices.
pub const MAX_STEPS: usize = 30;
/// Longest step title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// Longest note, in characters.
pub const MAX_NOTE_CHARS: usize = 2_000;
/// Most steps in progress at the same time.
pub const MAX_IN_PROGRESS: usize = 2;
/// Largest declared effort of one step (1 = a normal step).
pub const MAX_EFFORT: u8 = 3;
/// Plans with fewer steps are nudged as coarse (when the task needs a plan of slices).
pub const COARSE_BELOW: usize = 3;
/// Plans with more steps are nudged as too fine.
pub const FINE_ABOVE: usize = 12;
/// A step in progress is never called stale before this long (milliseconds).
pub const STALE_FLOOR_MS: i64 = 45 * 60 * 1000;
/// A step in progress is stale after this many times the typical duration of a finished step
/// of the same effort.
pub const STALE_FACTOR: i64 = 3;
/// Window in which several finished steps count as a burst (milliseconds).
pub const BURST_WINDOW_MS: i64 = 10 * 60 * 1000;
/// Finished steps in one window that make a burst.
pub const BURST_MIN_STEPS: usize = 3;
/// Share of the plan's effort (percent) finished in one window that makes a burst.
pub const BURST_MIN_PERCENT: u32 = 30;
/// Titles named in one nudge at most.
const MAX_TITLES_NAMED: usize = 3;

/// Status of one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepStatus {
    /// Not started.
    Pending,
    /// Being worked on.
    InProgress,
    /// Finished.
    Done,
    /// Deliberately not done; counts as done for progress and delivery (STP-005).
    Skipped,
    /// Cannot proceed; the note says why.
    Blocked,
}

impl StepStatus {
    /// Every status, in wire order.
    pub const ALL: [Self; 5] = [
        Self::Pending,
        Self::InProgress,
        Self::Done,
        Self::Skipped,
        Self::Blocked,
    ];

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
        Self::ALL.into_iter().find(|status| status.as_str() == text)
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

/// Who changes a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum By {
    /// The task's agent, through the Yulius tools.
    Agent,
    /// A person (in Yulius, or by ruling a step out, STP-005).
    Person,
    /// Yulius itself (for example syncing the spec's Plan checklist).
    Yulius,
}

/// One step as the rules see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepState {
    /// Single-line title.
    pub title: String,
    /// Status.
    pub status: StepStatus,
    /// Declared effort, 1 to [`MAX_EFFORT`].
    pub effort: u8,
    /// When it last went in progress, if known.
    pub started_at: Option<Timestamp>,
    /// When it was last finished (done or skipped), if known.
    pub finished_at: Option<Timestamp>,
    /// Its current note.
    pub note: Option<String>,
}

impl StepState {
    /// A pending step of effort 1 without history.
    #[must_use]
    pub fn pending(title: &str) -> Self {
        Self {
            title: normalize_title(title),
            status: StepStatus::Pending,
            effort: 1,
            started_at: None,
            finished_at: None,
            note: None,
        }
    }
}

/// One step of a plan submitted by `steps_set`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedStep {
    /// Title (normalized by [`validate_plan`]).
    pub title: String,
    /// Declared effort; `None` keeps the existing step's effort (or 1).
    pub effort: Option<u8>,
    /// Requested status; `None` keeps the existing step's status (or pending).
    pub status: Option<StepStatus>,
}

/// One step of an accepted plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStep {
    /// Normalized title.
    pub title: String,
    /// Resulting status.
    pub status: StepStatus,
    /// Resulting effort.
    pub effort: u8,
    /// Index of the current step with the same title, whose identity and note it keeps.
    pub previous: Option<usize>,
}

/// A rejected step operation. The text is shown to the agent as the fix-it message, so each
/// variant says what to do.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StepError {
    /// No steps.
    #[error("a plan needs at least one step")]
    EmptyPlan,
    /// More than [`MAX_STEPS`].
    #[error(
        "{count} steps is too many: a plan has at most {MAX_STEPS} steps of similar effort; merge \
         related steps"
    )]
    TooManySteps {
        /// Steps submitted.
        count: usize,
    },
    /// A blank title.
    #[error("step {position} has no title")]
    EmptyTitle {
        /// 1-based position.
        position: usize,
    },
    /// Longer than [`MAX_TITLE_CHARS`].
    #[error("step {position} has a title longer than {MAX_TITLE_CHARS} characters")]
    TitleTooLong {
        /// 1-based position.
        position: usize,
    },
    /// The same title twice (step updates find steps by title).
    #[error("the step \"{title}\" appears twice; give every step its own title")]
    DuplicateTitle {
        /// The title.
        title: String,
    },
    /// Effort outside 1..=[`MAX_EFFORT`].
    #[error(
        "step {position} has effort {effort}: use 1 (a normal step), 2 or {MAX_EFFORT} for a step \
         that cannot be split"
    )]
    EffortOutOfRange {
        /// 1-based position.
        position: usize,
        /// Declared effort.
        effort: u8,
    },
    /// Starting another step would exceed [`MAX_IN_PROGRESS`].
    #[error(
        "{} already in progress; finish, block or skip one before starting another, so steps \
         are completed one after another",
        quoted_list(.open)
    )]
    TooManyInProgress {
        /// Titles of the steps in progress.
        open: Vec<String>,
    },
    /// An agent tried to reopen a finished step.
    #[error(
        "the step \"{title}\" is already {from} and stays finished; add a follow-up step for the \
         new work instead (steps_set), so progress never runs backwards"
    )]
    Reopen {
        /// The step.
        title: String,
        /// Its finished status.
        from: StepStatus,
    },
    /// Blocking or skipping without a reason.
    #[error("a {status} step needs a note saying why")]
    NeedsNote {
        /// The requested status.
        status: StepStatus,
    },
    /// Longer than [`MAX_NOTE_CHARS`].
    #[error("step notes must be at most {MAX_NOTE_CHARS} characters")]
    NoteTooLong,
}

fn quoted_list(titles: &[String]) -> String {
    let quoted: Vec<String> = titles.iter().map(|t| format!("\"{t}\"")).collect();
    match quoted.len() {
        0 => "no step is".to_owned(),
        1 => format!("{} is", quoted[0]),
        _ => format!("{} are", quoted.join(" and ")),
    }
}

/// One line, whitespace collapsed, control characters removed.
#[must_use]
pub fn normalize_title(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

fn same_title(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// Validates a plan submitted by `steps_set` against the current steps and resolves each
/// entry's status and effort: an omitted status keeps the status of the current step with the
/// same title (else pending); an omitted effort keeps its effort (else 1).
///
/// # Errors
/// The first [`StepError`] found; nothing is accepted then.
pub fn validate_plan(
    plan: &[PlannedStep],
    current: &[StepState],
    by: By,
) -> Result<Vec<ResolvedStep>, StepError> {
    if plan.is_empty() {
        return Err(StepError::EmptyPlan);
    }
    if plan.len() > MAX_STEPS {
        return Err(StepError::TooManySteps { count: plan.len() });
    }
    let mut resolved: Vec<ResolvedStep> = Vec::with_capacity(plan.len());
    let mut taken = vec![false; current.len()];
    for (index, step) in plan.iter().enumerate() {
        let position = index + 1;
        let title = normalize_title(&step.title);
        if title.is_empty() {
            return Err(StepError::EmptyTitle { position });
        }
        if title.chars().count() > MAX_TITLE_CHARS {
            return Err(StepError::TitleTooLong { position });
        }
        if resolved
            .iter()
            .any(|other| same_title(&other.title, &title))
        {
            return Err(StepError::DuplicateTitle { title });
        }
        if let Some(effort) = step.effort
            && !(1..=MAX_EFFORT).contains(&effort)
        {
            return Err(StepError::EffortOutOfRange { position, effort });
        }
        let previous = current
            .iter()
            .enumerate()
            .position(|(i, old)| !taken[i] && same_title(&old.title, &title));
        if let Some(i) = previous {
            taken[i] = true;
        }
        let old = previous.map(|i| &current[i]);
        let status = match (step.status, old) {
            (Some(StepStatus::Blocked), _) => {
                return Err(StepError::NeedsNote {
                    status: StepStatus::Blocked,
                });
            }
            (Some(requested), Some(old))
                if by == By::Agent && old.status.is_resolved() && !requested.is_resolved() =>
            {
                return Err(StepError::Reopen {
                    title: old.title.clone(),
                    from: old.status,
                });
            }
            (Some(requested), _) => requested,
            (None, Some(old)) => old.status,
            (None, None) => StepStatus::Pending,
        };
        resolved.push(ResolvedStep {
            title,
            status,
            effort: step.effort.or(old.map(|o| o.effort)).unwrap_or(1),
            previous,
        });
    }
    let open: Vec<String> = resolved
        .iter()
        .filter(|s| s.status == StepStatus::InProgress)
        .map(|s| s.title.clone())
        .collect();
    if open.len() > MAX_IN_PROGRESS {
        return Err(StepError::TooManyInProgress { open });
    }
    Ok(resolved)
}

/// Whether `by` may set step `index` to `to` with `note` (`Some("")` clears the note).
///
/// # Errors
/// [`StepError::NoteTooLong`], [`StepError::Reopen`] (agents only),
/// [`StepError::TooManyInProgress`] or [`StepError::NeedsNote`] (blocked always; skipped by an
/// agent).
pub fn check_update(
    steps: &[StepState],
    index: usize,
    to: StepStatus,
    note: Option<&str>,
    by: By,
) -> Result<(), StepError> {
    let Some(step) = steps.get(index) else {
        return Ok(());
    };
    let note = note.map(str::trim);
    if note.is_some_and(|n| n.chars().count() > MAX_NOTE_CHARS) {
        return Err(StepError::NoteTooLong);
    }
    if by == By::Agent && step.status.is_resolved() && !to.is_resolved() {
        return Err(StepError::Reopen {
            title: step.title.clone(),
            from: step.status,
        });
    }
    if to == StepStatus::InProgress && step.status != StepStatus::InProgress {
        let open: Vec<String> = steps
            .iter()
            .filter(|s| s.status == StepStatus::InProgress)
            .map(|s| s.title.clone())
            .collect();
        if open.len() >= MAX_IN_PROGRESS {
            return Err(StepError::TooManyInProgress { open });
        }
    }
    let reason = match note {
        Some("") => None,
        Some(text) => Some(text),
        None => step.note.as_deref().filter(|n| !n.trim().is_empty()),
    };
    let needs_reason = match to {
        StepStatus::Blocked => true,
        StepStatus::Skipped => by == By::Agent,
        _ => false,
    };
    if needs_reason && to != step.status && reason.is_none() {
        return Err(StepError::NeedsNote { status: to });
    }
    Ok(())
}

/// Progress of a plan, weighted by effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    /// Finished steps (done or skipped).
    pub done: usize,
    /// All steps.
    pub total: usize,
    /// Effort of the finished steps.
    pub done_effort: u32,
    /// Effort of the steps in progress.
    pub active_effort: u32,
    /// Effort of all steps.
    pub total_effort: u32,
}

impl Progress {
    /// Finished share of the effort, rounded down: 100 only when every step is finished.
    #[must_use]
    pub fn percent(&self) -> u8 {
        if self.total_effort == 0 {
            return 0;
        }
        u8::try_from(self.done_effort.saturating_mul(100) / self.total_effort).unwrap_or(100)
    }

    /// Share of the effort in progress, so that `percent() + active_percent()` is the finished
    /// and in-progress share rounded down: the progress bar's second segment ends exactly there
    /// and never passes 100 (#150).
    #[must_use]
    pub fn active_percent(&self) -> u8 {
        if self.total_effort == 0 {
            return 0;
        }
        let through =
            (self.done_effort + self.active_effort).saturating_mul(100) / self.total_effort;
        u8::try_from(through.min(100))
            .unwrap_or(100)
            .saturating_sub(self.percent())
    }
}

fn effort_of(step: &StepState) -> u32 {
    u32::from(step.effort.clamp(1, MAX_EFFORT))
}

/// Effort-weighted progress; skipped steps count as finished (STP-005), steps in progress apart.
#[must_use]
pub fn progress(steps: &[StepState]) -> Progress {
    let mut out = Progress::default();
    for step in steps {
        out.total += 1;
        out.total_effort += effort_of(step);
        if step.status.is_resolved() {
            out.done += 1;
            out.done_effort += effort_of(step);
        } else if step.status == StepStatus::InProgress {
            out.active_effort += effort_of(step);
        }
    }
    out
}

/// The step being worked on: the first in progress, else the first pending one.
#[must_use]
pub fn current(steps: &[StepState]) -> Option<usize> {
    steps
        .iter()
        .position(|s| s.status == StepStatus::InProgress)
        .or_else(|| steps.iter().position(|s| s.status == StepStatus::Pending))
}

/// Whether a plan shows no progress at all: steps exist, none is in progress and none is
/// finished. Used with "the branch has changes" for the untracked-work nudge.
#[must_use]
pub fn untouched(steps: &[StepState]) -> bool {
    !steps.is_empty()
        && steps
            .iter()
            .all(|s| matches!(s.status, StepStatus::Pending | StepStatus::Blocked))
}

/// Words that name a phase of work rather than a slice of it.
const PHASE_WORDS: [&str; 44] = [
    "a",
    "all",
    "and",
    "changes",
    "cleanup",
    "code",
    "deploy",
    "deployment",
    "design",
    "diff",
    "docs",
    "documentation",
    "done",
    "everything",
    "feature",
    "final",
    "finish",
    "fix",
    "fixes",
    "implement",
    "implementation",
    "it",
    "merge",
    "open",
    "plan",
    "planning",
    "polish",
    "pr",
    "pull",
    "qa",
    "refactor",
    "request",
    "research",
    "review",
    "spec",
    "specification",
    "test",
    "testing",
    "tests",
    "the",
    "verify",
    "verification",
    "work",
    "write",
];

/// Whether a title names only a phase ("Implement", "Write tests", "Open PR").
#[must_use]
pub fn is_phase_title(title: &str) -> bool {
    let words: Vec<String> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    !words.is_empty() && words.len() <= 3 && words.iter().all(|w| PHASE_WORDS.contains(&w.as_str()))
}

/// A gentle, never-blocking hint about the plan or the pace of progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nudge {
    /// Fewer than [`COARSE_BELOW`] steps for a task that needs a plan of slices.
    CoarsePlan {
        /// Steps in the plan.
        count: usize,
    },
    /// More than [`FINE_ABOVE`] steps.
    FinePlan {
        /// Steps in the plan.
        count: usize,
    },
    /// Titles that name a phase, not a slice of work.
    PhaseTitles {
        /// Up to three such titles.
        titles: Vec<String>,
    },
    /// More than a third of the effort sits in steps of the largest effort.
    HeavyPlan {
        /// Steps of effort [`MAX_EFFORT`].
        heavy: usize,
    },
    /// Steps remain but none is in progress.
    NotStarted {
        /// The next pending step.
        next: String,
    },
    /// Two steps are in progress at once.
    ManyOpen {
        /// Their titles.
        titles: Vec<String>,
    },
    /// A step has been in progress much longer than finished steps took.
    Stale {
        /// The step.
        title: String,
        /// Minutes in progress.
        minutes: i64,
    },
    /// Several steps were finished within a few minutes of each other just now.
    Burst {
        /// Steps finished in the window.
        count: usize,
        /// Share of the plan's effort they carry, in percent.
        percent: u32,
    },
}

impl Nudge {
    /// Stable identity, so a nudge delivered once is not repeated (for example `stale:<title>`).
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::CoarsePlan { .. } => "coarse".to_owned(),
            Self::FinePlan { .. } => "fine".to_owned(),
            Self::PhaseTitles { titles } => format!("phase:{}", titles.join("|")),
            Self::HeavyPlan { .. } => "heavy".to_owned(),
            Self::NotStarted { next } => format!("not-started:{next}"),
            Self::ManyOpen { titles } => format!("many-open:{}", titles.join("|")),
            Self::Stale { title, .. } => format!("stale:{title}"),
            Self::Burst { .. } => "burst".to_owned(),
        }
    }

    /// Whether the nudge is about the pace of the work (worth interrupting a turn for) rather
    /// than the shape of the plan.
    #[must_use]
    pub fn is_pace(&self) -> bool {
        matches!(self, Self::Stale { .. } | Self::Burst { .. })
    }
}

impl fmt::Display for Nudge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CoarsePlan { count } => write!(
                f,
                "The plan has {count} step(s). Split the work into {COARSE_BELOW}-{FINE_ABOVE} \
                 steps of similar effort, each a slice that leaves the project working, so \
                 progress means something."
            ),
            Self::FinePlan { count } => write!(
                f,
                "The plan has {count} steps. Merge related steps into {COARSE_BELOW}-{FINE_ABOVE} \
                 slices of similar effort; tiny steps make the bar jump."
            ),
            Self::PhaseTitles { titles } => write!(
                f,
                "{} name{} a phase, not a slice of work. Name what each step delivers (\"Store \
                 drafts per user\", not \"Implement\"), so steps have similar effort.",
                titles
                    .iter()
                    .map(|t| format!("\"{t}\""))
                    .collect::<Vec<_>>()
                    .join(", "),
                if titles.len() == 1 { "s" } else { "" }
            ),
            Self::HeavyPlan { heavy } => write!(
                f,
                "{heavy} step(s) declare effort {MAX_EFFORT}. Split big steps where possible; \
                 effort above 1 is for the step that cannot be split."
            ),
            Self::NotStarted { next } => write!(
                f,
                "No step is in progress. Mark \"{next}\" in_progress when you start it, and done \
                 as soon as it is finished."
            ),
            Self::ManyOpen { titles } => write!(
                f,
                "{} are in progress at once. Finish one step before starting the next, so \
                 progress arrives steadily.",
                titles
                    .iter()
                    .map(|t| format!("\"{t}\""))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
            Self::Stale { title, minutes } => write!(
                f,
                "\"{title}\" has been in progress for {minutes} min, much longer than finished \
                 steps took. If it is bigger than the others, split it (steps_set) and finish the \
                 part that is done; if something blocks it, mark it blocked with a note."
            ),
            Self::Burst { count, percent } => write!(
                f,
                "{count} steps ({percent} % of the plan) were finished within a few minutes. Mark \
                 each step done as soon as it is finished, and size steps so progress arrives \
                 steadily instead of jumping at the end."
            ),
        }
    }
}

/// Nudges about the shape of a plan of `(title, effort)`. `coarse_ok` silences the coarse-plan
/// nudge for tasks that need no plan of slices (research, questions, docs, chores).
#[must_use]
pub fn plan_nudges(plan: &[(&str, u8)], coarse_ok: bool) -> Vec<Nudge> {
    let mut out = Vec::new();
    if plan.is_empty() {
        return out;
    }
    if plan.len() < COARSE_BELOW && !coarse_ok {
        out.push(Nudge::CoarsePlan { count: plan.len() });
    }
    if plan.len() > FINE_ABOVE {
        out.push(Nudge::FinePlan { count: plan.len() });
    }
    let phase: Vec<String> = plan
        .iter()
        .filter(|(title, _)| is_phase_title(title))
        .take(MAX_TITLES_NAMED)
        .map(|(title, _)| (*title).to_owned())
        .collect();
    if !phase.is_empty() && !(coarse_ok && plan.len() < COARSE_BELOW) {
        out.push(Nudge::PhaseTitles { titles: phase });
    }
    let heavy = plan
        .iter()
        .filter(|(_, effort)| *effort >= MAX_EFFORT)
        .count();
    if heavy > 0 && heavy * 3 > plan.len() {
        out.push(Nudge::HeavyPlan { heavy });
    }
    out
}

fn median(mut values: Vec<i64>) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len() / 2])
}

/// Nudges about the pace of the work at `now`.
#[must_use]
pub fn pace_nudges(steps: &[StepState], now: Timestamp) -> Vec<Nudge> {
    let mut out = Vec::new();
    let open: Vec<&StepState> = steps
        .iter()
        .filter(|s| s.status == StepStatus::InProgress)
        .collect();
    if open.is_empty() {
        if let Some(next) = steps.iter().find(|s| s.status == StepStatus::Pending) {
            out.push(Nudge::NotStarted {
                next: next.title.clone(),
            });
        }
    } else if open.len() >= 2 {
        out.push(Nudge::ManyOpen {
            titles: open
                .iter()
                .take(MAX_TITLES_NAMED)
                .map(|s| s.title.clone())
                .collect(),
        });
    }
    // Typical duration of one unit of effort, from finished steps with a known start.
    let per_unit = median(
        steps
            .iter()
            .filter(|s| s.status == StepStatus::Done)
            .filter_map(|s| {
                let (start, end) = (s.started_at?, s.finished_at?);
                let took = end.millis_since(start);
                (took > 0).then(|| took / i64::from(s.effort.clamp(1, MAX_EFFORT)))
            })
            .collect(),
    );
    for step in &open {
        let Some(started) = step.started_at else {
            continue;
        };
        let elapsed = now.millis_since(started);
        let expected = per_unit.map_or(0, |unit| {
            unit.saturating_mul(i64::from(step.effort.clamp(1, MAX_EFFORT)))
                .saturating_mul(STALE_FACTOR)
        });
        if elapsed > STALE_FLOOR_MS.max(expected) {
            out.push(Nudge::Stale {
                title: step.title.clone(),
                minutes: elapsed / 60_000,
            });
        }
    }
    let total = progress(steps).total_effort.max(1);
    let recent: Vec<&StepState> = steps
        .iter()
        .filter(|s| s.status.is_resolved())
        .filter(|s| {
            s.finished_at
                .is_some_and(|at| (0..=BURST_WINDOW_MS).contains(&now.millis_since(at)))
        })
        .collect();
    let percent = recent.iter().map(|s| effort_of(s)).sum::<u32>() * 100 / total;
    if recent.len() >= BURST_MIN_STEPS && percent >= BURST_MIN_PERCENT {
        out.push(Nudge::Burst {
            count: recent.len(),
            percent,
        });
    }
    out
}

/// Every nudge for a plan at `now` (shape first, then pace).
#[must_use]
pub fn nudges(steps: &[StepState], now: Timestamp, coarse_ok: bool) -> Vec<Nudge> {
    let plan: Vec<(&str, u8)> = steps.iter().map(|s| (s.title.as_str(), s.effort)).collect();
    let mut out = plan_nudges(&plan, coarse_ok);
    out.extend(pace_nudges(steps, now));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const MIN: i64 = 60_000;

    fn plan(titles: &[&str]) -> Vec<PlannedStep> {
        titles
            .iter()
            .map(|t| PlannedStep {
                title: (*t).to_owned(),
                effort: None,
                status: None,
            })
            .collect()
    }

    fn state(title: &str, status: StepStatus) -> StepState {
        StepState {
            status,
            ..StepState::pending(title)
        }
    }

    fn timed(
        title: &str,
        status: StepStatus,
        started: Option<i64>,
        finished: Option<i64>,
    ) -> StepState {
        StepState {
            started_at: started.map(|m| Timestamp(m * MIN)),
            finished_at: finished.map(|m| Timestamp(m * MIN)),
            ..state(title, status)
        }
    }

    #[test]
    fn statuses_round_trip() {
        for status in StepStatus::ALL {
            assert_eq!(StepStatus::parse(status.as_str()), Some(status));
            assert_eq!(status.to_string(), status.as_str());
        }
        assert_eq!(StepStatus::parse("finished"), None);
        assert!(StepStatus::Skipped.is_resolved() && !StepStatus::Blocked.is_resolved());
    }

    #[test]
    fn plans_are_bounded() {
        assert_eq!(
            validate_plan(&[], &[], By::Agent),
            Err(StepError::EmptyPlan)
        );
        let many: Vec<String> = (0..=MAX_STEPS).map(|n| format!("Slice {n}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(
            validate_plan(&plan(&many), &[], By::Agent),
            Err(StepError::TooManySteps {
                count: MAX_STEPS + 1
            })
        );
        assert_eq!(
            validate_plan(&plan(&["A", " \n "]), &[], By::Agent),
            Err(StepError::EmptyTitle { position: 2 })
        );
        let long = "x".repeat(MAX_TITLE_CHARS + 1);
        assert_eq!(
            validate_plan(&plan(&[&long]), &[], By::Agent),
            Err(StepError::TitleTooLong { position: 1 })
        );
        assert_eq!(
            validate_plan(&plan(&["Store drafts", "store   DRAFTS"]), &[], By::Agent),
            Err(StepError::DuplicateTitle {
                title: "store DRAFTS".to_owned()
            })
        );
        let mut heavy = plan(&["A", "B"]);
        heavy[1].effort = Some(4);
        assert_eq!(
            validate_plan(&heavy, &[], By::Agent),
            Err(StepError::EffortOutOfRange {
                position: 2,
                effort: 4
            })
        );
        heavy[1].effort = Some(0);
        assert!(validate_plan(&heavy, &[], By::Agent).is_err());
        let mut open = plan(&["A", "B", "C"]);
        for step in &mut open {
            step.status = Some(StepStatus::InProgress);
        }
        assert!(matches!(
            validate_plan(&open, &[], By::Agent),
            Err(StepError::TooManyInProgress { open }) if open.len() == 3
        ));
        let mut blocked = plan(&["A"]);
        blocked[0].status = Some(StepStatus::Blocked);
        assert_eq!(
            validate_plan(&blocked, &[], By::Agent),
            Err(StepError::NeedsNote {
                status: StepStatus::Blocked
            })
        );
    }

    #[test]
    fn plans_keep_status_and_effort_by_title_and_never_reopen_for_agents() -> TestResult {
        let current = vec![
            StepState {
                effort: 2,
                ..state("Store drafts", StepStatus::Done)
            },
            state("List drafts", StepStatus::InProgress),
        ];
        let resolved = validate_plan(
            &plan(&["  list drafts ", "Store drafts", "Delete drafts"]),
            &current,
            By::Agent,
        )?;
        let summary: Vec<(&str, StepStatus, u8, Option<usize>)> = resolved
            .iter()
            .map(|s| (s.title.as_str(), s.status, s.effort, s.previous))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("list drafts", StepStatus::InProgress, 1, Some(1)),
                ("Store drafts", StepStatus::Done, 2, Some(0)),
                ("Delete drafts", StepStatus::Pending, 1, None),
            ]
        );
        let mut reopen = plan(&["Store drafts"]);
        reopen[0].status = Some(StepStatus::Pending);
        assert_eq!(
            validate_plan(&reopen, &current, By::Agent),
            Err(StepError::Reopen {
                title: "Store drafts".to_owned(),
                from: StepStatus::Done
            })
        );
        assert_eq!(
            validate_plan(&reopen, &current, By::Person)?[0].status,
            StepStatus::Pending
        );
        Ok(())
    }

    #[test]
    fn updates_complete_steps_one_after_another() {
        let steps = vec![
            state("A", StepStatus::InProgress),
            state("B", StepStatus::InProgress),
            state("C", StepStatus::Pending),
            state("D", StepStatus::Done),
        ];
        // A third step cannot start while two are open.
        assert_eq!(
            check_update(&steps, 2, StepStatus::InProgress, None, By::Agent),
            Err(StepError::TooManyInProgress {
                open: vec!["A".to_owned(), "B".to_owned()]
            })
        );
        assert_eq!(
            StepError::TooManyInProgress {
                open: vec!["A".to_owned(), "B".to_owned()]
            }
            .to_string(),
            "\"A\" and \"B\" are already in progress; finish, block or skip one before starting \
             another, so steps are completed one after another"
        );
        // Re-marking an open step, finishing one, and notes on unchanged statuses are fine.
        assert!(check_update(&steps, 0, StepStatus::InProgress, Some("still"), By::Agent).is_ok());
        assert!(check_update(&steps, 0, StepStatus::Done, None, By::Agent).is_ok());
        assert!(check_update(&steps, 3, StepStatus::Done, Some("verified"), By::Agent).is_ok());
        // Agents never reopen; people may.
        assert!(matches!(
            check_update(&steps, 3, StepStatus::Pending, None, By::Agent),
            Err(StepError::Reopen { .. })
        ));
        assert!(check_update(&steps, 3, StepStatus::Pending, None, By::Person).is_ok());
        // Blocked always needs a reason; skipped needs one from an agent.
        assert_eq!(
            check_update(&steps, 2, StepStatus::Blocked, None, By::Agent),
            Err(StepError::NeedsNote {
                status: StepStatus::Blocked
            })
        );
        assert_eq!(
            check_update(&steps, 2, StepStatus::Skipped, Some(" "), By::Agent),
            Err(StepError::NeedsNote {
                status: StepStatus::Skipped
            })
        );
        assert!(
            check_update(
                &steps,
                2,
                StepStatus::Skipped,
                Some("out of scope"),
                By::Agent
            )
            .is_ok()
        );
        assert!(check_update(&steps, 2, StepStatus::Skipped, None, By::Person).is_ok());
        let noted = vec![StepState {
            note: Some("waiting for the API key".to_owned()),
            ..state("A", StepStatus::Pending)
        }];
        assert!(check_update(&noted, 0, StepStatus::Blocked, None, By::Agent).is_ok());
        assert_eq!(
            check_update(&noted, 0, StepStatus::Blocked, Some(""), By::Agent),
            Err(StepError::NeedsNote {
                status: StepStatus::Blocked
            })
        );
        assert_eq!(
            check_update(
                &steps,
                0,
                StepStatus::Done,
                Some(&"n".repeat(MAX_NOTE_CHARS + 1)),
                By::Agent
            ),
            Err(StepError::NoteTooLong)
        );
    }

    #[test]
    fn nine_of_ten_equal_steps_is_ninety_percent_and_effort_weights_it() {
        let mut steps: Vec<StepState> = (0..10)
            .map(|n| state(&format!("Slice {n}"), StepStatus::Done))
            .collect();
        steps[9].status = StepStatus::Pending;
        let p = progress(&steps);
        assert_eq!((p.done, p.total, p.percent()), (9, 10, 90));
        // A skipped step counts as finished (STP-005); 100 only when all are finished.
        steps[9].status = StepStatus::Skipped;
        assert_eq!(progress(&steps).percent(), 100);
        // Effort 3 for the open step: 9 of 12 units.
        steps[9].status = StepStatus::InProgress;
        steps[9].effort = 3;
        let p = progress(&steps);
        assert_eq!((p.done_effort, p.total_effort, p.percent()), (9, 12, 75));
        assert_eq!(progress(&[]).percent(), 0);
        assert_eq!(current(&steps), Some(9));
        assert_eq!(current(&[state("A", StepStatus::Done)]), None);
    }

    #[test]
    fn active_percent_is_the_share_of_steps_in_progress() {
        // #150: "Step 2/4" with one step done shows 25 % done and 25 % in progress.
        let steps = [
            state("A", StepStatus::Done),
            state("B", StepStatus::InProgress),
            state("C", StepStatus::Pending),
            state("D", StepStatus::Blocked),
        ];
        let p = progress(&steps);
        assert_eq!(
            (p.active_effort, p.percent(), p.active_percent()),
            (1, 25, 25)
        );
        // Nothing in progress, or no plan: no in-progress share.
        assert_eq!(progress(&steps[2..]).active_percent(), 0);
        assert_eq!(progress(&[]).active_percent(), 0);
    }

    #[test]
    fn active_percent_ends_at_the_cumulative_floor() {
        // 2 of 9 effort units done (22 %), 3 in progress: the segment ends at 5/9 = 55 %, so it
        // is 33 wide, and the two never pass 100.
        let mut steps: Vec<StepState> = (0..7)
            .map(|n| state(&format!("S{n}"), StepStatus::Pending))
            .collect();
        steps[0].status = StepStatus::Done;
        steps[1].status = StepStatus::Done;
        steps[2].status = StepStatus::InProgress;
        steps[2].effort = 3;
        let p = progress(&steps);
        assert_eq!((p.percent(), p.active_percent()), (22, 33));
        let all_open = [
            state("A", StepStatus::InProgress),
            state("B", StepStatus::InProgress),
            state("C", StepStatus::Done),
        ];
        let p = progress(&all_open);
        assert_eq!((p.percent(), p.active_percent()), (33, 67));
    }

    #[test]
    fn phase_titles_are_recognized() {
        for title in [
            "Implement",
            "Write tests",
            "Open PR",
            "Review the diff",
            "Write spec",
        ] {
            assert!(is_phase_title(title), "{title}");
        }
        for title in [
            "Implement the draft API",
            "Store drafts per user",
            "Test the migration rollback",
            "",
        ] {
            assert!(!is_phase_title(title), "{title}");
        }
    }

    #[test]
    fn plan_shape_nudges() {
        let coarse = plan_nudges(&[("Write spec", 1), ("Implement", 1)], false);
        assert_eq!(
            coarse,
            vec![
                Nudge::CoarsePlan { count: 2 },
                Nudge::PhaseTitles {
                    titles: vec!["Write spec".to_owned(), "Implement".to_owned()]
                }
            ]
        );
        // A research task may have one or two steps without being told to split them.
        assert!(plan_nudges(&[("Answer the question", 1)], true).is_empty());
        let fine: Vec<String> = (0..13).map(|n| format!("Slice {n}")).collect();
        let fine: Vec<(&str, u8)> = fine.iter().map(|t| (t.as_str(), 1)).collect();
        assert_eq!(
            plan_nudges(&fine, false),
            vec![Nudge::FinePlan { count: 13 }]
        );
        let heavy = [("Parser", 3), ("Store", 3), ("API", 1), ("UI", 1)];
        assert_eq!(
            plan_nudges(&heavy, false),
            vec![Nudge::HeavyPlan { heavy: 2 }]
        );
        let good = [("Parser", 3), ("Store", 1), ("API", 1), ("UI", 1)];
        assert!(plan_nudges(&good, false).is_empty());
        assert!(plan_nudges(&[], false).is_empty());
    }

    #[test]
    fn pace_nudges_with_a_fake_clock() {
        let now = Timestamp(1_000 * MIN);
        // Nothing started yet.
        let fresh = vec![state("Parser", StepStatus::Pending)];
        assert_eq!(
            pace_nudges(&fresh, now),
            vec![Nudge::NotStarted {
                next: "Parser".to_owned()
            }]
        );
        // Two finished steps took 10 and 20 min (typical: 20); the open one has run 70 min:
        // stale once past max(45 min floor, 3 × 20 min).
        let mut steps = vec![
            timed("Parser", StepStatus::Done, Some(900), Some(910)),
            timed("Store", StepStatus::Done, Some(910), Some(930)),
            timed("API", StepStatus::InProgress, Some(930), None),
            state("UI", StepStatus::Pending),
        ];
        assert_eq!(
            pace_nudges(&steps, now),
            vec![Nudge::Stale {
                title: "API".to_owned(),
                minutes: 70
            }]
        );
        assert!(pace_nudges(&steps, Timestamp(970 * MIN)).is_empty());
        // Effort scales what is expected of a step.
        steps[2].effort = 3;
        assert!(pace_nudges(&steps, now).is_empty());
        // Without finished steps the floor applies.
        let lone = vec![timed("API", StepStatus::InProgress, Some(960), None)];
        assert!(pace_nudges(&lone, now).is_empty());
        assert_eq!(pace_nudges(&lone, Timestamp(1_006 * MIN)).len(), 1);
        // Two open steps.
        let two = vec![
            timed("API", StepStatus::InProgress, Some(990), None),
            timed("UI", StepStatus::InProgress, Some(995), None),
        ];
        assert_eq!(
            pace_nudges(&two, now),
            vec![Nudge::ManyOpen {
                titles: vec!["API".to_owned(), "UI".to_owned()]
            }]
        );
        assert_eq!(
            Nudge::ManyOpen {
                titles: vec!["API".to_owned(), "UI".to_owned()]
            }
            .key(),
            "many-open:API|UI"
        );
    }

    #[test]
    fn bursts_are_noticed_while_they_are_recent() {
        let now = Timestamp(1_000 * MIN);
        // Everything ticked in the last minutes: 0 % → 75 %.
        let steps = vec![
            timed("Parser", StepStatus::Done, None, Some(995)),
            timed("Store", StepStatus::Done, None, Some(996)),
            timed("API", StepStatus::Skipped, None, Some(999)),
            state("UI", StepStatus::Pending),
        ];
        let found = pace_nudges(&steps, now);
        assert!(
            found.contains(&Nudge::Burst {
                count: 3,
                percent: 75
            }),
            "{found:?}"
        );
        assert!(found.iter().any(Nudge::is_pace));
        // Later, the same history is no longer news.
        assert!(
            !pace_nudges(&steps, Timestamp(1_020 * MIN))
                .iter()
                .any(|n| matches!(n, Nudge::Burst { .. }))
        );
        // Steady progress across a long plan is not a burst.
        let steady: Vec<StepState> = (0..12)
            .map(|n| {
                let status = if n < 3 {
                    StepStatus::Done
                } else {
                    StepStatus::Pending
                };
                timed(&format!("Slice {n}"), status, None, Some(992 + n))
            })
            .collect();
        assert!(
            !pace_nudges(&steady, now)
                .iter()
                .any(|n| matches!(n, Nudge::Burst { .. }))
        );
    }

    #[test]
    fn untouched_plans_and_nudge_texts() {
        assert!(untouched(&[state("A", StepStatus::Pending)]));
        assert!(!untouched(&[state("A", StepStatus::InProgress)]));
        assert!(!untouched(&[state("A", StepStatus::Skipped)]));
        assert!(!untouched(&[]));
        let all = [
            Nudge::CoarsePlan { count: 1 },
            Nudge::FinePlan { count: 20 },
            Nudge::PhaseTitles {
                titles: vec!["Test".to_owned()],
            },
            Nudge::HeavyPlan { heavy: 2 },
            Nudge::NotStarted {
                next: "A".to_owned(),
            },
            Nudge::ManyOpen {
                titles: vec!["A".to_owned(), "B".to_owned()],
            },
            Nudge::Stale {
                title: "A".to_owned(),
                minutes: 90,
            },
            Nudge::Burst {
                count: 3,
                percent: 60,
            },
        ];
        for nudge in &all {
            let text = nudge.to_string();
            assert!(text.ends_with('.'), "{text}");
            assert!(!nudge.key().is_empty());
        }
        assert!(all[2].to_string().starts_with("\"Test\" names a phase"));
        assert_eq!(
            nudges(
                &[state("Implement", StepStatus::Pending)],
                Timestamp(0),
                false
            )
            .len(),
            3
        );
    }
}
