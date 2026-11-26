//! Delivery profiles: reading, validating and changing a sandbox's profile.

use super::*;

impl App {
    // ---------------------------------------------------------------- profile

    /// After the production protection switch changed: audited.
    async fn production_protection_switched(&self, actor: &str, sandbox: &SandboxRecord, on: bool) {
        self.dlv_audit(
            actor,
            "delivery.production_protection.update",
            &sandbox.slug,
            if on { "on" } else { "off" },
        )
        .await;
    }

    /// After the switch changed: CI's gate and the virtual key are read again, and it is audited.
    pub(crate) async fn vidoc_switched(&self, actor: &str, sandbox: &SandboxRecord, on: bool) {
        self.ci_forget_gate(&sandbox.id);
        // The instance key comes or goes as the sandbox's virtual agent secret.
        self.bump_vault_generation();
        self.dlv_audit(
            actor,
            "delivery.vidoc.update",
            &sandbox.slug,
            if on { "on" } else { "off" },
        )
        .await;
    }

    /// `GET /api/sandboxes/{sid}/vidoc` (anyone who sees the sandbox): its Vidoc switch with the
    /// instance's, the repository's committed one at the integration head, whether a key reaches
    /// it, whether the caller may change it and its last change (SPEC-359 VSB-001).
    ///
    /// # Errors
    /// `NotFound` without access, `Invalid` for a playground; store failures.
    pub async fn sandbox_vidoc_view(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
    ) -> AppResult<SandboxVidocView> {
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.require_for(ctx, &sid, Action::View).await?;
        if sandbox.is_playground() {
            return Err(playground_has_no_vidoc());
        }
        let branch = delivery_profile(&sandbox).integration_branch;
        let repository = self.repository_vidoc(&sandbox, &branch).await;
        let project = vidoc_switched_on(&sandbox);
        let instance = self.vidoc_enabled();
        let key = self.vidoc_key().is_some()
            || self
                .applied_agent_secrets(&sid, crate::vault::Holder::Nobody)
                .await
                .is_ok_and(|records| {
                    records
                        .iter()
                        .any(|r| r.name == yulius_domain::ci::VIDOC_SECRET)
                });
        let changed = self
            .deps
            .store
            // A removed sandbox's slug is given to the next one (`release_removed_slug`): its
            // entries are not this one's.
            .last_audit(&sandbox.slug, "delivery.vidoc.update", sandbox.created_at)
            .await?;
        let (repository, repository_problem) = match repository {
            RepositoryVidoc::On => ("on", None),
            RepositoryVidoc::Off => ("off", None),
            RepositoryVidoc::Unreadable(problem) => ("unreadable", Some(problem)),
        };
        Ok(SandboxVidocView {
            project,
            instance,
            key,
            repository: repository.to_owned(),
            repository_problem,
            branch,
            scans: project && instance && repository != "off",
            can_change: level.allows(Action::Administer),
            changed_by: changed.as_ref().map(|entry| entry.actor.clone()),
            changed_at: changed.map(|entry| rfc3339(entry.at)),
        })
    }

    /// `.config/vidoc.toml` at `origin/<branch>` of the sandbox's clone (ADR-0107); on when there
    /// is no clone or branch yet, as CI reads `yulius.toml` there.
    async fn repository_vidoc(&self, sandbox: &SandboxRecord, branch: &str) -> RepositoryVidoc {
        let repo = self.sandbox_repo(&sandbox.id);
        if !repo.join(".git").exists() {
            return RepositoryVidoc::On;
        }
        let head = self
            .deps
            .git
            .resolve_commit(&repo, &format!("origin/{branch}"))
            .await
            .ok()
            .flatten();
        let Some(head) = head else {
            return RepositoryVidoc::On;
        };
        match self.deps.git.show_file(&repo, &head, VIDOC_CONFIG).await {
            Ok(text) => repository_vidoc(text.as_deref()),
            Err(err) => RepositoryVidoc::Unreadable(format!(
                "{VIDOC_CONFIG} can't be read: {}",
                err.public_message()
            )),
        }
    }

    /// `PATCH /api/sandboxes/{sid}` (admin): validates and stores the delivery profile, the Vidoc
    /// switch (SPEC-338 VOF-001) and the production protection switch (SPEC-423 PPR-002) in one
    /// write, or changes nothing when any is refused.
    ///
    /// # Errors
    /// `Forbidden` below admin, `Invalid` profiles, `Invalid` switches of a playground.
    pub async fn update_sandbox_profile(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        req: &UpdateSandbox,
    ) -> AppResult<Sandbox> {
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        let (_, level) = self.require_for(ctx, &sid, Action::Administer).await?;
        let actor = Self::dlv_actor_login(&ctx.user);
        let writing = self.profile_writes.lock().await;
        // Read under the lock: a switch or profile saved meanwhile is kept.
        let sandbox = self
            .deps
            .store
            .sandbox(&sid)
            .await?
            .ok_or(AppError::NotFound)?;
        if req.vidoc.is_some() && sandbox.is_playground() {
            return Err(playground_has_no_vidoc());
        }
        if req.protect_production.is_some() && sandbox.is_playground() {
            return Err(playground_has_no_production_protection());
        }
        let was_on = vidoc_switched_on(&sandbox);
        let on = req.vidoc.unwrap_or(was_on);
        let was_protected = production_protection_required(&sandbox);
        let protect = req.protect_production.unwrap_or(was_protected);
        // Letting agents write production is a person's decision (SPEC-423 PPR-002): never a
        // `yulius login` token, which an agent may hold.
        if was_protected && !protect && !ctx.acts_for_person() {
            return Err(AppError::Forbidden(
                "Turning the production protection off is a person's decision: switch it off on \
                 the sandbox's Git page (Production protection card), or with an operate token"
                    .into(),
            ));
        }
        let Some(requested) = &req.profile else {
            // Both switches in one write of the stored JSON, the rest kept as it is.
            if on != was_on || protect != was_protected {
                let mut stored = match serde_json::from_str::<Value>(&sandbox.profile_json) {
                    Ok(Value::Object(map)) => map,
                    _ => serde_json::Map::new(),
                };
                for (key, value) in [(VIDOC_SWITCH, on), (PRODUCTION_SWITCH, protect)] {
                    if value {
                        stored.remove(key);
                    } else {
                        stored.insert(key.to_owned(), Value::Bool(false));
                    }
                }
                let json = serde_json::to_string(&stored).map_err(AppError::internal)?;
                self.deps.store.set_profile(&sid, &json).await?;
            }
            drop(writing);
            if on != was_on {
                self.vidoc_switched(&actor, &sandbox, on).await;
            }
            if protect != was_protected {
                self.production_protection_switched(&actor, &sandbox, protect)
                    .await;
            }
            let updated = self
                .deps
                .store
                .sandbox(&sid)
                .await?
                .ok_or(AppError::NotFound)?;
            return self.sandbox_view(&updated, level).await;
        };
        let before = delivery_profile(&sandbox);
        let profile = profile_from_contract(requested)?;
        // A playground's maker may be anyone signed in, and commands run on the server: a
        // playground has no deploy (SPEC-099).
        if sandbox.is_playground() && (!profile.environments.is_empty() || profile.auto_promote) {
            return Err(invalid(
                "a playground has no deploy or promotion: its profile has no environment and no \
                 automatic promotion"
                    .to_owned(),
            ));
        }
        let gate = match requested.gate.as_deref() {
            None => None,
            Some(text) if text.trim().is_empty() => None,
            Some(text) => Some(
                yulius_domain::ci::validate_gate(text)
                    .map_err(|err| invalid(format!("gate: {err}")))?,
            ),
        };
        let mut stored = profile_to_contract(&profile);
        stored.gate.clone_from(&gate);
        // The switch is the stored one unless this request sets it: the profile editor does not
        // carry it (VOF-001).
        stored.vidoc = (!on).then_some(false);
        stored.protect_production = (!protect).then_some(false);
        // A sandbox's opt-out of handoffs is kept too (SPEC-512 HOF-002, review round 1).
        let mut stored = serde_json::to_value(&stored).map_err(AppError::internal)?;
        if !crate::handoffs::accepts_handoffs(&sandbox) {
            stored[crate::handoffs::ACCEPT_KEY] = Value::Bool(false);
        }
        let json = serde_json::to_string(&stored).map_err(AppError::internal)?;
        self.deps.store.set_profile(&sid, &json).await?;
        drop(writing);
        self.ci_forget_gate(&sid);
        if on != was_on {
            self.vidoc_switched(&actor, &sandbox, on).await;
        }
        if protect != was_protected {
            self.production_protection_switched(&actor, &sandbox, protect)
                .await;
        }
        self.dlv_audit(
            &actor,
            "delivery.profile.update",
            &sandbox.slug,
            &format!(
                "from preset {}: {}, gate {}",
                before.preset.as_str(),
                profile_summary(&profile),
                gate.as_deref().unwrap_or("(from yulius.toml)")
            ),
        )
        .await;
        let updated = self
            .deps
            .store
            .sandbox(&sid)
            .await?
            .ok_or(AppError::NotFound)?;
        self.sandbox_view(&updated, level).await
    }
}

// ---------------------------------------------------------------- profiles

fn invalid(message: impl Into<String>) -> AppError {
    AppError::Invalid(message.into())
}

/// Converts and validates an API profile. Unknown names are rejected, blank commands mean "no
/// command", a missing `require_checks` takes the preset's default, and the result is
/// relabeled (`custom` unless every setting equals the named preset's defaults, DLV-005).
///
/// # Errors
/// `Invalid` with the first problem found.
pub fn profile_from_contract(profile: &Profile) -> AppResult<DeliveryProfile> {
    let preset = Preset::parse(&profile.preset)
        .ok_or_else(|| invalid("preset must be standard, product, studio or custom"))?;
    let merge = MergeMethod::parse(&profile.merge)
        .ok_or_else(|| invalid("merge must be squash, merge or rebase"))?;
    let promote = PromoteMethod::parse(&profile.promote)
        .ok_or_else(|| invalid("promote must be merge_commit or fast_forward"))?;
    let mut environments = Vec::with_capacity(profile.environments.len());
    for env in &profile.environments {
        let after = Stage::parse(&env.after).ok_or_else(|| {
            invalid(format!(
                "environment '{}': after must be integration or production",
                env.name
            ))
        })?;
        let verify = Verify::parse(&env.verify).ok_or_else(|| {
            invalid(format!(
                "environment '{}': verify must be manual or command",
                env.name
            ))
        })?;
        environments.push(Environment {
            name: env.name.trim().to_owned(),
            after,
            command: env
                .command
                .as_deref()
                .filter(|c| !c.trim().is_empty())
                .map(str::to_owned),
            verify,
        });
    }
    // DLV-010: the manual `dev` checkpoint of the former standard preset is gone.
    if preset == Preset::Standard
        && matches!(environments.as_slice(), [only] if DeliveryProfile::is_legacy_standard_checkpoint(only))
    {
        environments.clear();
    }
    let parsed = DeliveryProfile {
        preset,
        integration_branch: profile.integration_branch.trim().to_owned(),
        production_branch: profile.production_branch.trim().to_owned(),
        merge,
        promote,
        auto_promote: profile.auto_promote,
        require_checks: profile
            .require_checks
            .unwrap_or_else(|| DeliveryProfile::for_preset(preset).require_checks),
        environments,
    };
    parsed
        .validate()
        .map_err(|err| AppError::Invalid(err.to_string()))?;
    Ok(parsed.relabeled())
}

/// The API form of a profile.
#[must_use]
pub fn profile_to_contract(profile: &DeliveryProfile) -> Profile {
    Profile {
        preset: profile.preset.as_str().to_owned(),
        integration_branch: profile.integration_branch.clone(),
        production_branch: profile.production_branch.clone(),
        merge: profile.merge.as_str().to_owned(),
        promote: profile.promote.as_str().to_owned(),
        auto_promote: profile.auto_promote,
        require_checks: Some(profile.require_checks),
        environments: profile
            .environments
            .iter()
            .map(|env| EnvironmentView {
                name: env.name.clone(),
                after: env.after.as_str().to_owned(),
                command: env.command.clone(),
                verify: env.verify.as_str().to_owned(),
            })
            .collect(),
        gate: None,
        vidoc: None,
        protect_production: None,
    }
}

/// The CI gate stored on the sandbox's profile (DLV-006); `None` when absent or invalid.
#[must_use]
pub fn profile_gate(sandbox: &SandboxRecord) -> Option<String> {
    serde_json::from_str::<Profile>(&sandbox.profile_json)
        .ok()
        .and_then(|profile| profile.gate)
        .and_then(|gate| yulius_domain::ci::validate_gate(&gate).ok())
}

/// Whether the sandbox's Vidoc security scans are on (SPEC-338 VOF-001): its stored profile's
/// `vidoc` is not `false`. Read on its own, so a profile that is otherwise unreadable keeps it.
#[must_use]
pub fn vidoc_switched_on(sandbox: &SandboxRecord) -> bool {
    profile_switch_on(&sandbox.profile_json, VIDOC_SWITCH)
}

/// Whether the sandbox's production branch must be protected from agents (SPEC-423 PPR-001): its
/// stored profile's `protect_production` is not `false`. Read on its own, like the Vidoc switch.
#[must_use]
pub fn production_protection_required(sandbox: &SandboxRecord) -> bool {
    profile_switch_on(&sandbox.profile_json, PRODUCTION_SWITCH)
}

/// The stored profile's key of the Vidoc switch.
const VIDOC_SWITCH: &str = "vidoc";
/// The stored profile's key of the production protection switch.
const PRODUCTION_SWITCH: &str = "protect_production";

/// Whether the switch `key` of a stored profile's JSON is on: anything but `false`, also in a
/// profile that is otherwise unreadable.
fn profile_switch_on(profile_json: &str, key: &str) -> bool {
    serde_json::from_str::<Value>(profile_json)
        .ok()
        .and_then(|profile| profile.get(key).and_then(Value::as_bool))
        != Some(false)
}

/// A playground runs no CI scan, so it has no Vidoc switch (SPEC-338 VOF-001).
fn playground_has_no_vidoc() -> AppError {
    invalid("a playground runs no Vidoc scan: it has no switch to turn".to_owned())
}

/// A playground has no GitHub branch to protect, so it has no protection switch (SPEC-423 PPR-002).
fn playground_has_no_production_protection() -> AppError {
    invalid(
        "a playground has no GitHub production branch: it has no protection switch to turn"
            .to_owned(),
    )
}

/// The sandbox's stored profile; an unreadable or invalid one falls back to `standard`.
#[must_use]
pub fn delivery_profile(sandbox: &SandboxRecord) -> DeliveryProfile {
    serde_json::from_str::<Profile>(&sandbox.profile_json)
        .map_err(|err| AppError::Invalid(err.to_string()))
        .and_then(|profile| profile_from_contract(&profile))
        .unwrap_or_else(|_| DeliveryProfile::standard())
}

/// The three named presets (DLV-002..004) in API form.
#[must_use]
pub fn preset_profiles() -> Vec<Profile> {
    [Preset::Standard, Preset::Product, Preset::Studio]
        .into_iter()
        .map(|preset| profile_to_contract(&DeliveryProfile::for_preset(preset)))
        .collect()
}

fn profile_summary(profile: &DeliveryProfile) -> String {
    let environments: Vec<String> = profile
        .environments
        .iter()
        .map(|env| {
            format!(
                "{} (after {}, verify {}, command: {})",
                env.name,
                env.after.as_str(),
                env.verify.as_str(),
                env.command.as_deref().unwrap_or("none")
            )
        })
        .collect();
    format!(
        "preset {}; {} -> {}; merge {}; promote {}; auto_promote {}; require_checks {}; environments: [{}]",
        profile.preset.as_str(),
        profile.integration_branch,
        profile.production_branch,
        profile.merge.as_str(),
        profile.promote.as_str(),
        profile.auto_promote,
        profile.require_checks,
        environments.join("; ")
    )
}

#[cfg(test)]
mod switch_tests {
    use super::{PRODUCTION_SWITCH, VIDOC_SWITCH, profile_switch_on};

    /// SPEC-423 PPR-001 (review round 1): a stored switch is off only for a boolean `false`; an
    /// absent, `true`, mistyped or unreadable value leaves it on, and an otherwise invalid profile
    /// keeps a stored `false`.
    #[test]
    fn a_switch_is_off_only_for_a_stored_false() {
        for key in [PRODUCTION_SWITCH, VIDOC_SWITCH] {
            for on in [
                "{}".to_owned(),
                format!(r#"{{"{key}":true}}"#),
                format!(r#"{{"{key}":"false"}}"#),
                format!(r#"{{"{key}":null}}"#),
                "not json".to_owned(),
                "[]".to_owned(),
            ] {
                assert!(profile_switch_on(&on, key), "{key} in {on}");
            }
            let off = format!(r#"{{"preset":42,"environments":"broken","{key}":false}}"#);
            assert!(!profile_switch_on(&off, key), "{key} in {off}");
        }
        assert!(profile_switch_on(r#"{"vidoc":false}"#, PRODUCTION_SWITCH));
        assert!(profile_switch_on(
            r#"{"protect_production":false}"#,
            VIDOC_SWITCH
        ));
    }
}
