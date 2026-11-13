//! A session's dynamic workflows (Claude Code's Workflow tool, "ultracode") as Yulius knows
//! them (SPEC-486, ADR-0326).
//!
//! A workflow runs in the session's process, its agents beside the session's own thread. A
//! usage limit that refuses those agents pauses the workflow without waking the thread, and a
//! process that ends (a move to another subscription) ends the workflow with it. Claude Code
//! keeps every finished agent's result in the run's journal and resumes a run by its id, in the
//! same session, with `Workflow({scriptPath, resumeFromRunId})`: only the unfinished agents are
//! asked again. So Yulius remembers which runs a session launched and did not see end, and
//! says so to the session's agent when it relaunches the session past a limit, or when a
//! prompt reaches a session whose workflow is paused at one.

use crate::ports::harness::Observation;

/// Most unfinished runs remembered for one session: the oldest go first.
pub const MAX_RUNS: usize = 8;

/// The workflow runs a session launched whose end was not read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unfinished {
    /// Claude Code's task id of each launch and its run id, oldest first.
    runs: Vec<(String, String)>,
    /// A usage limit was met while they were unfinished, and nothing relaunched them since.
    at_limit: bool,
}

impl Unfinished {
    /// Folds one observation of the session's transcript, in its order: a launch (a relaunch of
    /// a run by its id is the run going on), the notification that ends a launch, a limit.
    pub fn observe(&mut self, observation: &Observation) {
        match observation {
            Observation::WorkflowLaunched { task, run } => {
                self.runs.retain(|(known, _)| known != task);
                self.runs.push((task.clone(), run.clone()));
                if self.runs.len() > MAX_RUNS {
                    self.runs.remove(0);
                }
                self.at_limit = false;
            }
            Observation::BackgroundEnded { id } => {
                self.runs.retain(|(task, _)| task != id);
                self.at_limit &= !self.runs.is_empty();
            }
            Observation::LimitReached { .. } | Observation::AccessLost { .. } => {
                self.at_limit = !self.runs.is_empty();
            }
            _ => {}
        }
    }

    /// The unfinished runs' ids, each once, oldest first.
    #[must_use]
    pub fn runs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (_, run) in &self.runs {
            if !out.contains(run) {
                out.push(run.clone());
            }
        }
        out
    }

    /// The runs a usage limit paused, when one did and nothing relaunched them since.
    #[must_use]
    pub fn paused(&self) -> Option<Vec<String>> {
        (self.at_limit && !self.runs.is_empty()).then(|| self.runs())
    }

    /// The unfinished runs, forgotten: the session's process ends, and they with it.
    pub fn take(&mut self) -> Vec<String> {
        let runs = self.runs();
        *self = Self::default();
        runs
    }
}

/// `runs` as the prompt names them: "run wf_1" or "runs wf_1 and wf_2".
fn named(runs: &[String]) -> String {
    match runs {
        [one] => format!("run {one}"),
        _ => format!("runs {}", runs.join(" and ")),
    }
}

/// What a session relaunched while workflow `runs` of it were unfinished is told (WFL-004), as
/// the prompt its new process opens with, or after the prompt it would open with (`after`).
/// Claude Code neither resumes them by itself nor, when the session's own turn had ended, has a
/// turn to continue.
#[must_use]
pub fn resume_prompt(after: Option<&str>, runs: &[String]) -> String {
    let told = format!(
        "Yulius continued this session in a new process (a move to another Claude subscription \
         or a restart), and its dynamic workflow ({}) stopped with the previous one. The results \
         of its finished agents are kept. Resume it now with the Workflow tool, giving the \
         scriptPath and the run id (resumeFromRunId) of its launch: finished agents return \
         their kept results and only the others run again. Do not start the workflow again \
         from the beginning. Then continue the task where you left off.",
        named(runs)
    );
    match after.map(str::trim).filter(|prompt| !prompt.is_empty()) {
        Some(prompt) => format!("{prompt}\n\n{told}"),
        None => told,
    }
}

/// What a prompt is told that reaches a session whose workflow `runs` are paused at a usage
/// limit (WFL-006): the session's agent should not start them again.
#[must_use]
pub fn paused_context(runs: &[String]) -> String {
    format!(
        "Note from Yulius: this session's dynamic workflow ({}) is paused at a usage limit. \
         Claude Code continues it by itself when the limit resets, and Yulius moves the session \
         to another subscription when one can run it. Do not start that workflow again from the \
         beginning: the results of its finished agents are kept. To continue it yourself, stop \
         the paused run and call the Workflow tool with its scriptPath and its run id \
         (resumeFromRunId).",
        named(runs)
    )
}

/// What the terminal says of the workflow when Yulius relaunches the session.
pub const RESUMED_NOTICE: &str = " Its agent is asked to resume the unfinished dynamic workflow.";

#[cfg(test)]
mod tests {
    use super::*;

    fn launched(task: &str, run: &str) -> Observation {
        Observation::WorkflowLaunched {
            task: task.into(),
            run: run.into(),
        }
    }

    fn ended(id: &str) -> Observation {
        Observation::BackgroundEnded { id: id.into() }
    }

    fn limit() -> Observation {
        Observation::LimitReached {
            resets_at: None,
            message: "limit".into(),
            window_minutes: None,
            scope: yulius_domain::usage::LimitScope::Account,
            at: None,
        }
    }

    fn fold(observations: &[Observation]) -> Unfinished {
        let mut unfinished = Unfinished::default();
        for observation in observations {
            unfinished.observe(observation);
        }
        unfinished
    }

    /// WFL-003: a launch is unfinished until the notification of its end; another task's end
    /// (an agent's, a Monitor's) leaves it.
    #[test]
    fn a_run_is_unfinished_from_its_launch_to_its_end() {
        let one = fold(&[launched("w1", "wf_a")]);
        assert_eq!(one.runs(), ["wf_a"]);
        assert_eq!(one.paused(), None, "no limit was met");
        assert_eq!(
            fold(&[launched("w1", "wf_a"), ended("a7")]).runs(),
            ["wf_a"]
        );
        assert!(
            fold(&[launched("w1", "wf_a"), ended("w1")])
                .runs()
                .is_empty()
        );
        // A run resumed by its id has a launch of its own: named once.
        let resumed = fold(&[launched("w1", "wf_a"), launched("w2", "wf_a")]);
        assert_eq!(resumed.runs(), ["wf_a"]);
        assert_eq!(
            fold(&[launched("w1", "wf_a"), launched("w2", "wf_b")]).runs(),
            ["wf_a", "wf_b"]
        );
    }

    /// WFL-006: a limit met while a run is unfinished pauses it, until the run is relaunched or
    /// ends; a limit with no unfinished run pauses nothing.
    #[test]
    fn a_limit_pauses_the_unfinished_runs_until_they_go_on() {
        assert_eq!(fold(&[limit()]).paused(), None);
        assert_eq!(
            fold(&[launched("w1", "wf_a"), ended("w1"), limit()]).paused(),
            None
        );
        let paused = fold(&[launched("w1", "wf_a"), limit()]);
        assert_eq!(paused.paused(), Some(vec!["wf_a".to_owned()]));
        let relaunched = fold(&[
            launched("w1", "wf_a"),
            limit(),
            ended("w1"),
            launched("w2", "wf_a"),
        ]);
        assert_eq!(
            (relaunched.paused(), relaunched.runs()),
            (None, vec!["wf_a".to_owned()])
        );
        assert_eq!(
            fold(&[launched("w1", "wf_a"), limit(), ended("w1")]).paused(),
            None
        );
        // The process ends: nothing is unfinished in the next one.
        let mut taken = paused;
        assert_eq!(taken.take(), ["wf_a"]);
        assert_eq!(taken, Unfinished::default());
    }

    #[test]
    fn only_the_last_runs_are_remembered() {
        let mut unfinished = Unfinished::default();
        for n in 0..MAX_RUNS + 2 {
            unfinished.observe(&launched(&format!("w{n}"), &format!("wf_{n}")));
        }
        let runs = unfinished.runs();
        assert_eq!(runs.len(), MAX_RUNS);
        assert_eq!(runs.first().map(String::as_str), Some("wf_2"));
    }

    /// WFL-004, WFL-006: the words name the runs and the call that resumes them, and say not to
    /// start again.
    #[test]
    fn the_words_name_the_runs_and_the_resume() {
        let one = resume_prompt(None, &["wf_13b2b557-e88".to_owned()]);
        assert!(one.contains("(run wf_13b2b557-e88)"), "{one}");
        assert!(
            one.contains("resumeFromRunId") && one.contains("scriptPath"),
            "{one}"
        );
        assert!(one.contains("Do not start the workflow again"), "{one}");
        assert!(
            one.ends_with("Then continue the task where you left off."),
            "{one}"
        );
        assert!(!one.contains("  "), "one line, single spaces: {one}");
        // After the prompt the launch would open with, which stays first and whole.
        let after = resume_prompt(Some("Fix the build.\n"), &["wf_a".to_owned()]);
        assert!(
            after.starts_with("Fix the build.\n\nYulius continued"),
            "{after}"
        );
        assert_eq!(
            resume_prompt(Some("  "), &["wf_a".to_owned()]),
            resume_prompt(None, &["wf_a".to_owned()])
        );
        let two = paused_context(&["wf_a".to_owned(), "wf_b".to_owned()]);
        assert!(two.contains("(runs wf_a and wf_b)"), "{two}");
        assert!(two.contains("paused at a usage limit"), "{two}");
        assert!(two.contains("Do not start that workflow again"), "{two}");
        assert!(!two.contains("  "), "{two}");
    }
}
