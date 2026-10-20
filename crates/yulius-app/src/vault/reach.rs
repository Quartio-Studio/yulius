//! Which agent secrets reach a run (SPEC-245 RES, ADR-0186). A project's candidates are its own
//! secrets and the global and personal ones switched on for it ([`VaultStore::vault_agent_in`]);
//! a run gets one secret per name, the project's before its person's before the global one, and
//! never another person's. Redaction covers every candidate.
//!
//! [`VaultStore::vault_agent_in`]: crate::ports::vault::VaultStore::vault_agent_in

use std::collections::HashMap;

use yulius_domain::ids::UserId;

use crate::model::{TaskOrigin, TaskRecord};
use crate::ports::vault::{AgentScope, VaultSecretRecord};

/// For whom a project's agent secrets are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder<'a> {
    /// A person's run (a task they started, their Superagent): their personal secrets apply.
    Person(&'a UserId),
    /// A run of no person (a CI job, a CD run, an automation's task): no personal secret does.
    Nobody,
}

impl<'a> Holder<'a> {
    /// A task's holder (RES-002): its owner, unless an automation started it. Others may edit an
    /// automation's prompt, so its tasks never hold a person's credential.
    #[must_use]
    pub fn of_task(task: &'a TaskRecord) -> Self {
        if task.origin == TaskOrigin::Automation {
            Self::Nobody
        } else {
            Self::Person(&task.owner)
        }
    }

    /// The person, if any.
    #[must_use]
    pub fn person(self) -> Option<&'a UserId> {
        match self {
            Self::Person(user) => Some(user),
            Self::Nobody => None,
        }
    }

    /// Whether `record` may reach this holder's runs at all (another person's never does).
    pub(crate) fn may_hold(self, record: &VaultSecretRecord) -> bool {
        match record.scope() {
            AgentScope::Personal(owner) => self.person() == Some(&owner),
            AgentScope::Project(_) | AgentScope::Global => true,
        }
    }

    /// The key a cached matcher of this holder is stored under.
    pub(crate) fn cache_key(self) -> String {
        self.person()
            .map_or_else(String::new, |user| format!("person:{}", user.as_str()))
    }
}

/// Precedence of a scope, lower wins (RES-001).
fn rank(record: &VaultSecretRecord) -> u8 {
    match record.scope() {
        AgentScope::Project(_) => 0,
        AgentScope::Personal(_) => 1,
        AgentScope::Global => 2,
    }
}

/// The secrets a run of `holder` gets from a project's `candidates`: one per name, by precedence,
/// in the candidates' order.
pub(crate) fn resolve(
    candidates: Vec<VaultSecretRecord>,
    holder: Holder<'_>,
) -> Vec<VaultSecretRecord> {
    let mut winner: HashMap<String, (u8, String)> = HashMap::new();
    for record in candidates.iter().filter(|r| holder.may_hold(r)) {
        let rank = rank(record);
        winner
            .entry(record.name.clone())
            .and_modify(|best| {
                if rank < best.0 {
                    *best = (rank, record.id.clone());
                }
            })
            .or_insert_with(|| (rank, record.id.clone()));
    }
    candidates
        .into_iter()
        .filter(|r| winner.get(&r.name).is_some_and(|(_, id)| id == &r.id))
        .collect()
}

/// For each candidate a run of `holder` would not get because another one of its name wins, the
/// winner's scope (`sandbox` or `personal`, MAN-005).
pub(crate) fn shadowed(
    candidates: &[VaultSecretRecord],
    holder: Holder<'_>,
) -> HashMap<String, &'static str> {
    let reaching = resolve(candidates.to_vec(), holder);
    candidates
        .iter()
        .filter(|r| holder.may_hold(r))
        .filter_map(|r| {
            let won = reaching.iter().find(|w| w.name == r.name)?;
            (won.id != r.id).then(|| (r.id.clone(), won.scope().as_str()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::Envelope;
    use crate::ports::vault::{EnvFiles, VaultSection};
    use yulius_domain::ids::SandboxId;
    use yulius_domain::time::Timestamp;

    fn record(
        id: &str,
        name: &str,
        sandbox: Option<&str>,
        owner: Option<&str>,
    ) -> VaultSecretRecord {
        VaultSecretRecord {
            id: id.into(),
            section: VaultSection::Agent,
            sandbox_id: sandbox.and_then(|s| SandboxId::parse(s).ok()),
            owner_id: owner.and_then(|u| UserId::parse(u).ok()),
            name: name.into(),
            description: String::new(),
            hosts: Vec::new(),
            injection: "none".into(),
            header_name: None,
            fields: vec!["value".into()],
            envelope: Envelope {
                kek_id: 1,
                wrapped_key: Vec::new(),
                ciphertext: Vec::new(),
            },
            created_by: "u".into(),
            created_at: Timestamp(1),
            updated_by: "u".into(),
            updated_at: Timestamp(1),
            value_updated_at: Timestamp(1),
            last_used_at: None,
            requested_hosts: Vec::new(),
            requested_by: None,
            env_files: EnvFiles::No,
            env_requested_by: None,
            cd_prod: false,
            auto_allow: false,
        }
    }

    fn ids(records: &[VaultSecretRecord]) -> Vec<&str> {
        records.iter().map(|r| r.id.as_str()).collect()
    }

    /// RES-001: project before personal before global, per name.
    #[test]
    fn a_name_resolves_project_then_personal_then_global() -> Result<(), Box<dyn std::error::Error>>
    {
        let alice = UserId::parse("alice")?;
        let candidates = vec![
            record("g-both", "BOTH", None, None),
            record("p-both", "BOTH", None, Some("alice")),
            record("s-both", "BOTH", Some("s1"), None),
            record("g-two", "TWO", None, None),
            record("p-two", "TWO", None, Some("alice")),
            record("g-one", "ONE", None, None),
        ];
        let run = resolve(candidates.clone(), Holder::Person(&alice));
        assert_eq!(ids(&run), ["s-both", "p-two", "g-one"]);
        let shadows = shadowed(&candidates, Holder::Person(&alice));
        assert_eq!(shadows.get("g-both"), Some(&"sandbox"));
        assert_eq!(shadows.get("p-both"), Some(&"sandbox"));
        assert_eq!(shadows.get("g-two"), Some(&"personal"));
        assert_eq!(shadows.len(), 3);
        Ok(())
    }

    /// RES-002, RES-003, RES-004: another person's secret never reaches a run, whatever its name;
    /// nobody's runs get no personal secret.
    #[test]
    fn personal_secrets_reach_only_their_owner() -> Result<(), Box<dyn std::error::Error>> {
        let (alice, bob) = (UserId::parse("alice")?, UserId::parse("bob")?);
        let candidates = vec![
            record("a-key", "KEY", None, Some("alice")),
            record("b-key", "KEY", None, Some("bob")),
            record("a-only", "ALICE", None, Some("alice")),
            record("g", "SHARED", None, None),
        ];
        assert_eq!(
            ids(&resolve(candidates.clone(), Holder::Person(&alice))),
            ["a-key", "a-only", "g"]
        );
        assert_eq!(
            ids(&resolve(candidates.clone(), Holder::Person(&bob))),
            ["b-key", "g"]
        );
        assert_eq!(ids(&resolve(candidates.clone(), Holder::Nobody)), ["g"]);
        assert!(
            shadowed(&candidates, Holder::Person(&bob)).is_empty(),
            "alice's are not bob's to see"
        );
        Ok(())
    }

    fn task(origin: TaskOrigin, owner: &str) -> Result<TaskRecord, Box<dyn std::error::Error>> {
        use yulius_domain::ids::TaskId;
        Ok(TaskRecord {
            id: TaskId::parse("t1")?,
            sandbox_id: SandboxId::parse("s1")?,
            number: 1,
            title: "t".into(),
            prompt: "t".into(),
            harness: yulius_domain::harness::Harness::ClaudeCode,
            model: None,
            effort: None,
            workflow_id: None,
            status: yulius_domain::task::TaskStatus::Running,
            activity: None,
            branch: None,
            worktree_path: None,
            spec_path: None,
            origin,
            automation_run: None,
            created_by: UserId::parse(owner)?,
            owner: UserId::parse(owner)?,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode: crate::model::TaskMode::Goal,
            supertask: false,
        })
    }

    /// RES-002: an automation's task holds nobody's secrets; any other task its owner's.
    #[test]
    fn a_task_holds_its_owners_secrets_unless_an_automation_started_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let alice = UserId::parse("alice")?;
        for origin in [
            TaskOrigin::Ui,
            TaskOrigin::Agent,
            TaskOrigin::Onboarding,
            TaskOrigin::Import,
        ] {
            let t = task(origin, "alice")?;
            assert_eq!(Holder::of_task(&t), Holder::Person(&alice));
        }
        let automated = task(TaskOrigin::Automation, "alice")?;
        assert_eq!(Holder::of_task(&automated), Holder::Nobody);
        Ok(())
    }
}
