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

/// The longest case: what the filing link takes of plain ASCII prose with the other texts at
/// their caps (review of slice 11, R3-3); other text, up to 9 bytes a character in the link,
/// can meet [`MAX_LINK_BYTES`] first, and that refusal gives the size.
const MAX_CASE_CHARS: usize = 1500;

/// The longest filing link answered: GitHub's new-issue form refuses much longer ones.
const MAX_LINK_BYTES: usize = 6000;

/// Whether `text` holds `name` as a whole word (any case): bounded by what is not a letter or
/// a digit, so a compound of it (`shop-api`, `shop_db`) holds it too (review of slice 11,
/// R3-1), and by a camelCase change (`ShopApi`, `ACMEShop`), the name's own humps or none;
/// a name shorter than 3 characters is no name.
fn names_word(text: &str, name: &str) -> bool {
    if name.chars().count() < 3 {
        return false;
    }
    lowercase_holds(&text.to_lowercase(), &name.to_lowercase())
        || lowercase_holds(&camel_split(text), &camel_split(name))
}

/// `text` lowercased with a space at each camelCase change: before an upper-case letter after a
/// lower-case letter or a digit, and before the last capital of a run followed by a lower-case
/// letter (`ACMEShop` holds `acme shop`).
fn camel_split(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    for (at, &c) in chars.iter().enumerate() {
        let before = at.checked_sub(1).map(|b| chars[b]);
        let next = chars.get(at + 1);
        let hump = c.is_uppercase()
            && before.is_some_and(|b| {
                b.is_lowercase()
                    || b.is_numeric()
                    || (b.is_uppercase() && next.is_some_and(|n| n.is_lowercase()))
            });
        if hump {
            out.push(' ');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// Whether the lowercased `text` holds the lowercased `name` bounded by what is not a letter or
/// a digit.
fn lowercase_holds(text: &str, name: &str) -> bool {
    let word = char::is_alphanumeric;
    text.match_indices(name).any(|(at, found)| {
        let before = text[..at].chars().next_back();
        let after = text[at + found.len()..].chars().next();
        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

/// Which of the project's names one of `texts` holds as a whole word (review of slice 11,
/// R2-2): `name`, `repository's name` or `owner's name`, with the name itself so the refusal
/// can say it (R3-2); none for the Yulius repository's own project, whose name is the
/// product's.
fn project_named<'a>(
    texts: &[&str],
    slug: &'a str,
    owner: &'a str,
    repo: &'a str,
) -> Option<(&'static str, &'a str)> {
    let (yulius_owner, yulius_repo) = YULIUS_REPO;
    if owner.eq_ignore_ascii_case(yulius_owner) && repo.eq_ignore_ascii_case(yulius_repo) {
        return None;
    }
    [
        ("name", slug),
        ("repository's name", repo),
        ("owner's name", owner),
    ]
    .into_iter()
    .find(|(_, name)| texts.iter().any(|text| names_word(text, name)))
}

/// The case, refused empty or past [`MAX_CASE_CHARS`].
fn checked_case(case: &str) -> AppResult<()> {
    if case.trim().is_empty() || case.chars().count() > MAX_CASE_CHARS {
        return Err(AppError::Invalid(format!(
            "the case is 1 to {MAX_CASE_CHARS} characters: what was asked, and what stopped you"
        )));
    }
    Ok(())
}

/// The filing link, refused past [`MAX_LINK_BYTES`] (R2-1).
fn checked_link(ability: &str, asked: &ReportArgs, from: &str) -> AppResult<String> {
    let link = filing_link(ability, asked, from);
    if link.len() > MAX_LINK_BYTES {
        return Err(AppError::Invalid(format!(
            "the report's link is {} bytes, at most {MAX_LINK_BYTES} (a letter outside ASCII \
             takes up to 9): shorten the case, the ability or needed_for",
            link.len()
        )));
    }
    Ok(link)
}

/// The link that opens the report as a new issue in the Yulius repository.
fn filing_link(ability: &str, asked: &ReportArgs, from: &str) -> String {
    let (owner, repo) = YULIUS_REPO;
    format!(
        "https://github.com/{owner}/{repo}/issues/new?title={}&body={}&labels={}",
        query_value(&format!("Missing ability: {ability}")),
        query_value(&issue_body(asked, from)),
        query_value("type: enhancement,priority: medium,area: superagent"),
    )
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
        checked_case(&asked.case)?;
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
        if let Some((kind, name)) = project_named(
            &texts,
            &sandbox.slug,
            &sandbox.repo_owner,
            &sandbox.repo_name,
        ) {
            return Err(AppError::Invalid(format!(
                "write the report in general terms: it holds this project's {kind} `{name}` (the \
                 report is meant for the Yulius repository)"
            )));
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
        let file_url = checked_link(ability, &asked, from)?;
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
    use super::{
        HOUR_MS, MAX_CASE_CHARS, MAX_LINK_BYTES, REPORTS_PER_HOUR, ReportArgs, checked_case,
        checked_link, filing_link, issue_body, names_word, project_named, query_value, take_report,
    };

    /// R2-2: the project's names are refused, but not the Yulius project's own.
    #[test]
    fn a_report_naming_the_project_is_known_but_not_yulius_own() {
        let texts = ["the CI of acme/shop failed"];
        assert_eq!(
            project_named(&texts, "shop", "acme", "shop"),
            Some(("name", "shop"))
        );
        assert_eq!(
            project_named(&["Acme said no"], "web", "acme", "site"),
            Some(("owner's name", "acme"))
        );
        assert_eq!(
            project_named(&["yulius restarted"], "yulius", "Quartio-Studio", "yulius"),
            None
        );
        assert_eq!(
            project_named(&["nothing here"], "shop", "acme", "shop"),
            None
        );
    }

    /// Review of slice 11, R2-2: a name is matched as a whole word only.
    #[test]
    fn a_project_name_is_matched_as_a_whole_word() {
        assert!(names_word("the CI of acme/shop failed", "shop"));
        assert!(names_word("Shop broke", "shop"));
        assert!(!names_word("the shopping list", "shop"));
        assert!(!names_word("an application error", "app"));
        assert!(names_word("deploy my-app now", "my-app"));
        // Review of slice 11, R3-1: a compound of the name holds the name.
        assert!(names_word("deploy my-app-2 now", "my-app"));
        assert!(names_word("the shop-api and acme-shop broke", "shop"));
        assert!(names_word("the shop_db is full", "shop"));
        // Review of slice 11 round 3's follow-up, m2: a camelCase compound too.
        assert!(names_word("the ShopApi client threw", "shop"));
        assert!(names_word("in AcmeShopService", "acme"));
        assert!(names_word("in AcmeShopService", "shop"));
        assert!(names_word("the ACMEShop logs", "acme"));
        assert!(names_word("MyApp broke", "MyApp"));
        assert!(names_word("myapp broke", "MyApp"));
        assert!(!names_word("the Shopping list", "shop"));
        assert!(!names_word("ab", "ab"), "too short to be a name");
    }

    /// R2-1: a report too long for GitHub's form gives a link past the bound.
    #[test]
    fn a_long_report_makes_a_link_past_the_bound() {
        let short = ReportArgs {
            ability: "a".into(),
            case: "c".repeat(500),
            needed_for: None,
        };
        assert!(filing_link("a", &short, "A Superagent").len() <= MAX_LINK_BYTES);
        let long = ReportArgs {
            ability: "a".into(),
            case: "c d\n".repeat(1000),
            needed_for: None,
        };
        assert!(filing_link("a", &long, "A Superagent").len() > MAX_LINK_BYTES);
        assert!(checked_link("a", &short, "A Superagent").is_ok());
        assert!(checked_link("a", &long, "A Superagent").is_err());
    }

    /// Review of slice 11, R3-3: a case of plain ASCII prose at its cap fits the link with the
    /// other texts at theirs; the cap itself is inclusive.
    #[test]
    fn a_prose_case_at_its_cap_fits_the_link() {
        let prose: String = "The build failed, and the log said why. "
            .chars()
            .cycle()
            .take(MAX_CASE_CHARS)
            .collect();
        assert!(checked_case(&prose).is_ok());
        assert!(checked_case(&format!("{prose}x")).is_err());
        assert!(checked_case(" \n").is_err());
        let asked = ReportArgs {
            ability: "read a task's whole scrollback ".repeat(7)[..200].to_string(),
            case: prose,
            needed_for: Some("answer from the whole output ".repeat(40)[..1000].to_string()),
        };
        assert!(checked_link(&asked.ability, &asked, "A Superagent").is_ok());
        assert_eq!(MAX_CASE_CHARS, 1500);
    }

    /// Follow-up review, m1 and m6: other text meets the link's bound first; the refusal gives
    /// the link's size and the bound, and names every text it holds.
    #[test]
    fn a_link_past_the_bound_says_its_size() {
        let asked = ReportArgs {
            ability: "a".into(),
            case: "ż".repeat(MAX_CASE_CHARS),
            needed_for: None,
        };
        let size = filing_link("a", &asked, "A Superagent").len();
        let Err(refused) = checked_link("a", &asked, "A Superagent") else {
            panic!("a link of {size} bytes is refused");
        };
        let said = refused.to_string();
        assert!(said.contains(&format!("{size} bytes")), "{said}");
        assert!(said.contains("at most 6000"), "{said}");
        assert!(said.contains("needed_for"), "{said}");
    }

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
