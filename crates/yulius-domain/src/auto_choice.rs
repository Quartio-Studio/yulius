//! "Auto" mode and workflow for a new task (SPEC-001 TSK-015; SPEC-002 ADR-16, item 5): two
//! Jev Choice questions with a fixed text and fixed thresholds, and the predictable defaults when
//! Jev is not configured, fails or is unsure.
//!
//! - *Mode:* `goal` or `prompt`. Jev's confident answer wins; otherwise Goal.
//! - *Workflow:* one of the sandbox's workflows — the defaults of the catalog, then instance and
//!   sandbox workflows, as `App::workflows_for` lists them — offered as options `w1`…`wN` with
//!   their names, whether they need a spec, descriptions, when to use them and steps, or `none`.
//!   Jev's confident answer wins; otherwise the workflow named "Feature" (the sandbox's own
//!   first) or none. A prompt-mode task is never given a spec-first workflow by Auto: features
//!   need specs; research, questions about the product and explanations do not.
//!
//! Everything here is deterministic; the caller asks Jev and passes the validated answers.

use crate::intake::{ChoiceAnswer, ChoiceOption, ChoiceQuestion, clip};

/// The wire value that asks for Auto (`mode` and `workflow_id` of a new task).
pub const AUTO: &str = "auto";

/// Probability Jev must give its mode before Auto follows it.
pub const MODE_THRESHOLD: f64 = 0.75;
/// Probability Jev must give its workflow (or "none") before Auto follows it.
pub const WORKFLOW_THRESHOLD: f64 = 0.6;
/// Most workflows offered to Jev (the first ones in the store's order: instance, then sandbox).
pub const MAX_WORKFLOW_OPTIONS: usize = 12;
/// Longest option text for one workflow, in bytes (before the cut marker). The name and whether
/// it needs a spec come first, so a cut never drops them.
pub const WORKFLOW_OPTION_BYTES: usize = 400;
/// The workflow Auto falls back to, by name (case-insensitive).
pub const DEFAULT_WORKFLOW_NAME: &str = "Feature";
/// The option key for "no workflow".
pub const NO_WORKFLOW_KEY: &str = "none";
/// The workflow question's key.
pub const WORKFLOW_QUESTION_ID: &str = "workflow";

/// Goal or prompt for a new task (TSK-010), asked when the mode is Auto.
pub const MODE_QUESTION: ChoiceQuestion<'static> = ChoiceQuestion {
    id: "mode",
    instructions: "A person starts a coding agent on the request below. Should the agent take it \
        as a goal (an outcome to build, change, fix or set up, worked on until it is done and \
        verified) or as a prompt (a question, research, an analysis, an explanation or a review \
        that the agent answers or reports on, without being asked to change the product)?",
    options: &[
        ChoiceOption {
            key: "goal",
            description: "Goal: build, change, fix, set up or deliver something, and keep \
                working until it is done and verified",
        },
        ChoiceOption {
            key: "prompt",
            description: "Prompt: a question, research, analysis, explanation, review or \
                discussion to answer or report on; no change to the product is asked for",
        },
    ],
};

/// The workflow question's text; its options are the sandbox's workflows plus "none".
pub const WORKFLOW_INSTRUCTIONS: &str = "A person starts a coding agent on the request below. \
    Which of this project's workflows fits it best? Each option names a workflow, whether it \
    needs a spec first, what it is for and when to use it. Features and other changes to the \
    product need a spec first; research, questions about the product, explanations and reviews \
    do not, so never pick a spec-first workflow for them. Choose none when no workflow clearly \
    fits.";

/// What "none" means to Jev.
const NO_WORKFLOW_DESCRIPTION: &str = "None: no workflow fits; the agent plans the work itself";

/// The mode Auto resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeChoice {
    /// The prompt becomes the session's goal.
    Goal,
    /// The prompt is sent as the first message.
    Prompt,
}

impl ModeChoice {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Goal => "goal",
            Self::Prompt => "prompt",
        }
    }
}

/// Who made an Auto choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChosenBy {
    /// Jev answered at or above the threshold.
    Jev,
    /// The fixed default: Jev not configured, failing or unsure, or a rule overrode its answer.
    Default,
}

impl ChosenBy {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jev => "jev",
            Self::Default => "default",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "jev" => Some(Self::Jev),
            "default" => Some(Self::Default),
            _ => None,
        }
    }
}

/// The mode: Jev's confident answer, else Goal.
#[must_use]
pub fn mode_choice(answer: Option<&ChoiceAnswer>) -> (ModeChoice, ChosenBy) {
    match answer.and_then(|a| a.confident(MODE_THRESHOLD)) {
        Some("prompt") => (ModeChoice::Prompt, ChosenBy::Jev),
        Some("goal") => (ModeChoice::Goal, ChosenBy::Jev),
        _ => (ModeChoice::Goal, ChosenBy::Default),
    }
}

/// A workflow Auto may choose, as stored.
#[derive(Debug, Clone, Copy)]
pub struct WorkflowCandidate<'a> {
    /// Name.
    pub name: &'a str,
    /// What it is for.
    pub description: &'a str,
    /// When to pick it, in one line (the catalog's defaults; empty for the others).
    pub when_to_use: &'a str,
    /// Its initial steps.
    pub steps: &'a [String],
    /// Whether it requires a spec before delivery.
    pub require_spec: bool,
    /// Defined by the sandbox itself (its "Feature" wins over the instance's).
    pub own: bool,
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The workflow question's options as `(key, description)`: `w1`…`wN` for the first
/// [`MAX_WORKFLOW_OPTIONS`] candidates — "Name (spec first|no spec): description Use when: …
/// Steps: a → b." — then `none`.
#[must_use]
pub fn workflow_options(candidates: &[WorkflowCandidate<'_>]) -> Vec<(String, String)> {
    let mut options: Vec<(String, String)> = candidates
        .iter()
        .take(MAX_WORKFLOW_OPTIONS)
        .enumerate()
        .map(|(i, c)| {
            let gate = if c.require_spec {
                "spec first"
            } else {
                "no spec"
            };
            let mut text = format!("{} ({gate}):", one_line(c.name));
            for (label, part) in [("", c.description), ("Use when: ", c.when_to_use)] {
                let part = one_line(part);
                if !part.is_empty() {
                    text.push_str(&format!(" {label}{part}"));
                }
            }
            let steps: Vec<String> = c.steps.iter().map(|s| one_line(s)).collect();
            if !steps.is_empty() {
                text.push_str(&format!(" Steps: {}.", steps.join(" → ")));
            }
            (format!("w{}", i + 1), clip(&text, WORKFLOW_OPTION_BYTES))
        })
        .collect();
    options.push((
        NO_WORKFLOW_KEY.to_owned(),
        NO_WORKFLOW_DESCRIPTION.to_owned(),
    ));
    options
}

/// The workflow question over options built by [`workflow_options`] (borrowed).
#[must_use]
pub fn workflow_question<'a>(options: &'a [ChoiceOption<'a>]) -> ChoiceQuestion<'a> {
    ChoiceQuestion {
        id: WORKFLOW_QUESTION_ID,
        instructions: WORKFLOW_INSTRUCTIONS,
        options,
    }
}

/// The workflow Auto falls back to: none for a prompt-mode task, else the workflow named
/// [`DEFAULT_WORKFLOW_NAME`] (the sandbox's own first), else none.
#[must_use]
pub fn default_workflow(candidates: &[WorkflowCandidate<'_>], mode: ModeChoice) -> Option<usize> {
    if mode == ModeChoice::Prompt {
        return None;
    }
    let named =
        |c: &WorkflowCandidate<'_>| c.name.trim().eq_ignore_ascii_case(DEFAULT_WORKFLOW_NAME);
    candidates
        .iter()
        .position(|c| c.own && named(c))
        .or_else(|| candidates.iter().position(named))
}

/// The workflow Auto resolves to (an index into `candidates`, or none) for a task in `mode`:
/// Jev's confident answer, unless it gives a prompt-mode task a spec-first workflow; otherwise
/// [`default_workflow`].
#[must_use]
pub fn workflow_choice(
    answer: Option<&ChoiceAnswer>,
    candidates: &[WorkflowCandidate<'_>],
    mode: ModeChoice,
) -> (Option<usize>, ChosenBy) {
    let offered = candidates.len().min(MAX_WORKFLOW_OPTIONS);
    let jev = answer
        .and_then(|a| a.confident(WORKFLOW_THRESHOLD))
        .and_then(|key| {
            if key == NO_WORKFLOW_KEY {
                return Some(None);
            }
            let index = key
                .strip_prefix('w')?
                .parse::<usize>()
                .ok()?
                .checked_sub(1)?;
            (index < offered).then_some(Some(index))
        });
    match jev {
        Some(Some(index)) if mode == ModeChoice::Prompt && candidates[index].require_spec => {
            (None, ChosenBy::Default)
        }
        Some(choice) => (choice, ChosenBy::Jev),
        None => (default_workflow(candidates, mode), ChosenBy::Default),
    }
}

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

    /// Feature (spec first), Quick task, Research (no spec), as a sandbox might list them.
    fn steps() -> Vec<Vec<String>> {
        vec![
            vec!["Write spec with plan".into(), "Implement".into()],
            Vec::new(),
            vec!["Investigate".into(), "Report".into()],
        ]
    }

    fn candidates(steps: &[Vec<String>]) -> Vec<WorkflowCandidate<'_>> {
        vec![
            WorkflowCandidate {
                name: "Feature",
                description: "Spec first, then implementation.",
                when_to_use: "The task adds or changes behaviour.",
                steps: &steps[0],
                require_spec: true,
                own: false,
            },
            WorkflowCandidate {
                name: "Quick task",
                description: "A small change.",
                when_to_use: "",
                steps: &steps[1],
                require_spec: false,
                own: false,
            },
            WorkflowCandidate {
                name: "Research",
                description: "Answers a question about the product.",
                when_to_use: "",
                steps: &steps[2],
                require_spec: false,
                own: true,
            },
        ]
    }

    #[test]
    fn the_mode_follows_a_confident_jev_and_defaults_to_goal() {
        let prompt = answer("prompt", &[("goal", 0.1), ("prompt", 0.9)]);
        assert_eq!(
            mode_choice(Some(&prompt)),
            (ModeChoice::Prompt, ChosenBy::Jev)
        );
        let goal = answer("goal", &[("goal", 0.8), ("prompt", 0.2)]);
        assert_eq!(mode_choice(Some(&goal)), (ModeChoice::Goal, ChosenBy::Jev));
        let unsure = answer("prompt", &[("goal", 0.3), ("prompt", 0.7)]);
        assert_eq!(
            mode_choice(Some(&unsure)),
            (ModeChoice::Goal, ChosenBy::Default)
        );
        // A choice that disagrees with its own distribution is uncertain.
        let disagree = answer("prompt", &[("goal", 0.9), ("prompt", 0.1)]);
        assert_eq!(
            mode_choice(Some(&disagree)),
            (ModeChoice::Goal, ChosenBy::Default)
        );
        assert_eq!(mode_choice(None), (ModeChoice::Goal, ChosenBy::Default));
        for option in MODE_QUESTION.options {
            assert!(["goal", "prompt"].contains(&option.key));
        }
    }

    #[test]
    fn workflow_options_name_each_workflow_and_end_with_none() {
        let steps = steps();
        let options = workflow_options(&candidates(&steps));
        let keys: Vec<&str> = options.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["w1", "w2", "w3", "none"]);
        assert_eq!(
            options[0].1,
            "Feature (spec first): Spec first, then implementation. Use when: The task adds or \
             changes behaviour. Steps: Write spec with plan → Implement."
        );
        assert_eq!(options[1].1, "Quick task (no spec): A small change.");
        let many: Vec<WorkflowCandidate<'_>> = (0..30)
            .map(|_| WorkflowCandidate {
                name: "Long",
                description: "x ",
                when_to_use: "",
                steps: &steps[0],
                require_spec: false,
                own: false,
            })
            .collect();
        let options = workflow_options(&many);
        assert_eq!(options.len(), MAX_WORKFLOW_OPTIONS + 1);
        let long_text = "word ".repeat(200);
        let long = [WorkflowCandidate {
            name: "Long",
            description: &long_text,
            when_to_use: "",
            steps: &[],
            require_spec: true,
            own: false,
        }];
        // Cut on a character boundary with a short marker (`clip`); the gate survives the cut.
        let cut = &workflow_options(&long)[0].1;
        assert!(cut.len() <= WORKFLOW_OPTION_BYTES + 8);
        assert!(cut.starts_with("Long (spec first): word"));
        // Option keys follow Jev's key pattern.
        for (key, _) in workflow_options(&many) {
            assert!(key.starts_with(|c: char| c.is_ascii_alphabetic()) && key.len() <= 64);
        }
    }

    #[test]
    fn the_workflow_follows_a_confident_jev() {
        let steps = steps();
        let list = candidates(&steps);
        let pick = |key: &str, p: f64| {
            let mut probs: Vec<(&str, f64)> = ["w1", "w2", "w3", "none"]
                .iter()
                .map(|k| (*k, (1.0 - p) / 3.0))
                .collect();
            for entry in &mut probs {
                if entry.0 == key {
                    entry.1 = p;
                }
            }
            answer(key, &probs)
        };
        assert_eq!(
            workflow_choice(Some(&pick("w2", 0.8)), &list, ModeChoice::Goal),
            (Some(1), ChosenBy::Jev)
        );
        assert_eq!(
            workflow_choice(Some(&pick("none", 0.7)), &list, ModeChoice::Goal),
            (None, ChosenBy::Jev)
        );
        // A research question gets the no-spec workflow Jev picked.
        assert_eq!(
            workflow_choice(Some(&pick("w3", 0.9)), &list, ModeChoice::Prompt),
            (Some(2), ChosenBy::Jev)
        );
        // Never a spec-first workflow for a prompt-mode task.
        assert_eq!(
            workflow_choice(Some(&pick("w1", 0.95)), &list, ModeChoice::Prompt),
            (None, ChosenBy::Default)
        );
        // An unknown key is no answer.
        assert_eq!(
            workflow_choice(Some(&pick("w9", 0.95)), &list, ModeChoice::Goal),
            (Some(0), ChosenBy::Default)
        );
    }

    #[test]
    fn unsure_or_missing_answers_fall_back_to_feature_or_none() {
        let steps = steps();
        let list = candidates(&steps);
        let unsure = answer(
            "w3",
            &[("w1", 0.2), ("w2", 0.1), ("w3", 0.5), ("none", 0.2)],
        );
        assert_eq!(
            workflow_choice(Some(&unsure), &list, ModeChoice::Goal),
            (Some(0), ChosenBy::Default)
        );
        assert_eq!(
            workflow_choice(None, &list, ModeChoice::Goal),
            (Some(0), ChosenBy::Default)
        );
        // Prompt mode: no workflow by default.
        assert_eq!(
            workflow_choice(None, &list, ModeChoice::Prompt),
            (None, ChosenBy::Default)
        );
        // No workflow named Feature: none.
        assert_eq!(
            workflow_choice(None, &list[1..], ModeChoice::Goal),
            (None, ChosenBy::Default)
        );
        assert_eq!(
            workflow_choice(None, &[], ModeChoice::Goal),
            (None, ChosenBy::Default)
        );
        // The sandbox's own "feature" wins over the instance's, whatever the case.
        let mut own = list.clone();
        own.push(WorkflowCandidate {
            name: " feature ",
            description: "",
            when_to_use: "",
            steps: &steps[1],
            require_spec: true,
            own: true,
        });
        assert_eq!(default_workflow(&own, ModeChoice::Goal), Some(3));
        assert_eq!(ChosenBy::parse(ChosenBy::Jev.as_str()), Some(ChosenBy::Jev));
        assert_eq!(
            ChosenBy::parse(ChosenBy::Default.as_str()),
            Some(ChosenBy::Default)
        );
        assert_eq!(ChosenBy::parse("person"), None);
    }
}
