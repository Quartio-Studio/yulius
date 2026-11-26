//! Opening and pushing pull requests, by a person and automatically, push problems, and the
//! task review's commits and changes.

use super::*;

impl App {
    // ---------------------------------------------------------------- pull request

    /// `POST /api/tasks/{tid}/delivery/pr`: pushes the task branch with an installation token
    /// and opens the pull request into the integration branch with the caller's token (the
    /// superadmin, who has no GitHub identity, opens it through the App, ACC-012).
    /// Refused without a valid spec (DSC-002) or with uncommitted changes.
    ///
    /// # Errors
    /// `Conflict` refusals, `Upstream` Git/GitHub failures.
    pub async fn open_task_pr(
        &self,
        ctx: &AuthContext,
        task_id: &str,
        req: &OpenPr,
    ) -> AppResult<Delivery> {
        let task = self.load_task(ctx, task_id, Action::Work).await?;
        let sandbox = self.dlv_sandbox_of(&task).await?;
        let profile = delivery_profile(&sandbox);
        let branch = task.branch.clone().ok_or_else(|| {
            AppError::Conflict("the task has no branch yet; start it first".into())
        })?;
        if task.worktree_path.is_none() {
            return Err(AppError::Conflict(
                "the task has no worktree yet; start it first".into(),
            ));
        }
        if branch == profile.integration_branch || branch == profile.production_branch {
            return Err(AppError::Invalid(format!(
                "the task branch '{branch}' is a protected delivery branch"
            )));
        }
        let token = self.dlv_author_token(&ctx.user, &sandbox).await?;
        let observed = self
            .dlv_observe(
                &sandbox,
                &task,
                &profile,
                ObserveAccess::Provided(Some(&token)),
            )
            .await?;
        dlv::apply(
            &observed.record.stage,
            &DeliveryAction::OpenPr,
            Initiator::User,
            &profile,
            &observed.facts(),
        )
        .map_err(|err| match err {
            DeliveryError::SpecInvalid => AppError::Conflict(format!(
                "{err} Problems: {}",
                observed.spec.problems.join(" ")
            )),
            other => refusal(&other),
        })?;
        if observed.dirty {
            return Err(AppError::Conflict(DIRTY_WORKTREE.into()));
        }
        if let (Some(branch), Some(worktree)) =
            (task.branch.as_deref(), task.worktree_path.as_deref())
        {
            self.dlv_push_head(
                &task.id,
                Path::new(worktree),
                branch,
                observed.worktree_head.as_deref().unwrap_or_default(),
                &token,
            )
            .await?;
        }
        self.dlv_open_pull(
            &sandbox,
            &task,
            &profile,
            &observed,
            &token,
            req.title.as_deref(),
            req.body.as_deref(),
            &Self::dlv_actor_login(&ctx.user),
        )
        .await?;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.delivery_view(&task, Some(&ctx.user)).await
    }

    /// Opens the task's pull request into the integration branch with `token` (its account
    /// pushes and authors the pull request: the person who delivers, or for Yulius the task's
    /// owner, F 2026-10-11); records `pr_open`, audits it as `actor` and publishes the task. The
    /// caller has checked the domain rules and the worktree and pushed the branch with
    /// [`App::dlv_push_head`] (`open_task_pr` for a person, `try_auto_open_pr` for Yulius).
    #[allow(clippy::too_many_arguments)]
    async fn dlv_open_pull(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        profile: &DeliveryProfile,
        observed: &Observed,
        token: &Secret,
        title: Option<&str>,
        body: Option<&str>,
        actor: &str,
    ) -> AppResult<u64> {
        let (Some(branch), Some(worktree)) = (task.branch.clone(), task.worktree_path.clone())
        else {
            return Err(AppError::Conflict(
                "the task has no branch or worktree yet; start it first".into(),
            ));
        };
        let (policy, _) = self.dlv_branch_policy(sandbox, profile).await;
        let title = match title.map(str::trim).filter(|t| !t.is_empty()) {
            Some(title) => title.to_owned(),
            None => {
                let base = format!("origin/{}", profile.integration_branch);
                let subjects: Vec<String> = self
                    .deps
                    .git
                    .review_commits(Path::new(&worktree), &base, "HEAD", 50)
                    .await
                    .map(|commits| commits.into_iter().map(|c| c.subject).collect())
                    .unwrap_or_default();
                default_pr_title(&task.title, &subjects, &branch, &policy)
            }
        };
        let pull = self
            .deps
            .github
            .create_pull(
                token,
                &sandbox.repo_owner,
                &sandbox.repo_name,
                &NewPull {
                    head: branch.clone(),
                    base: profile.integration_branch.clone(),
                    title: sent_title(&title, max_title_chars(&policy)),
                    body: pull_request_body(
                        &format!(
                            "Delivered with Yulius task #{}: {}/s/{}/t/{}",
                            task.number, self.config.public_url, sandbox.slug, task.number
                        ),
                        &observed.spec,
                        &observed.steps,
                        body,
                    ),
                },
            )
            .await?;
        let next = DeliveryRecord {
            task_id: task.id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(pull.number),
            updated_at: self.now(),
        };
        if !self
            .dlv_transition_at(
                &task.sandbox_id,
                &observed.record.stage,
                &next,
                Some(&profile.integration_branch),
                None,
            )
            .await?
        {
            return Err(AppError::Conflict(
                "the delivery state changed concurrently; refresh".into(),
            ));
        }
        self.dlv_audit(
            actor,
            "delivery.pr.open",
            &format!("{}#{}", sandbox.slug, task.number),
            &format!(
                "pull request #{} {} -> {} at {}",
                pull.number, branch, profile.integration_branch, pull.head_sha
            ),
        )
        .await;
        self.publish_task(&task.id).await;
        Ok(pull.number)
    }

    // ---------------------------------------------------------------- task review

    /// Commits listed on a task's review.
    const REVIEW_COMMITS: u32 = 100;

    /// `GET /api/tasks/{tid}/changes` (read): the task's changes for review (UI-031) — its
    /// worktree, committed and uncommitted work and untracked files, against the merge base with
    /// `origin/<integration>`, bounded.
    ///
    /// # Errors
    /// `NotFound` without access; `Conflict` when the task has no worktree (any more);
    /// `Upstream` Git failures.
    pub async fn task_changes(&self, ctx: &AuthContext, task_id: &str) -> AppResult<TaskChanges> {
        let task = self.load_task(ctx, task_id, Action::View).await?;
        let (worktree, base, integration) = self.review_target(&task).await?;
        let changes = self.deps.git.review_changes(&worktree, &base).await?;
        let (ahead, behind) = self
            .deps
            .git
            .ahead_behind(&worktree, &base, "HEAD")
            .await
            .unwrap_or((0, 0));
        Ok(TaskChanges {
            base: integration,
            head: task.branch.clone().unwrap_or_default(),
            base_sha: changes.base_sha,
            head_sha: changes.head_sha,
            ahead,
            behind,
            uncommitted: changes.uncommitted,
            truncated: changes.truncated,
            files: changes.files,
        })
    }

    /// `GET /api/tasks/{tid}/commits` (read): the task branch's commits not on the integration
    /// branch, newest first, each with its size (UI-031).
    ///
    /// # Errors
    /// As [`App::task_changes`].
    pub async fn task_commits(
        &self,
        ctx: &AuthContext,
        task_id: &str,
    ) -> AppResult<Vec<TaskCommit>> {
        let task = self.load_task(ctx, task_id, Action::View).await?;
        let (worktree, base, _) = self.review_target(&task).await?;
        self.deps
            .git
            .review_commits(&worktree, &base, "HEAD", Self::REVIEW_COMMITS)
            .await
    }

    /// The task's worktree, `origin/<integration>` and the integration branch name.
    async fn review_target(&self, task: &TaskRecord) -> AppResult<(PathBuf, String, String)> {
        let sandbox = self.dlv_sandbox_of(task).await?;
        let integration = delivery_profile(&sandbox).integration_branch;
        match (worktree_state(task), task.worktree_path.as_deref()) {
            (WorktreeState::Present, Some(worktree)) => Ok((
                PathBuf::from(worktree),
                format!("origin/{integration}"),
                integration,
            )),
            (WorktreeState::Gone, _) => Err(AppError::Conflict(
                "the task's worktree is gone; its branch and pull request are on GitHub".into(),
            )),
            _ => Err(AppError::Conflict(
                "the task has not started yet, so it has no worktree to read changes from".into(),
            )),
        }
    }

    // ---------------------------------------------------------------- automatic delivery

    /// Opens the task's pull request by itself when the task is ready for delivery (#35), so
    /// nobody has to: called when the agent ends a turn cleanly. Merging and promotion stay
    /// human (GIT-002). The outcome is said in the task's terminal; failures are logged and not
    /// retried for the same head.
    pub async fn auto_open_pr(&self, task_id: &TaskId) {
        match self.try_auto_open_pr(task_id, DeliveryAsk::TurnEnd).await {
            Ok(AutoDelivery::Delivered(done)) => self.runtime(task_id).hub.notice(done),
            Ok(AutoDelivery::Unchanged | AutoDelivery::Blocked(_)) => {}
            Err(AutoDeliveryError { error: err, note }) => {
                tracing::warn!(task = %task_id, error = %err, "automatic delivery failed");
                self.runtime(task_id).hub.notice(note.unwrap_or_else(|| {
                    format!(
                        "Yulius could not deliver this head by itself: {err} Use Deliver… on the task."
                    )
                }));
            }
        }
    }

    /// Records why Yulius did not push or open the pull request at the end of a turn, for
    /// `yulius wait` (SPEC-050 DLV-204): an agent that expects delivery learns what blocks it
    /// instead of waiting for a pull request that never comes (#34 waited 42 minutes). Only for
    /// a branch with commits to deliver.
    async fn note_undelivered(&self, task: &TaskRecord, worktree: &str, reason: &str) {
        self.note_undelivered_with(task, worktree, reason, false)
            .await;
    }

    async fn note_undelivered_with(
        &self,
        task: &TaskRecord,
        worktree: &str,
        reason: &str,
        actionable_in_watch: bool,
    ) {
        let path = Path::new(worktree);
        let Some(head) = self
            .deps
            .git
            .log(path, "HEAD", 1)
            .await
            .ok()
            .and_then(|log| log.into_iter().next())
            .map(|commit| commit.sha)
        else {
            return;
        };
        let Ok(sandbox) = self.dlv_sandbox_of(task).await else {
            return;
        };
        let integration = delivery_profile(&sandbox).integration_branch;
        let ahead = self
            .deps
            .git
            .ahead_behind(path, &format!("origin/{integration}"), "HEAD")
            .await
            .map(|(ahead, _)| ahead)
            .unwrap_or(0);
        if ahead == 0 {
            return;
        }
        let short = head.get(..9).unwrap_or(&head);
        // With a pull request open (an earlier head delivered), what is missing is the push.
        let open = matches!(
            self.deps.store.delivery(&task.id).await,
            Ok(Some(record)) if matches!(record.stage, DeliveryStage::PrOpen)
        );
        let missing = if open {
            "not pushed to its open pull request"
        } else {
            "no push, no pull request"
        };
        let text = format!(
            "Yulius has not delivered {short} ({missing}): {reason}. It delivers at the end of \
             your turn once nothing blocks it."
        );
        if let Ok(mut problem) = self.runtime(&task.id).push_problem.lock() {
            *problem = Some(PushNote {
                head,
                text,
                failed_push: false,
                actionable_in_watch,
            });
        }
    }

    /// Records an actionable push/publication failure note on `task_id` for `head` (SPEC-553
    /// REP-004).
    fn set_actionable_note(
        &self,
        task_id: &TaskId,
        head: &str,
        text: String,
        failed_push: bool,
    ) -> String {
        if let Ok(mut problem) = self.runtime(task_id).push_problem.lock() {
            *problem = Some(PushNote {
                head: head.to_owned(),
                text: text.clone(),
                failed_push,
                actionable_in_watch: true,
            });
        }
        text
    }

    /// Records the non-fast-forward remedy when a local repair branch diverged from or rewound
    /// behind its open pull request head (SPEC-553 REP-003, REP-004).
    fn set_diverged_problem(
        &self,
        task_id: &TaskId,
        head: &str,
        branch: &str,
        pull: u64,
    ) -> String {
        let short = head.get(..9).unwrap_or(head);
        let text = format!(
            "Yulius could not push {short} to pull request #{pull}: the branch on GitHub is not \
             an ancestor of your HEAD (your branch was rebased, amended, or rewound after it was \
             pushed). Do not force-push. Merge the pushed head back in: `git fetch origin && git \
             merge --no-edit origin/{branch}` (the same changes merge cleanly), rerun the tests \
             if anything changed, and end your turn: Yulius pushes again. Bring the integration \
             branch in by merging it, never by rebasing."
        );
        self.set_actionable_note(task_id, head, text, true)
    }

    /// The actionable push/publication note for `head`, if one is recorded for `task_id`
    /// (SPEC-553 REP-004).
    pub(crate) fn actionable_push_problem_of(
        &self,
        task_id: &TaskId,
        head: &str,
    ) -> Option<String> {
        let runtime = self.existing_runtime(task_id)?;
        let note = runtime.push_problem.lock().ok()?.clone()?;
        (note.actionable_in_watch
            && !head.is_empty()
            && (note.head.starts_with(head) || head.starts_with(note.head.as_str())))
        .then_some(note.text)
    }

    /// The current actionable push/publication note `(head, text)` on `task_id`, regardless of
    /// head match (for `pr_watch` and `pr_news_due`, SPEC-553 REP-004).
    pub(crate) fn actionable_push_problem(&self, task_id: &TaskId) -> Option<(String, String)> {
        let runtime = self.existing_runtime(task_id)?;
        let note = runtime.push_problem.lock().ok()?.clone()?;
        note.actionable_in_watch.then_some((note.head, note.text))
    }

    /// Whether `task_id` already has a `push_problem` recorded for `head`.
    pub(crate) fn has_push_problem_for(&self, task_id: &TaskId, head: &str) -> bool {
        self.existing_runtime(task_id).is_some_and(|runtime| {
            runtime
                .push_problem
                .lock()
                .ok()
                .and_then(|p| p.clone())
                .is_some_and(|note| {
                    !head.is_empty()
                        && (note.head.starts_with(head) || head.starts_with(note.head.as_str()))
                })
        })
    }

    /// Records why the agent's head could not be pushed, for `yulius wait` (DLV-201), and
    /// returns that note. A branch rewritten after it was pushed is refused as not a
    /// fast-forward; the remedy merges the pushed head back in, so nothing is force-pushed.
    pub(crate) fn set_push_problem(
        &self,
        task_id: &TaskId,
        head: &str,
        branch: &str,
        pull: Option<u64>,
        err: &AppError,
    ) -> String {
        let reason = err.public_message();
        let target = pull.map_or_else(|| format!("`{branch}`"), |n| format!("pull request #{n}"));
        let short = head.get(..9).unwrap_or(head);
        let text = if ["non-fast-forward", "fetch first", "[rejected]"]
            .iter()
            .any(|needle| reason.contains(needle))
        {
            format!(
                "Yulius could not push {short} to {target}: GitHub refused it because the branch \
                 there is not an ancestor of your HEAD (your branch was rebased or amended after \
                 it was pushed). Do not force-push. Merge the pushed head back in: `git fetch \
                 origin && git merge --no-edit origin/{branch}` (the same changes merge \
                 cleanly), rerun the tests if anything changed, and end your turn: Yulius pushes \
                 again. Bring the integration branch in by merging it, never by rebasing."
            )
        } else {
            format!(
                "Yulius could not push {short} to {target}: {reason} It tries again at the end of \
                 your next turn; `delivery_status` shows the branch state."
            )
        };
        if let Ok(mut problem) = self.runtime(task_id).push_problem.lock() {
            *problem = Some(PushNote {
                head: head.to_owned(),
                text: text.clone(),
                failed_push: true,
                actionable_in_watch: true,
            });
        }
        text
    }

    fn clear_push_problem(&self, task_id: &TaskId) {
        if let Ok(mut problem) = self.runtime(task_id).push_problem.lock() {
            *problem = None;
        }
    }

    /// Pushes the task's `branch`, at `head`, with `token` (SPEC-139 PSH-001): every push of a
    /// task's head goes through here. A push Git reports failed while the remote's branch is at
    /// `head` is done (GitHub's `remote rejected … (failed)` after the ref moved, or the head
    /// pushed some other way, #366); a push that is done clears the task's note about `head`.
    ///
    /// # Errors
    /// The push's own error when the remote's branch is not at `head` (or cannot be read).
    pub(crate) async fn dlv_push_head(
        &self,
        task_id: &TaskId,
        worktree: &Path,
        branch: &str,
        head: &str,
        token: &Secret,
    ) -> AppResult<()> {
        if let Err(err) = self.deps.git.push(worktree, branch, branch, token).await
            && !self.dlv_branch_at(worktree, branch, head, token).await
        {
            return Err(err);
        }
        if let Ok(mut problem) = self.runtime(task_id).push_problem.lock()
            && problem.as_ref().is_some_and(|note| note.head == head)
        {
            *problem = None;
        }
        Ok(())
    }

    /// Whether the remote's `branch` is at commit `head`, as the remote answers now (not a
    /// remote-tracking ref, which a push from elsewhere never moves); `false` when it cannot be
    /// read.
    async fn dlv_branch_at(
        &self,
        worktree: &Path,
        branch: &str,
        head: &str,
        token: &Secret,
    ) -> bool {
        if head.is_empty() {
            return false;
        }
        match self
            .deps
            .git
            .remote_branch_head(worktree, branch, token)
            .await
        {
            Ok(remote) => remote.is_some_and(|sha| sha.eq_ignore_ascii_case(head)),
            Err(err) => {
                tracing::debug!(%branch, error = %err, "the remote's branch could not be read");
                false
            }
        }
    }

    /// Records that `head` was pushed but its pull request could not be opened, for `yulius
    /// wait`, and returns that note: not a push failure, so the branch at the head does not make
    /// it stale (SPEC-139 PSH-002).
    fn set_open_problem(&self, task_id: &TaskId, head: &str, err: &AppError) -> String {
        let short = head.get(..9).unwrap_or(head);
        let text = format!(
            "Yulius pushed {short} but could not open its pull request: {} It tries again at the \
             end of your next turn and on `delivery_merge`; `delivery_status` shows the branch \
             state.",
            err.public_message()
        );
        if let Ok(mut problem) = self.runtime(task_id).push_problem.lock() {
            *problem = Some(PushNote {
                head: head.to_owned(),
                text: text.clone(),
                failed_push: false,
                actionable_in_watch: false,
            });
        }
        text
    }

    /// Drops the note that a push of `head` failed once the remote's branch is at `head`, so
    /// `yulius wait` waits for the checks and the armed merge instead of ending on it (SPEC-139
    /// PSH-002, #366). Without a pull request there is nothing to wait for: the note then says
    /// the branch is pushed and how the pull request opens. One read of the remote, only while
    /// such a note exists; unreadable, the note stands.
    pub(crate) async fn forget_push_problem_if_pushed(&self, task: &TaskRecord, head: &str) {
        let noted = self
            .runtime(&task.id)
            .push_problem
            .lock()
            .ok()
            .and_then(|problem| problem.clone())
            .filter(|note| {
                note.failed_push
                    && !head.is_empty()
                    && (note.head.starts_with(head) || head.starts_with(note.head.as_str()))
            });
        let (Some(note), Some(branch), Some(worktree)) =
            (noted, task.branch.as_deref(), task.worktree_path.as_deref())
        else {
            return;
        };
        let Ok(sandbox) = self.dlv_sandbox_of(task).await else {
            return;
        };
        let Ok(token) = self.task_github_token(task, &sandbox).await else {
            return;
        };
        if !self
            .dlv_branch_at(Path::new(worktree), branch, &note.head, &token)
            .await
        {
            return;
        }
        let unopened = !matches!(
            self.deps.store.delivery(&task.id).await,
            Ok(Some(record)) if record.stage != DeliveryStage::None
        );
        let next = unopened.then(|| PushNote {
            head: note.head.clone(),
            text: format!(
                "Yulius sees {} pushed to `{branch}`, but its pull request is not open yet: call \
                 `delivery_merge` (or end your turn) and Yulius opens it.",
                note.head.get(..9).unwrap_or(&note.head)
            ),
            failed_push: false,
            actionable_in_watch: false,
        });
        if let Ok(mut problem) = self.runtime(&task.id).push_problem.lock()
            && problem.as_ref() == Some(&note)
        {
            *problem = next;
        }
    }

    /// What Yulius did for a ready task: opened its pull request, or pushed commits made since
    /// to the open one (#68), or opened a new one for work committed after its merge (SPEC-425).
    /// [`AutoDelivery::Blocked`] says why a head with commits to deliver is not ready (an invalid
    /// spec, what the plan still holds for this `ask`, uncommitted work) or why Yulius cannot
    /// judge what a merged task's head adds; [`AutoDelivery::Unchanged`] covers everything else
    /// (no branch or worktree, no commits ahead of the integration branch, a merged task with
    /// nothing new, nothing new). At a turn's end every step has to be finished; the agent's
    /// `delivery_merge` delivers the finished part of the plan (SPEC-484 PLN-002).
    pub(crate) async fn try_auto_open_pr(
        &self,
        task_id: &TaskId,
        ask: DeliveryAsk,
    ) -> Result<AutoDelivery, AutoDeliveryError> {
        let store = &self.deps.store;
        let task = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        let (Some(branch), Some(worktree)) = (task.branch.clone(), task.worktree_path.clone())
        else {
            return Ok(AutoDelivery::Unchanged);
        };
        // A playground has no pull requests: its agent merges with `delivery_merge` (SPEC-099).
        if self.dlv_sandbox_of(&task).await?.is_playground() {
            return Ok(AutoDelivery::Unchanged);
        }
        // Cheap checks first: most turns end with steps still open and no pull request yet, and
        // they need no GitHub call (a merged task's head is judged before them; a covered head
        // only once: SPEC-425).
        let steps = store.steps(&task.id).await?;
        let hold = plan_hold(&steps, ask);
        if steps.is_empty() && !self.branch_changed(&task).await? {
            return Ok(AutoDelivery::Unchanged);
        }
        // Past its pull request's merge a task delivers again only work committed since
        // (SPEC-425 FUP-001); a later stage with nothing new stays as it is (FUP-002), and one
        // Yulius cannot judge says why to `delivery_merge` (FUP-003).
        let record = store.delivery(&task.id).await?;
        // An open pull request may hold this very head: a part of the plan that `delivery_merge`
        // delivered with steps still ahead (SPEC-484 PLN-002). Whether the head is delivered is
        // GitHub's answer, read below; what the plan holds is said after it (`not_ready`), so a
        // turn that ends on a delivered, armed head leaves no "not delivered" note, and a call
        // for a head the pull request already has is not held by a step started since. The
        // cost: with a pull request open and steps ahead, a turn's end reads GitHub once.
        let pull_open = record
            .as_ref()
            .is_some_and(|r| matches!(r.stage, DeliveryStage::PrOpen));
        let merged =
            record.filter(|r| !matches!(r.stage, DeliveryStage::None | DeliveryStage::PrOpen));
        let follow_up = match &merged {
            Some(record) => match self
                .work_after_merge(&task, Path::new(&worktree), record)
                .await
            {
                AfterMerge::FollowUp(number) => Some((record, number)),
                AfterMerge::Covered => {
                    // Nothing undelivered: an earlier "not delivered" note is stale.
                    self.clear_push_problem(task_id);
                    return Ok(AutoDelivery::Unchanged);
                }
                AfterMerge::Unknown(reason) => {
                    // Said to `yulius wait` too, so an agent that only ends its turn hears it.
                    self.note_undelivered(&task, &worktree, &reason).await;
                    return Ok(AutoDelivery::Blocked(reason));
                }
            },
            None => None,
        };
        if let Some(reason) = hold.filter(|_| !pull_open) {
            self.note_undelivered(&task, &worktree, &reason).await;
            return Ok(AutoDelivery::Blocked(reason));
        }
        let sandbox = self.dlv_sandbox_of(&task).await?;
        let profile = delivery_profile(&sandbox);
        if branch == profile.integration_branch || branch == profile.production_branch {
            return Ok(AutoDelivery::Unchanged);
        }
        if let Some((record, number)) = follow_up {
            // Ready first, as for any delivery: a head that cannot go out keeps the task merged.
            let spec = self.dlv_check_spec(&task).await;
            let dirty = self
                .deps
                .git
                .has_changes(Path::new(&worktree))
                .await
                .unwrap_or(false);
            if let Some(reason) = not_ready(&spec, &steps, ask, dirty) {
                self.note_undelivered(&task, &worktree, &reason).await;
                return Ok(AutoDelivery::Blocked(reason));
            }
            // The stage alone is compared on write: a record that moved on to another pull
            // request meanwhile (a concurrent Stop delivered and merged it) is left alone.
            let current = store.delivery(&task.id).await?;
            let fresh = DeliveryRecord {
                updated_at: self.now(),
                ..DeliveryRecord::initial(&task.id)
            };
            if current.as_ref().map(|r| (&r.stage, r.pr_number))
                != Some((&record.stage, record.pr_number))
                || !self
                    .dlv_transition(&task.sandbox_id, &record.stage, &fresh)
                    .await?
            {
                return Ok(AutoDelivery::Unchanged);
            }
            self.dlv_audit(
                "yulius",
                "delivery.follow_up",
                &format!("{}#{}", sandbox.slug, task.number),
                &format!(
                    "{} -> none: commits after pull request #{number} merged are delivered anew",
                    record.stage
                ),
            )
            .await;
            // How the merged pull request's armed merge ended is that pull request's: the new
            // one starts without it (SPEC-444 live check, #1135).
            self.forget_merge_outcome(&task);
        }
        // The task's owner pushes and authors the pull request (their GitHub account, as if
        // they had pressed Deliver…, F 2026-10-11); without a usable token the App does.
        let token = self.task_github_token(&task, &sandbox).await?;
        let observed = self
            .dlv_observe(
                &sandbox,
                &task,
                &profile,
                ObserveAccess::Provided(Some(&token)),
            )
            .await?;
        // Nothing new to deliver: the open pull request has this head already. `delivery_merge`
        // asks on every call, and a delivered head is not "undelivered" (SPEC-116 DMG-001).
        if let (Some(head), Some(pull)) = (
            observed.worktree_head.as_deref(),
            observed.pr.as_ref().filter(|p| is_open(p)),
        ) && pull.head_sha == head
        {
            // Delivered some other way meanwhile: an earlier "not delivered" note is stale.
            self.clear_push_problem(task_id);
            return Ok(AutoDelivery::Unchanged);
        }
        let dirty = observed.dirty || !observed.dirty_known;
        // SPEC-553 (ADR-0391): for an existing task-owned open pull request whose previous head
        // failed CI, an eligible clean committed fast-forward repair on the same feature branch
        // publishes even while verification steps remain open, breaking the circular wait.
        let mut repair_publish = false;
        if ask == DeliveryAsk::TurnEnd
            && let (Some(head), Some(pull)) = (
                observed.worktree_head.as_deref(),
                observed.pr.as_ref().filter(|p| is_open(p)),
            )
        {
            match self
                .open_pr_repair_decision(
                    &task,
                    &branch,
                    Path::new(&worktree),
                    &profile,
                    &observed,
                    head,
                )
                .await
            {
                dlv::OpenPrRepairDecision::Publish => repair_publish = true,
                dlv::OpenPrRepairDecision::NotApplicable => {}
                dlv::OpenPrRepairDecision::WrongBranch => {
                    let short = head.get(..9).unwrap_or(head);
                    let text = format!(
                        "Yulius could not push {short} to pull request #{}: your worktree HEAD is \
                         not on `{branch}`. Check out `{branch}` with your committed repair and \
                         end your turn so Yulius pushes it.",
                        pull.number
                    );
                    self.set_actionable_note(task_id, head, text.clone(), false);
                    return Ok(AutoDelivery::Blocked(text));
                }
                dlv::OpenPrRepairDecision::DirtyWorktree => {
                    let reason = if observed.dirty_known {
                        "the worktree has uncommitted changes: commit them"
                    } else {
                        "Yulius could not confirm the worktree is clean: check `git status` and \
                         commit or discard any uncommitted changes"
                    };
                    self.note_undelivered_with(&task, &worktree, reason, true)
                        .await;
                    return Ok(AutoDelivery::Blocked(reason.to_owned()));
                }
                dlv::OpenPrRepairDecision::SpecOrPlanInvalid => {
                    let reason =
                        not_ready(&observed.spec, &observed.steps, DeliveryAsk::Review, false)
                            .unwrap_or_else(|| "the task spec or plan is not valid".to_owned());
                    self.note_undelivered_with(&task, &worktree, &reason, true)
                        .await;
                    return Ok(AutoDelivery::Blocked(reason));
                }
                dlv::OpenPrRepairDecision::Diverged => {
                    let note = self.set_diverged_problem(task_id, head, &branch, pull.number);
                    return Err(AutoDeliveryError {
                        error: AppError::Conflict(note.clone()),
                        note: Some(note),
                    });
                }
            }
        }
        // A task that needs a spec needs its plan too; research, docs and chores may have none.
        if !repair_publish
            && let Some(reason) = not_ready(&observed.spec, &observed.steps, ask, dirty)
        {
            self.note_undelivered(&task, &worktree, &reason).await;
            return Ok(AutoDelivery::Blocked(reason));
        }
        let Some(head) = observed.worktree_head.clone() else {
            return Ok(AutoDelivery::Unchanged);
        };
        // The pull request exists: push what was committed since, as the briefing promises,
        // so it does not stay on an old head while the task shows Done (#68).
        if observed.record.stage == DeliveryStage::PrOpen || observed.pr.is_some() {
            let Some(pull) = observed.pr.as_ref().filter(|p| is_open(p)) else {
                return Ok(AutoDelivery::Unchanged);
            };
            if !self.is_task_owned_open_pull(pull, &observed.record, &branch, &profile)
                || !self
                    .worktree_on_branch(Path::new(&worktree), &branch, &head)
                    .await
            {
                return Ok(AutoDelivery::Unchanged);
            }
            if pull.head_sha == head || !self.claim_auto_head(task_id, &head)? {
                return Ok(AutoDelivery::Unchanged);
            }
            let pushed = self
                .dlv_push_head(task_id, Path::new(&worktree), &branch, &head, &token)
                .await;
            if let Err(error) = pushed {
                self.release_auto_head(task_id);
                let note =
                    self.set_push_problem(task_id, &head, &branch, Some(pull.number), &error);
                return Err(AutoDeliveryError {
                    error,
                    note: Some(note),
                });
            }
            self.trigger_ci_after_push(&sandbox, task_id).await;
            return Ok(AutoDelivery::Delivered(format!(
                "Yulius pushed your new commits to pull request #{}.",
                pull.number
            )));
        }
        let (ahead, _) = self
            .deps
            .git
            .ahead_behind(
                Path::new(&worktree),
                &format!("origin/{}", profile.integration_branch),
                "HEAD",
            )
            .await?;
        if ahead == 0 || !self.claim_auto_head(task_id, &head)? {
            return Ok(AutoDelivery::Unchanged);
        }
        // Which step failed: only a push failure's note is about pushing, and a refusal pushed
        // nothing (SPEC-139 PSH-002).
        let opened = match dlv::apply(
            &observed.record.stage,
            &DeliveryAction::OpenPr,
            Initiator::Policy,
            &profile,
            &observed.facts(),
        ) {
            Ok(_) => match self
                .dlv_push_head(task_id, Path::new(&worktree), &branch, &head, &token)
                .await
            {
                Ok(()) => self
                    .dlv_open_pull(
                        &sandbox, &task, &profile, &observed, &token, None, None, "yulius",
                    )
                    .await
                    .map_err(|err| (err, Failed::Open)),
                Err(err) => Err((err, Failed::Push)),
            },
            Err(err) => Err((refusal(&err), Failed::Refused)),
        };
        match opened {
            Ok(number) => {
                self.clear_push_problem(task_id);
                self.trigger_ci_after_push(&sandbox, task_id).await;
                Ok(AutoDelivery::Delivered(format!(
                    "Yulius opened pull request #{number} into {}: the task is ready for review.",
                    profile.integration_branch
                )))
            }
            Err((error, failed)) => {
                self.release_auto_head(task_id);
                let note = match failed {
                    Failed::Push => {
                        Some(self.set_push_problem(task_id, &head, &branch, None, &error))
                    }
                    Failed::Open => Some(self.set_open_problem(task_id, &head, &error)),
                    // Defensive: `not_ready` already holds back what the rules refuse here.
                    Failed::Refused => {
                        self.note_undelivered(&task, &worktree, &error.public_message())
                            .await;
                        None
                    }
                };
                Err(AutoDeliveryError { error, note })
            }
        }
    }

    /// Whether `pull` is an open task-owned pull request on this task's own non-protected feature
    /// branch targeting the integration branch (SPEC-553 REP-001, REP-003).
    fn is_task_owned_open_pull(
        &self,
        pull: &GhPull,
        record: &DeliveryRecord,
        branch: &str,
        profile: &DeliveryProfile,
    ) -> bool {
        is_open(pull)
            && record.stage == DeliveryStage::PrOpen
            && record.pr_number == Some(pull.number)
            && !pull.from_fork
            && pull.head == branch
            && pull.base == profile.integration_branch
            && branch != profile.integration_branch
            && branch != profile.production_branch
    }

    /// Whether `worktree` currently has `branch` checked out (not detached, not another branch)
    /// and `refs/heads/<branch>` resolves to `head` (fail-closed so Yulius never pushes an
    /// unrelated branch or detached HEAD).
    async fn worktree_on_branch(&self, worktree: &Path, branch: &str, head: &str) -> bool {
        let Ok(list) = self.deps.git.worktrees(worktree).await else {
            return false;
        };
        let want = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
        let checked_out = list.iter().any(|wt| {
            let path = std::fs::canonicalize(&wt.path).unwrap_or_else(|_| wt.path.clone());
            path == want && wt.branch.as_deref() == Some(branch)
        });
        if !checked_out {
            return false;
        }
        match self
            .deps
            .git
            .resolve_commit(worktree, &format!("refs/heads/{branch}"))
            .await
        {
            Ok(Some(sha)) => sha.eq_ignore_ascii_case(head),
            Ok(None) | Err(_) => false,
        }
    }

    /// Whether an actual human decision or explicit hold is pending for `task` (SPEC-553
    /// REP-001, REP-003): fail-closed on store errors.
    async fn human_decision_pending(&self, task: &TaskRecord, steps: &[StepRecord]) -> bool {
        if task.activity == Some(yulius_domain::task::Activity::NeedsAttention)
            || steps
                .iter()
                .any(|s| StepStatus::parse(&s.status) == Some(StepStatus::Blocked))
        {
            return true;
        }
        let store = &self.deps.store;
        if !matches!(store.task_needs_you(&task.id).await, Ok(None))
            || !matches!(store.pending_hold(&task.id).await, Ok(None))
            || !matches!(store.open_pause(&task.id).await, Ok(None))
        {
            return true;
        }
        self.browser_tab_requests(&task.sandbox_id)
            .await
            .contains(task.id.as_str())
    }

    /// Evaluates [`dlv::evaluate_open_pr_repair`] for `task` against `observed` and `head`.
    async fn open_pr_repair_decision(
        &self,
        task: &TaskRecord,
        branch: &str,
        worktree: &Path,
        profile: &DeliveryProfile,
        observed: &Observed,
        head: &str,
    ) -> dlv::OpenPrRepairDecision {
        let pull = observed
            .pr
            .as_ref()
            .filter(|pull| self.is_task_owned_open_pull(pull, &observed.record, branch, profile));
        let task_owned_open_pr = pull.is_some();
        let pr_checks_failing = observed.checks == ChecksState::Failure && !observed.checks_errored;
        let new_commit = pull.is_some_and(|p| !p.head_sha.eq_ignore_ascii_case(head));
        let merge_armed = self.merge_is_armed(&task.id);
        if !task_owned_open_pr || !pr_checks_failing || !new_commit || merge_armed {
            return dlv::OpenPrRepairDecision::NotApplicable;
        }
        let human_decision_pending = self.human_decision_pending(task, &observed.steps).await;
        if human_decision_pending {
            return dlv::OpenPrRepairDecision::NotApplicable;
        }
        let on_task_branch = self.worktree_on_branch(worktree, branch, head).await;
        let clean_worktree = observed.dirty_known && !observed.dirty;
        let spec_and_plan_ready =
            observed.spec.valid && !(observed.steps.is_empty() && observed.spec.required);
        let fast_forward = match pull {
            Some(p) => matches!(
                self.deps.git.is_ancestor(worktree, &p.head_sha, head).await,
                Ok(true)
            ),
            None => false,
        };
        dlv::evaluate_open_pr_repair(&dlv::OpenPrRepairFacts {
            task_owned_open_pr,
            on_task_branch,
            clean_worktree,
            spec_and_plan_ready,
            pr_checks_failing,
            new_commit,
            fast_forward,
            merge_armed,
            human_decision_pending,
        })
    }

    /// Queues exact-head CI right after an automatic push when Yulius CI is enabled (SPEC-553
    /// REP-002); failures are logged without undoing the already-completed push.
    async fn trigger_ci_after_push(&self, sandbox: &SandboxRecord, task_id: &TaskId) {
        if self.config.ci.enabled
            && let Err(err) = self.ci_sync_sandbox(sandbox).await
        {
            tracing::warn!(
                error = %err,
                task = %task_id,
                "exact-head CI sync after automatic push failed"
            );
        }
    }

    /// Publishes an eligible clean committed CI repair to `task_id`'s existing task-owned open
    /// pull request and triggers exact-head CI (SPEC-553 REP-004). Used by `yulius wait` and
    /// `pr_watch` restart recovery; never opens a new pull request and never writes routine
    /// step-hold notes for non-failing pull requests. Returns `true` when it pushed a repair.
    pub(crate) async fn try_publish_open_pr_repair(
        &self,
        task_id: &TaskId,
        expected_head: Option<&str>,
    ) -> bool {
        let store = &self.deps.store;
        let Ok(Some(task)) = store.task(task_id).await else {
            return false;
        };
        let (Some(branch), Some(worktree)) = (task.branch.clone(), task.worktree_path.clone())
        else {
            return false;
        };
        let Ok(sandbox) = self.dlv_sandbox_of(&task).await else {
            return false;
        };
        if sandbox.is_playground() {
            return false;
        }
        let profile = delivery_profile(&sandbox);
        if branch == profile.integration_branch || branch == profile.production_branch {
            return false;
        }
        let Ok(Some(record)) = store.delivery(&task.id).await else {
            return false;
        };
        if record.stage != DeliveryStage::PrOpen || record.pr_number.is_none() {
            return false;
        }
        if self.merge_is_armed(task_id) {
            return false;
        }
        let Ok(token) = self.task_github_token(&task, &sandbox).await else {
            return false;
        };
        let Ok(observed) = self
            .dlv_observe(
                &sandbox,
                &task,
                &profile,
                ObserveAccess::Provided(Some(&token)),
            )
            .await
        else {
            return false;
        };
        let (Some(head), Some(pull)) = (
            observed.worktree_head.clone(),
            observed.pr.as_ref().filter(|p| is_open(p)),
        ) else {
            return false;
        };
        if let Some(expected) = expected_head
            && !expected.is_empty()
            && !head
                .to_ascii_lowercase()
                .starts_with(&expected.to_ascii_lowercase())
            && !expected
                .to_ascii_lowercase()
                .starts_with(&head.to_ascii_lowercase())
        {
            return false;
        }
        if pull.head_sha.eq_ignore_ascii_case(&head) {
            self.clear_push_problem(task_id);
            return false;
        }
        match self
            .open_pr_repair_decision(
                &task,
                &branch,
                Path::new(&worktree),
                &profile,
                &observed,
                &head,
            )
            .await
        {
            dlv::OpenPrRepairDecision::NotApplicable => false,
            dlv::OpenPrRepairDecision::WrongBranch => {
                let short = head.get(..9).unwrap_or(&head);
                let text = format!(
                    "Yulius could not push {short} to pull request #{}: your worktree HEAD is \
                     not on `{branch}`. Check out `{branch}` with your committed repair and end \
                     your turn so Yulius pushes it.",
                    pull.number
                );
                self.set_actionable_note(task_id, &head, text, false);
                false
            }
            dlv::OpenPrRepairDecision::DirtyWorktree => {
                let reason = if observed.dirty_known {
                    "the worktree has uncommitted changes: commit them"
                } else {
                    "Yulius could not confirm the worktree is clean: check `git status` and \
                     commit or discard any uncommitted changes"
                };
                self.note_undelivered_with(&task, &worktree, reason, true)
                    .await;
                false
            }
            dlv::OpenPrRepairDecision::SpecOrPlanInvalid => {
                let reason = not_ready(&observed.spec, &observed.steps, DeliveryAsk::Review, false)
                    .unwrap_or_else(|| "the task spec or plan is not valid".to_owned());
                self.note_undelivered_with(&task, &worktree, &reason, true)
                    .await;
                false
            }
            dlv::OpenPrRepairDecision::Diverged => {
                let note = self.set_diverged_problem(task_id, &head, &branch, pull.number);
                self.runtime(task_id).hub.notice(note);
                false
            }
            dlv::OpenPrRepairDecision::Publish => {
                let Ok(true) = self.claim_auto_head(task_id, &head) else {
                    return false;
                };
                match self
                    .dlv_push_head(task_id, Path::new(&worktree), &branch, &head, &token)
                    .await
                {
                    Ok(()) => {
                        self.trigger_ci_after_push(&sandbox, task_id).await;
                        self.runtime(task_id).hub.notice(format!(
                            "Yulius pushed your new commits to pull request #{}.",
                            pull.number
                        ));
                        true
                    }
                    Err(error) => {
                        self.release_auto_head(task_id);
                        let note = self.set_push_problem(
                            task_id,
                            &head,
                            &branch,
                            Some(pull.number),
                            &error,
                        );
                        self.runtime(task_id).hub.notice(note);
                        false
                    }
                }
            }
        }
    }

    /// Claims `head` for one automatic attempt, so two Stop hooks never push or open twice;
    /// `false` when this head was already handled.
    fn claim_auto_head(&self, task_id: &TaskId, head: &str) -> AppResult<bool> {
        let runtime = self.runtime(task_id);
        let mut tried = runtime.auto_pr_head.lock().map_err(AppError::internal)?;
        if tried.as_deref() == Some(head) {
            return Ok(false);
        }
        *tried = Some(head.to_owned());
        Ok(true)
    }

    /// Forgets a failed attempt, so the next clean Stop tries again (#68).
    fn release_auto_head(&self, task_id: &TaskId) {
        if let Ok(mut tried) = self.runtime(task_id).auto_pr_head.lock() {
            *tried = None;
        }
    }
}
