//! Hosts agents ask for (SPEC-121, ADR-0098): a person adds an agent secret with a name, a value
//! and a description; the agent works out the service's hosts and proposes them with
//! `secret_request_hosts`; a person who manages the secret allows or dismisses them. An agent
//! never binds a host itself: the binding is what keeps a secret from leaving for another server.

use serde::Deserialize;
use serde_json::{Value, json};
use yulius_contracts::api::secrets::AgentSecretView;
use yulius_domain::task::Activity;
use yulius_domain::vault::VaultError;
use yulius_domain::vault::hosts::{HostPattern, MAX_HOSTS, parse_hosts};

use super::invalid;
use crate::auth::AuthContext;
use crate::browser::percent_encode;
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};

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

fn reason(text: Option<&str>) -> String {
    text.unwrap_or_default()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_REASON)
        .collect::<String>()
        .trim()
        .to_owned()
}

impl App {
    /// `secret_request_hosts`: stores the hosts an agent proposes for an agent secret of its
    /// project as a request (never a binding), audits it and flags the task as needing a person.
    pub(crate) async fn tool_secret_request_hosts(
        &self,
        ctx: &AgentContext,
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
            .vault_agent(Some(&ctx.task.sandbox_id))
            .await?
            .into_iter()
            .find(|r| r.name == args.secret.trim())
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "no agent secret named {:?} applies to this project (see secrets_list)",
                    args.secret
                ))
            })?;
        let merged = merge(&record.hosts, &record.requested_hosts, &asked).map_err(invalid)?;
        let sandbox = store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let link = format!(
            "{}/s/{}/secrets?section=agents",
            self.config.public_url,
            percent_encode(&sandbox.slug, false)
        );
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
        let by = format!("#{}", ctx.task.number);
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
        let why = reason(args.reason.as_deref());
        self.vault_audit(
            &format!("task:{}", ctx.task.number),
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
        if let Some(task) = store.task(&ctx.task.id).await?
            && let Err(err) = self
                .set_task_activity(&task, Activity::NeedsAttention)
                .await
        {
            tracing::warn!(task = %task.id, error = %err, "could not flag a host request");
        }
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

    /// `POST /api/agent-secrets/{id}/request`: a person who manages the secret allows the hosts
    /// an agent asked for (they join the bindings) or dismisses them.
    ///
    /// # Errors
    /// `NotFound` for invisible secrets, `Forbidden` for people who cannot manage it,
    /// `Conflict` when nothing waits or the request changed meanwhile, validation and store
    /// failures.
    pub async fn answer_host_request(
        &self,
        ctx: &AuthContext,
        id: &str,
        allow: bool,
    ) -> AppResult<AgentSecretView> {
        let mut record = self.agent_record(ctx, id).await?;
        self.require_manage_agent(ctx, record.sandbox_id.as_ref())
            .await?;
        if record.requested_hosts.is_empty() {
            return Err(AppError::Conflict(
                "no hosts wait to be allowed for this secret".into(),
            ));
        }
        let asked = std::mem::take(&mut record.requested_hosts);
        let by = record.requested_by.take().unwrap_or_default();
        let hosts = if allow {
            let all: Vec<String> = record.hosts.iter().chain(&asked).cloned().collect();
            let parsed = parse_hosts(&all).map_err(invalid)?;
            Some(parsed.iter().map(ToString::to_string).collect::<Vec<_>>())
        } else {
            None
        };
        // Clear first: a request made meanwhile is never answered unseen.
        if !self
            .deps
            .store
            .vault_set_request(id, &asked, &[], None)
            .await?
        {
            return Err(AppError::Conflict(
                "the request changed meanwhile: reload and answer again".into(),
            ));
        }
        if let Some(hosts) = hosts {
            record.hosts = hosts;
            record.updated_by = ctx.user.id.as_str().to_owned();
            record.updated_at = self.now();
            if !self.deps.store.vault_update(&record).await? {
                return Err(AppError::NotFound);
            }
            self.vault.bump();
        }
        self.vault_audit(
            &Self::vault_actor(ctx),
            if allow {
                "vault.agent.allow"
            } else {
                "vault.agent.dismiss"
            },
            Some(id),
            &format!("{} → {} (asked by {by})", record.name, asked.join(", ")),
        )
        .await?;
        self.agent_view(&record, true).await
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
    fn reasons_are_short_single_lines() {
        assert_eq!(reason(Some(" upload\nvideos ")), "upload videos");
        assert_eq!(reason(Some(&"x".repeat(400))).len(), MAX_REASON);
        assert_eq!(reason(None), "");
    }
}
