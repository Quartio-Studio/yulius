//! An agent secret's method and path rules at the egress proxy (SPEC-627 MPR-002 to MPR-005,
//! ADR-0458): which secrets a request may use, the refusal the agent reads, and its audit.

use yulius_domain::vault::rules::{
    METHOD_OVERRIDE_HEADERS, request_path, rules_allow, unsafe_path,
};

use crate::context::App;
use crate::ports::egress::{EgressClient, EgressDecision, EgressRequest};
use crate::ports::vault::VaultSecretRecord;

/// Longest path (and method) an audit entry or a refusal repeats.
const SHOWN_MAX: usize = 200;

/// A request as a secret's rules see it: its method, its path as sent (without the query) and,
/// when a server could read it as another request (MPR-004), why.
pub(super) struct RuleCheck<'a> {
    method: &'a str,
    path: &'a str,
    escape: Option<String>,
}

impl<'a> RuleCheck<'a> {
    pub(super) fn of(request: &'a EgressRequest) -> Self {
        let path = request_path(&request.path_and_query);
        let escape = unsafe_path(path)
            .map(|why| format!("its path {why}, which a server could read as another path"))
            .or_else(|| {
                request
                    .headers
                    .iter()
                    .find(|(name, _)| METHOD_OVERRIDE_HEADERS.contains(&name.as_str()))
                    .map(|(name, _)| {
                        format!("it carries {name}, which a server could take as its method")
                    })
            });
        Self {
            method: &request.method,
            path,
            escape,
        }
    }

    /// Whether the request may use `record`: always without rules; with rules, only when one
    /// matches and the request cannot be read as another (MPR-002, MPR-004).
    pub(super) fn fits(&self, record: &VaultSecretRecord) -> bool {
        record.rules.is_empty()
            || (self.escape.is_none() && rules_allow(&record.rules, self.method, self.path))
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
        let why = self
            .escape
            .as_ref()
            .map_or_else(String::new, |why| format!(" ({why})"));
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
            self.vault_audit_once(
                format!(
                    "vault.agent.refused|{}|{}|{shown}|{host}",
                    client.use_key(),
                    record.id
                ),
                &client.actor(),
                "vault.agent.refused",
                Some(&record.id),
                &format!("{} → {shown} on {host} (outside its rules)", record.name),
            )
            .await;
        }
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
