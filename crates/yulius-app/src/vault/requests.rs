//! Hosts agents ask for (SPEC-121, ADR-0098): a person adds an agent secret with a name, a value
//! and a description; the agent works out the service's hosts and proposes them with
//! `secret_request_hosts`; a person who manages the secret allows or dismisses them. An agent
//! binds a host itself only for a secret whose creator let agents set it up (SPEC-216,
//! `auto_allow`, the default for new secrets); otherwise the binding is what keeps a secret from
//! leaving for another server.

use serde::Deserialize;
use serde_json::{Value, json};
use yulius_contracts::api::secrets::{AgentSecretView, HostRequestAnswer};
use yulius_contracts::events::UiEvent;
use yulius_domain::vault::VaultError;
use yulius_domain::vault::hosts::{HostPattern, MAX_HOSTS, parse_hosts};

use super::{SecretUser, invalid};
use crate::auth::AuthContext;
use crate::browser::percent_encode;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::vault::VaultSecretRecord;

/// Longest reason an agent gives (audited).
const MAX_REASON: usize = 300;

/// Arguments of `secret_request_hosts`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestArgs {
    secret: String,
    hosts: Vec<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// What an agent's request changes: the hosts newly asked for, those already bound, and the
/// whole request that waits for a person.
#[derive(Debug, PartialEq, Eq)]
struct Merged {
    fresh: Vec<String>,
    already_bound: Vec<String>,
    waiting: Vec<String>,
}

fn patterns(stored: &[String]) -> Vec<HostPattern> {
    stored
        .iter()
        .filter_map(|h| HostPattern::parse(h).ok())
        .collect()
}

/// Merges `asked` into the open request of a secret bound to `bound`; at most [`MAX_HOSTS`]
/// bound and waiting together.
fn merge(
    bound: &[String],
    waiting: &[String],
    asked: &[HostPattern],
) -> Result<Merged, VaultError> {
    let bound = patterns(bound);
    let mut open = patterns(waiting);
    let mut fresh = Vec::new();
    let mut already_bound = Vec::new();
    for host in asked {
        if bound.contains(host) {
            already_bound.push(host.to_string());
        } else if !open.contains(host) {
            fresh.push(host.to_string());
            open.push(host.clone());
        }
    }
    if bound.len() + open.len() > MAX_HOSTS {
        return Err(VaultError::Host(format!(
            "at most {MAX_HOSTS} hosts per secret, bound and requested together"
        )));
    }
    Ok(Merged {
        fresh,
        already_bound,
        waiting: open.iter().map(ToString::to_string).collect(),
    })
}

/// Bidirectional and invisible format characters, which could make an audit line read
/// differently from what it says.
fn hidden_format(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}')
}

pub(super) fn reason(text: Option<&str>) -> String {
    text.unwrap_or_default()
        .chars()
        .filter(|c| !hidden_format(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_REASON)
        .collect::<String>()
        .trim()
        .to_owned()
}

impl App {
    /// The Secrets page of a sandbox, at its agent secrets (printed for the person who answers).
    pub(super) fn agent_secrets_link(&self, slug: &str) -> String {
        format!(
            "{}/s/{}/secrets?section=agents",
            self.config.public_url,
            percent_encode(slug, false)
        )
    }

    /// Tells the open Secrets pages that an agent asked for something about `record` (or created
    /// it, SPEC-211), so it shows without a reload (SPEC-209): those of its sandbox, or every
    /// sandbox's for a secret of every sandbox. The event names nothing; the page reads the list
    /// again.
    pub(super) fn announce_request(&self, record: &VaultSecretRecord) {
        self.events.publish(
            record.sandbox_id.clone(),
            false,
            UiEvent::SecretsUpdated {
                sandbox_id: record.sandbox_id.as_ref().map(ToString::to_string),
            },
        );
    }

    /// `secret_request_hosts`: stores the hosts an agent or a Superagent proposes for an agent
    /// secret of its project as a request (never a binding), audits it and flags a task as needing
    /// a person (SPEC-121, SPEC-186 SAV-003).
    pub(crate) async fn tool_secret_request_hosts(
        &self,
        user: SecretUser<'_>,
        args: &Value,
    ) -> AppResult<Value> {
        let args: RequestArgs = serde_json::from_value(args.clone())
            .map_err(|e| AppError::Invalid(format!("invalid arguments: {e}")))?;
        let asked = parse_hosts(&args.hosts).map_err(invalid)?;
        if asked.is_empty() {
            return Err(AppError::Invalid(
                "name at least one host, like api.example.com".into(),
            ));
        }
        let store = &self.deps.store;
        let record = store
            .vault_agent(Some(user.sandbox()))
            .await?
            .into_iter()
            .find(|r| r.name == args.secret.trim())
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "no agent secret named {:?} applies to this project (see secrets_list)",
                    args.secret
                ))
            })?;
        if record.cd_prod {
            return Err(AppError::Forbidden(super::PRODUCTION_SECRET.into()));
        }
        let merged = merge(&record.hosts, &record.requested_hosts, &asked).map_err(invalid)?;
        let sandbox = store
            .sandbox(user.sandbox())
            .await?
            .ok_or(AppError::NotFound)?;
        let link = self.agent_secrets_link(&sandbox.slug);
        if record.auto_allow {
            return self
                .auto_bind(user, &record, &asked, merged, link, args.reason.as_deref())
                .await;
        }
        if merged.fresh.is_empty() {
            return Ok(json!({
                "requested": [],
                "already_bound": merged.already_bound,
                "waiting": merged.waiting,
                "link": link,
                "next": if merged.waiting.is_empty() {
                    "Every host is already bound: call it, the proxy adds the secret."
                } else {
                    "These hosts already wait for a person on the Secrets page: print the link so they can allow them."
                },
            }));
        }
        let by = requester(
            record.requested_by.as_deref(),
            !record.requested_hosts.is_empty(),
            &user.asker(),
            record.sandbox_id.is_none().then_some(sandbox.slug.as_str()),
        );
        if !store
            .vault_set_request(
                &record.id,
                &record.requested_hosts,
                &merged.waiting,
                Some(&by),
            )
            .await?
        {
            return Err(AppError::Conflict(
                "the secret changed meanwhile: ask again".into(),
            ));
        }
        self.announce_request(&record);
        let why = reason(args.reason.as_deref());
        self.vault_audit(
            &user.actor(),
            "vault.agent.request",
            Some(&record.id),
            &format!(
                "{} → {}{}",
                record.name,
                merged.fresh.join(", "),
                if why.is_empty() {
                    String::new()
                } else {
                    format!(" ({why})")
                }
            ),
        )
        .await?;
        self.flag_secret_wait(user, "a host request").await?;
        Ok(json!({
            "requested": merged.fresh,
            "already_bound": merged.already_bound,
            "waiting": merged.waiting,
            "link": link,
            "next": format!(
                "A person who manages {} allows these hosts on the Secrets page: print the link so they can. Until then requests go out without the secret; go on with other work and use it once secrets_list shows the hosts bound.",
                record.name
            ),
        }))
    }

    /// `secret_request_hosts` for a secret agents set up themselves (SPEC-216 SSA-002): every
    /// host asked for that is not bound yet is bound at once (also one that waited for a person),
    /// audited as `vault.agent.auto_allow`; no person is needed.
    async fn auto_bind(
        &self,
        user: SecretUser<'_>,
        record: &crate::ports::vault::VaultSecretRecord,
        asked: &[HostPattern],
        merged: Merged,
        link: String,
        why: Option<&str>,
    ) -> AppResult<Value> {
        let bound = patterns(&record.hosts);
        let mut to_bind: Vec<HostPattern> = Vec::new();
        for host in asked {
            if !bound.contains(host) && !to_bind.contains(host) {
                to_bind.push(host.clone());
            }
        }
        let fresh: Vec<String> = to_bind.iter().map(ToString::to_string).collect();
        let waiting: Vec<String> = merged
            .waiting
            .iter()
            .filter(|h| !fresh.contains(h) && !record.hosts.contains(h))
            .cloned()
            .collect();
        if !fresh.is_empty() {
            let all: Vec<String> = record.hosts.iter().chain(&fresh).cloned().collect();
            let hosts: Vec<String> = parse_hosts(&all)
                .map_err(invalid)?
                .iter()
                .map(ToString::to_string)
                .collect();
            if !self
                .deps
                .store
                .vault_auto_bind(
                    &record.id,
                    (&record.hosts, &hosts),
                    (&record.requested_hosts, &waiting),
                )
                .await?
            {
                return Err(AppError::Conflict(
                    "the secret changed meanwhile: ask again".into(),
                ));
            }
            self.vault.bump();
            self.announce_request(record);
            // A host that waited for a person is bound: a task may no longer need one (SPEC-213).
            if waiting.len() < record.requested_hosts.len() {
                self.settle_secret_waits(record.sandbox_id.as_ref()).await;
            }
            let why = reason(why);
            self.vault_audit(
                &user.actor(),
                "vault.agent.auto_allow",
                Some(&record.id),
                &format!(
                    "{} → {}{}",
                    record.name,
                    fresh.join(", "),
                    if why.is_empty() {
                        String::new()
                    } else {
                        format!(" ({why})")
                    }
                ),
            )
            .await?;
        }
        Ok(json!({
            "bound": fresh,
            "requested": [],
            "already_bound": merged.already_bound,
            "waiting": waiting,
            "link": link,
            "next": format!(
                "Agents set {} up themselves: these hosts are bound now. Call them, the proxy adds the secret.",
                record.name
            ),
        }))
    }

    /// `POST /api/agent-secrets/{id}/request`: a person who manages the secret allows the hosts
    /// an agent asked for (they join the bindings) or dismisses them. `answer.hosts` must be
    /// exactly the hosts waiting, so a host asked for after the person looked is never allowed
    /// unseen; the bindings and the request change in one step, nothing else in the row does.
    ///
    /// # Errors
    /// `NotFound` for invisible secrets, `Forbidden` for people who cannot manage it,
    /// `Conflict` when nothing waits or the request or bindings changed meanwhile, `Invalid`
    /// when allowing would bind more than 16 hosts (the request stays), store failures.
    pub async fn answer_host_request(
        &self,
        ctx: &AuthContext,
        id: &str,
        answer: &HostRequestAnswer,
    ) -> AppResult<AgentSecretView> {
        let mut record = self.agent_record(ctx, id).await?;
        self.require_manage_agent(ctx, record.sandbox_id.as_ref())
            .await?;
        if record.requested_hosts.is_empty() {
            return Err(AppError::Conflict(
                "no hosts wait to be allowed for this secret".into(),
            ));
        }
        let seen = parse_hosts(&answer.hosts).map_err(invalid)?;
        if !same_hosts(&seen, &patterns(&record.requested_hosts)) {
            return Err(AppError::Conflict(format!(
                "an agent changed the request meanwhile: reload to see the hosts that wait ({})",
                record.requested_hosts.join(", ")
            )));
        }
        if answer.allow && record.cd_prod {
            return Err(AppError::Conflict(super::PRODUCTION_SECRET.into()));
        }
        let asked = std::mem::take(&mut record.requested_hosts);
        let by = record.requested_by.take().unwrap_or_default();
        let hosts = if answer.allow {
            let all: Vec<String> = record.hosts.iter().chain(&asked).cloned().collect();
            let parsed = parse_hosts(&all).map_err(invalid)?;
            parsed.iter().map(ToString::to_string).collect::<Vec<_>>()
        } else {
            record.hosts.clone()
        };
        let now = self.now();
        let user = ctx.user.id.as_str();
        if !self
            .deps
            .store
            .vault_answer_request(id, &asked, &record.hosts, &hosts, user, now)
            .await?
        {
            return Err(AppError::Conflict(
                "the secret changed meanwhile: reload and answer again".into(),
            ));
        }
        record.hosts = hosts;
        user.clone_into(&mut record.updated_by);
        record.updated_at = now;
        if answer.allow {
            self.vault.bump();
        }
        self.vault_audit(
            &Self::vault_actor(ctx),
            if answer.allow {
                "vault.agent.allow"
            } else {
                "vault.agent.dismiss"
            },
            Some(id),
            &format!("{} → {} (asked by {by})", record.name, asked.join(", ")),
        )
        .await?;
        self.settle_secret_waits(record.sandbox_id.as_ref()).await;
        self.agent_view(&record, true).await
    }
}

/// Whether two lists name the same hosts, in any order.
fn same_hosts(a: &[HostPattern], b: &[HostPattern]) -> bool {
    a.len() == b.len() && a.iter().all(|h| b.contains(h))
}

/// Whether `asker` (as [`requester`] writes it) is among the askers `by` of a request.
pub(super) fn among_askers(by: Option<&str>, asker: &str) -> bool {
    by.is_some_and(|by| by.split(", ").any(|e| e == asker))
}

/// Who asks: `asker` (`#N`, `<login>'s Superagent`), with the sandbox for a secret of every
/// sandbox (shown on every sandbox's page); added to the earlier askers of a request that still
/// waits.
pub(super) fn requester(
    earlier: Option<&str>,
    waiting: bool,
    asker: &str,
    slug: Option<&str>,
) -> String {
    let me = match slug {
        Some(slug) => format!("{asker} in {slug}"),
        None => asker.to_owned(),
    };
    match earlier {
        Some(earlier) if waiting && among_askers(Some(earlier), &me) => earlier.to_owned(),
        Some(earlier) if waiting => format!("{earlier}, {me}"),
        _ => me,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(list: &[&str]) -> Vec<String> {
        list.iter().map(|h| (*h).to_owned()).collect()
    }

    #[test]
    fn a_request_adds_only_new_hosts_to_what_waits() -> Result<(), VaultError> {
        let asked = parse_hosts(&hosts(&[
            "API.vidoc.test",
            "eu.vidoc.test",
            "app.vidoc.test",
        ]))?;
        let merged = merge(
            &hosts(&["app.vidoc.test"]),
            &hosts(&["eu.vidoc.test"]),
            &asked,
        )?;
        assert_eq!(merged.fresh, hosts(&["api.vidoc.test"]));
        assert_eq!(merged.already_bound, hosts(&["app.vidoc.test"]));
        assert_eq!(merged.waiting, hosts(&["eu.vidoc.test", "api.vidoc.test"]));
        Ok(())
    }

    #[test]
    fn bound_and_waiting_hosts_stay_within_the_limit() -> Result<(), VaultError> {
        let bound: Vec<String> = (0..MAX_HOSTS - 1).map(|i| format!("h{i}.test")).collect();
        let one = parse_hosts(&hosts(&["new.test"]))?;
        assert!(merge(&bound, &[], &one).is_ok());
        let two = parse_hosts(&hosts(&["new.test", "other.test"]))?;
        assert!(merge(&bound, &[], &two).is_err());
        Ok(())
    }

    #[test]
    fn requesters_name_each_task_once() {
        assert_eq!(requester(None, false, "#7", None), "#7");
        assert_eq!(requester(Some("#3"), true, "#7", None), "#3, #7");
        assert_eq!(requester(Some("#3, #7"), true, "#7", None), "#3, #7");
        assert_eq!(
            requester(Some("#3"), false, "#7", None),
            "#7",
            "an answered request is forgotten"
        );
        assert_eq!(requester(None, false, "#7", Some("app")), "#7 in app");
        // SPEC-186 SAV-003: a Superagent asks under its person's name.
        assert_eq!(
            requester(Some("#3"), true, "alice's Superagent", Some("app")),
            "#3, alice's Superagent in app"
        );
        assert!(same_hosts(
            &parse_hosts(&hosts(&["b.test", "a.test"])).unwrap_or_default(),
            &parse_hosts(&hosts(&["A.test", "b.test"])).unwrap_or_default()
        ));
    }

    #[test]
    fn reasons_are_short_single_lines() {
        assert_eq!(reason(Some("evil\u{202e}moc.test")), "evilmoc.test");
        assert_eq!(reason(Some(" upload\nvideos ")), "upload videos");
        assert_eq!(reason(Some(&"x".repeat(400))).len(), MAX_REASON);
        assert_eq!(reason(None), "");
    }
}
