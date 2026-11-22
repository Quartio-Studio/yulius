//! An agent secret's method and path rules at the egress proxy (SPEC-627 MPR-002 to MPR-005,
//! ADR-0458): which secrets a request may use, the refusal the agent reads, and its audit.

use yulius_domain::vault::rules::{
    OVERRIDE_HEADERS, overrides_method, request_path, rules_allow, unsafe_path,
};

use crate::context::App;
use crate::ports::egress::{EgressClient, EgressDecision, EgressRequest};
use crate::ports::vault::VaultSecretRecord;

/// Longest path (and method) an audit entry or a refusal repeats.
const SHOWN_MAX: usize = 200;
/// Different refusals of one secret a run's audit lists; past them, one entry says further ones
/// are not listed (MPR-005: varying the path cannot flood the trail).
pub(super) const REFUSALS_LISTED: usize = 20;

/// A request as a secret's rules see it: its method, its path as sent and as it goes out (another
/// secret's value replaced in it), each without the query, and, when a client or server could
/// read it as another request (MPR-004), why.
pub(super) struct RuleCheck<'a> {
    method: &'a str,
    path: &'a str,
    sent: &'a str,
    escape: Option<String>,
}

impl<'a> RuleCheck<'a> {
    /// `request` as the agent sent it; `outgoing`: its path and query with placeholders replaced.
    pub(super) fn of(request: &'a EgressRequest, outgoing: &'a str) -> Self {
        let path = request_path(&request.path_and_query);
        let sent = request_path(outgoing);
        let escape = unsafe_path(path)
            .or_else(|| unsafe_path(sent))
            .map(|why| format!("its path {why}, which a server could read as another path"))
            .or_else(|| {
                request
                    .headers
                    .iter()
                    .find(|(name, _)| OVERRIDE_HEADERS.contains(&name.replace('_', "-").as_str()))
                    .map(|(name, _)| {
                        format!("it carries {name}, which a server could take over its request")
                    })
            })
            .or_else(|| {
                (overrides_method(&request.path_and_query) || overrides_method(outgoing)).then(
                    || "its query sets _method, which a server could take as its method".into(),
                )
            });
        Self {
            method: &request.method,
            path,
            sent,
            escape,
        }
    }

    /// Whether the request may use `record`: always without rules; with rules, only when one
    /// matches the path as sent and as it goes out, and the request cannot be read as another
    /// (MPR-002, MPR-004).
    pub(super) fn fits(&self, record: &VaultSecretRecord) -> bool {
        record.rules.is_empty()
            || (self.escape.is_none()
                && rules_allow(&record.rules, self.method, self.path)
                && rules_allow(&record.rules, self.method, self.sent))
    }

    /// `METHOD /path`, bounded, for texts people and agents read.
    fn shown(&self) -> String {
        format!("{} {}", bounded(self.method), bounded(self.path))
    }

    /// The 403 the agent reads (MPR-003): the request, the host, each secret with its rules, that
    /// nothing was sent and who can change the rules. Never a value.
    pub(super) fn refusal(&self, host: &str, denied: &[&VaultSecretRecord]) -> EgressDecision {
        let secrets: Vec<String> = denied
            .iter()
            .map(|r| {
                let rules: Vec<String> = r.rules.iter().map(ToString::to_string).collect();
                format!("{} ({})", r.name, rules.join("; "))
            })
            .collect();
        let why = match &self.escape {
            Some(why) => format!(" ({why})"),
            // Only the outgoing path failed; never shown: it holds a secret's value.
            None if denied
                .iter()
                .all(|r| rules_allow(&r.rules, self.method, self.path)) =>
            {
                " (a secret's value replaced in its path takes it outside them)".into()
            }
            None => String::new(),
        };
        EgressDecision::Refuse {
            status: 403,
            message: format!(
                "Yulius: {} to {host} is outside the rules of agent secret {}{why}, so nothing was \
                 sent. A person who manages it can change its rules on the Secrets page.",
                self.shown(),
                secrets.join(" and ")
            ),
        }
    }
}

/// `text` cut to [`SHOWN_MAX`] characters.
fn bounded(text: &str) -> String {
    match text.char_indices().nth(SHOWN_MAX) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

impl App {
    /// Audits a refusal of each secret in `denied` (MPR-005): its name, the method, the path and
    /// the host, by the client's actor; the same refusal by the same run is one entry.
    pub(super) async fn note_refusals(
        &self,
        client: &EgressClient,
        check: &RuleCheck<'_>,
        host: &str,
        denied: &[&VaultSecretRecord],
    ) {
        let shown = check.shown();
        for record in denied {
            let tally = format!("{}|{}", client.use_key(), record.id);
            let exact = format!("vault.agent.refused|{tally}|{shown}|{host}");
            let Some(listed) = self.refusal_listed(&tally, exact) else {
                continue;
            };
            let detail = if listed {
                format!("{} → {shown} on {host} (outside its rules)", record.name)
            } else {
                format!(
                    "{} → further refusals of this run are not listed (more than \
                     {REFUSALS_LISTED} different requests outside its rules)",
                    record.name
                )
            };
            if let Err(err) = self
                .vault_audit(
                    &client.actor(),
                    "vault.agent.refused",
                    Some(&record.id),
                    &detail,
                )
                .await
            {
                tracing::warn!(error = %err, "vault audit entry not written");
            }
        }
    }

    /// Whether a refusal is audited: `Some(true)` listed, `Some(false)` the one entry saying
    /// further ones are not, `None` not at all (a repeat, or past that entry). `tally` is the run
    /// and the secret, `exact` the refusal itself.
    fn refusal_listed(&self, tally: &str, exact: String) -> Option<bool> {
        let mut seen = self.vault.audited.lock().ok()?;
        let mut counts = self.vault.refusals.lock().ok()?;
        if seen.contains(&exact) || counts.get(tally).is_some_and(|n| *n > REFUSALS_LISTED) {
            return None;
        }
        if seen.len() >= super::AUDITED_KEYS_MAX {
            seen.clear();
        }
        if counts.len() >= super::AUDITED_KEYS_MAX {
            counts.clear();
        }
        seen.insert(exact);
        let count = counts.entry(tally.to_owned()).or_insert(0);
        *count += 1;
        Some(*count <= REFUSALS_LISTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_are_cut_on_a_character() {
        let long = format!("/{}", "é".repeat(300));
        let cut = bounded(&long);
        assert_eq!(cut.chars().count(), SHOWN_MAX + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(bounded("/short"), "/short");
    }
}
