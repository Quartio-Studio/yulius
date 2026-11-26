//! Parking: verifying a parked task's pull requests and whether the task is done.

use super::*;

impl App {
    /// Validate the Git facts again after remote lookups and the host probe.
    pub(crate) async fn parking_prs_still_current(
        &self,
        task: &TaskRecord,
        facts: &ParkingPrs,
        before: Option<&crate::stages::SpanRecord>,
    ) -> AppResult<bool> {
        if !self.parking_checkout_matches(task, facts).await?
            || self.parking_ci_blocker(task, facts).await?.is_some()
        {
            return Ok(false);
        }
        let wait = self.deps.store.latest_merge_wait(&task.id).await?;
        if wait.as_ref() != before {
            return Ok(false);
        }
        if facts.pulls.is_empty() {
            return Ok(true);
        }
        let (_, mut outcome) = self.armed_view(&task.id);
        let Some((_, decision)) = crate::pr_watch::candidate_from_wait(wait, &mut outcome)? else {
            return Ok(true);
        };
        let Some(pull) = facts
            .pulls
            .iter()
            .find(|pull| pull.number == decision.singleton.pr)
        else {
            return Ok(true);
        };
        let sandbox = self.dlv_sandbox_of(task).await?;
        Ok(self
            .candidate_pull_signal(task, &sandbox, &decision, pull)
            .is_none())
    }

    async fn parking_checkout_matches(
        &self,
        task: &TaskRecord,
        facts: &ParkingPrs,
    ) -> AppResult<bool> {
        Ok(facts.pulls.is_empty()
            || self
                .verification_checkout(task, PrVerification::WatchPending)
                .await?
                == facts.checkout)
    }

    async fn parking_ci_blocker(
        &self,
        task: &TaskRecord,
        facts: &ParkingPrs,
    ) -> AppResult<Option<(u64, PullSignal)>> {
        for pull in &facts.pulls {
            if let Some(signal) = self
                .parking_ci_signal(task, pull.number, &pull.head_sha, facts.watched)
                .await?
            {
                return Ok(Some((pull.number, signal)));
            }
        }
        Ok(None)
    }

    pub(crate) async fn parking_ci_signal(
        &self,
        task: &TaskRecord,
        number: u64,
        head: &str,
        watched: Option<u64>,
    ) -> AppResult<Option<PullSignal>> {
        use yulius_domain::ci::{CiRef, RunState};
        let runs = self
            .deps
            .store
            .ci_runs_for_sha(&task.sandbox_id, head)
            .await?;
        let Some(own) = runs
            .iter()
            .find(|run| run.reference == CiRef::PullRequest(number))
        else {
            return Ok(None);
        };
        match own.state {
            RunState::Failed => Ok(Some(PullSignal::Failing)),
            RunState::Error | RunState::Cancelled => Ok(Some(PullSignal::Errored)),
            RunState::Queued | RunState::Running => {
                if watched == Some(number) && self.config.ci.enabled {
                    let recent = self
                        .deps
                        .store
                        .ci_runs(&task.sandbox_id, crate::pr_watch::RUNS_READ)
                        .await?;
                    if recent
                        .iter()
                        .find(|run| run.reference == own.reference)
                        .is_some_and(|run| run.id == own.id && run.state.is_active())
                    {
                        return Ok(None);
                    }
                }
                Ok(Some(PullSignal::InReview))
            }
            RunState::Passed | RunState::Skipped => Ok(None),
        }
    }

    /// Whether the task's worktree head carries work its merged pull request (`record`) did not
    /// (SPEC-425 FUP-001): the merged head is an ancestor of it, it is more than that head plus
    /// Git's own clean merges of the integration branch, and it adds something to the integration
    /// branch (a follow-up merged outside Yulius adds nothing). A head that is the merged one,
    /// behind it, or adds nothing is [`AfterMerge::Covered`] (FUP-002), remembered per head so
    /// later Stops ask GitHub nothing; anything Yulius cannot judge is [`AfterMerge::Unknown`].
    pub(super) async fn work_after_merge(
        &self,
        task: &TaskRecord,
        worktree: &Path,
        record: &DeliveryRecord,
    ) -> AfterMerge {
        let Some(number) = record.pr_number else {
            return AfterMerge::Unknown(
                "Yulius has no merged pull request on record for this task".to_owned(),
            );
        };
        let head = match self.deps.git.log(worktree, "HEAD", 1).await {
            Ok(commits) => match commits.into_iter().next() {
                Some(commit) => commit.sha,
                None => return AfterMerge::Unknown("the task branch has no commit".to_owned()),
            },
            Err(err) => {
                return AfterMerge::Unknown(format!(
                    "the task branch head cannot be read: {}",
                    err.public_message()
                ));
            }
        };
        let covered = (head.clone(), number);
        if let Ok(known) = self.runtime(&task.id).merged_covers.lock()
            && known.as_ref() == Some(&covered)
        {
            return AfterMerge::Covered;
        }
        let verdict = self.judge_after_merge(task, worktree, number, &head).await;
        if matches!(verdict, AfterMerge::Covered)
            && let Ok(mut known) = self.runtime(&task.id).merged_covers.lock()
        {
            *known = Some(covered);
        }
        verdict
    }

    /// [`App::work_after_merge`] for `head`, asking GitHub and Git.
    async fn judge_after_merge(
        &self,
        task: &TaskRecord,
        worktree: &Path,
        number: u64,
        head: &str,
    ) -> AfterMerge {
        let unknown = |why: String| {
            AfterMerge::Unknown(format!(
                "whether this head adds to pull request #{number}, merged already, is unknown: \
                 {why}"
            ))
        };
        let sandbox = match self.dlv_sandbox_of(task).await {
            Ok(sandbox) => sandbox,
            Err(err) => return unknown(err.public_message()),
        };
        let merged = match self.task_github_token(task, &sandbox).await {
            Ok(token) => match self
                .deps
                .github
                .get_pull(&token, &sandbox.repo_owner, &sandbox.repo_name, number)
                .await
            {
                Ok(pull) => pull.head_sha,
                Err(err) => return unknown(format!("GitHub: {}", github_reason(&err))),
            },
            Err(err) => return unknown(err.public_message()),
        };
        if merged == head {
            return AfterMerge::Covered;
        }
        // Every check below reads the integration branch and the merged head from a fresh fetch.
        if let Err(err) = self.dlv_refresh_clone(&sandbox).await {
            return unknown(format!(
                "the clone could not be refreshed: {}",
                err.public_message()
            ));
        }
        let git = &self.deps.git;
        let short = merged.get(..9).unwrap_or(&merged);
        if !matches!(git.resolve_commit(worktree, &merged).await, Ok(Some(_))) {
            return unknown(format!(
                "its merged head {short} is not in this clone (made on GitHub?). Fetch it with \
                 `git fetch origin pull/{number}/head` and merge it without forcing (`git merge \
                 --no-edit {merged}`), and Yulius delivers what this head adds"
            ));
        }
        match git.is_ancestor(worktree, &merged, head).await {
            Ok(true) => {}
            Ok(false)
                if git
                    .is_ancestor(worktree, head, &merged)
                    .await
                    .unwrap_or(false) =>
            {
                return AfterMerge::Covered;
            }
            Ok(false) => {
                return unknown(format!(
                    "the branch no longer contains its merged head {short} (rewritten?). Merge \
                     it back without forcing (`git merge --no-edit {merged}`), and Yulius \
                     delivers what this head adds"
                ));
            }
            Err(err) => return unknown(err.public_message()),
        }
        if self.review_covers(task, worktree, &merged, head).await {
            return AfterMerge::Covered;
        }
        // What the head changed since the merged head, applied to the integration branch: a
        // follow-up merged outside Yulius changes nothing there, whatever else `dev` gained.
        let integration = format!("origin/{}", delivery_profile(&sandbox).integration_branch);
        match git.adds_to(worktree, &integration, &merged, head).await {
            // A live merge train counts on its members' recorded stages until it ends: only new
            // work would move this one's back.
            Ok(Some(true)) if self.in_train(task) => unknown(
                "it is in a merge train that has not ended; call again once the train ends"
                    .to_owned(),
            ),
            Ok(Some(true)) => AfterMerge::FollowUp(number),
            Ok(Some(false)) => AfterMerge::Covered,
            // A conflict may be work merged outside Yulius that `dev` changed since, or new work
            // that conflicts: the merge settles it either way.
            Ok(None) => unknown(format!(
                "what it changed conflicts with `{integration}`. Merge it (`git merge --no-edit \
                 {integration}`) and Yulius delivers what this head still adds"
            )),
            Err(err) => unknown(err.public_message()),
        }
    }

    /// Verify that the task's own current PR leaves no work in the session about to park.
    /// The parked PR watcher delivers the result of an active Yulius CI run on this head.
    pub(crate) async fn verify_parking_prs(
        &self,
        task: &TaskRecord,
        record: Option<&DeliveryRecord>,
        purpose: PrVerification,
    ) -> AppResult<Option<ParkingPrs>> {
        let mut observed = ParkedPullRead::recorded(record);
        self.verify_task_prs(task, record, purpose, &mut observed, None)
            .await
    }

    pub(crate) fn known_parked_pull(
        &self,
        task: &TaskRecord,
        record: Option<&DeliveryRecord>,
    ) -> ParkedPullRead {
        let mut observed = ParkedPullRead::recorded(record);
        let merged = observed.pr.filter(|pr| !pr.open).map(|pr| pr.number);
        // Keep the owner's known PR when installation or branch discovery fails.
        let known = self
            .observed_pulls
            .known_pull(&task.sandbox_id, &task.id)
            .flatten()
            .filter(|pr| {
                Some(pr.number) != merged
                    && (pr.open || observed.pr.is_none_or(|r| r.number == pr.number))
            });
        if let Some(pr) = known {
            observed.pr = Some(yulius_contracts::api::tasks::ParkedPr {
                number: pr.number,
                open: pr.open,
            });
            observed.signal = None;
            observed.head = pr.head;
        }
        observed
    }

    pub(crate) async fn restore_parked_followup(
        &self,
        task: &TaskRecord,
        observed: &mut ParkedPullRead,
    ) -> AppResult<()> {
        if observed.pr.is_none_or(|pr| !pr.open)
            && let Some(number) = self.deps.store.open_followup(&task.id).await?
        {
            observed.pr = Some(yulius_contracts::api::tasks::ParkedPr { number, open: true });
            observed.head = None;
            observed.signal = None;
            observed.fresh_head = false;
        }
        Ok(())
    }

    pub(crate) fn last_pull_signal(
        &self,
        task: &TaskRecord,
        observed: &ParkedPullRead,
    ) -> Option<PullSignal> {
        let pr = observed.pr?;
        observed.signal.or_else(|| {
            self.observed_pulls
                .known_pull(&task.sandbox_id, &task.id)
                .flatten()
                .filter(|known| known.number == pr.number && known.open == pr.open)
                .and_then(|known| {
                    if observed.head.is_some() && observed.head != known.head {
                        Some(PullSignal::InReview)
                    } else {
                        known.signal
                    }
                })
        })
    }

    /// Waits for all dispatched parked-display Git work to return its permits.
    ///
    /// # Errors
    /// The pool closed or its configured size cannot be represented.
    #[cfg(feature = "test-support")]
    pub async fn wait_for_parked_git_workers_for_test(&self) -> AppResult<()> {
        let count = u32::try_from(crate::sandboxes::PARKED_READS).map_err(AppError::internal)?;
        let _permits = self
            .parked_git
            .acquire_many(count)
            .await
            .map_err(AppError::internal)?;
        Ok(())
    }

    /// List reads can be cancelled while Git is still running. Release their
    /// permit only when the dispatched Git work has returned.
    pub(super) async fn verification_git<
        T: Send + 'static,
        F: std::future::Future<Output = AppResult<T>> + Send + 'static,
    >(
        &self,
        purpose: PrVerification,
        read: impl FnOnce(Arc<App>) -> F + Send + 'static,
    ) -> AppResult<T> {
        let app = self.parked_git_app.upgrade().ok_or(AppError::NotFound)?;
        if purpose != PrVerification::Display {
            return read(app).await;
        }
        let permit = self
            .parked_git
            .clone()
            .acquire_owned()
            .await
            .map_err(AppError::internal)?;
        tokio::spawn(async move {
            let _permit = permit;
            read(app).await
        })
        .await
        .map_err(AppError::internal)?
    }

    pub(crate) async fn verify_task_prs(
        &self,
        task: &TaskRecord,
        record: Option<&DeliveryRecord>,
        purpose: PrVerification,
        observed: &mut ParkedPullRead,
        mut discovery: Option<&mut FollowupDiscovery>,
    ) -> AppResult<Option<ParkingPrs>> {
        let tracked = record
            .filter(|r| r.stage == DeliveryStage::PrOpen)
            .map(|r| {
                r.pr_number.ok_or_else(|| {
                    AppError::Internal("open delivery has no pull request number".into())
                })
            })
            .transpose()?;
        // A confirmed follow-up belongs to the task even if an overview lists a newer
        // PR or omits it. Start-rule decisions and parking must check it by number too.
        let stored_open = self
            .deps
            .store
            .open_followups(&task.id, MAX_PULLS + 1)
            .await?;
        let sandbox = self.dlv_sandbox_of(task).await?;
        // A playground or an instance with no GitHub App can still park an answered task.
        // A known open PR requires verification, even when its installation is unavailable.
        let merged = record
            .filter(|r| past_merge(&r.stage))
            .and_then(|r| r.pr_number);
        if sandbox.is_playground() || !self.github_configured().await? {
            return Ok(
                (stored_open.is_empty() && observed.pr.is_none_or(|pr| !pr.open))
                    .then(ParkingPrs::default),
            );
        }
        let branch_pulls = async |token: &Secret| {
            let Some(branch) = task.branch.as_deref() else {
                return Ok(Vec::new());
            };
            let mut pulls = self
                .deps
                .github
                .list_pulls(token, &sandbox.repo_owner, &sandbox.repo_name, Some(branch))
                .await?;
            pulls.retain(|pull| {
                !pull.from_fork
                    && pull.head == branch
                    && is_open(pull)
                    && Some(pull.number) != merged
            });
            pulls.sort_unstable_by_key(|pull| pull.number);
            Ok::<_, AppError>(pulls)
        };
        // Branch-scoped discovery also finds PRs omitted by a partial sandbox overview
        // and new PRs opened externally while an older recorded delivery is being reconciled.
        let watched = tracked.filter(|_| purpose.watches_pending(self.config.ci.enabled));
        let discover = async {
            let token = self.dlv_installation_token(&sandbox).await?;
            let branch = branch_pulls(&token).await?;
            Ok::<_, AppError>((token, branch))
        };
        let inconclusive = |observed: &mut ParkedPullRead| {
            let latest = self.known_parked_pull(task, record);
            if latest.pr.is_some_and(|pr| pr.open) || observed.pr.is_none_or(|pr| !pr.open) {
                *observed = latest;
            }
            observed.unavailable = true;
            observed.pr.filter(|pr| !pr.open).map(|_| ParkingPrs {
                followup_unverified: true,
                ..ParkingPrs::default()
            })
        };
        let (token, branch) = if let Some(read) = discovery.as_ref() {
            let Some(found) = read.lookup(discover).await else {
                return Ok(inconclusive(observed));
            };
            found
        } else {
            discover.await?
        };
        let mut closed = std::collections::BTreeSet::new();
        if let Some(read) = discovery.as_mut() {
            read.listed(&branch, &closed);
        }
        if purpose != PrVerification::Display
            || observed.pr.is_none_or(|pr| {
                !pr.open
                    || (Some(pr.number) != tracked
                        && !stored_open.contains(&pr.number)
                        && branch.iter().all(|listed| listed.number != pr.number))
            })
        {
            *observed = ParkedPullRead::recorded(record);
        }
        observed.note_open_branch(&branch);
        if branch.len() > MAX_PULLS {
            return Ok(None);
        }
        let numbers: std::collections::BTreeSet<_> = branch
            .iter()
            .map(|p| p.number)
            .chain(tracked)
            .chain(stored_open.iter().copied())
            .collect();
        if numbers.len() > MAX_PULLS {
            return Ok(None);
        }
        if numbers.is_empty() {
            return Ok(Some(ParkingPrs::default()));
        }
        let checkout = self.verification_checkout(task, purpose).await?;
        let mut pulls = Vec::new();
        let mut verified = true;
        for number in numbers {
            let checks = if purpose == PrVerification::WatchPending && watched != Some(number) {
                PrVerification::Settled
            } else {
                purpose
            };
            // Recorded deliveries may originate on a different branch. An independent
            // follow-up identified only by its number must still belong to this task.
            let source = task.branch.as_deref().filter(|_| {
                stored_open.contains(&number)
                    && tracked != Some(number)
                    && branch.iter().all(|pr| pr.number != number)
            });
            let pull = self
                .pull_allows_parking(task, &sandbox, &token, (number, source), checks, observed)
                .await?;
            if observed
                .pr
                .is_some_and(|pr| !pr.open && pr.number == number)
            {
                closed.insert(number);
                if let Some(read) = discovery.as_mut() {
                    read.listed(&branch, &closed);
                }
            }
            let Some(pull) = pull.filter(|pull| pull.number == number) else {
                verified = false;
                break;
            };
            // A successor needs the current open deliveries. Old checks on a PR
            // already merged cannot block the verified replacement.
            if pull.merged && purpose != PrVerification::WatchPending {
                continue;
            }
            if let Some(head) = &checkout
                && !pull.from_fork
                && task.branch.as_ref() == Some(&pull.head)
                && head != &pull.head_sha
                && !self.verification_git(purpose, {
                    let repo = std::path::PathBuf::from(task.worktree_path.as_deref().ok_or(AppError::NotFound)?);
                    let (local, remote) = (head.clone(), pull.head_sha.clone());
                    move |app| async move { app.deps.git.is_ancestor(&repo, &local, &remote).await }
                }).await?
            {
                verified = false;
                break;
            }
            pulls.push(pull);
        }
        if !verified && purpose != PrVerification::Display {
            return Ok(None);
        }
        let mut facts = ParkingPrs {
            followup_unverified: false,
            checkout,
            pulls,
            watched,
        };
        let current = if let Some(read) = discovery
            .as_mut()
            .filter(|_| verified && closed.len() == branch.len())
        {
            let Some(current) = read.lookup(branch_pulls(&token)).await else {
                // Publish the directly confirmed merges before consulting concurrent observations.
                read.publish(&task.id, observed, None);
                return Ok(inconclusive(observed));
            };
            current
        } else {
            branch_pulls(&token).await?
        };
        if let Some(read) = discovery {
            read.listed(&current, &closed);
            if let Some(number) = observed.pr.filter(|pr| pr.open).map(|pr| pr.number)
                && stored_open.contains(&number)
                && observed.fresh_head
                && !observed.unavailable
                && branch.iter().all(|pr| pr.number != number)
                && let Some(open) = &mut read.open
                && !open.contains(&number)
            {
                open.push(number);
            }
        }
        if purpose != PrVerification::Display {
            return Ok((current == branch).then_some(facts));
        }
        // A title edit carries no eligibility evidence. Changes in membership,
        // head, base or mergeability do. Prefer a directly confirmed blocker
        // that still has the same facts over a newly listed, unchecked PR.
        let unchanged =
            |candidate: &GhPull| branch.iter().any(|prior| same_open_pull(prior, candidate));
        let changed = current.iter().rev().find(|candidate| !unchanged(candidate));
        if changed.is_some() || current.len() != branch.len() {
            // A recorded PR can be published on a different branch. In that
            // case its direct reads are independent of both branch listings.
            let survives = |number, head: Option<&str>| {
                if let Some(pr) = current.iter().find(|pr| pr.number == number) {
                    unchanged(pr) && head.is_none_or(|sha| sha == pr.head_sha)
                } else {
                    (tracked == Some(number) || stored_open.contains(&number))
                        && branch.iter().all(|pr| pr.number != number)
                }
            };
            let blocker_survived = !verified
                && observed
                    .pr
                    .is_some_and(|pr| pr.open && survives(pr.number, None));
            if !blocker_survived {
                observed.signal = None;
                // A newly listed PR has no check result yet. A durable ordinary
                // failure on a still-current, directly verified head takes priority.
                facts
                    .pulls
                    .retain(|pull| survives(pull.number, Some(&pull.head_sha)));
                if let Some((number, signal)) = self.parking_ci_blocker(task, &facts).await? {
                    observed.head = facts
                        .pulls
                        .iter()
                        .find(|p| p.number == number)
                        .map(|p| Arc::from(p.head_sha.as_str()));
                    observed.signal = Some(signal);
                    observed.pr =
                        Some(yulius_contracts::api::tasks::ParkedPr { number, open: true });
                    return Ok(None);
                }
                let known_open = changed
                    .filter(|pr| !closed.contains(&pr.number))
                    .or_else(|| current.iter().rev().find(|pr| !closed.contains(&pr.number)));
                if let Some(open) = known_open {
                    observed.fresh_head = true;
                    observed.head = Some(Arc::from(open.head_sha.as_str()));
                    observed.pr = Some(yulius_contracts::api::tasks::ParkedPr {
                        number: open.number,
                        open: true,
                    });
                } else if observed.pr.is_some_and(|pr| pr.open) {
                    *observed = ParkedPullRead::recorded(record);
                    if let Some(pr) = observed
                        .pr
                        .as_mut()
                        .filter(|pr| closed.contains(&pr.number))
                    {
                        pr.open = false;
                    }
                }
            }
            return Ok(None);
        }
        // A branch list can lag a direct get_pull confirmation of a merge.
        // Among successful reads only a PR confirmed open can replace it.
        if verified && let Some(pull) = facts.pulls.last() {
            observed.head = Some(Arc::from(pull.head_sha.as_str()));
            observed.pr = Some(yulius_contracts::api::tasks::ParkedPr {
                number: pull.number,
                open: true,
            });
        }
        Ok(verified.then_some(facts))
    }

    /// Read the commit only when the registered checkout still owns the task branch.
    async fn verification_checkout(
        &self,
        task: &TaskRecord,
        purpose: PrVerification,
    ) -> AppResult<Option<String>> {
        let (Some(path), Some(branch)) = (&task.worktree_path, &task.branch) else {
            return Ok(None);
        };
        let (path, branch) = (path.clone(), branch.clone());
        self.verification_git(purpose, move |app| async move {
            let path = std::fs::canonicalize(path).map_err(AppError::internal)?;
            let trees = app.deps.git.worktrees(&path).await?;
            let tree = trees.iter().find(|tree| tree.path == path).ok_or_else(|| {
                AppError::internal("task checkout missing from registered worktrees")
            })?;
            if tree.branch.as_ref() != Some(&branch) {
                return Ok(None);
            }
            let head = app
                .deps
                .git
                .resolve_commit(&path, "HEAD")
                .await?
                .ok_or_else(|| AppError::internal("unreadable task checkout HEAD"))?;
            Ok(Some(head))
        })
        .await
    }

    /// Check one PR and reconfirm its head and mergeability after the slower checks.
    async fn pull_allows_parking(
        &self,
        task: &TaskRecord,
        sandbox: &SandboxRecord,
        token: &Secret,
        (number, source): (u64, Option<&str>),
        purpose: PrVerification,
        observed: &mut ParkedPullRead,
    ) -> AppResult<Option<GhPull>> {
        use yulius_domain::ci::CiRef;
        let read = async || {
            if purpose == PrVerification::Display
                && crate::ports::github::current_purpose()
                    == crate::ports::github::GitHubPurpose::Display
                && let Some(pull) = self
                    .deps
                    .store
                    .terminal_pull(&task.id, Some(number))
                    .await?
            {
                return Ok(pull);
            }
            let pull = self
                .deps
                .github
                .get_pull(token, &sandbox.repo_owner, &sandbox.repo_name, number)
                .await?;
            if pull.number != number
                || (is_open(&pull)
                    && source.is_some_and(|branch| pull.from_fork || branch != pull.head))
            {
                return Err(AppError::Upstream(
                    "GitHub returned a different pull request or source branch".into(),
                ));
            }
            if !is_open(&pull) {
                self.deps
                    .store
                    .remember_pull_observation(&task.id, &pull)
                    .await?;
            }
            Ok::<_, AppError>(pull)
        };
        // Each unfinished or unreadable check belongs to the PR being verified.
        if observed.pr.is_none_or(|pr| pr.number != number) {
            observed.fresh_head = false;
            observed.head = None;
            observed.signal = None;
            observed.pr = Some(yulius_contracts::api::tasks::ParkedPr { number, open: true });
        }
        let pull = read().await?;
        observed.signal = None;
        observed.note_current(&pull);
        if !is_open(&pull) {
            return Ok(pull.merged.then_some(pull));
        }
        if pull.behind {
            if purpose == PrVerification::Display {
                let current = read().await?;
                observed.note_current(&current);
                if same_open_pull(&pull, &current) {
                    observed.signal = self
                        .parking_ci_signal(task, number, &pull.head_sha, None)
                        .await?
                        .or(Some(PullSignal::Behind));
                }
                return Ok(current.merged.then_some(current));
            }
            return Ok(None);
        }
        let gate = if self.config.ci.enabled {
            let sandbox = sandbox.clone();
            let sha = pull.head_sha.clone();
            self.verification_git(purpose, move |app| async move {
                let reference =
                    (purpose == PrVerification::Display).then_some(CiRef::PullRequest(number));
                Ok(app
                    .ci_gate_result_for_ref(&sandbox, &sha, reference.as_ref())
                    .await)
            })
            .await?
        } else {
            yulius_domain::ci::GateResult::NotConfigured
        };
        let head = match self
            .checks_with_gate(sandbox, token, &pull.head_sha, gate)
            .await
        {
            Ok(report) => Some(report),
            Err(_) if purpose == PrVerification::Display => None,
            Err(err) => return Err(err),
        };
        let profile = delivery_profile(sandbox);
        let checks = tested_checks_word(
            &profile,
            head.as_ref().map_or("unknown", |report| report.word),
        );
        let unreadable = head
            .as_ref()
            .is_none_or(|report| !report.unreadable.is_empty());
        let settled = match checks {
            "success" | "none" => true,
            "pending" if purpose.watches_pending(self.config.ci.enabled) => self
                .deps
                .store
                .ci_runs(&sandbox.id, crate::pr_watch::RUNS_READ)
                .await?
                .iter()
                .find(|run| run.reference == CiRef::PullRequest(number))
                .is_some_and(|run| run.sha == pull.head_sha && run.state.is_active()),
            _ => false,
        };
        if !settled || unreadable {
            if purpose != PrVerification::Display {
                return Ok(None);
            }
            let current = read().await?;
            observed.note_current(&current);
            if same_open_pull(&pull, &current) {
                let own = self
                    .parking_ci_signal(task, number, &pull.head_sha, None)
                    .await?;
                let external = match gate {
                    yulius_domain::ci::GateResult::Run(Some(_)) => None,
                    _ if checks == "failure" => Some(PullSignal::Failing),
                    _ if unreadable => {
                        observed.unavailable = true;
                        None
                    }
                    _ => PullSignal::from_wire("open", checks, false),
                };
                observed.signal = if external == Some(PullSignal::Failing) {
                    external
                } else {
                    own.or(external)
                };
            }
            return Ok(current.merged.then_some(current));
        }
        // The local merge-driver check uses the configured integration branch. For a
        // PR targeting another branch, GitHub computes mergeability against its real base.
        let mergeable = if pull.base == profile.integration_branch && task.worktree_path.is_some() {
            let task = task.clone();
            let sandbox = sandbox.clone();
            let sha = pull.head_sha.clone();
            self.verification_git(purpose, move |app| async move {
                Ok(app.local_conflict(&task, &sandbox, &sha).await)
            })
            .await?
            .map(|conflict| !conflict)
            .or(pull.mergeable)
        } else {
            pull.mergeable
        };
        if mergeable != Some(true) && purpose != PrVerification::Display {
            return Ok(None);
        }
        // Checks and local conflict detection can take long enough for another push.
        let current = read().await?;
        observed.note_current(&current);
        let same_open_head = same_open_pull(&pull, &current)
            && (purpose == PrVerification::Display || pull.base_sha == current.base_sha);
        if same_open_head && mergeable != Some(true) {
            let own = self
                .parking_ci_signal(task, number, &current.head_sha, None)
                .await?;
            observed.signal = own.or((mergeable == Some(false)).then_some(PullSignal::Conflicting));
            return Ok(None);
        }
        Ok((current.merged || same_open_head).then_some(current))
    }

    /// Local work and decisions must still be settled after GitHub replies.
    pub(crate) async fn read_parked_local_work(
        &self,
        task: &TaskRecord,
        requested: Option<&std::collections::HashSet<String>>,
        status: &mut crate::sandboxes::ParkedStatus,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let facts = &mut status.facts;
        facts.browser_request = match requested {
            Some(requests) => requests.contains(task.id.as_str()),
            None => self
                .browser_tab_requests(&task.sandbox_id)
                .await
                .contains(task.id.as_str()),
        } || store.task_needs_you(&task.id).await?.is_some();
        facts.staging_wait = store.task_staging_wait(&task.id).await?;
        facts.held_prompt = store.pending_hold(&task.id).await?.is_some();
        let steps = store.steps(&task.id).await?;
        facts.plan_open = crate::sandboxes::plan_open(&steps);
        status.steps = Some(steps);
        if !facts.parked_has_local_work_or_request() {
            facts.spec_unfinished = self.read_spec_unfinished(&task.id).await?;
            facts.goal_unfinished = self.goal_unfinished_of(task).await;
        }
        Ok(())
    }

    async fn parked_predecessor_done(&self, task: &TaskRecord) -> AppResult<bool> {
        let store = &self.deps.store;
        let record = store.delivery(&task.id).await?;
        let delivered = record
            .as_ref()
            .is_some_and(|r| r.stage == DeliveryStage::PrOpen || past_merge(&r.stage));
        if task.activity != Some(yulius_domain::task::Activity::Done) && !delivered {
            return Ok(false);
        }
        let mut status = crate::sandboxes::ParkedStatus::new(task);
        self.read_parked_local_work(task, None, &mut status).await?;
        if status.facts.parked_has_local_work_or_request() {
            return Ok(false);
        }
        crate::race::reached(
            crate::race::Point::ParkedPredecessorChecksPr,
            std::path::Path::new(task.id.as_str()),
        );
        let Some(prs) = self
            .verify_parking_prs(task, record.as_ref(), PrVerification::Settled)
            .await?
        else {
            return Ok(false);
        };
        self.parked_evidence_current(
            task,
            record.as_ref(),
            &prs,
            PrVerification::Settled,
            &mut ParkedPullRead::default(),
            &mut status,
        )
        .await
    }

    /// Reconfirm the checkout and owner decisions before the final durable CI
    /// and task reads. A resume can happen during a Git or browser lookup.
    pub(crate) async fn parked_evidence_current(
        &self,
        task: &TaskRecord,
        record: Option<&DeliveryRecord>,
        prs: &ParkingPrs,
        purpose: PrVerification,
        observed: &mut ParkedPullRead,
        status: &mut crate::sandboxes::ParkedStatus,
    ) -> AppResult<bool> {
        let store = &self.deps.store;
        let checkout_changed = !prs.pulls.is_empty()
            && self.verification_checkout(task, purpose).await? != prs.checkout;
        self.read_parked_local_work(task, None, status).await?;
        if checkout_changed || status.facts.parked_has_local_work_or_request() {
            return Ok(false);
        }
        if let Some((number, signal)) = self.parking_ci_blocker(task, prs).await? {
            observed.head = prs
                .pulls
                .iter()
                .find(|p| p.number == number)
                .map(|p| Arc::from(p.head_sha.as_str()));
            observed.signal = Some(signal);
            observed.pr = Some(yulius_contracts::api::tasks::ParkedPr { number, open: true });
            return Ok(false);
        }
        if store.delivery(&task.id).await?.as_ref() != record {
            return Ok(false);
        }
        // Browser startup and checkout verification can outlast an owner resume.
        Ok(store.task(&task.id).await?.is_some_and(|current| {
            current.status == task.status
                && current.activity == task.activity
                && current.branch == task.branch
                && current.worktree_path == task.worktree_path
        }))
    }

    /// An armed merge or an untold staging request still needs the task's session.
    /// A failed store read defers completion until the outstanding work can be checked.
    async fn delivery_will_wake_task(&self, task: &TaskId) -> bool {
        self.merge_is_armed(task)
            || !matches!(self.deps.store.task_staging_wait(task).await, Ok(None))
    }

    /// The part of [`App::task_is_done`] that needs no GitHub call: the turn ended, no background
    /// shell runs, and the goal is achieved or no declared work is left with the change merged.
    pub(crate) async fn task_is_done_locally(&self, task: &TaskRecord) -> bool {
        use yulius_domain::task::Activity;
        if !self.agent_idle(task) {
            return false;
        }
        // Yulius merges it and tells its agent, an achieved goal's too (SPEC-487 CHN-001).
        if self.delivery_will_wake_task(&task.id).await {
            return false;
        }
        if task.activity == Some(Activity::Done) {
            return true;
        }
        // One reading of its delivery for the whole decision: a merge recorded between two
        // readings would be "no news due" by the first and "merged" by the second.
        let record = self.deps.store.delivery(&task.id).await.ok().flatten();
        if self.pr_news_due_of(task, record.as_ref(), false).await {
            return false;
        }
        if self.work_left(&task.id).await {
            return false;
        }
        matches!(record, Some(record) if past_merge(&record.stage))
    }

    /// Read the task-local part of capacity eligibility. Its delivery is rechecked
    /// after the other live attempts have been read and before releasing the slot.
    pub(crate) async fn local_completion(
        &self,
        task: TaskRecord,
    ) -> Option<(TaskRecord, Option<DeliveryRecord>)> {
        use yulius_domain::task::Activity;
        if !self.agent_idle(&task) || self.delivery_will_wake_task(&task.id).await {
            return None;
        }
        crate::race::reached(
            crate::race::Point::CandidateCapacityRead,
            std::path::Path::new(task.id.as_str()),
        );
        let record = self.deps.store.delivery(&task.id).await.ok()?;
        if task.activity != Some(Activity::Done)
            && (!record.as_ref().is_some_and(|r| past_merge(&r.stage))
                || self.pr_news_due_of(&task, record.as_ref(), false).await
                || self.work_left(&task.id).await)
        {
            return None;
        }
        Some((task, record))
    }

    /// Keep the checked delivery stable until the capacity reservation is made.
    pub(crate) async fn hold_live_delivery(
        &self,
        task: &TaskRecord,
        record: Option<&DeliveryRecord>,
    ) -> Option<tokio::sync::OwnedRwLockReadGuard<()>> {
        let writer = self
            .runtime(&task.id)
            .arm_writer
            .clone()
            .try_read_owned()
            .ok()?;
        crate::race::reached(
            crate::race::Point::LiveDeliveryHeld,
            std::path::Path::new(task.id.as_str()),
        );
        let store = &self.deps.store;
        if self
            .local_candidate_blocks(task, record)
            .await
            .unwrap_or(true)
            || self.delivery_will_wake_task(&task.id).await
            || store.delivery(&task.id).await.ok()?.as_ref() != record
        {
            return None;
        }
        let now = store.task(&task.id).await.ok()??;
        (now.status == task.status
            && now.activity == task.activity
            && now.archived_at == task.archived_at
            && now.branch == task.branch
            && now.worktree_path == task.worktree_path)
            .then_some(writer)
    }

    /// Whether work the task declared is left (SPEC-491 DON-004): steps of its plan not
    /// finished, or a spec that does not say `done`. A plan that cannot be read counts as open.
    async fn work_left(&self, task: &TaskId) -> bool {
        match self.deps.store.steps(task).await {
            Ok(steps) if !crate::sandboxes::plan_open(&steps) => self.spec_unfinished(task).await,
            _ => true,
        }
    }

    /// Whether a task is Done (#35, the sidebar's rule; SPEC-001 TSK-012, TSK-014): its turn
    /// ended (the agent is idle, or the session was closed), no background shell of the agent is
    /// known to run, and either Claude Code recorded its goal as achieved (activity `done`) or
    /// no work it declared is left (its plan is complete and its spec does not say otherwise,
    /// SPEC-491 DON-004) and its pull request is merged, or open and tested (mergeable into
    /// its base, with checks green or none required: [`PullSignal::from_wire`], SPEC-491
    /// DON-001), and Yulius has no news of that pull request left for its agent
    /// ([`App::pr_news_due`]). Anything unknown (no pull request, GitHub unreachable) is not Done.
    pub(crate) async fn task_is_done(&self, task: &TaskRecord) -> bool {
        use yulius_domain::task::{Activity, TaskStatus};
        let running = task.status == TaskStatus::Running;
        // A parked task is an idle run whose session went to sleep (SPEC-512 PRK-004): nothing of
        // it goes on by itself, so no shell, armed merge or news keeps it from Done.
        let parked = task.status == TaskStatus::Parked;
        let turn_ended = ((running || parked) && task.activity.is_some_and(Activity::is_idle))
            || task.status == TaskStatus::Closed;
        if !turn_ended || (running && self.background_shells_of(&task.id).unwrap_or(0) > 0) {
            return false;
        }
        // Yulius merges it and tells its agent, an achieved goal's too (SPEC-487 CHN-001).
        if running && self.delivery_will_wake_task(&task.id).await {
            return false;
        }
        if parked {
            return self.parked_predecessor_done(task).await.unwrap_or(false);
        }
        if running && task.activity == Some(Activity::Done) {
            return true;
        }
        // One reading of its delivery for the whole decision: a merge recorded between two
        // readings would be "no news due" by the first and "merged" by the second.
        let Ok(record) = self.deps.store.delivery(&task.id).await else {
            return false;
        };
        // Yulius still has news of its pull request for its agent (an armed merge, a merge not
        // said yet): it goes on by itself, so it is not Done (SPEC-487 CHN-001, CHN-002).
        if self.pr_news_due_of(task, record.as_ref(), false).await {
            return false;
        }
        // A pull request makes a task Done only when no work it declared is left: its plan is
        // complete (#139, TSK-014) and its spec does not say otherwise (SPEC-491 DON-004).
        if self.work_left(&task.id).await {
            return false;
        }
        let Some(record) = record else {
            return false;
        };
        if past_merge(&record.stage) {
            return true;
        }
        if record.stage != DeliveryStage::PrOpen {
            return false;
        }
        let Ok(sandbox) = self.dlv_sandbox_of(task).await else {
            return false;
        };
        let Ok(token) = self.dlv_installation_token(&sandbox).await else {
            return false;
        };
        let profile = delivery_profile(&sandbox);
        match self
            .dlv_observe(
                &sandbox,
                task,
                &profile,
                ObserveAccess::Provided(Some(&token)),
            )
            .await
        {
            Ok(observed) => match observed.pr_state() {
                // Merged on GitHub before the record says so: its news is as due as then.
                PrState::Merged => !self.pr_news_due_of(task, Some(&record), true).await,
                PrState::Open => {
                    let Some(pull) = observed.pr.as_ref() else {
                        return false;
                    };
                    let word = if observed.checks_errored {
                        "error"
                    } else {
                        observed.checks.as_str()
                    };
                    // Tested, by the reading the sidebar and the counts have (DON-001).
                    PullSignal::from_wire(
                        "open",
                        tested_checks_word(&profile, word),
                        self.pull_conflicts(task, &sandbox, pull).await,
                    ) == Some(PullSignal::Tested)
                }
                PrState::Absent | PrState::Closed => false,
            },
            Err(_) => false,
        }
    }
}

/// A parked PR watcher can finish pending checks. A chained task needs the result.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrVerification {
    WatchPending,
    Settled,
    Display,
}

impl PrVerification {
    /// Only parking can hand an active ordinary CI run to the parked PR watcher.
    pub(super) fn watches_pending(self, ci_enabled: bool) -> bool {
        self == Self::WatchPending && ci_enabled
    }
}

/// Identity to show even when the checks are unfinished or unreadable.
#[derive(Default)]
pub(crate) struct ParkedPullRead {
    pub(crate) unavailable: bool,
    pub(crate) fresh_head: bool,
    pub(crate) head: Option<Arc<str>>,
    pub(crate) signal: Option<PullSignal>,
    pub(crate) pr: Option<yulius_contracts::api::tasks::ParkedPr>,
}

impl ParkedPullRead {
    fn note_open_branch(&mut self, branch: &[GhPull]) {
        if let Some(pull) = branch.last() {
            self.fresh_head = true;
            self.head = Some(Arc::from(pull.head_sha.as_str()));
            self.signal = None;
            self.pr = Some(yulius_contracts::api::tasks::ParkedPr {
                number: pull.number,
                open: true,
            });
        }
    }

    pub(crate) fn note_current(&mut self, pull: &GhPull) {
        let Some(pr) = self.pr.as_mut().filter(|pr| pr.number == pull.number) else {
            return;
        };
        self.head = Some(Arc::from(pull.head_sha.as_str()));
        self.fresh_head = true;
        pr.open = is_open(pull);
        if !pr.open {
            // A directly observed unmerged closure still needs delivery reconciliation.
            self.signal = Some(if pull.merged {
                PullSignal::Merged
            } else {
                PullSignal::InReview
            });
        }
    }

    pub(crate) fn recorded(record: Option<&DeliveryRecord>) -> Self {
        let pr = record.and_then(|r| {
            let open = r.stage == DeliveryStage::PrOpen;
            r.pr_number
                .filter(|_| open || past_merge(&r.stage))
                .map(|number| yulius_contracts::api::tasks::ParkedPr { number, open })
        });
        Self {
            pr,
            signal: pr.filter(|pr| !pr.open).map(|_| PullSignal::Merged),
            ..Self::default()
        }
    }
}

#[derive(Default)]
pub(crate) struct ParkingPrs {
    pub(crate) followup_unverified: bool,
    checkout: Option<String>,
    watched: Option<u64>,
    pulls: Vec<GhPull>,
}

/// A display read publishes one ordered observation of a merged task's branch. Its
/// direct PR checks refine the list, including when a later check hits the deadline.
pub(crate) struct FollowupDiscovery {
    read: Option<crate::sandboxes::OverviewRead>,
    merged: u64,
    deadline: tokio::time::Instant,
    pub(super) open: Option<Vec<u64>>,
}

impl FollowupDiscovery {
    pub(crate) fn new(
        read: Option<crate::sandboxes::OverviewRead>,
        merged: u64,
        deadline: tokio::time::Instant,
    ) -> Self {
        Self {
            read,
            merged,
            deadline,
            open: None,
        }
    }

    async fn lookup<T>(&self, read: impl std::future::Future<Output = AppResult<T>>) -> Option<T> {
        self.read.as_ref()?;
        // Reserve time for the local task and delivery recheck after an inconclusive list.
        let remaining = self
            .deadline
            .saturating_duration_since(tokio::time::Instant::now());
        tokio::time::timeout_at(self.deadline - remaining / 4, read)
            .await
            .ok()?
            .ok()
    }

    fn listed(&mut self, pulls: &[GhPull], closed: &std::collections::BTreeSet<u64>) {
        self.open = Some(
            pulls
                .iter()
                .map(|pr| pr.number)
                .filter(|n| !closed.contains(n))
                .collect(),
        );
    }

    pub(crate) fn publish(
        &mut self,
        task: &TaskId,
        observed: &ParkedPullRead,
        signal: Option<PullSignal>,
    ) {
        if let (Some(read), Some(open)) = (self.read.take(), self.open.take()) {
            let replacement = observed
                .pr
                .filter(|pr| pr.open && open.contains(&pr.number))
                .map(|pr| pr.number)
                .or_else(|| open.last().copied());
            if let (Some(number), Some(signal)) = (replacement, signal)
                && observed.pr.is_some_and(|pr| pr.number == number && pr.open)
            {
                read.verified(
                    task,
                    Some(crate::sandboxes::ObservedPull {
                        head: observed.head.clone(),
                        number,
                        open: true,
                        conflicts: signal == PullSignal::Conflicting,
                        signal: Some(signal),
                    }),
                );
            } else if let Some(number) = replacement
                && observed.fresh_head
                && observed.pr.is_some_and(|pr| pr.open && pr.number == number)
            {
                read.record_identity(
                    task,
                    crate::sandboxes::ObservedPull {
                        number,
                        open: true,
                        conflicts: false,
                        head: observed.head.clone(),
                        signal: Some(PullSignal::InReview),
                    },
                );
            } else {
                read.record_branch(task, self.merged, replacement);
            }
        }
    }
}
