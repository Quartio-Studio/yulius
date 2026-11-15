//! A missing ability reported in one call (SPEC-487 slice 11, ABL-001 to ABL-004; wall 12): the
//! Superagent or a task's agent that meets a wall names it, and it reaches the Yulius project.
//!
//! Contract:
//! - every report is audited on this instance (`ability.reported`, with the ability and the
//!   case as written), where its administrators read it;
//! - it is also opened as an issue in the Yulius repository ([`YULIUS_REPO`]) under the person's
//!   GitHub name when their access reaches that repository; the issue holds the agent's words
//!   only, never the project's name;
//! - at most [`REPORTS_PER_HOUR`] per person an hour.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, UserRecord};

/// The Yulius project's repository (its updates come from there).
pub const YULIUS_REPO: (&str, &str) = ("Quartio-Studio", "yulius");
/// The most reports a person makes an hour.
pub const REPORTS_PER_HOUR: usize = 5;
const HOUR_MS: i64 = 3_600_000;

/// When each person reported last hour.
#[derive(Default)]
pub struct AbilityReports {
    sent: Mutex<HashMap<String, Vec<i64>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportArgs {
    ability: String,
    case: String,
    #[serde(default)]
    needed_for: Option<String>,
}

/// The issue's body: the agent's words, never the project's name.
fn issue_body(asked: &ReportArgs, from: &str) -> String {
    let expected = asked
        .needed_for
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("The agent does it for its person, with their rights, by a tool.");
    format!(
        "## Summary\n\n{from} of a Yulius instance met a wall: {ability}.\n\n## Current behavior\n\n{case}\n\n## Expected behavior\n\n{expected}\n\n## Acceptance criteria\n\n- [ ] The ability exists as a tool, or SPEC-487's ability table says why it is withheld.\n\n---\nReported with ability_report (SPEC-487).",
        ability = asked.ability.trim(),
        case = asked.case.trim(),
    )
}

impl App {
    /// `ability_report {ability, case, needed_for?}` (ABL-001): audits the report and opens it
    /// in the Yulius repository when the person's GitHub access reaches it. `from` names the
    /// reporter (`A Superagent`, `A task's agent`); `actor` is the audit's.
    ///
    /// # Errors
    /// `Invalid` texts (empty, too long, invisible characters, unknown arguments); `Capacity`
    /// past [`REPORTS_PER_HOUR`].
    pub(crate) async fn report_missing_ability(
        &self,
        person: &UserRecord,
        sandbox: &SandboxRecord,
        from: &str,
        actor: &str,
        args: &Value,
    ) -> AppResult<Value> {
        let asked: ReportArgs = serde_json::from_value(args.clone())
            .map_err(|err| AppError::Invalid(format!("invalid arguments: {err}")))?;
        crate::cross_sandbox::check_visible(args)?;
        let ability = asked.ability.trim();
        if ability.is_empty() || ability.chars().count() > 200 || ability.contains('\n') {
            return Err(AppError::Invalid(
                "the ability is 1 to 200 characters on one line".into(),
            ));
        }
        if asked.case.trim().is_empty() || asked.case.chars().count() > 4000 {
            return Err(AppError::Invalid(
                "the case is 1 to 4000 characters: what was asked, and what stopped you".into(),
            ));
        }
        if asked
            .needed_for
            .as_ref()
            .is_some_and(|t| t.chars().count() > 1000)
        {
            return Err(AppError::Invalid(
                "needed_for is at most 1000 characters".into(),
            ));
        }
        let now = self.now().0;
        {
            let mut sent = self
                .ability_reports
                .sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mine = sent.entry(person.id.as_str().to_owned()).or_default();
            mine.retain(|at| now.saturating_sub(*at) < HOUR_MS);
            if mine.len() >= REPORTS_PER_HOUR {
                return Err(AppError::Capacity(format!(
                    "{REPORTS_PER_HOUR} missing abilities were reported in the last hour: \
                     name this one to the person instead"
                )));
            }
            mine.push(now);
        }
        let detail =
            json!({"ability": ability, "case": asked.case.trim(), "from": from}).to_string();
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                actor,
                "ability.reported",
                Some(sandbox.id.as_str()),
                Some(&detail),
            )
            .await
        {
            tracing::warn!(error = %err, "a missing ability was not audited");
        }
        let (owner, repo) = YULIUS_REPO;
        let issue = match self
            .person_github_token(person, "Reporting a missing ability")
            .await
        {
            Ok(token) => match self.deps.github.repo_access(&token, owner, repo).await {
                Ok(crate::ports::github::RepoAccess::Access { .. }) => {
                    let new = crate::ports::github::NewIssue {
                        title: format!("Missing ability: {ability}"),
                        body: issue_body(&asked, from),
                        labels: vec![
                            "type: enhancement".into(),
                            "priority: medium".into(),
                            "area: superagent".into(),
                        ],
                    };
                    match self
                        .deps
                        .github
                        .create_issue(&token, owner, repo, &new)
                        .await
                    {
                        Ok(opened) => Some(opened),
                        Err(err) => {
                            tracing::info!(error = %err, "a missing ability's issue was not opened");
                            None
                        }
                    }
                }
                _ => None,
            },
            Err(_) => None,
        };
        Ok(match issue {
            Some(opened) => json!({
                "issue": opened.number,
                "url": opened.html_url,
                "note": "Reported: an issue in the Yulius repository under your person's name, and kept on this instance. Tell them.",
            }),
            None => json!({
                "issue": null,
                "note": "Reported and kept on this instance (its administrators read it); your person's GitHub access does not reach the Yulius repository, so no issue was opened. Tell them.",
            }),
        })
    }
}

impl App {
    /// `ability_report` from a Superagent, as its person.
    ///
    /// # Errors
    /// As [`App::report_missing_ability`]; the person may no longer work in the project.
    pub(crate) async fn superagent_ability_report(
        &self,
        caller: &crate::coordination::ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let person = self.manage_authority(caller).await?;
        let sandbox = self
            .deps
            .store
            .sandbox(&caller.sandbox)
            .await?
            .ok_or(AppError::NotFound)?;
        self.report_missing_ability(&person, &sandbox, "A Superagent", &caller.actor, args)
            .await
    }

    /// `ability_report` from a task's agent, as the task's owner.
    ///
    /// # Errors
    /// As [`App::report_missing_ability`]; an owner no longer active.
    pub(crate) async fn task_ability_report(
        &self,
        ctx: &crate::coordination::AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let owner = self
            .deps
            .store
            .user(&ctx.task.owner)
            .await?
            .filter(|u| u.disabled_at.is_none())
            .ok_or_else(|| AppError::Forbidden("the task's owner is no longer active".into()))?;
        let sandbox = self
            .deps
            .store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let actor = format!("task:{}", ctx.task.number);
        self.report_missing_ability(&owner, &sandbox, "A task's agent", &actor, args)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::{ReportArgs, issue_body};

    /// SPEC-487 ABL-002: the issue holds the agent's words and an acceptance criterion.
    #[test]
    fn an_issue_holds_the_words_and_a_criterion() {
        let asked = ReportArgs {
            ability: " read scrollback ".into(),
            case: "the error scrolled off".into(),
            needed_for: None,
        };
        let body = issue_body(&asked, "A Superagent");
        assert!(body.contains("met a wall: read scrollback."), "{body}");
        assert!(body.contains("## Acceptance criteria"), "{body}");
        assert!(body.contains("by a tool"), "{body}");
    }
}
