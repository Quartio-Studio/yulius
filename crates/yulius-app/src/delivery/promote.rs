//! Promotion from the integration branch to production.

use super::*;

impl App {
    // ---------------------------------------------------------------- promotion

    /// Tasks of the sandbox (archived ones included) with their delivery rows.
    pub(super) async fn dlv_sandbox_deliveries(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Vec<(TaskRecord, DeliveryRecord)>> {
        let store = &self.deps.store;
        let records = store.deliveries(&sandbox.id).await?;
        let mut tasks = store.tasks(&sandbox.id, false).await?;
        tasks.extend(store.tasks(&sandbox.id, true).await?);
        tasks.sort_by_key(|t| t.number);
        Ok(tasks
            .into_iter()
            .map(|task| {
                let record = records
                    .iter()
                    .find(|r| r.task_id == task.id)
                    .cloned()
                    .unwrap_or_else(|| DeliveryRecord::initial(&task.id));
                (task, record)
            })
            .collect())
    }

    /// Moves tasks to their computed stages (compare-and-set each); returns the numbers moved.
    pub(super) async fn dlv_advance(
        &self,
        moves: Vec<(TaskRecord, DeliveryRecord, DeliveryStage)>,
    ) -> AppResult<Vec<u32>> {
        let mut moved = Vec::new();
        for (task, record, next) in moves {
            if next == record.stage {
                continue;
            }
            let updated = DeliveryRecord {
                stage: next,
                updated_at: self.now(),
                ..record.clone()
            };
            if self
                .dlv_transition(&task.sandbox_id, &record.stage, &updated)
                .await?
            {
                moved.push(task.number);
                self.publish_task(&task.id).await;
            }
        }
        Ok(moved)
    }

    /// `(integration-only commits, production-only commits)` from the sandbox clone, fetched
    /// first unless `fetched`; `None` when Git cannot tell.
    pub(crate) async fn dlv_branch_relation(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        fetched: bool,
    ) -> Option<(u32, u32)> {
        if !fetched {
            self.dlv_refresh_clone(sandbox).await.ok()?;
        }
        let repo = self.sandbox_repo(&sandbox.id);
        self.deps
            .git
            .ahead_behind(
                &repo,
                &format!("origin/{}", profile.production_branch),
                &format!("origin/{}", profile.integration_branch),
            )
            .await
            .ok()
    }

    /// Promotes the integration branch (fast-forward, or open/merge the promotion pull request with
    /// a merge commit); with `expected`, only that integration commit a person confirmed or the
    /// staging queue verified (SPEC-134 review round 3, SPEC-575 MFS-009): refused, before
    /// anything moves, when the integration branch or the promotion pull request's head is
    /// another commit.
    #[allow(clippy::too_many_arguments)] // The promotion's context and the expected commit.
    pub(crate) async fn dlv_promote_exactly(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        by: Initiator,
        actor: &UserRecord,
        token: &Secret,
        fetched: bool,
        source: PromotionSource<'_>,
    ) -> AppResult<PromoteResult> {
        let expected = match source {
            PromotionSource::Head(sha) => sha,
            PromotionSource::Verified { sha, .. } => Some(sha),
        };
        crate::playground::refuse_in_playground(sandbox, "Promotion")?;
        let moved = |at: &str| {
            AppError::Conflict(format!(
                "nothing was promoted: '{}' moved since it was confirmed (it is now {}); review \
                 it and deploy again",
                profile.integration_branch,
                at.get(..7).unwrap_or(at)
            ))
        };
        let mut candidates = Vec::new();
        for (task, record) in self.dlv_sandbox_deliveries(sandbox).await? {
            if region(&record.stage, profile) != Some(Stage::Integration) {
                continue;
            }
            if let PromotionSource::Verified { sha, mirror, .. } = source {
                let mut landing = self
                    .deps
                    .store
                    .landed_sha(&task.id, record.pr_number, None)
                    .await?;
                if landing.is_none()
                    && let Some(number) = record.pr_number
                {
                    let pull = self
                        .deps
                        .github
                        .get_pull(token, &sandbox.repo_owner, &sandbox.repo_name, number)
                        .await?;
                    let known = evidence::Landing::closed_pull(&pull);
                    landing = self
                        .deps
                        .store
                        .landed_sha(&task.id, Some(number), known.as_ref())
                        .await?;
                }
                let landing = landing.ok_or_else(|| {
                    AppError::Conflict(format!(
                        "the merged revision of task #{} is unknown",
                        task.number
                    ))
                })?;
                if !self.deps.git.is_ancestor(mirror, &landing, sha).await? {
                    continue;
                }
            }
            candidates.push((task, record));
        }
        for (task, record) in &candidates {
            if let Some(env) = pending_integration_environment(&record.stage, profile) {
                return Err(AppError::Conflict(format!(
                    "task #{}: environment '{}' is not done yet; finish it before promoting",
                    task.number, env.name
                )));
            }
        }
        if by == Initiator::Policy && candidates.is_empty() {
            return Err(AppError::Conflict(
                "nothing merged through Yulius is waiting for promotion".into(),
            ));
        }
        let github = &self.deps.github;
        let (owner, name) = (&sandbox.repo_owner, &sandbox.repo_name);
        let integration_sha = github
            .branch_sha(token, owner, name, &profile.integration_branch)
            .await?
            .ok_or_else(|| {
                AppError::Conflict(format!(
                    "branch '{}' does not exist on GitHub",
                    profile.integration_branch
                ))
            })?;
        let observed = match source {
            PromotionSource::Head(sha) => sha,
            PromotionSource::Verified { head, .. } => Some(head),
        };
        if observed.is_some_and(|sha| !sha.eq_ignore_ascii_case(&integration_sha)) {
            return Err(moved(&integration_sha));
        }
        let integration_sha = expected.unwrap_or(&integration_sha).to_owned();
        let production_sha = github
            .branch_sha(token, owner, name, &profile.production_branch)
            .await?;
        if let PromotionSource::Verified { base, .. } = source
            && production_sha.as_deref() != Some(base)
        {
            return Err(AppError::Conflict(
                "nothing was promoted: the production base changed after the release preview"
                    .into(),
            ));
        }
        let relation = match (source, production_sha.as_deref()) {
            (PromotionSource::Verified { mirror, .. }, Some(prod)) => Some(
                self.deps
                    .git
                    .ahead_behind(mirror, prod, &integration_sha)
                    .await?,
            ),
            _ => self.dlv_branch_relation(sandbox, profile, fetched).await,
        };
        let contained = production_sha.as_deref() == Some(integration_sha.as_str())
            || relation.is_some_and(|(ahead, _)| ahead == 0);
        if contained {
            if matches!(source, PromotionSource::Verified { .. }) && profile.require_checks {
                self.dlv_checks_allow(sandbox, token, &integration_sha, false)
                    .await?;
            }
            if candidates.is_empty() && matches!(source, PromotionSource::Verified { .. }) {
                return Ok(PromoteResult {
                    result: "already_promoted".into(),
                    sha: production_sha,
                });
            }
            if candidates.is_empty() {
                return Err(AppError::Conflict(format!(
                    "nothing to promote: '{}' already contains '{}'",
                    profile.production_branch, profile.integration_branch
                )));
            }
            // Promoted outside Yulius (for example the promotion pull request merged on
            // GitHub): record it instead of leaving the tasks waiting.
            let facts = merged_facts(false);
            let mut moves = Vec::new();
            for (task, record) in candidates {
                let next = dlv::apply(&record.stage, &DeliveryAction::Promote, by, profile, &facts)
                    .map_err(|err| refusal(&err))?;
                moves.push((task, record, next));
            }
            let numbers: Vec<String> = moves
                .iter()
                .map(|(t, _, _)| format!("#{}", t.number))
                .collect();
            self.dlv_audit(
                &Self::dlv_policy_actor(by, actor),
                "delivery.promote.reconciled",
                &sandbox.slug,
                &format!(
                    "'{}' already contains '{}'; tasks [{}] recorded as promoted",
                    profile.production_branch,
                    profile.integration_branch,
                    numbers.join(", ")
                ),
            )
            .await;
            self.dlv_advance(moves).await?;
            return Ok(PromoteResult {
                result: "promoted".into(),
                sha: production_sha,
            });
        }
        let diverged = profile.promote == PromoteMethod::FastForward
            && relation.is_some_and(|(_, behind)| behind > 0);
        let facts = merged_facts(diverged);
        let mut moves = Vec::new();
        if candidates.is_empty() {
            if diverged {
                return Err(refusal(&DeliveryError::ProductionDiverged {
                    production: profile.production_branch.clone(),
                    integration: profile.integration_branch.clone(),
                }));
            }
        } else {
            for (task, record) in candidates {
                let next = dlv::apply(&record.stage, &DeliveryAction::Promote, by, profile, &facts)
                    .map_err(|err| refusal(&err))?;
                moves.push((task, record, next));
            }
        }
        let numbers: Vec<String> = moves
            .iter()
            .map(|(t, _, _)| format!("#{}", t.number))
            .collect();
        let (result, sha) = match profile.promote {
            PromoteMethod::FastForward => {
                // What goes to production is the integration head: its checks decide, as a
                // promotion pull request's do (the head is the same commit).
                if profile.require_checks {
                    // An automatic promotion (after a merge, or after the last integration
                    // environment of a merged change) does not wait for a verdict of the
                    // integration head: what it promotes went in on green checks of a current
                    // head, the commit the merge made may have no run yet, and nothing
                    // promotes later if this one is refused as pending. A verdict that is
                    // there (failed, unreadable) stops it.
                    let automatic = matches!(by, Initiator::Policy);
                    self.dlv_checks_allow(sandbox, token, &integration_sha, automatic)
                        .await?;
                }
                github
                    .fast_forward(
                        token,
                        owner,
                        name,
                        &profile.production_branch,
                        &integration_sha,
                    )
                    .await?;
                ("promoted", integration_sha.clone())
            }
            PromoteMethod::MergeCommit => {
                let from = if let PromotionSource::Verified { sha, .. } = source {
                    let branch = format!("yulius/release/{sha}");
                    if github
                        .branch_sha(token, owner, name, &branch)
                        .await?
                        .is_none()
                    {
                        // Another request can create the same ref between the read and the POST.
                        if let Err(err) =
                            github.create_branch(token, owner, name, &branch, sha).await
                            && github
                                .branch_sha(token, owner, name, &branch)
                                .await?
                                .as_deref()
                                != Some(sha)
                        {
                            return Err(err);
                        }
                    }
                    if github
                        .branch_sha(token, owner, name, &branch)
                        .await?
                        .as_deref()
                        != Some(sha)
                    {
                        return Err(AppError::Conflict(format!(
                            "promotion ref {branch} has changed"
                        )));
                    }
                    branch
                } else {
                    profile.integration_branch.clone()
                };
                let existing = github
                    .list_pulls(token, owner, name, None)
                    .await?
                    .into_iter()
                    .find(|p| {
                        // A fork's `dev` is never this repository's promotion (SPEC-069).
                        is_open(p)
                            && !p.from_fork
                            && p.head == from
                            && p.base == profile.production_branch
                    });
                match existing {
                    None => {
                        let mut body = format!(
                            "Promotes `{}` to `{}` with a merge commit.\n\n",
                            profile.integration_branch, profile.production_branch
                        );
                        for (task, _, _) in &moves {
                            body.push_str(&format!("- #{} {}\n", task.number, task.title));
                        }
                        body.push_str(&format!(
                            "\n---\nOpened from Yulius: {}/s/{}/git",
                            self.config.public_url, sandbox.slug
                        ));
                        let pull = github
                            .create_pull(
                                token,
                                owner,
                                name,
                                &NewPull {
                                    head: from.clone(),
                                    base: profile.production_branch.clone(),
                                    // The branch policy's release title (SPEC-069 BPL-005).
                                    title: format!(
                                        "release: promote {} to {}",
                                        profile.integration_branch, profile.production_branch
                                    ),
                                    body: clip_bytes(&body, MAX_PR_BODY_BYTES).to_owned(),
                                },
                            )
                            .await?;
                        self.dlv_audit(
                            &Self::dlv_policy_actor(by, actor),
                            "delivery.promotion_pr.open",
                            &sandbox.slug,
                            &format!(
                                "pull request #{} {} -> {} for tasks [{}]",
                                pull.number,
                                profile.integration_branch,
                                profile.production_branch,
                                numbers.join(", ")
                            ),
                        )
                        .await;
                        return Ok(PromoteResult {
                            result: "pr_opened".into(),
                            sha: Some(pull.html_url),
                        });
                    }
                    Some(_) if by == Initiator::Policy => {
                        return Err(AppError::Conflict(
                            "the promotion pull request waits for a person to merge it".into(),
                        ));
                    }
                    Some(pull) => {
                        if expected.is_some_and(|sha| !sha.eq_ignore_ascii_case(&pull.head_sha)) {
                            return Err(moved(&pull.head_sha));
                        }
                        let mut facts = pull_facts(&pull);
                        // The exact PR SHA and same-repository ref were checked above. This
                        // ref represents a verified commit of the integration branch for policy.
                        if matches!(source, PromotionSource::Verified { .. }) {
                            facts.head = &profile.integration_branch;
                        }
                        self.dlv_check_policy(sandbox, profile, pull.number, &facts)
                            .await?;
                        if profile.require_checks {
                            self.dlv_checks_allow(sandbox, token, &pull.head_sha, false)
                                .await?;
                        }
                        if let PromotionSource::Verified { base, .. } = source
                            && github
                                .branch_sha(token, owner, name, &profile.production_branch)
                                .await?
                                .as_deref()
                                != Some(base)
                        {
                            return Err(AppError::Conflict("nothing was promoted: the production base changed while checking the release pull request".into()));
                        }
                        let sha = github
                            .merge_pull(
                                token,
                                owner,
                                name,
                                pull.number,
                                MergeMethod::Merge.as_str(),
                                &pull.head_sha,
                            )
                            .await?;
                        ("promoted", sha)
                    }
                }
            }
        };
        if let PromotionSource::Verified {
            sha: candidate,
            mirror,
            ..
        } = source
        {
            // GitHub guards the PR head atomically. An outside writer can still move its base
            // inside the merge call. Check the result before recording any delivery as released.
            self.dlv_refresh_clone(sandbox).await?;
            let current = self
                .cd_verified_head(sandbox, &profile.production_branch, token)
                .await?;
            let tree = self.deps.git.tree_of(mirror, candidate).await?;
            if current != sha
                || tree.is_none()
                || self.deps.git.tree_of(mirror, &current).await? != tree
            {
                return Err(AppError::Conflict(
                    "nothing was deployed: the production merge did not land the confirmed tree"
                        .into(),
                ));
            }
        }
        self.dlv_audit(
            &Self::dlv_policy_actor(by, actor),
            "delivery.promote",
            &sandbox.slug,
            &format!(
                "{} -> {} ({}) at {}; tasks [{}]",
                profile.integration_branch,
                profile.production_branch,
                profile.promote.as_str(),
                sha,
                numbers.join(", ")
            ),
        )
        .await;
        self.dlv_advance(moves).await?;
        Ok(PromoteResult {
            result: result.into(),
            sha: Some(sha),
        })
    }

    fn dlv_policy_actor(by: Initiator, actor: &UserRecord) -> String {
        match by {
            Initiator::User => Self::dlv_actor_login(actor),
            Initiator::Policy => format!(
                "policy:auto_promote (after {})",
                Self::dlv_actor_login(actor)
            ),
        }
    }

    /// Runs the profile's automatic promotion when it is enabled and every merged change has
    /// finished its integration environments. Returns a note describing the outcome.
    pub(super) async fn dlv_auto_promote_if_enabled(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        actor: &UserRecord,
        token: &Secret,
        fetched: bool,
    ) -> Option<String> {
        if !profile.auto_promote {
            return None;
        }
        let deliveries = self.dlv_sandbox_deliveries(sandbox).await.ok()?;
        let waiting: Vec<&DeliveryRecord> = deliveries
            .iter()
            .map(|(_, record)| record)
            .filter(|record| region(&record.stage, profile) == Some(Stage::Integration))
            .collect();
        if waiting.is_empty()
            || waiting
                .iter()
                .any(|record| pending_integration_environment(&record.stage, profile).is_some())
        {
            return None;
        }
        // With a merge-first target, only the revision staging verified is promoted (SPEC-575
        // MFS-009); the staging queue tries again once staging verified the head.
        // Waiting for staging is the expected state of a merge-first project's merge, not a
        // failed promotion (review round 1 of slice 4).
        let head = match self.staging_promotion_head(sandbox).await {
            Ok(Ok(head)) => head,
            Ok(Err(waits)) => {
                self.dlv_audit(
                    &Self::dlv_policy_actor(Initiator::Policy, actor),
                    "delivery.promote.waiting",
                    &sandbox.slug,
                    &waits,
                )
                .await;
                return Some(format!("Automatic promotion waits for staging: {waits}"));
            }
            Err(err) => {
                self.dlv_audit(
                    &Self::dlv_policy_actor(Initiator::Policy, actor),
                    "delivery.promote.failed",
                    &sandbox.slug,
                    &err.public_message(),
                )
                .await;
                return Some(format!(
                    "Automatic promotion did not happen: {}",
                    err.public_message()
                ));
            }
        };
        let promoted = self
            .dlv_promote_exactly(
                sandbox,
                profile,
                Initiator::Policy,
                actor,
                token,
                fetched,
                PromotionSource::Head(head.as_deref()),
            )
            .await;
        match promoted {
            Ok(result) => Some(format!(
                "Automatic promotion ({}): {} {}",
                profile.promote.as_str(),
                result.result,
                result.sha.unwrap_or_default()
            )),
            Err(err) => {
                self.dlv_audit(
                    &Self::dlv_policy_actor(Initiator::Policy, actor),
                    "delivery.promote.failed",
                    &sandbox.slug,
                    &err.public_message(),
                )
                .await;
                Some(format!(
                    "Automatic promotion did not happen: {}",
                    err.public_message()
                ))
            }
        }
    }

    /// `POST /api/sandboxes/{sid}/delivery/promote` (admin, `{"confirm": "<slug>"}`): an explicit
    /// promotion, a production release as the CD page's (ADR-0110). It acts as
    /// the caller on GitHub, so the superadmin without a GitHub identity is told to release on
    /// GitHub (ACC-012).
    ///
    /// # Errors
    /// `Conflict` when environments are pending, production diverged or nothing changed;
    /// `Forbidden` without a GitHub identity.
    pub async fn promote_sandbox(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        confirm: Option<&str>,
    ) -> AppResult<PromoteResult> {
        // A promotion is a production release: an administrator's, with the project's slug typed,
        // as a production deploy on the CD page (SPEC-134, ADR-0110).
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.require_for(ctx, &sid, Action::Work).await?;
        crate::playground::refuse_in_playground(&sandbox, "Promotion")?;
        yulius_domain::cd::may_deploy(
            yulius_domain::cd::CdStage::Prod,
            yulius_domain::cd::Requester::Person {
                administrator: level.allows(Action::Administer),
                confirmation: confirm,
            },
            &sandbox.slug,
        )
        .map_err(|refused| match refused {
            yulius_domain::cd::CdRefusal::ConfirmationMismatch { .. } => {
                AppError::Invalid(refused.to_string())
            }
            other => AppError::Forbidden(other.to_string()),
        })?;
        let profile = delivery_profile(&sandbox);
        let token = self
            .person_github_token(
                &ctx.user,
                &format!("Releasing to {}", profile.production_branch),
            )
            .await?;
        // With a merge-first target, only the revision staging verified (SPEC-575 MFS-009).
        let head = self
            .staging_promotion_head(&sandbox)
            .await?
            .map_err(AppError::Conflict)?;
        self.dlv_promote_exactly(
            &sandbox,
            &profile,
            Initiator::User,
            &ctx.user,
            &token,
            false,
            PromotionSource::Head(head.as_deref()),
        )
        .await
    }

    /// The staging queue verified `request`'s revision: the profile's automatic promotion that
    /// waited for it runs now (SPEC-575 MFS-009), as the request's task's owner.
    pub(crate) async fn staging_auto_promote(&self, sandbox: &SandboxRecord, owner: &UserRecord) {
        let profile = delivery_profile(sandbox);
        if !profile.auto_promote {
            return;
        }
        let token = match self.user_github_token(owner).await {
            Ok(token) => token,
            Err(err) => {
                // Said, never dropped silently (review round 1 of slice 4).
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "the promotion waiting for staging could not run: no GitHub token of the task's owner");
                self.dlv_audit(
                    &Self::dlv_actor_login(owner),
                    "delivery.promote.failed",
                    &sandbox.slug,
                    &format!("after a verified staging deploy: {}", err.public_message()),
                )
                .await;
                return;
            }
        };
        if let Some(note) = self
            .dlv_auto_promote_if_enabled(sandbox, &profile, owner, &token, false)
            .await
        {
            tracing::info!(sandbox = %sandbox.slug, note = %note, "promotion after a verified staging deploy");
        }
    }
}
