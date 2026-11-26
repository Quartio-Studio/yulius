//! Environment runs: deploy, verify and their records.

use super::*;

impl App {
    // ---------------------------------------------------------------- environments

    /// Tasks for which `action` applies now. When none does but some task in the
    /// environment's stage has not reached it yet, that task's refusal is returned (for example
    /// an environment out of order). Otherwise the action is a standalone run (a redeploy, or
    /// no delivered change waiting), which changes no stage.
    async fn dlv_due_tasks(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        environment: &Environment,
        action: &DeliveryAction,
        by: Initiator,
    ) -> AppResult<Vec<(TaskRecord, DeliveryRecord, DeliveryStage)>> {
        let facts = merged_facts(false);
        let mut due = Vec::new();
        let mut refused = None;
        for (task, record) in self.dlv_sandbox_deliveries(sandbox).await? {
            match dlv::apply(&record.stage, action, by, profile, &facts) {
                Ok(next) => due.push((task, record, next)),
                Err(err) => {
                    if refused.is_none() && not_yet_reached(&record.stage, profile, environment) {
                        refused = Some((task.number, err));
                    }
                }
            }
        }
        if due.is_empty()
            && let Some((number, err)) = refused
        {
            return Err(match refusal(&err) {
                AppError::Conflict(message) => {
                    AppError::Conflict(format!("task #{number}: {message}"))
                }
                other => other,
            });
        }
        Ok(due)
    }

    fn dlv_environment_named<'p>(
        profile: &'p DeliveryProfile,
        name: &str,
    ) -> AppResult<&'p Environment> {
        profile
            .environment(name)
            .ok_or_else(|| refusal(&DeliveryError::UnknownEnvironment(clip_chars(name, 64))))
    }

    /// `POST /api/sandboxes/{sid}/delivery/environments/{env}/runs` (write). Requires
    /// `{"confirm": true}`. An environment with a command starts it in the background (see the
    /// module trust contract); one without a command records the person's confirmation.
    ///
    /// # Errors
    /// `Invalid` without confirmation or for unknown environments, `Conflict` when out of order
    /// or while another run of the sandbox is in progress.
    pub async fn start_environment_run(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sandbox_id: &str,
        environment: &str,
        req: &RunEnvironment,
    ) -> AppResult<EnvironmentRun> {
        self.dlv_start_environment_run(ctx, sandbox_id, environment, req, None)
            .await
    }

    /// [`App::start_environment_run`]; for a task's agent (`delivery_deploy`, `for_task`) the
    /// automatic promotion after the run uses the task's GitHub token (its owner's, else the
    /// App's, F 2026-10-11) instead of the caller's own.
    pub(crate) async fn dlv_start_environment_run(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sandbox_id: &str,
        environment: &str,
        req: &RunEnvironment,
        for_task: Option<&TaskRecord>,
    ) -> AppResult<EnvironmentRun> {
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.require_for(ctx, &sid, Action::Work).await?;
        // A playground has no deploy: its maker may be anyone signed in (SPEC-099).
        crate::playground::refuse_in_playground(&sandbox, "A deploy")?;
        let profile = delivery_profile(&sandbox);
        let env = Self::dlv_environment_named(&profile, environment)?.clone();
        let Some(command) = env.command.clone() else {
            return self
                .dlv_record_confirmation(ctx, &sandbox, &profile, &env, req.confirm)
                .await;
        };
        // The command of an environment that follows production is a production deploy: an
        // administrator's, with the project's slug typed, by the rule every production deploy
        // goes through (ADR-0110; SPEC-483 P8).
        if env.after == Stage::Production {
            yulius_domain::cd::may_deploy(
                yulius_domain::cd::CdStage::Prod,
                yulius_domain::cd::Requester::Person {
                    administrator: level.allows(Action::Administer),
                    confirmation: req.slug.as_deref(),
                },
                &sandbox.slug,
            )
            .map_err(|refused| match refused {
                yulius_domain::cd::CdRefusal::ConfirmationMismatch { .. } => {
                    AppError::Invalid(refused.to_string())
                }
                other => AppError::Forbidden(other.to_string()),
            })?;
        }
        if !req.confirm {
            return Err(refusal(&DeliveryError::ConfirmationRequired(
                env.name.clone(),
            )));
        }
        // As a CD run: none starts while an update is about to restart the server (SPEC-342).
        let permit = self.deploy_permit()?;
        let action = DeliveryAction::RunEnvironment {
            name: env.name.clone(),
            confirmed: true,
        };
        self.dlv_due_tasks(&sandbox, &profile, &env, &action, Initiator::User)
            .await?;
        let run = EnvironmentRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            environment: env.name.clone(),
            kind: EnvironmentRunKind::Command,
            requested_by: ctx.user.id.clone(),
            command: Some(command),
            commit: None,
            state: EnvironmentRunState::Running,
            output: String::new(),
            exit_code: None,
            started_at: self.now(),
            ended_at: None,
        };
        // Detached from the request from its record on, as a CD run (SPEC-348, ADR-0233): a
        // client that goes away mid-start leaves the run executing, or ended as an error, never
        // running with nothing executing it. The permit goes with it.
        let app = Arc::clone(self);
        let (user, for_task) = (ctx.user.clone(), for_task.cloned());
        let run = Self::run_detached(async move {
            let _permit = permit;
            app.dlv_record_and_execute_run(run, (sandbox, profile, env), user, for_task)
                .await
        })
        .await?;
        self.dlv_run_view(&run, None).await
    }

    /// Records the run and its audit entry, then starts its execution; a run whose audit entry
    /// could not be written ends as an error at once.
    async fn dlv_record_and_execute_run(
        self: &Arc<Self>,
        run: EnvironmentRunRecord,
        (sandbox, profile, env): (SandboxRecord, DeliveryProfile, Environment),
        user: UserRecord,
        for_task: Option<TaskRecord>,
    ) -> AppResult<EnvironmentRunRecord> {
        let command = run.command.as_deref().unwrap_or_default();
        self.deps
            .store
            .insert_environment_run(&run)
            .await
            .map_err(|err| match err {
                AppError::Conflict(_) => AppError::Conflict(
                    "another environment run is in progress in this project; wait for it to finish"
                        .into(),
                ),
                other => other,
            })?;
        // The audit entry is part of the trust contract: no entry, no run.
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &Self::dlv_actor_login(&user),
                "delivery.environment.run",
                Some(&format!("{}/{}", sandbox.slug, env.name)),
                Some(&format!("run {}; command: {command}", run.id)),
            )
            .await
        {
            let _ = self
                .deps
                .store
                .finish_environment_run(
                    &run.id,
                    EnvironmentRunState::Error,
                    "[yulius] not started: the audit entry could not be written\n",
                    None,
                    self.now(),
                )
                .await;
            return Err(err);
        }
        let app = Arc::clone(self);
        let run_id = run.id.clone();
        tokio::spawn(async move {
            app.dlv_execute_environment_run(sandbox, profile, env, run_id, user, for_task)
                .await;
        });
        Ok(run)
    }

    /// `POST /api/sandboxes/{sid}/delivery/environments/{env}/confirm` (write): a person
    /// confirms a manually verified environment (`{"confirm": true}` required).
    ///
    /// # Errors
    /// `Invalid` without confirmation or for command-verified environments, `Conflict` out of
    /// order.
    pub async fn confirm_environment(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        environment: &str,
        req: &RunEnvironment,
    ) -> AppResult<EnvironmentRun> {
        let sandbox = self
            .dlv_require_sandbox(ctx, sandbox_id, Action::Work)
            .await?;
        let profile = delivery_profile(&sandbox);
        let env = Self::dlv_environment_named(&profile, environment)?.clone();
        self.dlv_record_confirmation(ctx, &sandbox, &profile, &env, req.confirm)
            .await
    }

    async fn dlv_record_confirmation(
        &self,
        ctx: &AuthContext,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        env: &Environment,
        confirmed: bool,
    ) -> AppResult<EnvironmentRun> {
        if !confirmed {
            return Err(AppError::Invalid(format!(
                "confirming environment '{}' requires explicit confirmation",
                env.name
            )));
        }
        if env.verify != Verify::Manual {
            return Err(refusal(&DeliveryError::WrongEvidence {
                name: env.name.clone(),
                verify: "its command",
            }));
        }
        let action = DeliveryAction::CompleteEnvironment {
            name: env.name.clone(),
            evidence: EnvironmentEvidence::UserConfirmed,
        };
        let due = self
            .dlv_due_tasks(sandbox, profile, env, &action, Initiator::User)
            .await?;
        let login = Self::dlv_actor_login(&ctx.user);
        let now = self.now();
        let moved = self.dlv_advance(due).await?;
        let mut output = format!(
            "Environment '{}' confirmed by {login} at {}.\n",
            env.name,
            rfc3339(now)
        );
        if moved.is_empty() {
            output.push_str("No delivered task was waiting for this environment.\n");
        } else {
            let numbers: Vec<String> = moved.iter().map(|n| format!("#{n}")).collect();
            output.push_str(&format!("Tasks advanced: {}.\n", numbers.join(", ")));
        }
        if env.after == Stage::Integration
            && let Ok(token) = self.user_github_token(&ctx.user).await
            && let Some(note) = self
                .dlv_auto_promote_if_enabled(sandbox, profile, &ctx.user, &token, false)
                .await
        {
            output.push_str(&note);
            output.push('\n');
        }
        let run = EnvironmentRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            environment: env.name.clone(),
            kind: EnvironmentRunKind::Manual,
            requested_by: ctx.user.id.clone(),
            command: None,
            commit: None,
            state: EnvironmentRunState::Succeeded,
            output,
            exit_code: None,
            started_at: now,
            ended_at: Some(now),
        };
        self.deps.store.insert_environment_run(&run).await?;
        self.dlv_audit(
            &login,
            "delivery.environment.confirm",
            &format!("{}/{}", sandbox.slug, env.name),
            &format!("run {}; tasks advanced: {moved:?}", run.id),
        )
        .await;
        self.dlv_run_view(&run, None).await
    }

    /// Background part of a command run: fetch, check out the stage's branch head, run the
    /// command, record the outcome and — for command-verified environments that succeeded —
    /// advance the waiting tasks (and auto-promote when enabled, with `for_task`'s GitHub token
    /// when a task's agent started the run, else `user`'s).
    async fn dlv_execute_environment_run(
        self: Arc<Self>,
        sandbox: SandboxRecord,
        profile: DeliveryProfile,
        env: Environment,
        run_id: String,
        user: UserRecord,
        for_task: Option<TaskRecord>,
    ) {
        let mut ring = OutputRing::new(ENVIRONMENT_OUTPUT_BYTES);
        let (tx, mut rx) = watch::channel(String::new());
        let store = self.deps.store.clone();
        let writer_id = run_id.clone();
        let writer = tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let text = rx.borrow_and_update().clone();
                if let Err(err) = store.update_environment_run(&writer_id, &text, None).await {
                    tracing::warn!(error = %err, "storing environment run output failed");
                }
            }
        });
        let outcome = self
            .dlv_environment_steps(&sandbox, &profile, &env, &run_id, &mut ring, &tx)
            .await;
        drop(tx);
        let _ = writer.await;
        let (state, exit_code) = match &outcome {
            Ok(end) if end.succeeded() => (EnvironmentRunState::Succeeded, Some(0)),
            Ok(end) => {
                ring.push_str(&match end {
                    CommandEnd::Exited(code) => format!("\n[yulius] exited with status {code}\n"),
                    CommandEnd::Signaled(signal) => format!("\n[yulius] terminated by signal {signal}\n"),
                    CommandEnd::TimedOut => format!(
                        "\n[yulius] the command exceeded {} minutes; its process group was killed\n",
                        self.config.ci.job_timeout.as_secs() / 60
                    ),
                    CommandEnd::SpawnFailed(message) => format!("\n[yulius] could not start: {message}\n"),
                });
                let state = if matches!(end, CommandEnd::SpawnFailed(_)) {
                    EnvironmentRunState::Error
                } else {
                    EnvironmentRunState::Failed
                };
                (state, end.exit_code())
            }
            Err(message) => {
                ring.push_str(&format!("\n[yulius] {message}\n"));
                (EnvironmentRunState::Error, None)
            }
        };
        let mut advanced = Vec::new();
        let mut note = None;
        if state == EnvironmentRunState::Succeeded && env.verify == Verify::Command {
            let action = DeliveryAction::CompleteEnvironment {
                name: env.name.clone(),
                evidence: EnvironmentEvidence::CommandSucceeded,
            };
            match self
                .dlv_due_tasks(&sandbox, &profile, &env, &action, Initiator::Policy)
                .await
            {
                Ok(due) => advanced = self.dlv_advance(due).await.unwrap_or_default(),
                Err(err) => note = Some(err.public_message()),
            }
            if env.after == Stage::Integration {
                let token = match &for_task {
                    Some(task) => self.task_github_token(task, &sandbox).await,
                    None => self.user_github_token(&user).await,
                };
                if let Ok(token) = token {
                    note = self
                        .dlv_auto_promote_if_enabled(&sandbox, &profile, &user, &token, false)
                        .await
                        .or(note);
                }
            }
        }
        if !advanced.is_empty() {
            let numbers: Vec<String> = advanced.iter().map(|n| format!("#{n}")).collect();
            ring.push_str(&format!(
                "[yulius] tasks advanced: {}\n",
                numbers.join(", ")
            ));
        }
        if let Some(note) = note {
            ring.push_str(&format!("[yulius] {note}\n"));
        }
        let finished = self
            .deps
            .store
            .finish_environment_run(&run_id, state, &ring.text(), exit_code, self.now())
            .await;
        if let Err(err) = finished {
            tracing::error!(error = %err, run = %run_id, "recording the environment run outcome failed");
        }
        self.dlv_audit(
            &Self::dlv_actor_login(&user),
            "delivery.environment.finished",
            &format!("{}/{}", sandbox.slug, env.name),
            &format!("run {run_id}: {} (exit {exit_code:?})", state.as_str()),
        )
        .await;
        // Once its end is recorded (SPEC-279 PDE-006, review round 2).
        if env.after == Stage::Production {
            self.after_production_run(&sandbox).await;
        }
    }

    async fn dlv_environment_steps(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        env: &Environment,
        run_id: &str,
        ring: &mut OutputRing,
        tx: &watch::Sender<String>,
    ) -> Result<CommandEnd, String> {
        let command = env
            .command
            .clone()
            .ok_or_else(|| format!("environment '{}' has no command", env.name))?;
        let branch = match env.after {
            Stage::Integration => &profile.integration_branch,
            Stage::Production => &profile.production_branch,
        };
        // Nothing of the run is read from the sandbox clone, whose configuration its agents
        // write (#656): the head comes from origin into Yulius's own mirror.
        ring.push_str(&format!(
            "[yulius] fetching {branch} of {}/{}\n",
            sandbox.repo_owner, sandbox.repo_name
        ));
        let _ = tx.send(ring.text());
        let head = self
            .environment_head(sandbox, branch)
            .await
            .map_err(|err| format!("fetching {branch} failed: {}", err.public_message()))?;
        if !is_hex_sha(&head) {
            return Err(format!("branch {branch} has no readable head commit"));
        }
        ring.push_str(&format!(
            "[yulius] running at {head} of {branch}, in the run's own directory\n"
        ));
        let _ = self
            .deps
            .store
            .update_environment_run(run_id, &ring.text(), Some(&head))
            .await;
        let vars = vec![
            ("YULIUS_ENVIRONMENT".into(), env.name.clone()),
            ("YULIUS_BRANCH".into(), branch.clone()),
            ("YULIUS_COMMIT".into(), head.clone()),
        ];
        ring.push_str(&format!("[yulius] $ {command}\n"));
        let _ = tx.send(ring.text());
        self.environment_command(sandbox, env.after, (run_id, &head), &command, vars, ring)
            .await
    }

    async fn dlv_run_view(
        &self,
        run: &EnvironmentRunRecord,
        output_tail: Option<usize>,
    ) -> AppResult<EnvironmentRun> {
        Ok(EnvironmentRun {
            run_id: run.id.clone(),
            environment: run.environment.clone(),
            kind: run.kind.as_str().to_owned(),
            state: run.state.as_str().to_owned(),
            command: run.command.clone(),
            commit: run.commit.clone(),
            requested_by: self.login_of(&run.requested_by).await,
            output: match output_tail {
                Some(max) => tail_bytes(&run.output, max).to_owned(),
                None => run.output.clone(),
            },
            exit_code: run.exit_code,
            started_at: rfc3339(run.started_at),
            ended_at: run.ended_at.map(rfc3339),
        })
    }

    /// A `running` row older than every run limit has no runner any more: record it as
    /// `error` (reads, and the update's wait for running deploys, SPEC-348).
    pub(crate) async fn dlv_expire_lost_run(
        &self,
        run: EnvironmentRunRecord,
    ) -> AppResult<EnvironmentRunRecord> {
        let limit = i64::try_from((self.config.ci.job_timeout + PREPARE_ALLOWANCE).as_millis())
            .unwrap_or(i64::MAX)
            .saturating_add(LOST_RUN_GRACE_MS);
        if run.state != EnvironmentRunState::Running
            || self.now().millis_since(run.started_at) <= limit
        {
            return Ok(run);
        }
        self.environment_end_lost(&run.sandbox_id, &run.id).await;
        let output = format!(
            "{}\n[yulius] no runner is attached to this run any more (server restart?); its outcome is unknown\n",
            run.output
        );
        let now = self.now();
        self.deps
            .store
            .finish_environment_run(&run.id, EnvironmentRunState::Error, &output, None, now)
            .await?;
        Ok(EnvironmentRunRecord {
            state: EnvironmentRunState::Error,
            output,
            ended_at: Some(now),
            ..run
        })
    }

    /// `GET /api/sandboxes/{sid}/delivery/runs/{run_id}` (read).
    ///
    /// # Errors
    /// `NotFound` for unknown runs or other sandboxes' runs.
    pub async fn environment_run(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        run_id: &str,
    ) -> AppResult<EnvironmentRun> {
        let sandbox = self
            .dlv_require_sandbox(ctx, sandbox_id, Action::View)
            .await?;
        let run = self
            .deps
            .store
            .environment_run(run_id)
            .await?
            .filter(|run| run.sandbox_id == sandbox.id)
            .ok_or(AppError::NotFound)?;
        let run = self.dlv_expire_lost_run(run).await?;
        self.dlv_run_view(&run, None).await
    }

    /// `GET /api/sandboxes/{sid}/delivery/runs` (read): recent runs with output tails.
    ///
    /// # Errors
    /// `NotFound` without read access.
    pub async fn environment_runs(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
    ) -> AppResult<Vec<EnvironmentRun>> {
        let sandbox = self
            .dlv_require_sandbox(ctx, sandbox_id, Action::View)
            .await?;
        let mut out = Vec::new();
        for run in self
            .deps
            .store
            .environment_runs(&sandbox.id, RUN_LIST_LIMIT)
            .await?
        {
            let run = self.dlv_expire_lost_run(run).await?;
            out.push(self.dlv_run_view(&run, Some(RUN_LIST_OUTPUT_BYTES)).await?);
        }
        Ok(out)
    }

    /// Startup reconciliation: runs recorded as `running` have no runner after a restart: they
    /// are recorded as `error`, then their process hosts are terminated. Recorded first, so a
    /// watcher still following the run (never after a real restart) cannot take its end while
    /// the host goes; a run whose record fails is still ended. Returns how many were closed. Call
    /// once before serving.
    ///
    /// # Errors
    /// Store failures.
    pub async fn reconcile_environment_runs(&self) -> AppResult<u32> {
        let mut closed = 0;
        for run in self.deps.store.running_environment_runs().await? {
            let output = format!(
                "{}\n[yulius] interrupted by a server restart; the command may have been killed or may have finished unobserved\n",
                run.output
            );
            let recorded = self
                .deps
                .store
                .finish_environment_run(
                    &run.id,
                    EnvironmentRunState::Error,
                    &output,
                    None,
                    self.now(),
                )
                .await;
            self.environment_end_lost(&run.sandbox_id, &run.id).await;
            if recorded? {
                closed += 1;
            }
        }
        Ok(closed)
    }
}
