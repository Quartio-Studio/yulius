//! Flaky tests quarantined with an issue (SPEC-497 OFC-002): the failing tests of every CI job are
//! kept; one that failed in two different pull requests' runs within 48 hours, while the
//! integration branch did not fail it, is quarantined at once with an issue in the project's
//! repository, and every later job of the project is told it in its environment
//! ([`yulius_domain::ci::flaky`]). The quarantine ends when its issue is closed (looked at
//! hourly). A failing test is never run again until it passes: nothing here runs anything.

use yulius_domain::ci::flaky::{
    FLAKY_WINDOW_MS, QUARANTINE_ENV, QUARANTINE_NEXTEST_ENV, is_flaky, nextest_filter,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

use super::{CiJobRecord, CiRunRecord, Quarantined};
use crate::context::App;
use crate::delivery::delivery_profile;
use crate::model::SandboxRecord;
use crate::ports::github::NewIssue;

/// How long a failing test's record is kept: past the window it decides nothing.
const KEEP_FAILURES_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Most tests of one job looked at for a quarantine (a job failing hundreds is broken, not
/// flaky, and each new quarantine files an issue).
const MAX_TESTS_LOOKED_AT: usize = 10;

impl App {
    /// After job `job` of `run` failed with its failing tests' blocks `failures` kept: keeps its
    /// failing tests and quarantines those that are flaky, filing an issue for each (SPEC-497
    /// OFC-002). Returns a line for the job's output per test quarantined now.
    pub(super) async fn ci_note_failing_tests(
        &self,
        sandbox: &SandboxRecord,
        run: &CiRunRecord,
        job: &CiJobRecord,
        failures: &str,
    ) -> Vec<String> {
        let tests = yulius_domain::ci::failures::failed_test_names(failures);
        if tests.is_empty() {
            return Vec::new();
        }
        let store = &self.deps.store;
        let now = self.now();
        let reference = run.reference.key();
        if let Err(err) = store
            .record_test_failures(
                &sandbox.id,
                &job.key,
                (&reference, &run.id),
                &tests,
                now,
                Timestamp(now.0 - KEEP_FAILURES_MS),
            )
            .await
        {
            tracing::warn!(error = %err, "recording a CI job's failing tests failed");
            return Vec::new();
        }
        // Only a pull request's failure makes a test flaky: a branch's is the branch's.
        if !reference.starts_with("pr/") || tests.len() > MAX_TESTS_LOOKED_AT {
            return Vec::new();
        }
        let integration = format!("branch/{}", delivery_profile(sandbox).integration_branch);
        let already: Vec<String> = store
            .quarantined_tests(Some(&sandbox.id))
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|q| q.test)
            .collect();
        let mut said = Vec::new();
        for test in tests {
            if already.contains(&test) {
                continue;
            }
            let Ok(failed) = store
                .test_failures(
                    &sandbox.id,
                    &job.key,
                    &test,
                    Timestamp(now.0 - FLAKY_WINDOW_MS),
                )
                .await
            else {
                continue;
            };
            if !is_flaky(&failed, &integration, now) {
                continue;
            }
            let mut pulls: Vec<&str> = failed
                .iter()
                .map(|f| f.reference.as_str())
                .filter(|r| r.starts_with("pr/"))
                .collect();
            pulls.sort_unstable();
            pulls.dedup();
            let named = pulls
                .iter()
                .map(|p| format!("#{}", p.trim_start_matches("pr/")))
                .collect::<Vec<_>>()
                .join(", ");
            match self.ci_quarantine(sandbox, job, &test, &pulls, now).await {
                Ok(issue) => said.push(format!(
                    "[yulius] `{test}` failed in the runs of {named} within 48 hours while `{}` \
                     did not fail it: quarantined as a flaky test ({issue}); later jobs are told \
                     it in ${QUARANTINE_ENV} (SPEC-497)",
                    delivery_profile(sandbox).integration_branch
                )),
                Err(why) => said.push(format!(
                    "[yulius] `{test}` looks flaky (it failed in the runs of {named}), but it was \
                     not quarantined: {why}"
                )),
            }
        }
        said
    }

    /// Files the issue for `test` and quarantines it; the issue's link, or why not.
    async fn ci_quarantine(
        &self,
        sandbox: &SandboxRecord,
        job: &CiJobRecord,
        test: &str,
        pulls: &[&str],
        now: Timestamp,
    ) -> Result<String, String> {
        let token = self
            .dlv_installation_token(sandbox)
            .await
            .map_err(|err| format!("no GitHub token for its issue ({})", err.public_message()))?;
        let issue = flaky_issue(test, &job.name, pulls);
        let opened = self
            .deps
            .github
            .create_issue(&token, &sandbox.repo_owner, &sandbox.repo_name, &issue)
            .await
            .map_err(|err| format!("its issue could not be filed ({})", err.public_message()))?;
        let entry = Quarantined {
            sandbox_id: sandbox.id.clone(),
            test: test.to_owned(),
            job_key: job.key.clone(),
            issue: Some((opened.number, opened.html_url.clone())),
            since: now,
        };
        self.deps
            .store
            .quarantine_test(&entry)
            .await
            .map_err(|err| format!("it could not be recorded ({})", err.public_message()))?;
        let detail = format!(
            "{test} (job {}) quarantined as flaky: failed in {}; issue #{}",
            job.name,
            pulls.join(", "),
            opened.number
        );
        if let Err(err) = self
            .deps
            .store
            .audit(
                now,
                "yulius",
                "ci.test.quarantined",
                Some(&sandbox.slug),
                Some(&detail),
            )
            .await
        {
            tracing::warn!(error = %err, "auditing a quarantine failed");
        }
        Ok(format!("issue #{}", opened.number))
    }

    /// The variables that tell a job of `sandbox` its quarantined tests (SPEC-497 OFC-002): the
    /// names one per line, and a nextest filter of them; empty values for none.
    pub(super) async fn ci_quarantine_env(&self, sandbox: &SandboxId) -> Vec<(String, String)> {
        let names: Vec<String> = self
            .deps
            .store
            .quarantined_tests(Some(sandbox))
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|q| q.test)
            .collect();
        vec![
            (QUARANTINE_ENV.to_owned(), names.join("\n")),
            (QUARANTINE_NEXTEST_ENV.to_owned(), nextest_filter(&names)),
        ]
    }

    /// Ends the quarantine of every test whose issue is closed or gone (SPEC-497 OFC-002); run
    /// with the hourly prune. A test whose issue cannot be read stays quarantined.
    pub async fn ci_release_quarantines(&self) {
        let Ok(entries) = self.deps.store.quarantined_tests(None).await else {
            return;
        };
        for entry in entries {
            let Some((number, _)) = entry.issue else {
                continue;
            };
            let Ok(Some(sandbox)) = self.deps.store.sandbox(&entry.sandbox_id).await else {
                continue;
            };
            let Ok(token) = self.dlv_installation_token(&sandbox).await else {
                continue;
            };
            let open = self
                .deps
                .github
                .issue_open(&token, &sandbox.repo_owner, &sandbox.repo_name, number)
                .await;
            if matches!(open, Ok(false)) {
                let _ = self
                    .deps
                    .store
                    .release_test(&entry.sandbox_id, &entry.test)
                    .await;
                let detail = format!("{} released: issue #{number} is closed", entry.test);
                let _ = self
                    .deps
                    .store
                    .audit(
                        self.now(),
                        "yulius",
                        "ci.test.released",
                        Some(&sandbox.slug),
                        Some(&detail),
                    )
                    .await;
            }
        }
    }
}

/// The issue filed for a test quarantined as flaky, in the Definition of Ready's form.
fn flaky_issue(test: &str, job: &str, pulls: &[&str]) -> NewIssue {
    let runs = pulls
        .iter()
        .map(|p| format!("- pull request #{}", p.trim_start_matches("pr/")))
        .collect::<Vec<_>>()
        .join("\n");
    NewIssue {
        title: format!("Flaky test quarantined: {test}"),
        body: format!(
            "Yulius CI quarantined `{test}` (job `{job}`): it failed in the runs of different pull \
             requests within 48 hours while the integration branch did not fail it, so it fails \
             on changes that are not its own. Until this issue is closed, every CI job of this \
             project is told it in `{QUARANTINE_ENV}` (one name per line) and \
             `{QUARANTINE_NEXTEST_ENV}` (a nextest filter: `cargo nextest run -E \"not \
             (${QUARANTINE_NEXTEST_ENV})\"` when it is not empty); a gate that reads them stops \
             failing on it. It is never run again until it passes.\n\n\
             It failed in:\n{runs}\n\n\
             ## Current behavior\n\n`{test}` fails on unrelated changes.\n\n\
             ## Expected behavior\n\nIt passes or fails only for the change it tests.\n\n\
             ## Acceptance criteria\n\n\
             - [ ] The failure is reproduced (a probe at the racing step, not a retry)\n\
             - [ ] Its cause is fixed in the product or the test, with the probe passing where it \
             failed\n\
             - [ ] This issue is closed, which ends the quarantine\n"
        ),
        labels: vec![
            "type: bug".to_owned(),
            "priority: high".to_owned(),
            "area: ci".to_owned(),
            "flaky test".to_owned(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_issue_names_the_test_its_pull_requests_and_how_it_ends() {
        let issue = flaky_issue("app tests::slow", "quality", &["pr/3", "pr/7"]);
        assert_eq!(issue.title, "Flaky test quarantined: app tests::slow");
        assert!(issue.body.contains("- pull request #3\n- pull request #7"));
        assert!(issue.body.contains("## Acceptance criteria"));
        assert!(issue.body.contains("YULIUS_CI_QUARANTINE_NEXTEST"));
        assert!(issue.labels.iter().any(|l| l == "type: bug"));
    }
}
