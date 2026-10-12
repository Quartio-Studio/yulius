//! Task intake (SPEC-001 TSK-011, TSK-013; SPEC-002 ADR-16): the fixed Jev Choice questions and
//! their thresholds, the prompts that always pass, the bounded texts Jev reads, and the checks a
//! split proposal must pass before anyone sees it.
//!
//! Everything here is deterministic. A question has a fixed option set; a verdict is a fixed
//! threshold on the validated probabilities; anything uncertain resolves to the default ("one
//! task", "context"). Predictability matters more than catching everything.

/// The exact Jev model (TypeSafe's Choice evaluator). Never `jev-latest`.
pub const JEV_MODEL: &str = "jev-1.13.0";

/// One option of a Choice question (fixed text, or built from stored records such as the
/// sandbox's workflows for the Auto workflow question).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChoiceOption<'a> {
    /// Option key (`^[a-zA-Z][a-zA-Z0-9_-]{0,63}$`).
    pub key: &'a str,
    /// What the option means, as Jev reads it.
    pub description: &'a str,
}

/// A Choice question with a fixed text: one question per Jev request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChoiceQuestion<'a> {
    /// Question key.
    pub id: &'a str,
    /// The question.
    pub instructions: &'a str,
    /// The complete option set (2 or more).
    pub options: &'a [ChoiceOption<'a>],
}

impl ChoiceQuestion<'_> {
    /// Whether `key` is one of the options.
    #[must_use]
    pub fn has_option(&self, key: &str) -> bool {
        self.options.iter().any(|o| o.key == key)
    }
}

/// "One task" or "several workstreams" for a new task's prompt (TSK-011).
pub const SPLIT_QUESTION: ChoiceQuestion<'static> = ChoiceQuestion {
    id: "split",
    instructions: "A person asks a coding agent for the work below. Is it one coherent thread of \
        work, or does it mix clearly separate workstreams in different areas of the product (for \
        example user-interface work and backend work) that could be built and reviewed \
        independently? A numbered or bulleted list of items in the same area or thread is still \
        one task.",
    options: &[
        ChoiceOption {
            key: "one",
            description: "One task: a single coherent thread of work, even when it lists several \
                steps, fixes or items",
        },
        ChoiceOption {
            key: "several",
            description: "Several tasks: clearly separate workstreams in different areas that \
                could be built and reviewed independently",
        },
    ],
};

/// How a prompt typed into a running task relates to the task's scope (TSK-013).
pub const SCOPE_QUESTION: ChoiceQuestion<'static> = ChoiceQuestion {
    id: "scope",
    instructions: "A coding agent works on the task described under TASK. A person just typed the \
        message under NEW MESSAGE into the agent's session. How does the message relate to the \
        task's scope?",
    options: &[
        ChoiceOption {
            key: "context",
            description: "Context: a clarification, detail, answer, correction or instruction \
                within the current task",
        },
        ChoiceOption {
            key: "new_scope",
            description: "New scope: asks for an extra feature or extra work that the task does \
                not include",
        },
        ChoiceOption {
            key: "drops_scope",
            description: "Drops scope: rules out or cancels part of the current task, without \
                replacing the whole task",
        },
        ChoiceOption {
            key: "redirect",
            description: "Redirect: replaces the task with a substantially different goal, so \
                most of the current work no longer applies",
        },
    ],
};

/// A Choice answer whose contract the adapter already checked: `choice` is an option and
/// `probabilities` holds every option exactly once.
#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceAnswer {
    /// The provider's typed choice.
    pub choice: String,
    /// The provider's confidence (0–1).
    pub confidence: f64,
    /// Probability per option key.
    pub probabilities: Vec<(String, f64)>,
}

impl ChoiceAnswer {
    /// Probability of `key` (0 when absent).
    #[must_use]
    pub fn probability(&self, key: &str) -> f64 {
        self.probabilities
            .iter()
            .find(|(k, _)| k == key)
            .map_or(0.0, |(_, p)| *p)
    }

    /// The option with the highest probability (ties: the earlier one).
    #[must_use]
    pub fn top(&self) -> Option<(&str, f64)> {
        let mut best: Option<(&str, f64)> = None;
        for (key, p) in &self.probabilities {
            if best.is_none_or(|(_, b)| *p > b) {
                best = Some((key.as_str(), *p));
            }
        }
        best
    }

    /// `Some(key)` when the typed choice is also the most probable option with at least
    /// `threshold`; `None` when the answer is uncertain (a choice that disagrees with its own
    /// distribution counts as uncertain).
    #[must_use]
    pub fn confident(&self, threshold: f64) -> Option<&str> {
        let (top, p) = self.top()?;
        (top == self.choice && p >= threshold).then_some(top)
    }
}

/// Default probability of "several" needed to propose a split: conservative (one task wins
/// unless Jev is sure the request mixes separate workstreams).
pub const DEFAULT_SPLIT_THRESHOLD: f64 = 0.9;
/// Default probability needed to hold a prompt as anything but context.
pub const DEFAULT_SCOPE_THRESHOLD: f64 = 0.8;
/// Prompts with fewer words are one task without asking Jev.
pub const SPLIT_MIN_WORDS: usize = 40;
/// Prompts of at most this many words pass into a running task without asking Jev ("yes",
/// "continue", "use option 2").
pub const SHORT_PROMPT_WORDS: usize = 3;
/// A prompt counts as a person's only when someone typed into the task's terminal this
/// recently; other turns are Claude Code's own (goal continuations, background agents) or
/// Yulius's (the first prompt, continuations).
pub const TYPED_WINDOW_MS: i64 = 60_000;
/// Largest task goal, user prompt and step list given to Jev (each), in bytes.
pub const STATE_PART_BYTES: usize = 8 * 1024;
/// Most steps listed in a scope question.
pub const STATE_MAX_STEPS: usize = 40;

/// The verdict on a new task's prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitVerdict {
    /// Keep one task (the default).
    One,
    /// Propose a split.
    Several,
}

impl SplitVerdict {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::One => "one",
            Self::Several => "several",
        }
    }
}

/// "Several" only when Jev is confident (see [`ChoiceAnswer::confident`]).
#[must_use]
pub fn split_verdict(answer: &ChoiceAnswer, threshold: f64) -> SplitVerdict {
    match answer.confident(threshold) {
        Some("several") => SplitVerdict::Several,
        _ => SplitVerdict::One,
    }
}

/// Whether a new task's prompt is long enough to be checked for a split.
#[must_use]
pub fn needs_split_check(prompt: &str) -> bool {
    prompt.split_whitespace().count() >= SPLIT_MIN_WORDS
}

/// The class of a prompt typed into a running task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptClass {
    /// Within scope; passes untouched.
    Context,
    /// Extra work; held.
    NewScope,
    /// Rules out part of the task; held.
    DropsScope,
    /// A different task; held.
    Redirect,
}

impl PromptClass {
    /// Wire name (the Jev option key).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::NewScope => "new_scope",
            Self::DropsScope => "drops_scope",
            Self::Redirect => "redirect",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "context" => Some(Self::Context),
            "new_scope" => Some(Self::NewScope),
            "drops_scope" => Some(Self::DropsScope),
            "redirect" => Some(Self::Redirect),
            _ => None,
        }
    }

    /// The short reason Claude Code shows the person (never the agent) when the prompt is held.
    #[must_use]
    pub fn held_reason(self) -> &'static str {
        match self {
            Self::Context => "",
            Self::NewScope => {
                "Held by Yulius: this looks like new work outside this task. Choose in the card \
                 above the terminal: start it in parallel, run it after this task, or send it \
                 anyway."
            }
            Self::DropsScope => {
                "Held by Yulius: this rules out part of this task. Choose in the card above the \
                 terminal: skip those steps and run the new direction after this task, or send it \
                 anyway."
            }
            Self::Redirect => {
                "Held by Yulius: this looks like a different task. Choose in the card above the \
                 terminal: cancel and revert this task and start the new one, or send it anyway."
            }
        }
    }
}

/// The class of a typed prompt: the confident non-context option, else context.
#[must_use]
pub fn scope_class(answer: &ChoiceAnswer, threshold: f64) -> PromptClass {
    answer
        .confident(threshold)
        .and_then(PromptClass::parse)
        .unwrap_or(PromptClass::Context)
}

/// Why a typed prompt passes without asking Jev.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bypass {
    /// Empty after trimming.
    Empty,
    /// A slash command (`/goal`, `/clear`, `/compact`, `/model` …): always Claude Code's.
    SlashCommand,
    /// At most [`SHORT_PROMPT_WORDS`] words: answers, confirmations, "continue".
    Short,
    /// Claude Code's own structured turn (a background agent's report and similar), which
    /// starts with a tag such as `<task-notification>`.
    HarnessTurn,
}

/// The fixed pass-through rules, applied before any Jev call.
#[must_use]
pub fn bypass(prompt: &str) -> Option<Bypass> {
    let text = prompt.trim();
    if text.is_empty() {
        return Some(Bypass::Empty);
    }
    if text.starts_with('/') {
        return Some(Bypass::SlashCommand);
    }
    if text.split_whitespace().count() <= SHORT_PROMPT_WORDS {
        return Some(Bypass::Short);
    }
    if harness_tag(text) {
        return Some(Bypass::HarnessTurn);
    }
    None
}

/// `<name>` or `<name ...>` at the start, with an ASCII tag name.
fn harness_tag(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('<') else {
        return false;
    };
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && rest[name.len()..].starts_with(['>', ' '])
}

/// Cuts `text` to at most `max` bytes on a character boundary, marking the cut.
#[must_use]
pub fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max.saturating_sub(4);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} […]", &text[..end])
}

/// The text Jev reads for a split check.
#[must_use]
pub fn split_state(prompt: &str) -> String {
    format!("REQUEST\n{}", clip(prompt.trim(), STATE_PART_BYTES * 2))
}

/// The text Jev reads for a scope question: the task (title, goal, steps with status) and the
/// new message.
#[must_use]
pub fn scope_state(title: &str, goal: &str, steps: &[(String, String)], prompt: &str) -> String {
    let mut out = format!(
        "TASK\nTitle: {}\nGoal:\n{}\n",
        clip(title.trim(), 300),
        clip(goal.trim(), STATE_PART_BYTES)
    );
    if !steps.is_empty() {
        let mut list = String::new();
        for (title, status) in steps.iter().take(STATE_MAX_STEPS) {
            list.push_str(&format!("- [{status}] {}\n", clip(title.trim(), 200)));
        }
        out.push_str("Steps:\n");
        out.push_str(&clip(&list, STATE_PART_BYTES));
        out.push('\n');
    }
    out.push_str("\nNEW MESSAGE\n");
    out.push_str(&clip(prompt.trim(), STATE_PART_BYTES));
    out
}

/// Most tasks a split proposal may have; more means "one task per item" and is rejected.
pub const MAX_PROPOSED_TASKS: usize = 4;
/// Longest proposed title kept, in characters.
pub const MAX_PROPOSED_TITLE_CHARS: usize = 80;
/// Largest proposed task prompt.
pub const MAX_PROPOSED_PROMPT_BYTES: usize = 64 * 1024;

/// One task of a split proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedTask {
    /// Title, `Area: what` (for example "UX: layout and terminal fixes").
    pub title: String,
    /// The task's prompt, carrying its items of the original request word for word.
    pub prompt: String,
    /// Whether this task must run after the previous one.
    pub after_previous: bool,
}

/// A checked split proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proposal {
    /// Keep one task (Claude found one thread).
    One,
    /// Two to [`MAX_PROPOSED_TASKS`] tasks.
    Split(Vec<ProposedTask>),
}

/// Why a proposal is not shown.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProposalError {
    /// No tasks.
    #[error("the proposal has no tasks")]
    Empty,
    /// More than [`MAX_PROPOSED_TASKS`].
    #[error("the proposal has {0} tasks; at most {MAX_PROPOSED_TASKS} are shown")]
    TooMany(usize),
    /// One task per listed item instead of workstreams.
    #[error("the proposal makes one task per item instead of grouping them")]
    OnePerItem,
    /// A listed item of the request is in no task (word for word).
    #[error("an item of the request is missing from the proposal")]
    MissingItem,
    /// A task has an empty title or prompt, or an oversized prompt.
    #[error("a proposed task has an empty or oversized title or prompt")]
    BadTask,
}

/// The listed items of a request: lines starting with `1.`, `1)`, `(1)`, `-`, `*` or `•`,
/// without the marker. Used only to check that a proposal keeps every item.
#[must_use]
pub fn list_items(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let rest = strip_marker(line)?;
            let item = normalize(rest);
            (!item.is_empty()).then_some(item)
        })
        .collect()
}

fn strip_marker(line: &str) -> Option<&str> {
    for bullet in ["- ", "* ", "• "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            return Some(rest);
        }
    }
    let (open, body) = match line.strip_prefix('(') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let digits = body.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let after = &body[digits..];
    let after = if open {
        after.strip_prefix(')')?
    } else {
        after
            .strip_prefix('.')
            .or_else(|| after.strip_prefix(')'))?
    };
    after.strip_prefix(' ')
}

/// Whitespace collapsed to single spaces.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Checks a proposal against its request: 2–4 tasks, each with a title and a prompt, never one
/// task per listed item, and every listed item of the request carried word for word by some
/// task. A single task means "keep one". Titles are trimmed and cut to
/// [`MAX_PROPOSED_TITLE_CHARS`]; the first task never runs "after the previous one".
///
/// # Errors
/// [`ProposalError`] when the proposal must not be shown.
pub fn check_proposal(request: &str, tasks: Vec<ProposedTask>) -> Result<Proposal, ProposalError> {
    match tasks.len() {
        0 => return Err(ProposalError::Empty),
        1 => return Ok(Proposal::One),
        n if n > MAX_PROPOSED_TASKS => return Err(ProposalError::TooMany(n)),
        _ => {}
    }
    let mut checked = Vec::with_capacity(tasks.len());
    for (i, task) in tasks.into_iter().enumerate() {
        let title: String = normalize(&task.title)
            .chars()
            .take(MAX_PROPOSED_TITLE_CHARS)
            .collect();
        let prompt = task.prompt.trim().to_owned();
        if title.is_empty() || prompt.is_empty() || prompt.len() > MAX_PROPOSED_PROMPT_BYTES {
            return Err(ProposalError::BadTask);
        }
        checked.push(ProposedTask {
            title,
            prompt,
            after_previous: i > 0 && task.after_previous,
        });
    }
    let items = list_items(request);
    if items.len() >= 3 && checked.len() == items.len() {
        return Err(ProposalError::OnePerItem);
    }
    let prompts: Vec<String> = checked.iter().map(|t| normalize(&t.prompt)).collect();
    if items
        .iter()
        .any(|item| !prompts.iter().any(|p| p.contains(item.as_str())))
    {
        return Err(ProposalError::MissingItem);
    }
    Ok(Proposal::Split(checked))
}

/// How long a "send anyway" stays valid for the identical prompt.
pub const SENT_HOLD_TTL_MS: i64 = 10 * 60 * 1000;

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(choice: &str, probabilities: &[(&str, f64)]) -> ChoiceAnswer {
        ChoiceAnswer {
            choice: choice.to_owned(),
            confidence: 0.9,
            probabilities: probabilities
                .iter()
                .map(|(k, p)| ((*k).to_owned(), *p))
                .collect(),
        }
    }

    #[test]
    fn split_needs_a_confident_several() {
        let sure = answer("several", &[("one", 0.04), ("several", 0.96)]);
        assert_eq!(split_verdict(&sure, 0.9), SplitVerdict::Several);
        let unsure = answer("several", &[("one", 0.15), ("several", 0.85)]);
        assert_eq!(split_verdict(&unsure, 0.9), SplitVerdict::One);
        // A choice that disagrees with its own distribution is uncertain.
        let disagree = answer("one", &[("one", 0.05), ("several", 0.95)]);
        assert_eq!(split_verdict(&disagree, 0.9), SplitVerdict::One);
        assert_eq!(
            split_verdict(&answer("one", &[("one", 0.99), ("several", 0.01)]), 0.9),
            SplitVerdict::One
        );
    }

    #[test]
    fn scope_classes_need_confidence_and_default_to_context() {
        let all = |c: &str, p: f64| {
            let rest = (1.0 - p) / 3.0;
            let mut probs: Vec<(&str, f64)> = ["context", "new_scope", "drops_scope", "redirect"]
                .iter()
                .map(|k| (*k, rest))
                .collect();
            for entry in &mut probs {
                if entry.0 == c {
                    entry.1 = p;
                }
            }
            answer(c, &probs)
        };
        assert_eq!(
            scope_class(&all("new_scope", 0.9), 0.8),
            PromptClass::NewScope
        );
        assert_eq!(
            scope_class(&all("drops_scope", 0.85), 0.8),
            PromptClass::DropsScope
        );
        assert_eq!(
            scope_class(&all("redirect", 0.81), 0.8),
            PromptClass::Redirect
        );
        assert_eq!(
            scope_class(&all("redirect", 0.7), 0.8),
            PromptClass::Context
        );
        assert_eq!(
            scope_class(&all("context", 0.95), 0.8),
            PromptClass::Context
        );
        for class in [
            PromptClass::Context,
            PromptClass::NewScope,
            PromptClass::DropsScope,
            PromptClass::Redirect,
        ] {
            assert_eq!(PromptClass::parse(class.as_str()), Some(class));
            assert!(SCOPE_QUESTION.has_option(class.as_str()));
        }
    }

    #[test]
    fn slash_commands_short_answers_and_harness_turns_pass() {
        for (prompt, expected) in [
            ("", Some(Bypass::Empty)),
            ("   \n", Some(Bypass::Empty)),
            (
                "/goal finish the checkout and test it",
                Some(Bypass::SlashCommand),
            ),
            ("/clear", Some(Bypass::SlashCommand)),
            ("/compact keep the API notes", Some(Bypass::SlashCommand)),
            ("yes", Some(Bypass::Short)),
            ("continue", Some(Bypass::Short)),
            ("use option 2", Some(Bypass::Short)),
            (
                "<task-notification> background agent finished its work </task-notification>",
                Some(Bypass::HarnessTurn),
            ),
            ("also add a dark mode to the settings page", None),
            ("<3 thanks, now also add a dark mode please", None),
        ] {
            assert_eq!(bypass(prompt), expected, "{prompt:?}");
        }
    }

    #[test]
    fn short_requests_skip_the_split_check() {
        assert!(!needs_split_check(
            "fix the login page and add a billing API"
        ));
        assert!(needs_split_check(&"word ".repeat(SPLIT_MIN_WORDS)));
    }

    #[test]
    fn states_are_bounded() {
        let long = "x".repeat(100_000);
        let steps: Vec<(String, String)> = (0..100)
            .map(|i| (format!("step {i} {long}"), "pending".to_owned()))
            .collect();
        let state = scope_state(&long, &long, &steps, &long);
        assert!(state.len() < 4 * STATE_PART_BYTES, "{}", state.len());
        assert!(state.contains("NEW MESSAGE"));
        assert!(split_state(&long).len() <= 2 * STATE_PART_BYTES + 16);
        assert_eq!(clip("żółw", 3), " […]");
        assert_eq!(clip("abc", 3), "abc");
    }

    /// Michał's own 10-item goal of 2026-10-10, shaped: UI fixes plus backend features.
    const TEN_ITEMS: &str = "Make it the final state.\n\
        1. The attention bar must be opaque over the terminal.\n\
        2. The tools rail pills overflow the rail.\n\
        3. Let the editor wrap lines.\n\
        4. The task terminal should appear once at its final size.\n\
        5. Messages panel shows the wrong count.\n\
        6. Subscription pool: route new tasks past exhausted accounts.\n\
        7. Make Done reliable on the server.\n\
        8. Task intake with Jev split detection.\n\
        9. Chaining tasks to run after the current one.\n\
        10. Usage refresh must honour Retry-After.";

    fn two_workstreams() -> Vec<ProposedTask> {
        let items: Vec<&str> = TEN_ITEMS.lines().skip(1).collect();
        vec![
            ProposedTask {
                title: "UX: layout, terminal and messages fixes".into(),
                prompt: items[..5].join("\n"),
                after_previous: false,
            },
            ProposedTask {
                title: "Backend: subscription pool, Done, intake and chaining".into(),
                prompt: items[5..].join("\n"),
                after_previous: false,
            },
        ]
    }

    #[test]
    fn a_ten_item_goal_becomes_two_workstreams() -> Result<(), ProposalError> {
        assert_eq!(list_items(TEN_ITEMS).len(), 10);
        match check_proposal(TEN_ITEMS, two_workstreams())? {
            Proposal::Split(tasks) => {
                assert_eq!(tasks.len(), 2);
                assert!(tasks[0].title.starts_with("UX"));
            }
            Proposal::One => return Err(ProposalError::Empty),
        }
        Ok(())
    }

    #[test]
    fn one_task_per_item_too_many_and_dropped_items_are_rejected() {
        let per_item: Vec<ProposedTask> = TEN_ITEMS
            .lines()
            .skip(1)
            .map(|line| ProposedTask {
                title: line.chars().take(30).collect(),
                prompt: line.to_owned(),
                after_previous: false,
            })
            .collect();
        assert_eq!(
            check_proposal(TEN_ITEMS, per_item.clone()),
            Err(ProposalError::TooMany(10))
        );
        // Three items, three tasks: one per item even under the cap.
        let three = "Do this:\n1. add a field\n2. migrate the table\n3. show it in the UI";
        let per_three: Vec<ProposedTask> = three
            .lines()
            .skip(1)
            .map(|line| ProposedTask {
                title: "Step".into(),
                prompt: line.to_owned(),
                after_previous: true,
            })
            .collect();
        assert_eq!(
            check_proposal(three, per_three),
            Err(ProposalError::OnePerItem)
        );
        let mut dropped = two_workstreams();
        dropped[1].prompt = "Subscription pool work, rewritten in other words.".into();
        assert_eq!(
            check_proposal(TEN_ITEMS, dropped),
            Err(ProposalError::MissingItem)
        );
        let mut blank = two_workstreams();
        blank[0].title = "  ".into();
        assert_eq!(
            check_proposal(TEN_ITEMS, blank),
            Err(ProposalError::BadTask)
        );
        assert_eq!(
            check_proposal(TEN_ITEMS, Vec::new()),
            Err(ProposalError::Empty)
        );
    }

    #[test]
    fn a_single_task_means_keep_one_and_the_first_task_never_waits() -> Result<(), ProposalError> {
        let one = vec![ProposedTask {
            title: "All of it".into(),
            prompt: TEN_ITEMS.into(),
            after_previous: true,
        }];
        assert_eq!(check_proposal(TEN_ITEMS, one)?, Proposal::One);
        let mut chained = two_workstreams();
        chained[0].after_previous = true;
        chained[1].after_previous = true;
        let Proposal::Split(tasks) = check_proposal(TEN_ITEMS, chained)? else {
            return Err(ProposalError::Empty);
        };
        assert!(!tasks[0].after_previous);
        assert!(tasks[1].after_previous);
        Ok(())
    }

    #[test]
    fn list_items_reads_common_markers() {
        let text = "Intro\n(1) first thing\n2) second  thing\n3. third\n- dash\n* star\n• dot\n10x faster\n";
        assert_eq!(
            list_items(text),
            vec![
                "first thing",
                "second thing",
                "third",
                "dash",
                "star",
                "dot"
            ]
        );
    }
}
