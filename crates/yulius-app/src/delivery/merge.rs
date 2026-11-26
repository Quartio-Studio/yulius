//! Merging task pull requests and reconciling deliveries with GitHub.

use super::*;

impl App {
    /// Whether an open pull request of `task` conflicts with its base (SPEC-491 DON-001). Yulius's
    /// own merge of its head with the integration branch, as last fetched and with the clone's
    /// merge drivers, decides (what the pull request watch tells the agent, PRN-004): GitHub
    /// knows nothing of the union rules Yulius gives the changes log and the decision index, so
    /// it calls a collision there a conflict that Yulius's merge resolves (review of SPEC-491,
    /// M1). Only where the clone cannot answer (no worktree) does GitHub's flag decide; a list
    /// of pull requests carries none, so there it is no conflict.
    pub(crate) async fn pull_conflicts(
        &self,
        task: &TaskRecord,
        sandbox: &SandboxRecord,
        pull: &GhPull,
    ) -> bool {
        match self.local_conflict(task, sandbox, &pull.head_sha).await {
            Some(true) => true,
            Some(false) if pull.mergeable == Some(false) => {
                // GitHub reports CONFLICTING/DIRTY while our local clone said clean: either the
                // clone's `origin/<integration>` is stale (the base moved on GitHub) or a union
                // merge driver resolves it (SPEC-491 DON-001). First check whether our local
                // `origin/<integration>` already matches `pull.base_sha` — if so, `local_conflict`
                // already evaluated the exact base GitHub used (clean union-driver merge) and no
                // network fetch is needed (SPEC-580 review F3).
                let integration =
                    format!("origin/{}", delivery_profile(sandbox).integration_branch);
                let base_matches = !pull.base_sha.is_empty()
                    && match task.worktree_path.as_deref() {
                        Some(wt) => self
                            .deps
                            .git
                            .resolve_commit(std::path::Path::new(wt), &integration)
                            .await
                            .ok()
                            .flatten()
                            .is_some_and(|local_base| {
                                local_base.eq_ignore_ascii_case(&pull.base_sha)
                            }),
                        None => false,
                    };
                if base_matches {
                    return false;
                }
                // Local clone base differs (or `pull.base_sha` was absent): refresh and re-check;
                // if the clone cannot be refreshed, trust GitHub's flag rather than a stale base
                // (SPEC-580 AWM-003).
                if self.dlv_refresh_clone(sandbox).await.is_ok() {
                    self.local_conflict(task, sandbox, &pull.head_sha)
                        .await
                        .unwrap_or(true)
                } else {
                    true
                }
            }
            Some(false) => false,
            None => pull.mergeable == Some(false),
        }
    }

    /// Whether a task's pull request is recorded as merged.
    pub(crate) async fn task_merged(&self, task: &TaskId) -> bool {
        matches!(self.deps.store.delivery(task).await, Ok(Some(record)) if past_merge(&record.stage))
    }

    // A confirmed branch identity must outlive the process that observed it. The normal
    // opening transaction also retires a closed-unmerged snapshot and records the base for
    // landing recovery. Its compare-and-set preserves a competing or already merged delivery.
    pub(super) async fn bind_observed_open_pull(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskId,
        pull: &GhPull,
    ) -> AppResult<()> {
        if pull.base == delivery_profile(sandbox).integration_branch {
            let next = DeliveryRecord {
                task_id: task.clone(),
                stage: DeliveryStage::PrOpen,
                pr_number: Some(pull.number),
                updated_at: self.now(),
            };
            if self
                .dlv_transition_at(
                    &sandbox.id,
                    &DeliveryStage::None,
                    &next,
                    Some(&pull.base),
                    None,
                )
                .await?
            {
                return Ok(());
            }
        }
        // The primary delivery can stay merged while its task branch has an open follow-up.
        // Replacing the observation retires any stale closure atomically, on any base branch.
        self.deps.store.remember_pull_observation(task, pull).await
    }

    /// Discover external follow-ups once per repository on the delivery tick. A task read
    /// can then verify a new number without probing each terminal branch for replacements.
    async fn refresh_terminal_branches(&self, sandbox: &SandboxRecord) -> AppResult<()> {
        const PAGE: u32 = 24;
        let after = self
            .terminal_scan_after
            .lock()
            .map_err(AppError::internal)?
            .get(&sandbox.id)
            .cloned();
        let store = &self.deps.store;
        let candidates = store
            .terminal_branches_for_discovery(&sandbox.id, after.as_ref(), PAGE)
            .await?;
        // At most one page reserves branch publication slots on a tick. Rotate even if a
        // credential or a single upstream pull is unavailable, so other pages get their turn.
        {
            let mut cursors = self
                .terminal_scan_after
                .lock()
                .map_err(AppError::internal)?;
            if candidates.len() == PAGE as usize
                && let Some(last) = candidates.last()
            {
                cursors.insert(sandbox.id.clone(), last.task.clone());
            } else {
                cursors.remove(&sandbox.id);
            }
        }
        if candidates.is_empty() {
            return Ok(());
        }
        let reads: Vec<_> = candidates
            .into_iter()
            .filter_map(|candidate| {
                let task = &candidate.task;
                let read = self.observed_pulls.begin_branch(&sandbox.id, task).ok()?;
                let known = self
                    .observed_pulls
                    .known_pull(&sandbox.id, task)
                    .flatten()
                    .filter(|p| p.open)
                    .map(|p| p.number)
                    .or_else(|| {
                        is_open(&candidate.observation).then_some(candidate.observation.number)
                    });
                Some((candidate, read, known))
            })
            .collect();
        let token = self.dlv_installation_token(sandbox).await?;
        let (owner, repo) = (&sandbox.repo_owner, &sandbox.repo_name);
        let github = &self.deps.github;
        for (candidate, _, _) in &reads {
            if let Some(number) = candidate.pending_landing {
                let refreshed = crate::ports::github::for_purpose(
                    crate::ports::github::GitHubPurpose::Merges,
                    github.get_pull(&token, owner, repo, number),
                )
                .await;
                if let Ok(pull) = refreshed
                    && pull.number == number
                    && let Some(landing) = evidence::Landing::closed_pull(&pull)
                {
                    store
                        .remember_pull_observation(&candidate.task, &pull)
                        .await?;
                    store
                        .landed_sha(&candidate.task, Some(pull.number), Some(&landing))
                        .await?;
                }
            }
        }
        let pulls = github.list_pulls(&token, owner, repo, None).await?;
        for (candidate, read, known) in reads {
            let (task, branch, terminal) =
                (candidate.task, candidate.branch, candidate.observation);
            let newest = pulls
                .iter()
                .filter(|p| !p.from_fork && p.head == branch && is_open(p))
                .max_by_key(|p| p.number);
            // A listing may lag a closure. Only a current numbered response can reopen
            // an already closed identity. A missing listing alone cannot retire a follow-up.
            let mut confirm = known;
            if let Some(listed) = newest {
                match store.terminal_pull(&task, Some(listed.number)).await? {
                    Some(closed) if !closed.merged => confirm = Some(listed.number),
                    None if listed.number >= terminal.number || known == Some(listed.number) => {
                        if !is_open(&terminal) || terminal.number != listed.number {
                            self.bind_observed_open_pull(sandbox, &task, listed).await?;
                        }
                        read.record_branch(&task, terminal.number, Some(listed.number));
                        continue;
                    }
                    _ => {}
                }
            }
            let Some(number) = confirm else { continue };
            let Ok(pull) = github.confirm_pull(&token, owner, repo, number).await else {
                continue;
            };
            if pull.number != number {
                continue;
            }
            if is_open(&pull) {
                if !pull.from_fork
                    && pull.head == branch
                    && !(number == terminal.number && terminal.merged)
                {
                    self.bind_observed_open_pull(sandbox, &task, &pull).await?;
                    read.record_branch(&task, terminal.number, Some(number));
                }
            } else {
                store.remember_pull_observation(&task, &pull).await?;
                if let Some(landing) = evidence::Landing::closed_pull(&pull) {
                    store
                        .landed_sha(&task, Some(number), Some(&landing))
                        .await?;
                }
                read.record_terminal(&task, &pull);
            }
        }
        Ok(())
    }

    /// Reconciles deliveries in progress with GitHub in the background (#35): a pull request
    /// merged or closed outside Yulius moves its task on the Git board without anyone opening
    /// the task. Uses each sandbox's installation token; failures leave the stage as recorded.
    pub async fn reconcile_deliveries(&self) {
        let store = &self.deps.store;
        let Ok(sandboxes) = store.sandboxes().await else {
            return;
        };
        for sandbox in sandboxes.iter().filter(|s| s.removed_at.is_none()) {
            if let Err(error) = crate::ports::github::for_purpose(
                crate::ports::github::GitHubPurpose::Display,
                self.refresh_terminal_branches(sandbox),
            )
            .await
            {
                tracing::debug!(%error, "could not refresh open follow-up pull requests");
            }
            let Ok(records) = store.deliveries(&sandbox.id).await else {
                continue;
            };
            let mut open = Vec::new();
            // Only open pull requests can change on GitHub (merged, closed); later stages move
            // through Yulius, so re-reading them would only cost requests (#68).
            for record in records
                .into_iter()
                .filter(|r| r.stage == DeliveryStage::PrOpen)
            {
                if let Ok(Some(task)) = store.task(&record.task_id).await
                    && task.archived_at.is_none()
                {
                    open.push((task, record.stage));
                }
            }
            if open.is_empty() {
                continue;
            }
            let Ok(token) = self.dlv_installation_token(sandbox).await else {
                continue;
            };
            let profile = delivery_profile(sandbox);
            for (task, before) in open {
                if let Ok(observed) = self
                    .dlv_observe(
                        sandbox,
                        &task,
                        &profile,
                        ObserveAccess::Provided(Some(&token)),
                    )
                    .await
                {
                    self.retry_errored_checks(&task, &observed).await;
                    if let Ok(Some(after)) = store.delivery(&task.id).await
                        && after.stage != before
                    {
                        self.publish_task(&task.id).await;
                    }
                }
            }
        }
    }

    /// Retry an infrastructure error once for the selected CI reference at this head.
    async fn retry_errored_run(&self, task: &TaskRecord, pr: u64, sha: &str) {
        let Ok(runs) = self
            .deps
            .store
            .ci_runs_for_pull_head(&task.sandbox_id, crate::ci::PullHead::Exact(sha))
            .await
        else {
            return;
        };
        let Some(latest) = crate::ci::run_for_pull_head(&runs, pr) else {
            return;
        };
        let errored = runs
            .iter()
            .filter(|r| {
                r.reference == latest.reference && r.state == yulius_domain::ci::RunState::Error
            })
            .count();
        if latest.state == yulius_domain::ci::RunState::Error
            && errored < 2
            && let Ok(Some(sandbox)) = self.deps.store.sandbox(&task.sandbox_id).await
        {
            let _ = self.ci_rerun(&sandbox, &latest.id, "yulius").await;
        }
    }

    /// An errored Yulius CI run at an open pull request's head (it could not run) is queued
    /// again once by itself (SPEC-020); `yulius wait` and `ci_status` say the rest.
    async fn retry_errored_checks(&self, task: &TaskRecord, observed: &Observed) {
        let Some(pull) = observed.pr.as_ref().filter(|p| is_open(p)) else {
            return;
        };
        if observed.checks == ChecksState::Failure && pull.mergeable != Some(false) {
            self.retry_errored_run(task, pull.number, &pull.head_sha)
                .await;
        }
    }

    // ---------------------------------------------------------------- merge

    /// `POST /api/tasks/{tid}/delivery/merge`: merges the task pull request with the profile's
    /// method at the head that was checked, after the domain gates (spec, a finished step of the
    /// plan, checks per profile, current head; the pull request has its head already, so no
    /// step started since holds it, SPEC-484 PLN-002). With `auto_promote`,
    /// promotes right after (policy). It acts as the caller on GitHub; the superadmin without a
    /// GitHub identity acts through the GitHub App (ACC-012, SPEC-677 SAP-005).
    ///
    /// # Errors
    /// `Conflict` refusals (including stale heads), `Forbidden` for a guest without a GitHub
    /// identity,
    /// `Upstream` GitHub failures.
    pub async fn merge_task_pr(&self, ctx: &AuthContext, task_id: &str) -> AppResult<Delivery> {
        let task = self.load_task(ctx, task_id, Action::Work).await?;
        // A playground merges here, with no GitHub identity needed (SPEC-099), on green (SPEC-163).
        let sandbox = self.dlv_sandbox_of(&task).await?;
        if sandbox.is_playground() {
            return self.playground_merge_now(&ctx.user, &task).await;
        }
        // The superadmin without a GitHub identity merges through the App (SPEC-677 SAP-005).
        let acting = self
            .person_or_app_token(&ctx.user, &sandbox, "Merging a pull request")
            .await?;
        let merged = self
            .dlv_merge(ctx, &task, &acting.token)
            .await
            .map_err(MergeFailure::into_error)
            .map_err(|err| {
                if acting.through_app {
                    crate::access::app_refusal(err, &delivery_profile(&sandbox).integration_branch)
                } else {
                    err
                }
            })?;
        if self.merge_is_armed(&task.id) {
            let steps = self.deps.store.steps(&task.id).await.unwrap_or_default();
            let gate = self
                .ci_gate_for_pull(&sandbox, merged.number, &merged.head)
                .await;
            let ahead = steps_ahead_note(&steps);
            let staging = self
                .green_merge_staging_note(&task, &sandbox, merged.number, gate)
                .await;
            let receipt = crate::agent_wait::merge_receipt_with_checks(
                &merged.receipt,
                &delivery_profile(&sandbox).integration_branch,
                gate,
                staging.is_none(),
                &ahead,
            );
            let note = match staging {
                Some(note) => format!("{receipt} {note}{ahead}"),
                None => receipt,
            };
            self.settle_armed(&task, Some((&merged.head, note))).await;
        }
        Ok(merged.view)
    }

    /// [`App::merge_task_pr`] for the task's agent (`delivery_merge`) under its owner's
    /// authority `ctx`: GitHub sees the owner merge, or the App when the owner has no usable
    /// GitHub token (F 2026-10-11).
    ///
    /// # Errors
    /// [`MergeFailure::Later`] when GitHub could not take the new title for now (the armed
    /// merge and the merge train try again on their next pass); [`MergeFailure::Refused`] as
    /// [`App::merge_task_pr`], without the refusal for an account with no GitHub identity.
    pub(crate) async fn merge_task_pr_for_task(
        &self,
        ctx: &AuthContext,
        task_id: &str,
    ) -> Result<MergedPull, MergeFailure> {
        let task = self.load_task(ctx, task_id, Action::Work).await?;
        let sandbox = self.dlv_sandbox_of(&task).await?;
        // A playground merges through `playground_merge` under the armed-merge pass, which its
        // callers hold (SPEC-163): never from here.
        if sandbox.is_playground() {
            return Err(MergeFailure::Refused(AppError::internal(
                "a playground's merge does not go through its pull request",
            )));
        }
        let token = self.task_github_token(&task, &sandbox).await?;
        self.dlv_merge(ctx, &task, &token).await
    }

    /// The branch policy of `yulius.toml` at the tip of the integration branch in the sandbox
    /// clone, so a pull request cannot relax the rules it is checked against (SPEC-069 BPL-003):
    /// the standard preset without the file, and when it cannot be read or is invalid — then with
    /// the problem, for the refusal to say why.
    pub(crate) async fn dlv_branch_policy(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
    ) -> (BranchPolicy, Option<String>) {
        let repo = self.sandbox_repo(&sandbox.id);
        let rev = format!("origin/{}", profile.integration_branch);
        let problem = match self.deps.git.show_file(&repo, &rev, "yulius.toml").await {
            Ok(None) => return (BranchPolicy::standard(), None),
            Ok(Some(text)) => match spec_format::parse_yulius_toml_branch_policy(&text) {
                Ok(policy) => return (policy, None),
                Err(err) => err.to_string(),
            },
            Err(err) => format!(
                "yulius.toml at `{rev}` can't be read: {}",
                err.public_message()
            ),
        };
        tracing::warn!(
            sandbox = %sandbox.slug,
            problem = %problem,
            "the branch policy can't be read; the standard preset applies"
        );
        (BranchPolicy::standard(), Some(problem))
    }

    /// Refuses pull request `number` when the project's branch policy does not accept it, with
    /// the rule in words and how to fix it (SPEC-069 BPL-004).
    pub(crate) async fn dlv_check_policy(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        number: u64,
        pull: &PullFacts<'_>,
    ) -> AppResult<()> {
        let (policy, problem) = self.dlv_branch_policy(sandbox, profile).await;
        policy
            .check(pull, &profile.integration_branch, &profile.production_branch)
            .map_err(|violation| {
                let fix = if violation.is_title() {
                    format!(
                        "edit the title on GitHub (`gh pr edit {number} --title \"…\"`), then merge again"
                    )
                } else {
                    "deliver the work from a branch and into a base the policy accepts".to_owned()
                };
                // A policy that can't be read is the standard one (fail closed); say so.
                let fallback = problem.map_or_else(String::new, |problem| {
                    format!(" (the standard rules apply because the project's policy can't be used: {problem})")
                });
                AppError::Conflict(format!(
                    "the branch policy refuses pull request #{number}: {violation}{fallback}. To fix it, {fix}"
                ))
            })
    }

    /// Before Yulius merges `task`'s pull request: when it still carries a title Yulius gave it,
    /// the default title of its final commits replaces it on GitHub, so the squash commit names
    /// the change (SPEC-069 BPL-008). Yulius opens a pull request as soon as the task is ready,
    /// often after the spec commit alone. Returns the new title; `None` when the title stays (a
    /// title a person or an agent wrote, or the commits cannot be read: then as before).
    ///
    /// # Errors
    /// Nothing is merged under a title Yulius knows is wrong: [`MergeFailure::Later`] when
    /// GitHub could not take the new title for now, [`MergeFailure::Refused`] (a conflict that
    /// says why and how to set a title) when GitHub refused it.
    async fn dlv_merge_title(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        task: &TaskRecord,
        pull: &GhPull,
        token: &Secret,
    ) -> Result<Option<String>, MergeFailure> {
        let (Some(worktree), Some(branch)) =
            (task.worktree_path.as_deref(), task.branch.as_deref())
        else {
            return Ok(None);
        };
        let base = format!("origin/{}", profile.integration_branch);
        let subjects: Vec<String> = match self
            .deps
            .git
            .review_commits(Path::new(worktree), &base, &pull.head_sha, 50)
            .await
        {
            Ok(commits) => commits.into_iter().map(|c| c.subject).collect(),
            Err(err) => {
                tracing::warn!(
                    task = %task.id,
                    error = %err,
                    "the commits of a pull request cannot be read; its title stays"
                );
                return Ok(None);
            }
        };
        let (policy, _) = self.dlv_branch_policy(sandbox, profile).await;
        let Some(title) = merge_title(&pull.title, &task.title, &subjects, branch, &policy) else {
            return Ok(None);
        };
        self.deps
            .github
            .update_pull_title(
                token,
                &sandbox.repo_owner,
                &sandbox.repo_name,
                pull.number,
                &title,
            )
            .await
            .map_err(|err| {
                let number = pull.number;
                let reason = err.public_message();
                if may_pass(&err) {
                    tracing::warn!(
                        task = %task.id,
                        error = %err,
                        "GitHub could not take a pull request's title for now; the merge waits"
                    );
                    MergeFailure::Later(AppError::Upstream(format!(
                        "GitHub could not take the title `{title}` for pull request #{number} \
                         before the merge ({reason}); nothing was merged, and Yulius tries again"
                    )))
                } else {
                    MergeFailure::Refused(AppError::Conflict(format!(
                        "GitHub refused the title `{title}` Yulius gives pull request #{number} \
                         before merging it ({reason}). Give the pull request a title GitHub \
                         accepts yourself (`gh pr edit {number} --title \"<type>(<scope>): <the \
                         change>\"`); Yulius keeps a title you write"
                    )))
                }
            })?;
        self.dlv_audit(
            "yulius",
            "delivery.pr.retitle",
            &format!("{}#{}", sandbox.slug, task.number),
            &format!(
                "pull request #{}: `{}` -> `{title}` (its commits' most significant type)",
                pull.number, pull.title
            ),
        )
        .await;
        Ok(Some(title))
    }

    /// Merges `task`'s pull request with `token` (see [`App::merge_task_pr`]).
    async fn dlv_merge(
        &self,
        ctx: &AuthContext,
        task: &TaskRecord,
        token: &Secret,
    ) -> Result<MergedPull, MergeFailure> {
        crate::ports::github::for_purpose(
            crate::ports::github::GitHubPurpose::Merges,
            self.dlv_merge_inner(ctx, task, token),
        )
        .await
    }

    async fn dlv_merge_inner(
        &self,
        ctx: &AuthContext,
        task: &TaskRecord,
        token: &Secret,
    ) -> Result<MergedPull, MergeFailure> {
        let sandbox = self.dlv_sandbox_of(task).await?;
        let profile = delivery_profile(&sandbox);
        let observed = self
            .dlv_observe(
                &sandbox,
                task,
                &profile,
                ObserveAccess::Provided(Some(token)),
            )
            .await?;
        dlv::apply(
            &observed.record.stage,
            &DeliveryAction::Merge,
            Initiator::User,
            &profile,
            &observed.facts(),
        )
        .map_err(|err| refusal(&err))?;
        let mut pull = observed
            .pr
            .clone()
            .ok_or_else(|| AppError::Conflict("the pull request cannot be read".into()))?;
        if let Some(title) = self
            .dlv_merge_title(&sandbox, &profile, task, &pull, token)
            .await?
        {
            pull.title = title;
        }
        self.dlv_check_policy(&sandbox, &profile, pull.number, &pull_facts(&pull))
            .await?;
        // GitHub's own refusal ("required status checks are expected") reads like a missing
        // permission; say what it means and what to do (F 2026-10-11).
        if pull.behind {
            return Err(AppError::Conflict(format!(
                "the branch is behind `{base}` and the repository requires it to be up to date: \
                 merge `origin/{base}` into it, push, wait for the checks on the new head, then \
                 merge that head",
                base = pull.base
            ))
            .into());
        }
        let merge_sha = self
            .deps
            .github
            .merge_pull(
                token,
                &sandbox.repo_owner,
                &sandbox.repo_name,
                pull.number,
                profile.merge.as_str(),
                &pull.head_sha,
            )
            .await?;
        // GitHub guards the head in the merge request, but the PR can be retargeted.
        // Bind evidence only when the closed PR confirms where this merge actually landed.
        let closed = self
            .deps
            .github
            .get_pull(token, &sandbox.repo_owner, &sandbox.repo_name, pull.number)
            .await
            .ok()
            .filter(|p| p.number == pull.number && p.merged);
        if let Some(closed) = &closed
            && let Err(error) = self
                .deps
                .store
                .remember_pull_observation(&task.id, closed)
                .await
        {
            tracing::warn!(%error, task = %task.id, "could not retain the merged pull for display");
        }
        let landing = closed.as_ref().and_then(|merged| {
            evidence::Landing::confirmed_merge(
                merged,
                &profile.integration_branch,
                pull.number,
                &pull.head_sha,
                &merge_sha,
            )
        });
        let commits = crate::agent_wait::merge_commits(
            landing.as_ref().map(|l| l.branch.as_str()),
            Some(&merge_sha),
            &pull.head_sha,
        );
        let receipt = format!("Yulius merged pull request #{}{commits}", pull.number);
        let landing = landing.or_else(|| {
            let mut expected =
                evidence::Landing::new(&profile.integration_branch, &pull.head_sha, &merge_sha)?;
            expected.source = evidence::LandingSource::AwaitingConfirmation;
            Some(expected)
        });
        let next = DeliveryRecord {
            task_id: task.id.clone(),
            stage: DeliveryStage::MergedIntegration,
            pr_number: Some(pull.number),
            updated_at: self.now(),
        };
        self.dlv_transition_at(
            &task.sandbox_id,
            &observed.record.stage,
            &next,
            None,
            Some(&crate::review::MergedHead {
                head: pull.head_sha.clone(),
                base: Some(format!("{merge_sha}^1")),
                landing,
            }),
        )
        .await?;
        let actor = Self::dlv_github_actor(&ctx.user);
        self.dlv_audit(
            &actor,
            "delivery.merge",
            &format!("{}#{}", sandbox.slug, task.number),
            &format!(
                "pull request #{} merged into {} ({}) at head {} -> {}",
                pull.number,
                profile.integration_branch,
                profile.merge.as_str(),
                pull.head_sha,
                merge_sha
            ),
        )
        .await;
        let fetched = match self.dlv_refresh_clone(&sandbox).await {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(error = %err, "refreshing the sandbox clone after a merge failed");
                false
            }
        };
        let mut notes = Vec::new();
        // Hygiene (DLV-007): the merged branch goes, unless protected or still in use.
        if let Some(note) = self.delete_merged_branch(&sandbox, &pull).await {
            notes.push(note);
        }
        if let Some(note) = self
            .dlv_auto_promote_if_enabled(&sandbox, &profile, &ctx.user, token, fetched)
            .await
        {
            notes.push(note);
        }
        self.publish_task(&task.id).await;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let mut view = self.delivery_view(&task, Some(&ctx.user)).await?;
        view.notes.extend(notes);
        Ok(MergedPull {
            number: pull.number,
            head: pull.head_sha,
            view,
            receipt,
        })
    }
}
