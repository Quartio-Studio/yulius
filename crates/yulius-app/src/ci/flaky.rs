//! Flaky tests quarantined with an issue (SPEC-497 OFC-002): the failing tests of every CI job are
//! kept; one that failed in two different pull requests' runs within 48 hours, at heads that share
//! no commit outside the integration branch, while the integration branch did not fail it, is
//! quarantined at once with an issue in the project's repository, and every later job of the
//! project is told it in its environment ([`yulius_domain::ci::flaky`]). The quarantine ends when
//! its issue is closed (looked at hourly), and its failures are forgotten with it. A failing test
//! is never run again until it passes: nothing here runs anything. A pull request's failure whose
//! block names a file that pull request changed is not kept (SPEC-618, ADR-0453).

use std::collections::HashMap;

use yulius_domain::ci::failures::{failed_test_blocks, omitted_tests};
use yulius_domain::ci::flaky::{
    FLAKY_WINDOW_MS, QUARANTINE_ENV, QUARANTINE_NEXTEST_ENV, TestFailure, is_flaky,
    named_changed_file, nextest_filter, quarantinable,
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
/// Most failing tests of one job, kept or only counted, for which a quarantine is looked at (a
/// job failing more is broken, not flaky, and each new quarantine files an issue).
const MAX_TESTS_LOOKED_AT: usize = 10;
/// Most commits of other pull requests a failure is compared with.
const MAX_HEADS_COMPARED: usize = 16;

/// What a failed job's failing tests led to (SPEC-497 OFC-002).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Noted {
    /// Lines for the job's output.
    pub lines: Vec<String>,
    /// Whether a test was quarantined now.
    pub quarantined: bool,
}

impl App {
    /// After job `job` of `run` failed with its failing tests' blocks `failures` kept: keeps its
    /// failing tests and quarantines those that are flaky, filing an issue for each (SPEC-497
    /// OFC-002). A pull request's failing test whose block names a file that pull request changed
    /// failed for the change itself: it is not kept, so it makes no test flaky (SPEC-618
    /// FOC-001). Returns the lines for the job's output and whether a test was quarantined.
    #[doc(hidden)]
    pub async fn ci_note_failing_tests(
        &self,
        sandbox: &SandboxRecord,
        run: &CiRunRecord,
        job: &CiJobRecord,
        failures: &str,
    ) -> Noted {
        let mut noted = Noted::default();
        let blocks = failed_test_blocks(failures);
        if blocks.is_empty() {
            return noted;
        }
        let failing = blocks.len();
        let reference = run.reference.key();
        let changed = if reference.starts_with("pr/") {
            self.ci_changed_paths(sandbox, &run.reference, &run.sha)
                .await
        } else {
            None
        };
        let mut tests = Vec::with_capacity(failing);
        for (test, block) in blocks {
            match changed
                .as_deref()
                .and_then(|changed| named_changed_file(&block, changed))
            {
                Some(file) if quarantinable(&test) => noted.lines.push(format!(
                    "[yulius] `{test}` names `{file}`, which this change touched: its failure is \
                     taken as the change's own and makes no test flaky (SPEC-618)"
                )),
                Some(_) => {}
                None => tests.push(test),
            }
        }
        if tests.is_empty() {
            return noted;
        }
        let store = &self.deps.store;
        let now = self.now();
        if let Err(err) = store
            .record_test_failures(
                &sandbox.id,
                &job.key,
                (&reference, &run.id, &run.sha),
                &tests,
                now,
                Timestamp(now.0 - KEEP_FAILURES_MS),
            )
            .await
        {
            tracing::warn!(error = %err, "recording a CI job's failing tests failed");
            return noted;
        }
        // Only a pull request's failure makes a test flaky: a branch's is the branch's.
        if !reference.starts_with("pr/") || failing + omitted_tests(failures) > MAX_TESTS_LOOKED_AT
        {
            return noted;
        }
        let integration = delivery_profile(sandbox).integration_branch;
        let integration_ref = format!("branch/{integration}");
        for test in tests.into_iter().filter(|t| quarantinable(t)) {
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
            // Cheap first: two pull requests at all; then whether their heads are one change.
            if !is_flaky(&failed, &integration_ref, now, |_, _| false) {
                continue;
            }
            let related = self.ci_heads_related(sandbox, &failed, &integration).await;
            if !is_flaky(&failed, &integration_ref, now, |a, b| {
                related.get(&ordered(a, b)).copied().unwrap_or(true)
            }) {
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
            // One at a time per sandbox, looked at again under the lock: two jobs failing it at
            // once file one issue (review round 1).
            let lock = self
                .ci
                .quarantine_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(sandbox.id.as_str().to_owned())
                .or_default()
                .clone();
            let _one = lock.lock().await;
            let Ok(quarantined) = store.quarantined_tests(Some(&sandbox.id)).await else {
                continue;
            };
            if let Some(entry) = quarantined.iter().find(|q| q.test == test) {
                noted.lines.push(format!(
                    "[yulius] `{test}` is quarantined as flaky{}: a job started before that, or a \
                     gate that does not read ${QUARANTINE_ENV}, still runs it (SPEC-497)",
                    entry
                        .issue
                        .as_ref()
                        .map(|(n, _)| format!(" (issue #{n})"))
                        .unwrap_or_default()
                ));
                continue;
            }
            match self.ci_quarantine(sandbox, job, &test, &pulls, now).await {
                Ok(issue) => {
                    noted.quarantined = true;
                    noted.lines.push(format!(
                        "[yulius] `{test}` failed in the runs of {named} within 48 hours while \
                         `{integration}` did not fail it: quarantined as a flaky test (issue \
                         #{issue}); later jobs are told it in ${QUARANTINE_ENV} (SPEC-497)"
                    ));
                }
                Err(why) => noted.lines.push(format!(
                    "[yulius] `{test}` looks flaky (it failed in the runs of {named}), but it was \
                     not quarantined: {why}"
                )),
            }
        }
        noted
    }

    /// For each pair of commits of different pull requests among `failed` (the most recent
    /// [`MAX_HEADS_COMPARED`]), whether they share a commit outside the integration branch: a
    /// chained pull request carries its predecessor's commits (review round 1). A pair Git
    /// cannot answer for counts as related, so no quarantine rests on it.
    async fn ci_heads_related(
        &self,
        sandbox: &SandboxRecord,
        failed: &[TestFailure],
        integration: &str,
    ) -> HashMap<(String, String), bool> {
        let repo = self.sandbox_repo(&sandbox.id);
        let base = format!("origin/{integration}");
        let mut heads: Vec<(&str, &str)> = Vec::new();
        for f in failed.iter().rev() {
            if f.reference.starts_with("pr/")
                && !heads.iter().any(|(_, sha)| *sha == f.sha)
                && heads.len() < MAX_HEADS_COMPARED
            {
                heads.push((f.reference.as_str(), f.sha.as_str()));
            }
        }
        let mut related = HashMap::new();
        for (i, (ref_a, a)) in heads.iter().enumerate() {
            for (ref_b, b) in &heads[i + 1..] {
                if ref_a == ref_b {
                    continue;
                }
                // Every merge base: a chained pair that both merged a newer integration commit
                // has that commit and the shared one as bases (review round 2).
                let shared = match self.deps.git.merge_bases(&repo, a, b).await {
                    Ok(bases) if bases.is_empty() => false,
                    Ok(bases) => {
                        let mut outside = false;
                        for common in &bases {
                            if !matches!(
                                self.deps.git.is_ancestor(&repo, common, &base).await,
                                Ok(true)
                            ) {
                                outside = true;
                                break;
                            }
                        }
                        outside
                    }
                    Err(_) => true,
                };
                related.insert(ordered(a, b), shared);
            }
        }
        related
    }

    /// Files the issue for `test` and quarantines it; the issue's number, or why not. Called
    /// under the quarantine's lock.
    async fn ci_quarantine(
        &self,
        sandbox: &SandboxRecord,
        job: &CiJobRecord,
        test: &str,
        pulls: &[&str],
        now: Timestamp,
    ) -> Result<u64, String> {
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
        let added = self
            .deps
            .store
            .quarantine_test(&entry)
            .await
            .map_err(|err| {
                format!(
                    "it could not be recorded ({}); issue #{} was filed",
                    err.public_message(),
                    opened.number
                )
            })?;
        if !added {
            return Err(format!(
                "it was quarantined meanwhile; issue #{} was filed twice",
                opened.number
            ));
        }
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
        Ok(opened.number)
    }

    /// The variables that tell a job of `sandbox` its quarantined tests (SPEC-497 OFC-002): the
    /// names one per line, and a nextest filter of their test paths; empty values for none.
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

    /// Ends the quarantine of every test whose issue is closed or gone, and of every test of a
    /// sandbox that is gone or removed (SPEC-497 OFC-002); run with the hourly prune. A test
    /// whose issue cannot be read stays quarantined.
    pub async fn ci_release_quarantines(&self) {
        let Ok(entries) = self.deps.store.quarantined_tests(None).await else {
            return;
        };
        for entry in entries {
            let sandbox = match self.deps.store.sandbox(&entry.sandbox_id).await {
                Ok(Some(sandbox)) if sandbox.removed_at.is_none() => sandbox,
                Ok(_) => {
                    let _ = self
                        .deps
                        .store
                        .release_test(&entry.sandbox_id, &entry.test)
                        .await;
                    continue;
                }
                Err(_) => continue,
            };
            let Some((number, _)) = entry.issue else {
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
            if !matches!(open, Ok(false)) {
                continue;
            }
            if let Err(err) = self
                .deps
                .store
                .release_test(&entry.sandbox_id, &entry.test)
                .await
            {
                tracing::warn!(error = %err, "ending a quarantine failed");
                continue;
            }
            let detail = format!(
                "{} (job {}) released: issue #{number} is closed",
                entry.test, entry.job_key
            );
            if let Err(err) = self
                .deps
                .store
                .audit(
                    self.now(),
                    "yulius",
                    "ci.test.released",
                    Some(&sandbox.slug),
                    Some(&detail),
                )
                .await
            {
                tracing::warn!(error = %err, "auditing a release failed");
            }
        }
    }
}

/// A pair of commits in one order, as [`App::ci_heads_related`] keys them.
fn ordered(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
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
