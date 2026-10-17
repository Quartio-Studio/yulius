//! Existing team secrets join their sandboxes (SPEC-216 SCO-004): before SPEC-216 every team
//! secret applied to the whole instance. The upgrade listed them (`vault_scope_pending`); at the
//! next start each becomes one secret per sandbox, re-sealed under the copy's id, and the original
//! is removed, in one transaction per secret. Agent secrets an administrator made team-wide stay
//! so: copies would let every sandbox's writers reveal and rebind them.

use super::aad;
use crate::context::App;
use crate::error::AppResult;
use crate::ports::vault::VaultSecretRecord;

/// The actor of the assignment in the vault's activity.
const ASSIGNER: &str = "yulius";

impl App {
    /// Assigns every pending secret to every sandbox that exists (not removed); with no sandbox,
    /// a secret stays team-wide. Returns how many secrets were assigned. A secret that fails
    /// stays pending for the next start; the others go on.
    ///
    /// # Errors
    /// Store failures listing the pending secrets or the sandboxes.
    pub async fn assign_vault_scopes(&self) -> AppResult<usize> {
        let store = &self.deps.store;
        let pending = store.vault_scope_pending().await?;
        if pending.is_empty() {
            return Ok(0);
        }
        let sandboxes: Vec<_> = store
            .sandboxes()
            .await?
            .into_iter()
            .filter(|s| s.removed_at.is_none())
            .collect();
        let mut assigned = 0;
        for id in pending {
            match self.assign_one(&id, &sandboxes).await {
                Ok(true) => assigned += 1,
                Ok(false) => {}
                // Stays pending: retried at the next start (e.g. an agent name that clashes in a
                // sandbox, from before names were kept unique).
                Err(err) => {
                    tracing::error!(secret = %id, error = %err, "cannot assign a secret to its sandboxes");
                }
            }
        }
        if assigned > 0 {
            self.vault.bump();
        }
        Ok(assigned)
    }

    async fn assign_one(
        &self,
        id: &str,
        sandboxes: &[crate::model::SandboxRecord],
    ) -> AppResult<bool> {
        let store = &self.deps.store;
        let Some(original) = store.vault_get(id).await? else {
            return store.vault_assign(id, &[]).await.map(|_| false);
        };
        if sandboxes.is_empty() || original.sandbox_id.is_some() {
            return store.vault_assign(id, &[]).await.map(|_| false);
        }
        let plain = self
            .deps
            .crypto
            .envelope_open(&aad(&original.id, original.section), &original.envelope)?;
        let mut copies: Vec<VaultSecretRecord> = Vec::with_capacity(sandboxes.len());
        for sandbox in sandboxes {
            let copy_id = self.deps.crypto.new_id();
            let mut copy = original.clone();
            copy.envelope = self
                .deps
                .crypto
                .envelope_seal(&aad(&copy_id, original.section), &plain)?;
            copy.id = copy_id;
            copy.sandbox_id = Some(sandbox.id.clone());
            copies.push(copy);
        }
        if !store.vault_assign(id, &copies).await? {
            return Ok(false);
        }
        // Done and no longer pending: an audit entry that fails to write is logged, not retried.
        for (copy, sandbox) in copies.iter().zip(sandboxes) {
            if let Err(err) = self
                .vault_audit(
                    ASSIGNER,
                    &format!("vault.{}.assign", original.section.as_str()),
                    Some(&copy.id),
                    &format!("{} → {} (from every sandbox)", copy.name, sandbox.slug),
                )
                .await
            {
                tracing::error!(secret = %copy.id, error = %err, "cannot audit a secret's assignment");
            }
        }
        Ok(true)
    }
}
