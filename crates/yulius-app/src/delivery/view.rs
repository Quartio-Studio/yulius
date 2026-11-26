//! Gathering a task's delivery facts and the delivery and Git page reads.

use super::*;

impl App {
    pub(crate) async fn display_terminal(
        &self,
        task: &TaskRecord,
        record: Option<&DeliveryRecord>,
    ) -> AppResult<Option<GhPull>> {
        let number = record.and_then(|r| r.pr_number).or_else(|| {
            self.observed_pulls
                .known_pull(&task.sandbox_id, &task.id)
                .flatten()
                .filter(|pull| pull.open)
                .map(|pull| pull.number)
        });
        self.deps.store.terminal_pull(&task.id, number).await
    }

    /// Gathers the facts of one task and reconciles its stage with GitHub (a pull request
    /// opened, merged or closed outside Yulius).
    pub(super) async fn dlv_observe(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        profile: &DeliveryProfile,
        access: ObserveAccess<'_>,
    ) -> AppResult<Observed> {
        let store = &self.deps.store;
        let record = store
            .delivery(&task.id)
            .await?
            .unwrap_or_else(|| DeliveryRecord::initial(&task.id));
        let spec = self.dlv_check_spec(task).await;
        let steps = store.steps(&task.id).await?;
        let steps_open = steps
            .iter()
            .filter(|s| !StepStatus::parse(&s.status).is_some_and(StepStatus::is_resolved))
            .count();
        let steps_finished = steps.len() - steps_open;
        let mut notes = Vec::new();
        let pre_merge = matches!(record.stage, DeliveryStage::None | DeliveryStage::PrOpen);
        let mut worktree_head = None;
        let mut dirty = false;
        let mut dirty_known = true;
        if let Some(worktree) = task.worktree_path.as_deref().map(Path::new)
            && pre_merge
        {
            match self.deps.git.log(worktree, "HEAD", 1).await {
                Ok(commits) => worktree_head = commits.into_iter().next().map(|c| c.sha),
                Err(err) => notes.push(format!(
                    "Git: the task branch head cannot be read: {}",
                    err.public_message()
                )),
            }
            match self.deps.git.has_changes(worktree).await {
                Ok(changed) => dirty = changed,
                Err(_) => {
                    dirty = false;
                    dirty_known = false;
                }
            }
        }
        let terminal = if matches!(access, ObserveAccess::Display(_))
            && crate::ports::github::current_purpose()
                == crate::ports::github::GitHubPurpose::Display
        {
            self.display_terminal(task, Some(&record)).await?
        } else {
            None
        };
        let display_token;
        let token = match access {
            ObserveAccess::Display(_) if terminal.is_some() => None,
            ObserveAccess::Display(user) => {
                display_token = match self.dlv_read_token(sandbox, user).await {
                    Ok(token) => Some(token),
                    Err(note) => {
                        notes.push(note);
                        None
                    }
                };
                display_token.as_ref()
            }
            ObserveAccess::Provided(token) => token,
        };
        let mut pr = terminal.filter(|_| record.pr_number.is_some());
        // Only an answer from GitHub may change the recorded stage; failures never do.
        let mut pr_known = record.stage == DeliveryStage::PrOpen && pr.is_some();
        let mut checks = ChecksState::None;
        let mut checks_unreadable = Vec::new();
        let mut checks_errored = false;
        if let Some(token) = token {
            let github = &self.deps.github;
            let (owner, repo) = (&sandbox.repo_owner, &sandbox.repo_name);
            if let Some(number) = record.pr_number {
                match github.get_pull(token, owner, repo, number).await {
                    Ok(pull) if pull.number == number => {
                        if !is_open(&pull) {
                            store.remember_pull_observation(&task.id, &pull).await?;
                        }
                        pr = Some(pull);
                        pr_known = true;
                    }
                    Ok(pull) => notes.push(format!(
                        "GitHub returned pull request #{} for the requested #{number}.",
                        pull.number
                    )),
                    Err(err) => notes.push(format!(
                        "Pull request #{number} can't be read: {}",
                        github_reason(&err)
                    )),
                }
            } else if let (Some(branch), DeliveryStage::None) = (&task.branch, &record.stage) {
                // Filtered here rather than through the adapter's `head` parameter, whose
                // GitHub form is `owner:branch`.
                match github.list_pulls(token, owner, repo, None).await {
                    Ok(pulls) => {
                        pr_known = true;
                        pr = pulls.into_iter().find(|p| {
                            p.head == *branch && p.base == profile.integration_branch && is_open(p)
                        });
                    }
                    Err(err) => notes.push(format!(
                        "Pull requests can't be listed: {}",
                        github_reason(&err)
                    )),
                }
            }
            if let Some(pull) = pr.as_ref().filter(|p| is_open(p)) {
                let gate = self
                    .ci_gate_for_pull(sandbox, pull.number, &pull.head_sha)
                    .await;
                match self
                    .checks_with_gate(sandbox, token, &pull.head_sha, gate)
                    .await
                {
                    Ok(at) => {
                        checks_errored = at.word == "error";
                        checks = at.state;
                        checks_unreadable = at.unreadable;
                    }
                    Err(err) => {
                        // Unknown checks never allow a merge that requires them.
                        checks = ChecksState::Unknown;
                        notes.push(format!("Checks can't be read: {}", github_reason(&err)));
                    }
                }
            }
        }
        let mut observed = Observed {
            record,
            spec,
            steps,
            steps_open,
            steps_finished,
            pr,
            checks,
            checks_unreadable,
            checks_errored,
            worktree_head,
            dirty,
            dirty_known,
            notes,
            integration: profile.integration_branch.clone(),
        };
        if !pr_known {
            return Ok(observed);
        }
        let next = dlv::reconcile(&observed.record.stage, &observed.facts());
        let found_number = observed.pr.as_ref().map(|p| p.number);
        if next != observed.record.stage
            || (found_number.is_some() && observed.record.pr_number.is_none())
        {
            let reconciled = DeliveryRecord {
                task_id: task.id.clone(),
                pr_number: if next == DeliveryStage::None {
                    None
                } else {
                    found_number.or(observed.record.pr_number)
                },
                stage: next,
                updated_at: self.now(),
            };
            // Merged elsewhere: its merge commit's first parent is the integration branch
            // before it, as for a merge of Yulius's own; without it, only an approval of the
            // merged head itself counts.
            let merged_head = observed
                .pr
                .as_ref()
                .filter(|pull| pull.merged)
                .map(crate::review::MergedHead::of_pull);
            if self
                .dlv_transition_at(
                    &task.sandbox_id,
                    &observed.record.stage,
                    &reconciled,
                    // If this transition adopts an open PR, capture its intended base now.
                    Some(&profile.integration_branch),
                    merged_head.as_ref(),
                )
                .await?
            {
                if reconciled.stage != observed.record.stage {
                    self.dlv_audit(
                        "yulius",
                        "delivery.reconcile",
                        &format!("{}#{}", sandbox.slug, task.number),
                        &format!(
                            "{} -> {} (GitHub state)",
                            observed.record.stage, reconciled.stage
                        ),
                    )
                    .await;
                }
                if reconciled.stage == DeliveryStage::None {
                    observed.pr = None;
                }
                observed.record = reconciled;
            }
        }
        Ok(observed)
    }

    async fn dlv_delivery_of(
        &self,
        playground: Option<(&SandboxRecord, &TaskRecord)>,
        profile: &DeliveryProfile,
        observed: &Observed,
        mut notes: Vec<String>,
        checks_access: Option<ChecksAccess>,
    ) -> Delivery {
        let facts = observed.facts();
        let stage = &observed.record.stage;
        let (next_actions, blocked) = if let Some((sandbox, task)) = playground {
            if past_merge(stage) {
                (Vec::new(), Vec::new())
            } else {
                let hold = self
                    .playground_view_hold(sandbox, task, observed, profile)
                    .await
                    .unwrap_or_else(|error| Some(error.public_message()));
                match hold {
                    Some(reason) => (
                        Vec::new(),
                        vec![BlockedAction {
                            action: NextAction::Merge.wire(),
                            reason,
                        }],
                    ),
                    None => (vec![NextAction::Merge.wire()], Vec::new()),
                }
            }
        } else {
            let next_actions = dlv::next_actions(stage, profile, &facts)
                .iter()
                .map(NextAction::wire)
                .collect();
            let blocked = dlv::blocked_actions(stage, profile, &facts)
                .into_iter()
                .map(|(action, err)| BlockedAction {
                    action: action.wire(),
                    reason: err.to_string(),
                })
                .collect();
            (next_actions, blocked)
        };
        if observed.dirty && *stage == DeliveryStage::None {
            notes.push(format!("Git: {DIRTY_WORKTREE}."));
        }
        notes.extend(observed.notes.iter().cloned());
        let checks = if !observed.pr.as_ref().is_some_and(is_open) {
            "none"
        } else if observed.checks_errored {
            "error"
        } else {
            observed.checks.as_str()
        };
        Delivery {
            stage: stage.wire_name().to_owned(),
            environment: match stage {
                DeliveryStage::EnvDone(name) => Some(name.clone()),
                _ => None,
            },
            pr: observed.pr.as_ref().map(|p| pull_view(p, checks)),
            spec_path: observed.spec.path.clone(),
            spec_valid: observed.spec.valid,
            spec_problems: observed.spec.problems.clone(),
            steps_open: u32::try_from(observed.steps_open).unwrap_or(u32::MAX),
            next_actions,
            blocked,
            notes,
            checks_access,
            review_rounds: Vec::new(),
            evidence: None,
        }
    }

    /// The GitHub App's permission settings page, for instance administrators (#111): where a
    /// missing check permission is added. `None` for everyone else and when the App or its
    /// owner is not known.
    async fn dlv_app_settings_url(&self, user: Option<&UserRecord>) -> Option<String> {
        if !user.is_some_and(|u| u.is_superadmin || u.is_instance_admin) {
            return None;
        }
        let app = self.github_app().await.ok()?;
        let instance = self.deps.store.instance().await.ok()??;
        let owner = instance.owner_login?;
        let is_org = match instance.owner_kind.as_deref() {
            Some("org") => true,
            Some("user") => false,
            _ => return None,
        };
        crate::creation_refusals::app_pages(&owner, is_org, &app.slug, None)
            .map(|pages| pages.permissions)
    }

    /// [`checks_access`] for the sandbox's repository, with the settings link for `user`.
    async fn dlv_checks_access(
        &self,
        sandbox: &SandboxRecord,
        unreadable: &[gh::ChecksSource],
        user: Option<&UserRecord>,
    ) -> Option<ChecksAccess> {
        if unreadable.is_empty() {
            return None;
        }
        let repo = format!("{}/{}", sandbox.repo_owner, sandbox.repo_name);
        checks_access(&repo, unreadable, self.dlv_app_settings_url(user).await)
    }

    // ---------------------------------------------------------------- reads

    /// Delivery state of a task for `user` (or for an agent with `None`, using the App's
    /// installation token for GitHub reads). Reconciles with GitHub; GitHub and Git failures
    /// become notes.
    ///
    /// # Errors
    /// Store failures and a missing sandbox.
    pub async fn delivery_view(
        &self,
        task: &TaskRecord,
        user: Option<&UserRecord>,
    ) -> AppResult<Delivery> {
        crate::ports::github::for_purpose(
            crate::ports::github::GitHubPurpose::Display,
            self.delivery_view_inner(task, user),
        )
        .await
    }

    async fn delivery_view_inner(
        &self,
        task: &TaskRecord,
        user: Option<&UserRecord>,
    ) -> AppResult<Delivery> {
        let sandbox = self.dlv_sandbox_of(task).await?;
        let profile = delivery_profile(&sandbox);
        let mut notes = Vec::new();
        let observed = self
            .dlv_observe(&sandbox, task, &profile, ObserveAccess::Display(user))
            .await?;
        // Where an armed merge stands, and what a merge counts as (SPEC-441 MQ-007, MQ-009).
        if let Some(note) = self.merge_queue_note(task) {
            notes.push(note.text);
        }
        // Surface an actionable push/publication failure for the current unpublished head
        // (SPEC-553 REP-004).
        if let Some(head) = observed.worktree_head.as_deref()
            && !observed
                .pr
                .as_ref()
                .is_some_and(|pull| is_open(pull) && pull.head_sha.eq_ignore_ascii_case(head))
            && let Some(note) = self.actionable_push_problem_of(&task.id, head)
        {
            notes.push(note);
        }
        if let Some(pull) = observed.pr.as_ref().filter(|pull| pull.merged) {
            notes.push(crate::agent_wait::merged_text(
                pull.number,
                &pull.base,
                &pull.head_sha,
                pull.merge_commit_sha.as_deref(),
                self.ci_gate_for_pull(&sandbox, pull.number, &pull.head_sha)
                    .await,
                false,
                "",
            ));
        }
        // Checks are read only for an open pull request, so this is empty otherwise.
        let access = self
            .dlv_checks_access(&sandbox, &observed.checks_unreadable, user)
            .await;
        let mut view = self
            .dlv_delivery_of(
                sandbox.is_playground().then_some((&sandbox, task)),
                &profile,
                &observed,
                notes,
                access,
            )
            .await;
        // `pull_view` leaves `conflicts = false` so callers decide per task via `pull_conflicts`
        // (SPEC-491, SPEC-580 AWM-003). Parked tasks use the bounded display Git pool and deadline
        // so overlapping detail reads cannot hang or exceed `PARKED_READS` when local Git stalls.
        if let Some(pull) = observed.pr.as_ref().filter(|p| is_open(p))
            && let Some(pr) = view.pr.as_mut()
        {
            pr.conflicts = if task.status == yulius_domain::task::TaskStatus::Parked {
                let (task, sandbox, pull_owned) = (task.clone(), sandbox.clone(), pull.clone());
                tokio::time::timeout_at(
                    self.status_deadline(),
                    self.verification_git(PrVerification::Display, move |app| async move {
                        Ok(app.pull_conflicts(&task, &sandbox, &pull_owned).await)
                    }),
                )
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or(pull.mergeable == Some(false))
            } else {
                self.pull_conflicts(task, &sandbox, pull).await
            };
        }
        if past_merge(&observed.record.stage) {
            let known = observed
                .pr
                .as_ref()
                .filter(|p| Some(p.number) == observed.record.pr_number)
                .and_then(evidence::Landing::closed_pull);
            if let Some(sha) = self
                .deps
                .store
                .landed_sha(&task.id, observed.record.pr_number, known.as_ref())
                .await?
            {
                view.evidence = self
                    .deps
                    .store
                    .merge_evidence(&task.id, &sha, None, None)
                    .await?;
            } else {
                view.notes.push(
                    "Post-merge evidence is unavailable because the landed commit is unknown."
                        .into(),
                );
            }
        }
        view.review_rounds = self.review_round_views(&observed.record).await;
        Ok(view)
    }

    /// The `TaskDetail.delivery` value: the full view, or the stored stage with a note when
    /// even that cannot be computed.
    pub async fn task_detail_delivery(&self, task: &TaskRecord, user: &UserRecord) -> Delivery {
        match self.delivery_view(task, Some(user)).await {
            Ok(view) => view,
            Err(err) => Delivery {
                stage: "none".into(),
                notes: vec![format!(
                    "The delivery state is unavailable: {}",
                    err.public_message()
                )],
                ..Delivery::default()
            },
        }
    }

    /// `GET /api/tasks/{tid}/delivery`.
    ///
    /// # Errors
    /// `NotFound` without read access.
    pub async fn task_delivery(&self, ctx: &AuthContext, task_id: &str) -> AppResult<Delivery> {
        let task = self.load_task(ctx, task_id, Action::View).await?;
        self.delivery_view(&task, Some(&ctx.user)).await
    }

    /// `GET /api/sandboxes/{sid}/git`: branches, per-task branch facts, recent integration
    /// commits and open pull requests with checks (GIT-001). Degrades to partial data with
    /// notes when Git or GitHub fail.
    ///
    /// # Errors
    /// `NotFound` without read access; store failures.
    pub async fn git_overview(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
    ) -> AppResult<GitOverview> {
        crate::ports::github::for_purpose(
            crate::ports::github::GitHubPurpose::Display,
            self.git_overview_inner(ctx, sandbox_id),
        )
        .await
    }

    async fn git_overview_inner(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
    ) -> AppResult<GitOverview> {
        let sandbox = self
            .dlv_require_sandbox(ctx, sandbox_id, Action::View)
            .await?;
        let observation = self.observed_pulls.begin(&sandbox.id)?;
        let profile = delivery_profile(&sandbox);
        let (owner, name) = (&sandbox.repo_owner, &sandbox.repo_name);
        let repo = self.sandbox_repo(&sandbox.id);
        let cloned = repo.join(".git").exists();
        let integration_ref = format!("origin/{}", profile.integration_branch);
        let mut notes = Vec::new();
        let token = match self.dlv_read_token(&sandbox, Some(&ctx.user)).await {
            Ok(token) => Some(token),
            Err(note) => {
                notes.push(note);
                None
            }
        };
        let mut open_pulls = Vec::new();
        let mut overview_complete = false;
        let mut checks_by_pr = Vec::new();
        let mut partial_checks = std::collections::HashSet::new();
        let mut checks_unreadable = Vec::new();
        if let Some(token) = &token {
            match self.deps.github.list_pulls(token, owner, name, None).await {
                Ok(pulls) => {
                    let mut open = pulls.into_iter().filter(is_open);
                    open_pulls = open.by_ref().take(MAX_PULLS).collect();
                    overview_complete = open.next().is_none();
                }
                Err(err) => notes.push(format!(
                    "Pull requests can't be listed: {}",
                    github_reason(&err)
                )),
            }
            // A pull request Yulius recorded as merged is merged, even while GitHub's list, read
            // just after the merge, still says open (SPEC-108 PRL-007): it reads as merged below.
            if !open_pulls.is_empty() {
                let merged: Vec<u64> = self
                    .deps
                    .store
                    .deliveries(&sandbox.id)
                    .await?
                    .into_iter()
                    .filter(|record| past_merge(&record.stage))
                    .filter_map(|record| record.pr_number)
                    .collect();
                open_pulls.retain(|pull| !merged.contains(&pull.number));
            }
            let mut checks_failed = false;
            for pull in &open_pulls {
                let state = match self.dlv_checks_at(&sandbox, token, &pull.head_sha).await {
                    Ok(at) => {
                        // A missing permission is said once for the repository, not per PR.
                        merge_sources(&mut checks_unreadable, &at.unreadable);
                        if !at.unreadable.is_empty() && at.word != "failure" {
                            partial_checks.insert(pull.number);
                        }
                        at.word
                    }
                    Err(err) => {
                        if !checks_failed {
                            checks_failed = true;
                            notes.push(format!("Checks can't be read: {}", github_reason(&err)));
                        }
                        "unknown"
                    }
                };
                checks_by_pr.push((pull.number, state));
            }
        }
        let checks_access = self
            .dlv_checks_access(&sandbox, &checks_unreadable, Some(&ctx.user))
            .await;
        let checks_of = |number: u64| {
            let state = checks_by_pr
                .iter()
                .find(|(n, _)| *n == number)
                .map_or("unknown", |(_, state)| *state);
            tested_checks_word(&profile, state)
        };
        let mut pulls: Vec<PullView> = open_pulls
            .iter()
            .map(|p| pull_view(p, checks_of(p.number)))
            .collect();

        if !cloned {
            notes.push("Git: the sandbox repository is not cloned yet.".into());
        }
        let mut tasks = Vec::new();
        let mut observed = std::collections::HashMap::new();
        let mut unavailable = std::collections::HashMap::new();
        let mut git_failed = false;
        for task in self.deps.store.tasks(&sandbox.id, false).await? {
            let Some(branch) = task.branch.clone() else {
                continue;
            };
            let listed = open_pulls.iter().find(|p| !p.from_fork && p.head == branch);
            let mut head = listed.map(|p| Arc::from(p.head_sha.as_str()));
            let mut unread_recorded = None;
            let mut view = TaskBranch {
                task_id: task.id.to_string(),
                title: task.title.clone(),
                branch: branch.clone(),
                ahead: 0,
                behind: 0,
                files_changed: 0,
                insertions: 0,
                deletions: 0,
                pr: listed.map(|p| pull_view(p, checks_of(p.number))),
                worktree: worktree_state(&task),
            };
            // A branch list may lag a closure. Before invalidating a closed-unmerged
            // snapshot on a positive overview, confirm the same PR itself reopened.
            if let Some(token) = &token
                && let Some(listed) = open_pulls.iter().find(|p| !p.from_fork && p.head == branch)
                && self
                    .deps
                    .store
                    .terminal_pull(&task.id, Some(listed.number))
                    .await?
                    .is_some_and(|prior| !prior.merged)
                && let Ok(direct) = self
                    .deps
                    .github
                    .confirm_pull(token, owner, name, listed.number)
                    .await
                && direct.number == listed.number
                && is_open(&direct)
                && !direct.from_fork
                && direct.head == branch
            {
                self.bind_observed_open_pull(&sandbox, &task.id, &direct)
                    .await?;
            }
            // GitHub's list says nothing about mergeability: Yulius's own clone does (DON-001).
            if let Some(pull) = listed
                && self.pull_conflicts(&task, &sandbox, pull).await
            {
                if let Some(pr) = view.pr.as_mut() {
                    pr.conflicts = true;
                }
                if let Some(listed) = pulls.iter_mut().find(|p| p.number == pull.number) {
                    listed.conflicts = true;
                }
            }
            if cloned
                && self
                    .deps
                    .git
                    .branch_exists(&repo, &branch)
                    .await
                    .unwrap_or(false)
            {
                let git = &self.deps.git;
                match (
                    git.ahead_behind(&repo, &integration_ref, &branch).await,
                    git.diff_stat(&repo, &integration_ref, &branch).await,
                ) {
                    (Ok((ahead, behind)), Ok(stat)) => {
                        view.ahead = ahead;
                        view.behind = behind;
                        view.files_changed = stat.files;
                        view.insertions = stat.insertions;
                        view.deletions = stat.deletions;
                    }
                    (Err(err), _) | (_, Err(err)) => {
                        if !git_failed {
                            git_failed = true;
                            notes.push(format!(
                                "Git: branch statistics are incomplete: {}",
                                err.public_message()
                            ));
                        }
                    }
                }
            }
            if view.pr.is_none()
                && let Some(token) = &token
                && let Ok(Some(record)) = self.deps.store.delivery(&task.id).await
                && record.stage == DeliveryStage::PrOpen
                && let Some(number) = record.pr_number
                && let Some(pull) =
                    match self.deps.github.get_pull(token, owner, name, number).await {
                        Ok(pull) if pull.number == number => Some(pull),
                        _ => {
                            unread_recorded = Some(number);
                            None
                        }
                    }
            {
                if !is_open(&pull) {
                    self.deps
                        .store
                        .remember_pull_observation(&task.id, &pull)
                        .await?;
                }
                head = Some(Arc::from(pull.head_sha.as_str()));
                // Merged or closed outside Yulius: show it and reconcile the stage. An open one
                // missing from the listed page has unread checks (#68).
                view.pr = Some(pull_view(
                    &pull,
                    if is_open(&pull) { "unknown" } else { "none" },
                ));
                if is_open(&pull)
                    && self.pull_conflicts(&task, &sandbox, &pull).await
                    && let Some(pr) = view.pr.as_mut()
                {
                    pr.conflicts = true;
                }
                let observed_state = if pull.merged {
                    PrState::Merged
                } else if is_open(&pull) {
                    PrState::Open
                } else {
                    PrState::Closed
                };
                let facts = DeliveryFacts {
                    pr_state: observed_state,
                    ..merged_facts(false)
                };
                let next = dlv::reconcile(&record.stage, &facts);
                if next != record.stage {
                    let reconciled = DeliveryRecord {
                        pr_number: (next != DeliveryStage::None).then_some(number),
                        stage: next,
                        updated_at: self.now(),
                        ..record.clone()
                    };
                    let merged_head = pull
                        .merged
                        .then(|| crate::review::MergedHead::of_pull(&pull));
                    let _ = self
                        .dlv_transition_at(
                            &sandbox.id,
                            &record.stage,
                            &reconciled,
                            None,
                            merged_head.as_ref(),
                        )
                        .await;
                }
            }
            if view.pr.is_none()
                && let Ok(Some(record)) = self.deps.store.delivery(&task.id).await
                && past_merge(&record.stage)
                && let Some(number) = record.pr_number
            {
                // Merged earlier: no longer listed as open, but still this task's pull request,
                // so the task stays "Done · merged" (#68). No GitHub call.
                view.pr = Some(PullView {
                    number,
                    url: format!("https://github.com/{owner}/{name}/pull/{number}"),
                    title: task.title.clone(),
                    state: "merged".into(),
                    base: profile.integration_branch.clone(),
                    head: branch.clone(),
                    checks: "none".into(),
                    conflicts: false,
                });
            }
            // Publish only facts from this request. A displayed fallback must still be
            // resolved in request order when another overview is in flight.
            if let Some(pr) = view
                .pr
                .as_mut()
                .filter(|pr| partial_checks.contains(&pr.number))
            {
                pr.checks = "unknown".into();
            }
            let fresh = view.pr.as_ref().map(|pr| crate::sandboxes::ObservedPull {
                head,
                open: pr.state == "open",
                conflicts: pr.conflicts,
                number: pr.number,
                signal: PullSignal::from_wire(&pr.state, &pr.checks, pr.conflicts),
            });
            let gap = unread_recorded.or_else(|| {
                view.pr
                    .as_ref()
                    .filter(|pr| pr.checks == "unknown")
                    .map(|pr| pr.number)
            });
            if let Some(number) = gap {
                unavailable.insert(task.id.clone(), number);
            }
            let mut display = crate::sandboxes::ObservedPull::fallback(
                self.observed_pulls
                    .known_pull(&sandbox.id, &task.id)
                    .flatten(),
                fresh.as_ref(),
                overview_complete,
                gap,
            );
            let unchecked = fresh.as_ref().filter(|pr| gap == Some(pr.number));
            if let Some(pr) = display.as_ref().or(unchecked).filter(|pr| pr.open)
                && let Some(sha) = pr.head.as_deref()
                && let Some(signal) = self.parking_ci_signal(&task, pr.number, sha, None).await?
            {
                display = Some(crate::sandboxes::ObservedPull {
                    signal: Some(signal),
                    ..pr.clone()
                });
            }
            if let Some(known) = display {
                let pr = view.pr.get_or_insert_with(|| PullView {
                    number: known.number,
                    url: String::new(),
                    title: task.title.clone(),
                    base: profile.integration_branch.clone(),
                    head: branch.clone(),
                    state: String::new(),
                    checks: String::new(),
                    conflicts: false,
                });
                pr.number = known.number;
                pr.url = format!("https://github.com/{owner}/{name}/pull/{}", pr.number);
                let (state, checks) = if known.open {
                    (
                        "open",
                        match known.signal {
                            Some(PullSignal::Tested | PullSignal::Conflicting) => "success",
                            Some(PullSignal::Errored) => "error",
                            Some(PullSignal::Failing) => "failure",
                            Some(PullSignal::Behind | PullSignal::InReview) => "pending",
                            _ => "unknown",
                        },
                    )
                } else {
                    (
                        if known.signal == Some(PullSignal::Merged) {
                            "merged"
                        } else {
                            "closed"
                        },
                        "none",
                    )
                };
                pr.state = state.into();
                pr.checks = checks.into();
                pr.conflicts |= known.conflicts;
            }
            observed.insert(task.id.clone(), fresh);
            tasks.push(view);
        }

        let mut recent_commits = Vec::new();
        if cloned {
            match self
                .deps
                .git
                .log(&repo, &integration_ref, RECENT_COMMITS)
                .await
            {
                Ok(commits) => {
                    recent_commits = commits
                        .into_iter()
                        .map(|c| CommitView {
                            sha: c.sha,
                            subject: c.subject,
                            author: c.author,
                            at: rfc3339(Timestamp(c.at_ms)),
                        })
                        .collect();
                }
                Err(err) => notes.push(format!(
                    "Git: recent commits of {} cannot be read: {}",
                    profile.integration_branch,
                    err.public_message()
                )),
            }
        }
        // Only a completed overview publishes. Cancellation during the final Git
        // log drops the guard and frees its slot for the next request.
        observation.record_overview(overview_complete, observed, unavailable);
        Ok(GitOverview {
            integration_branch: profile.integration_branch.clone(),
            production_branch: profile.production_branch.clone(),
            default_branch: sandbox.default_branch.clone(),
            tasks,
            recent_commits,
            pulls,
            notes,
            checks_access,
        })
    }
}
