//! A missing ability reported in one call (SPEC-487 slice 11, ABL-001 to ABL-004; wall 12): the
//! Superagent or a task's agent that meets a wall names it, and it reaches the Yulius project
//! through its person.
//!
//! Contract:
//! - every report is audited on this instance (`ability.reported`, with the ability and the
//!   case as written), where its administrators read it;
//! - the answer carries a link that opens a new issue in the Yulius repository ([`YULIUS_REPO`])
//!   filled with the report: the person reads it and submits it, under their own name, or not.
//!   Nothing leaves the instance without them (review of slice 11, M3);
//! - a report that names the project (its slug, repository or owner) or holds one of its agent
//!   secrets' values is refused: written in general terms only (M2);
//! - at most [`REPORTS_PER_HOUR`] per person an hour.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{Value, json};
use yulius_domain::permission::Action;

use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, UserRecord};

/// The Yulius project's repository (its updates come from there).
pub const YULIUS_REPO: (&str, &str) = ("Quartio-Studio", "yulius");
/// The most reports a person makes an hour.
pub const REPORTS_PER_HOUR: usize = 5;
const HOUR_MS: i64 = 3_600_000;

/// Takes one report from a person's last hour (`sent`, their reports' times), unless they made
/// [`REPORTS_PER_HOUR`] already; drops the times older than an hour.
fn take_report(sent: &mut Vec<i64>, now: i64) -> bool {
    sent.retain(|at| now.saturating_sub(*at) < HOUR_MS);
    if sent.len() >= REPORTS_PER_HOUR {
        return false;
    }
    sent.push(now);
    true
}

/// `text` as a URL query value: unreserved characters kept, the rest percent-encoded.
fn query_value(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

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
    /// `ability_report {ability, case, needed_for?}` (ABL-001): audits the report and answers a
    /// link that files it in the Yulius repository once the person submits it. `from` names
    /// the reporter (`A Superagent`, `A task's agent`); `actor` is the audit's.
    ///
    /// # Errors
    /// `Invalid` texts (empty, too long, invisible characters, the project's name or one of
    /// its secrets, unknown arguments); `Capacity` past [`REPORTS_PER_HOUR`].
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
        if ability.is_empty() || ability.chars().count() > 200 || ability.contains(['\n', '\r']) {
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
        // Nothing of the project in a report meant for another repository (M2).
        let texts = [
            ability,
            asked.case.as_str(),
            asked.needed_for.as_deref().unwrap_or(""),
        ];
        let names = [
            sandbox.slug.to_lowercase(),
            sandbox.repo_name.to_lowercase(),
            sandbox.repo_owner.to_lowercase(),
        ];
        if texts.iter().any(|text| {
            let text = text.to_lowercase();
            names
                .iter()
                .any(|name| name.chars().count() >= 3 && text.contains(name.as_str()))
        }) {
            return Err(AppError::Invalid(
                "write the report in general terms: it names this project (its name, repository \
                 or owner), and it is meant for the Yulius repository"
                    .into(),
            ));
        }
        for text in texts {
            if self.redact_for_sandbox(&sandbox.id, text).await != text {
                return Err(AppError::Invalid(
                    "the report holds an agent secret's value (or could not be checked for one): \
                     leave it out"
                        .into(),
                ));
            }
        }
        let now = self.now().0;
        {
            let mut sent = self
                .ability_reports
                .sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !take_report(sent.entry(person.id.as_str().to_owned()).or_default(), now) {
                return Err(AppError::Capacity(format!(
                    "{REPORTS_PER_HOUR} missing abilities were reported in the last hour: \
                     name this one to the person instead"
                )));
            }
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
        let file_url = format!(
            "https://github.com/{owner}/{repo}/issues/new?title={}&body={}&labels={}",
            query_value(&format!("Missing ability: {ability}")),
            query_value(&issue_body(&asked, from)),
            query_value("type: enhancement,priority: medium,area: superagent"),
        );
        Ok(json!({
            "kept": true,
            "file_url": file_url,
            "note": "Kept on this instance (its administrators read it). Give your person this link: it opens the report as a new issue in the Yulius repository for them to read and submit under their name; nothing is sent without them.",
        }))
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
        // As its owner, while they may still work in the project (review of slice 11, m2).
        self.require(&owner, &sandbox.id, Action::Work).await?;
        let actor = format!("task:{}", ctx.task.number);
        self.report_missing_ability(&owner, &sandbox, "A task's agent", &actor, args)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::{HOUR_MS, REPORTS_PER_HOUR, ReportArgs, issue_body, query_value, take_report};

    /// SPEC-487 ABL-003 (review of slice 11, m4): five an hour, and the window moves on.
    #[test]
    fn reports_are_bounded_per_hour_and_the_window_moves_on() {
        let mut sent = Vec::new();
        for _ in 0..REPORTS_PER_HOUR {
            assert!(take_report(&mut sent, 1_000));
        }
        assert!(!take_report(&mut sent, 1_000 + HOUR_MS - 1));
        assert!(take_report(&mut sent, 1_000 + HOUR_MS), "an hour later");
    }

    /// The link's texts are query-encoded.
    #[test]
    fn a_query_value_is_percent_encoded() {
        assert_eq!(query_value("a b:c/d&e"), "a%20b%3Ac%2Fd%26e");
        assert_eq!(query_value("é"), "%C3%A9");
        assert_eq!(query_value("A-z_0.9~"), "A-z_0.9~");
    }

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
