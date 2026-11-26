//! The sandbox's Docker front: its mode, its upkeep at each tick, and the engine behind it.

use super::*;

/// A live holder's Docker front and what the upkeep keeps of it.
#[derive(Default)]
pub(super) struct FrontState {
    /// The front, once it serves (ADR-0158).
    pub(super) record: Option<DockerFrontRecord>,
    /// Mount namespaces of its runs: attempts to bind the Docker directory, or [`BIND_DONE`].
    pub(super) bound: HashMap<u64, u8>,
    /// What the journal said of its state (SPEC-494 DFJ-005).
    pub(super) watch: crate::front_gate::StateWatch,
    /// When it is restarted by itself (SPEC-494 DFJ-008).
    pub(super) repair: crate::front_repair::RepairWatch,
    /// A restart whose new front did not start: counted and journaled once one starts.
    pub(super) restart_pending: Option<crate::front_repair::Why>,
    /// When the journal last said that it runs an earlier build and stays busy.
    pub(super) outdated_said: Option<Instant>,
}

impl FrontState {
    /// The front's pid, when it has one.
    pub(super) fn pid(&self) -> u32 {
        self.record.as_ref().map_or(0, |f| f.pid)
    }

    /// Whether the front may run an engine.
    pub(super) fn serves_engine(&self) -> bool {
        self.record.as_ref().is_some_and(|f| f.mode != "refuse")
    }
}

impl Manager {
    /// Why sandboxes get no Docker engine (SPEC-196 DKL-005), `None` when they may (each one
    /// that has `dockerd` in its own system, SPEC-199): the setting, the cgroup delegation, and a
    /// `yulius` executable the sandbox's root neither owns nor may write through its group or as
    /// anyone (the engine's read-only binds are not locked, so an engine could remount and
    /// rewrite it; ADR-0151).
    #[must_use]
    pub fn docker_unavailable(&self) -> Option<String> {
        use std::os::unix::fs::MetadataExt as _;
        if let Err(reason) = &self.config.docker {
            return Some(reason.clone());
        }
        if self.config.delegation.is_none() {
            return Some(DOCKER_NO_DELEGATION.to_owned());
        }
        // Owned by the sandbox's root, or writable by its group (the service group) or anyone.
        let exe_writable = std::fs::metadata(&self.config.exe)
            .is_ok_and(|meta| meta.uid() == self.config.uid.host || meta.mode() & 0o022 != 0);
        exe_writable.then(|| DOCKER_WRITABLE_EXE.to_owned())
    }

    /// Whether sandboxes get a Docker engine ([`Manager::docker_unavailable`] says why not).
    #[must_use]
    pub fn docker(&self) -> bool {
        self.docker_unavailable().is_none()
    }

    /// Whether `sandbox`'s runs reach a Docker engine (SPEC-187 DKR-010). Every live holder gets
    /// a front whatever its age (SPEC-196 DKL-003), so this is the host's answer.
    pub async fn docker_for(&self, _sandbox: &str) -> bool {
        self.docker()
    }

    /// How `request`'s front serves its socket beside the holder of `record` (ADR-0158). Asked
    /// at every tick, it only reads paths: the sandbox's groups are prepared when an engine front
    /// starts ([`Manager::start_front`]).
    pub(super) fn front_mode(
        &self,
        request: &SandboxRequest,
        record: &HolderRecord,
    ) -> DockerServe {
        if let Some(reason) = self.docker_unavailable() {
            return DockerServe::Refuse { reason };
        }
        let base = self.bases.path(&record.base_id);
        if !system_has_dockerd(&base, &self.sysroot(&request.id).join("upper")) {
            return DockerServe::Refuse {
                reason: DOCKER_NO_DOCKERD.to_owned(),
            };
        }
        match self.group_paths(&request.id) {
            Some(groups) => DockerServe::Engine {
                engine: self.engine_spec(request, &groups.engine_root),
            },
            None => DockerServe::Refuse {
                reason: DOCKER_NO_DELEGATION.to_owned(),
            },
        }
    }

    /// The paths of `sandbox`'s groups, without preparing them; `None` without delegation.
    pub(super) fn group_paths(&self, sandbox: &str) -> Option<SandboxGroups> {
        Some(self.config.delegation.as_ref()?.sandbox_paths(sandbox))
    }

    /// Forwarded ports the front also serves on the engine's bridge: those of a holder whose
    /// forwards listen on loopback only (started before engines; SPEC-187 specs carry `engine`,
    /// newer ones `containers`).
    pub(super) fn bridge_forwards(&self, sandbox: &str) -> Vec<u16> {
        // A production holder forwards nothing (SPEC-279 PDE-001).
        if production_sandbox(sandbox).is_some() {
            return Vec::new();
        }
        let spec = std::fs::read(StatePaths::new(self.state_dir(sandbox)).spec())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        let containers = spec
            .as_ref()
            .is_some_and(|s| s["containers"].as_bool() == Some(true) || !s["engine"].is_null());
        if containers {
            Vec::new()
        } else {
            self.config.forwards.keys().copied().collect()
        }
    }

    /// Keeps `sandbox`'s front serving in the mode it should (SPEC-196 DKL-003): started when the
    /// holder has none or it ended, replaced when the mode changed. An engine's other settings
    /// change only with a new holder, so a running engine is never cut for them; but a front
    /// whose engine group is gone (the sandbox's groups moved to another unit, SPEC-458 LAY-005)
    /// is replaced, since it could start no engine again.
    pub(super) async fn keep_front(&self, sandbox: &str, state: &mut SandboxState) {
        let request = state
            .request
            .clone()
            .unwrap_or_else(|| self.adopted_request(sandbox));
        let Some(live) = state.live.as_mut() else {
            return;
        };
        let net_ns = live.record.net_ns;
        let current = live
            .front
            .record
            .as_ref()
            .map(|front| (front, crate::docker_front::is_front(front.pid, net_ns)));
        let wanted = self.front_mode(&request, &live.record);
        // Only an engine front that runs is asked for its group: one file read per tick.
        let engine_front = current.filter(|(front, running)| *running && front.mode == "engine");
        let paths = StatePaths::new(self.state_dir(sandbox));
        let moved = engine_front.is_some()
            && engine_group_moved(
                served_engine_group(&paths).as_deref(),
                &wanted,
                Path::exists,
            );
        let serve = match front_action(current, wanted, moved) {
            FrontAction::Keep => {
                // A restart of `yulius.service` prunes an idle engine's empty leaf, and a front
                // from an older binary does not make it again (SPEC-201 DBX-006). It also passes
                // `cpu` and `pids` down to the engine's root again, which the stop took back.
                if live
                    .front
                    .record
                    .as_ref()
                    .is_some_and(|f| f.mode == "engine")
                    && let Some(groups) = self.group_paths(sandbox)
                {
                    match crate::cgroup::restore_leaf(&groups.engine_root) {
                        Ok(true) => tracing::info!(sandbox, "recreated the engine's cgroup"),
                        Ok(false) => {}
                        Err(error) => {
                            tracing::warn!(sandbox, %error, "cannot recreate the engine's cgroup");
                        }
                    }
                }
                return;
            }
            FrontAction::Start(serve) => {
                if live.front.record.is_some() {
                    tracing::warn!(
                        sandbox,
                        "the sandbox's Docker front ended; starting it again"
                    );
                }
                serve
            }
            FrontAction::Replace(serve) => {
                tracing::info!(
                    sandbox,
                    to = serve.kind(),
                    "replacing the sandbox's Docker front"
                );
                let paths = StatePaths::new(self.state_dir(sandbox));
                let _ = tokio::task::spawn_blocking(move || {
                    crate::docker_front::stop(&paths, net_ns);
                })
                .await;
                serve
            }
        };
        live.front.record = None;
        let forwards = self.bridge_forwards(sandbox);
        match self
            .start_front(sandbox, &live.record, serve, forwards)
            .await
        {
            Ok(front) => {
                tracing::info!(sandbox, mode = %front.mode, pid = front.pid, "sandbox Docker front serving");
                live.front.watch.started(front.pid);
                // A new front starts its own count toward a repair (review round 2 of slice 3).
                live.front.repair = crate::front_repair::RepairWatch::default();
                if let Some(why) = live.front.restart_pending.take() {
                    self.docker_restarts.fetch_add(1, Ordering::Relaxed);
                    self.docker_event(sandbox, "restarted", 1, why.says().to_owned());
                    tracing::warn!(
                        sandbox,
                        why = why.says(),
                        "restarted the sandbox's Docker front, which started at a later tick"
                    );
                }
                live.front.record = Some(front);
            }
            Err(error) => {
                tracing::warn!(sandbox, %error, "cannot start the sandbox's Docker front");
            }
        }
    }

    /// Journals what grew in the state of `sandbox`'s front since the last tick, each kind at most
    /// once a minute (SPEC-494 DFJ-005), and reports a full or refusing front as the sandbox's
    /// Docker trouble (DFJ-006). A state file of an earlier front is ignored.
    /// Returns the state read, for the repair.
    pub(super) fn watch_front(
        &self,
        sandbox: &str,
        state: &mut SandboxState,
    ) -> Option<crate::front_repair::Reading> {
        let live = state.live.as_mut()?;
        let front = live.front.record.as_ref()?;
        let paths = StatePaths::new(self.state_dir(sandbox));
        let seen = crate::front_gate::read_state(&paths.docker_state(), front.pid)?;
        let grown = live.front.watch.observe(&seen, Instant::now());
        let gave_way = live.front.watch.new_gave_way(&seen);
        if gave_way > 0 {
            self.docker_event(sandbox, "gave_way", gave_way, String::new());
        }
        // A production holder's front (SPEC-279) serves CD deploys, which fail and say so; its
        // name is no sandbox the panel knows, so it is journaled only.
        let jammed = if production_sandbox(sandbox).is_none() {
            live.front.watch.report(&seen, sandbox, &self.config.health)
        } else {
            let jammed = live.front.watch.jammed(&seen);
            live.front.watch.counted(&seen);
            jammed
        };
        for grew in grown {
            tracing::warn!(
                sandbox,
                new = grew.new,
                total = grew.total,
                held = seen.held,
                max = seen.max,
                "the sandbox's Docker front: {}",
                grew.what
            );
        }
        Some(crate::front_repair::Reading {
            jammed,
            engine_running: seen.engine_running,
            engine_busy: seen.engine_busy,
            held: seen.held,
        })
    }

    /// Restarts `sandbox`'s front when a repair is due and the moment is right (SPEC-494
    /// DFJ-008, [`crate::front_repair`]), after a tick's `reading` of its state. The engine is
    /// pinged through the front only beside a client ([`crate::front_repair::should_ping`]). A
    /// production holder's front (SPEC-279) is journaled only: its one user is a production CD
    /// run, which a restart would cut.
    pub(super) async fn repair_front(
        &self,
        sandbox: &str,
        state: &mut SandboxState,
        reading: Option<crate::front_repair::Reading>,
    ) {
        let Some(reading) = reading else {
            // No state of this front (none written yet, or another front's): its count starts
            // over, so a successor is not restarted for its predecessor's jam.
            if let Some(live) = state.live.as_mut() {
                live.front.repair = crate::front_repair::RepairWatch::default();
            }
            return;
        };
        if production_sandbox(sandbox).is_some() {
            return;
        }
        let ping = if crate::front_repair::should_ping(&reading) {
            let socket = StatePaths::new(self.state_dir(sandbox)).docker_socket();
            crate::front_repair::ping(&socket).await
        } else {
            None
        };
        let ci_jobs = self
            .config
            .delegation
            .as_ref()
            .map_or(0, |delegation| delegation.ci_jobs_of(sandbox));
        let Some(live) = state.live.as_mut() else {
            return;
        };
        let now = Instant::now();
        live.front.repair.observe(reading.jammed, ping, now);
        if let Some(why) = live.front.repair.decide(ci_jobs, now) {
            let _ = self.restart_front(sandbox, state, why).await;
        }
    }

    /// Replaces `sandbox`'s front when it runs another build than this server (it was kept
    /// across an update) and is quiet: no connection held, no engine running (SPEC-494
    /// DFJ-010). A busy one is kept, said once a day.
    pub(super) async fn replace_outdated_front(&self, sandbox: &str, state: &mut SandboxState) {
        let Some(live) = state.live.as_mut() else {
            return;
        };
        let Some(front) = live.front.record.clone() else {
            return;
        };
        if crate::front_repair::runs_another_build(front.pid, &self.config.exe) != Some(true) {
            return;
        }
        let paths = StatePaths::new(self.state_dir(sandbox));
        let seen = crate::front_gate::read_state(&paths.docker_state(), front.pid);
        if !crate::front_repair::quiet(seen.as_ref(), front.pid, &paths.docker_socket()) {
            if live
                .front
                .outdated_said
                .is_none_or(|at| at.elapsed() >= crate::front_repair::STALE_SAID_EVERY)
            {
                live.front.outdated_said = Some(Instant::now());
                tracing::info!(
                    sandbox,
                    pid = front.pid,
                    "the sandbox's Docker front runs an earlier build; it is replaced once it is quiet"
                );
            }
            return;
        }
        let _ = self
            .restart_front(sandbox, state, crate::front_repair::Why::Outdated)
            .await;
    }

    /// Stops `sandbox`'s front (and its engine: running containers stop, volumes stay) and
    /// starts a new one at once, as was done by hand on 2026-11-15 (SPEC-494 DFJ-008).
    ///
    /// # Errors
    /// No live holder, or no front could be started again (the next tick tries again).
    async fn restart_front(
        &self,
        sandbox: &str,
        state: &mut SandboxState,
        why: crate::front_repair::Why,
    ) -> Result<(), String> {
        let live = state
            .live
            .as_mut()
            .ok_or("the sandbox has no running holder: its next run starts Docker afresh")?;
        let net_ns = live.record.net_ns;
        // Only a front that was stopped makes a restart: one that never ran is merely started.
        let stopped = live.front.record.is_some();
        let paths = StatePaths::new(self.state_dir(sandbox));
        let _ = tokio::task::spawn_blocking(move || {
            crate::docker_front::stop(&paths, net_ns);
        })
        .await;
        live.front.record = None;
        live.front.repair = crate::front_repair::RepairWatch::default();
        // This restart takes over one still pending: one new front, one count (review round 3).
        let stopped = stopped || live.front.restart_pending.take().is_some();
        self.keep_front(sandbox, state).await;
        if !stopped {
            // Nothing was stopped: a front that starts is no restart.
            return if state
                .live
                .as_ref()
                .is_some_and(|live| live.front.record.is_some())
            {
                Ok(())
            } else {
                Err("the sandbox's Docker front did not start; the upkeep tries again".into())
            };
        }
        if state
            .live
            .as_ref()
            .is_some_and(|live| live.front.record.is_some())
        {
            self.docker_restarts.fetch_add(1, Ordering::Relaxed);
            self.docker_event(sandbox, "restarted", 1, why.says().to_owned());
            if why == crate::front_repair::Why::Outdated {
                // Quiet when read: no engine ran; besides the relay front's limit (DFJ-010), only a
                // Docker call that came between that read and the new front's `ready` failed at
                // once (its socket missing or refused, or cut once it had connected, accepted or
                // still waiting) and can be run again (DFJ-010).
                tracing::info!(
                    sandbox,
                    why = why.says(),
                    "replaced the sandbox's Docker front with this build's"
                );
            } else {
                tracing::warn!(
                    sandbox,
                    why = why.says(),
                    "restarted the sandbox's Docker front: an engine it ran stopped with it (containers stopped, volumes stay) and starts at its next call"
                );
            }
            Ok(())
        } else {
            if let Some(live) = state.live.as_mut() {
                live.front.restart_pending = Some(why);
            }
            tracing::warn!(
                sandbox,
                why = why.says(),
                "stopped the sandbox's Docker front, which did not start again; the upkeep tries again"
            );
            Err("the sandbox's Docker front did not start again; the upkeep tries again".into())
        }
    }

    /// Restarts `sandbox`'s Docker front and engine now (the Superagent's
    /// `docker_restart`, SPEC-494 DFJ-009).
    ///
    /// # Errors
    /// Why it could not: no live holder, or the new front did not start.
    pub async fn restart_docker(&self, sandbox: &str) -> Result<(), String> {
        let entry = self.entry(sandbox);
        let mut state = entry.lock().await;
        self.restart_front(sandbox, &mut state, crate::front_repair::Why::Asked)
            .await
    }

    /// Keeps an event for the instance's measures, dropping the oldest past the bound.
    fn docker_event(&self, sandbox: &str, kind: &'static str, count: u64, detail: String) {
        let mut events = lock(&self.docker_events);
        if events.len() >= DOCKER_EVENTS_MAX {
            events.pop_front();
        }
        events.push_back(DockerEvent {
            sandbox: sandbox.to_owned(),
            kind,
            count,
            detail,
        });
    }

    /// The front events kept since the last call, oldest first.
    pub fn take_docker_events(&self) -> Vec<DockerEvent> {
        lock(&self.docker_events).drain(..).collect()
    }

    /// Fronts restarted since the server started, by repair or on request.
    #[must_use]
    pub fn docker_restarts(&self) -> u64 {
        self.docker_restarts.load(Ordering::Relaxed)
    }

    /// Starts `sandbox`'s front beside the holder of `record` and waits until it serves.
    async fn start_front(
        &self,
        sandbox: &str,
        record: &HolderRecord,
        serve: DockerServe,
        bridge_forwards: Vec<u16>,
    ) -> Result<DockerFrontRecord, String> {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        use tokio::io::AsyncBufReadExt as _;
        let paths = StatePaths::new(self.state_dir(sandbox));
        std::fs::create_dir_all(paths.docker_dir()).map_err(|e| format!("docker/: {e}"))?;
        // The sandbox's groups are prepared here, once per start: the engine's leaf must exist.
        let groups = self.cgroup_of(sandbox);
        let serve = match serve {
            DockerServe::Engine { .. } if groups.is_none() => DockerServe::Refuse {
                reason: DOCKER_NO_CGROUP.to_owned(),
            },
            serve => serve,
        };
        if matches!(serve, DockerServe::Engine { .. }) {
            // A holder from before engines has no storage yet (review round 1).
            let storage = self.sysroot(sandbox).join("docker");
            std::fs::create_dir_all(&storage).map_err(|e| format!("{}: {e}", storage.display()))?;
            // The production engine binds the production runs directory (SPEC-279 PDE-004).
            if let Some(of) = production_sandbox(sandbox) {
                let runs = production_runs_dir(&self.config.data_dir, of);
                std::fs::create_dir_all(&runs).map_err(|e| format!("{}: {e}", runs.display()))?;
            }
        }
        let _ =
            std::fs::set_permissions(paths.docker_dir(), std::fs::Permissions::from_mode(0o755));
        let reason = match &serve {
            DockerServe::Refuse { reason } => Some(reason.clone()),
            _ => None,
        };
        let mode = serve.kind();
        let spec = DockerFrontSpec {
            version: HOLDER_RECORD_VERSION,
            sandbox: sandbox.to_owned(),
            state_dir: paths.dir.to_string_lossy().into_owned(),
            holder_pid: record.inner_pid,
            user_ns: record.user_ns,
            net_ns: record.net_ns,
            mnt_ns: record.mnt_ns,
            cgroup: groups.map(|g| g.system.to_string_lossy().into_owned()),
            serve,
            bridge_forwards,
        };
        spec.validate().map_err(|e| e.to_string())?;
        write_json(&paths.docker_spec(), &spec).map_err(|e| e.to_string())?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(paths.docker_log())
            .map_err(|e| format!("docker-front.log: {e}"))?;
        // Detached like the holder: it outlives this server process.
        let mut child = tokio::process::Command::new(&self.config.exe)
            .args(["sandbox-host", "docker", "--spec"])
            .arg(paths.docker_spec())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .process_group(0)
            .kill_on_drop(false)
            .spawn()
            .map_err(|e| format!("sandbox-host docker: {e}"))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let line = tokio::time::timeout(
            FRONT_START_TIMEOUT,
            tokio::io::BufReader::new(stdout).lines().next_line(),
        )
        .await;
        let line = match line {
            Ok(Ok(Some(line))) => line,
            Ok(_) => String::from("it exited"),
            Err(_) => String::from("it did not report in time"),
        };
        if line.trim() != crate::docker_front::READY {
            let _ = child.kill().await;
            return Err(line.chars().take(2000).collect());
        }
        let pid = child.id().ok_or("the front exited")?;
        // Reaped while this server runs; it keeps running when the server stops.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        let front = DockerFrontRecord {
            pid,
            mode: mode.to_owned(),
            reason,
            started_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
        };
        write_json(&paths.docker_record(), &front).map_err(|e| e.to_string())?;
        Ok(front)
    }

    /// Moves the Docker directory into `sandbox`'s runs that started without it (SPEC-196
    /// DKL-004): their mount namespaces are planned here, and only those needing a bind go to
    /// `yulius sandbox-host bind-docker`, at most [`BIND_TRIES`] times each.
    pub(super) async fn bind_docker_dirs(&self, sandbox: &str, state: &mut SandboxState) {
        let Some(live) = state.live.as_mut() else {
            return;
        };
        if live.front.record.is_none() {
            return;
        }
        let (record, front) = (live.record.clone(), live.front.pid());
        // Without delegation there is no engine, so no container to leave out.
        let engine = self.group_paths(sandbox).map(|g| g.engine);
        let known = live.front.bound.clone();
        let planned = tokio::task::spawn_blocking(move || {
            crate::docker_bind::run_namespaces(
                record.net_ns,
                &[record.inner_pid, front],
                &[record.mnt_ns],
                engine.as_deref(),
            )
            .into_iter()
            .map(|(pid, mnt)| {
                // Done with or given up: no need to read its mounts again.
                let plan = if known.get(&mnt).is_some_and(|tries| *tries >= BIND_TRIES) {
                    crate::docker_bind::Plan::NotARun
                } else {
                    std::fs::read_to_string(format!("/proc/{pid}/mountinfo"))
                        .map_or(crate::docker_bind::Plan::NotARun, |t| {
                            crate::docker_bind::plan(&t)
                        })
                };
                (pid, mnt, plan)
            })
            .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let pids = bind_plan(&mut live.front.bound, &planned);
        if pids.is_empty() {
            return;
        }
        let paths = StatePaths::new(self.state_dir(sandbox));
        let mut command = tokio::process::Command::new(&self.config.exe);
        command
            .args(["sandbox-host", "bind-docker", "--spec"])
            .arg(paths.docker_spec());
        for (pid, _) in &pids {
            command.arg("--pid").arg(pid.to_string());
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = match command.spawn() {
            Ok(child) => tokio::time::timeout(BIND_TIMEOUT, child.wait_with_output()).await,
            Err(error) => {
                tracing::warn!(sandbox, %error, "cannot start bind-docker");
                return;
            }
        };
        let text = match output {
            Ok(Ok(output)) => {
                if !output.status.success() {
                    tracing::warn!(sandbox, error = %String::from_utf8_lossy(&output.stderr).trim(), "bind-docker failed");
                }
                String::from_utf8_lossy(&output.stdout).into_owned()
            }
            _ => String::new(),
        };
        for line in text.lines() {
            if line.starts_with("bound") {
                tracing::info!(sandbox, %line, "gave a running run the sandbox's Docker directory");
            } else if line.starts_with("failed") {
                tracing::warn!(sandbox, %line, "cannot give a run the sandbox's Docker directory");
            }
        }
        bind_record(&mut live.front.bound, &pids, &text);
    }

    /// How the holder of `request` starts its Docker engine, in `cgroup` (SPEC-187).
    pub(super) fn engine_spec(&self, request: &SandboxRequest, cgroup: &Path) -> EngineSpec {
        let state = StatePaths::new(self.state_dir(&request.id));
        let text = |path: &Path| path.to_string_lossy().into_owned();
        let view = crate::engine::EngineView {
            bwrap: text(&self.config.bwrap),
            exe: text(&self.config.exe),
            hostname: request.hostname.clone(),
            state_dir: text(&state.dir),
            system_root: text(&state.root()),
            data_dir: text(&self.config.data_dir),
            sandbox_dir: text(&request.dir),
            storage: text(&self.sysroot(&request.id).join("docker")),
            cgroup: text(cgroup),
        };
        let mut command = vec![
            text(&self.config.exe),
            "sandbox-exec".into(),
            "--engine".into(),
            "--cgroup".into(),
            text(cgroup),
            "--guard".into(),
            serde_json::to_string(&crate::engine::engine_guard(&view)).unwrap_or_default(),
            "--".into(),
        ];
        command.extend(crate::engine::bwrap_argv(&view));
        EngineSpec {
            command,
            cgroup: text(cgroup),
            idle_secs: self.config.idle.stop_after.as_secs().clamp(60, 86_400),
        }
    }

    /// The Docker front `sandbox`'s live holder has: its pid, mode and refusal reason.
    pub async fn docker_front(&self, sandbox: &str) -> Option<DockerFrontRecord> {
        let entry = self.entry(sandbox);
        let state = entry.lock().await;
        state.live.as_ref()?.front.record.clone()
    }

    /// Prunes `sandbox`'s Docker engine when it runs, through its own control socket: build cache
    /// and unused images (SPEC-187 DKR-006). The bytes credited are the engine's figure, at most
    /// the engine storage last measured (0 when unmeasured or not among the largest consumers, so
    /// the other caches are pruned as before): the engine controls its answer. Never through the sandbox's socket, which would
    /// start a stopped engine and reset its idle time, keeping it and the holder up for as long
    /// as the disk is under pressure. `None` without a live holder serving one, with the engine
    /// stopped, or when it could not prune.
    ///
    /// The sandbox's running production engine is pruned the same way (SPEC-279 PDE-006),
    /// credited up to the production files measured; the sum is returned.
    pub async fn prune_engine(&self, sandbox: &str) -> Option<u64> {
        let own = self.prune_one(sandbox, sandbox, "containers").await;
        let production = match production_sandbox(sandbox) {
            Some(_) => None,
            None => {
                let name = production_holder(sandbox);
                // Only a production holder this server knows (no empty entry is made).
                if lock(&self.sandboxes).contains_key(&name) {
                    self.prune_one(&name, sandbox, "production").await
                } else {
                    None
                }
            }
        };
        match (own, production) {
            (None, None) => None,
            (own, production) => Some(own.unwrap_or(0).saturating_add(production.unwrap_or(0))),
        }
    }

    /// The control socket of `sandbox`'s engine while its holder serves a running one.
    pub(super) async fn engine_socket(&self, sandbox: &str) -> Option<PathBuf> {
        let entry = self.entry(sandbox);
        let state = entry.lock().await;
        if !state.live.as_ref()?.front.serves_engine() {
            return None;
        }
        Some(
            StatePaths::new(self.state_dir(sandbox))
                .engine_control()
                .join(yulius_contracts::sandbox::ENGINE_SOCKET),
        )
    }

    /// What the networks of `sandbox`'s running engine take of its address pools (SPEC-497
    /// HBA-003). `None` with no running engine or no answer (an engine stopped for idleness
    /// refuses the connection: logged at debug, review round 1); never through the sandbox's
    /// socket, which would start a stopped engine.
    pub async fn engine_networks(&self, sandbox: &str) -> Option<crate::engine::Networks> {
        let socket = self.engine_socket(sandbox).await?;
        match crate::engine::networks(&socket).await {
            Ok(networks) => Some(networks),
            Err(error) => {
                tracing::debug!(sandbox, %error, "cannot count the Docker engine's networks");
                None
            }
        }
    }

    /// CI job `job` of `sandbox` ended (SPEC-504 CLB-004): its group goes (one that still holds
    /// a process stays for the sweep), and the containers and networks labelled with it are
    /// removed from the sandbox's running engine while its group holds no process. A stopped
    /// engine is not started for it: the sweep takes them once it runs again.
    pub async fn ci_job_ended(&self, sandbox: &str, job: &str) {
        let Some(delegation) = self.config.delegation.as_ref() else {
            return;
        };
        // Its ptyhost leaves the group a moment after it reported the job's end (review round 3).
        let deadline = Instant::now() + JOB_EXIT_WAIT;
        while delegation.ci_job_running(sandbox, job) == Some(true) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        delegation.end_ci_job_group(sandbox, job);
        if crate::cgroup::ci_job_group_name(job).is_none() {
            return;
        }
        // Only while its group holds no process, asked for each container and network: the
        // same job id may run again at once (a run queued again after a restart), and its new
        // attempt's must stay (review round 2). A group that still holds one waits for the
        // sweep.
        let delegation = delegation.clone();
        let name = sandbox.to_owned();
        let ended = move |job: &str| delegation.ci_job_running(&name, job) == Some(false);
        self.remove_ci_leftovers(
            sandbox,
            &crate::engine::Jobs::OneEnded(job, &ended),
            "job-end",
        )
        .await;
    }

    /// Removes from `sandbox`'s running engine what CI jobs that ended left (SPEC-504 CLB-005):
    /// the containers and networks labelled with a job whose group holds no process (gone or
    /// empty; one that cannot be read counts as running), and the job groups emptied long ago.
    /// Each upkeep pass, so first after a server starts.
    pub async fn sweep_ci_leftovers(&self, sandbox: &str) {
        let Some(delegation) = self.config.delegation.clone() else {
            return;
        };
        let name = sandbox.to_owned();
        let ended = move |job: &str| delegation.ci_job_running(&name, job) == Some(false);
        self.remove_ci_leftovers(sandbox, &crate::engine::Jobs::Ended(&ended), "sweep")
            .await;
        if let Some(delegation) = self.config.delegation.clone() {
            let sandbox = sandbox.to_owned();
            let _ =
                tokio::task::spawn_blocking(move || delegation.sweep_ci_job_groups(&sandbox)).await;
        }
    }

    /// [`crate::engine::remove_ci_leftovers`] on `sandbox`'s running engine, recorded for the
    /// instance's measures as found `when` (`job-end` or `sweep`).
    pub(super) async fn remove_ci_leftovers(
        &self,
        sandbox: &str,
        jobs: &crate::engine::Jobs<'_>,
        when: &str,
    ) {
        let Some(socket) = self.engine_socket(sandbox).await else {
            return;
        };
        let removal = crate::engine::remove_ci_leftovers(&socket, jobs).await;
        let removed = removal.removed;
        if !removed.is_empty() {
            tracing::info!(
                sandbox,
                containers = removed.containers,
                networks = removed.networks,
                when,
                "removed what CI jobs left in the sandbox's Docker engine"
            );
            // Counted also when a later call failed (review round 1).
            self.docker_event(
                sandbox,
                "ci_leftovers",
                u64::from(removed.containers),
                format!("{} {} {when}", removed.containers, removed.networks),
            );
        }
        if let Some(error) = removal.error {
            // A stopped engine refuses its control socket at every pass: not worth a warning.
            if removed.is_empty() {
                tracing::debug!(sandbox, %error, when, "cannot remove CI jobs' leftovers now");
            } else {
                tracing::warn!(sandbox, %error, when, "removed only part of what CI jobs left");
            }
        }
    }

    /// Removes the networks of `sandbox`'s running engine that no container used for 10
    /// minutes ([`crate::engine::prune_networks`]); how many.
    pub async fn prune_engine_networks(&self, sandbox: &str) -> Option<u32> {
        let socket = self.engine_socket(sandbox).await?;
        match crate::engine::prune_networks(&socket).await {
            Ok(removed) => {
                if removed > 0 {
                    tracing::info!(
                        sandbox,
                        removed,
                        "pruned the Docker engine's unused networks"
                    );
                }
                Some(removed)
            }
            Err(error) => {
                tracing::debug!(sandbox, %error, "cannot prune the Docker engine's networks");
                None
            }
        }
    }

    /// Prunes the running engine of the holder `name`, credited up to the consumer `label` of
    /// `measured_in`'s last measurement.
    async fn prune_one(&self, name: &str, measured_in: &str, label: &str) -> Option<u64> {
        let socket = {
            let entry = self.entry(name);
            let state = entry.lock().await;
            if !state.live.as_ref()?.front.serves_engine() {
                return None;
            }
            StatePaths::new(self.state_dir(name))
                .engine_control()
                .join(yulius_contracts::sandbox::ENGINE_SOCKET)
        };
        let measured = {
            let entry = self.entry(measured_in);
            let state = entry.lock().await;
            state.usage.as_ref().map_or(0, |(usage, _)| {
                usage
                    .largest
                    .iter()
                    .find(|(consumer, _)| consumer == label)
                    .map_or(0, |(_, bytes)| *bytes)
            })
        };
        let sandbox = name;
        match crate::engine::prune(&socket).await {
            Ok(reported) => {
                let bytes = reported.min(measured);
                tracing::info!(sandbox, freed = %human(bytes), "pruned the sandbox's Docker engine");
                Some(bytes)
            }
            Err(error) => {
                tracing::warn!(sandbox, %error, "cannot prune the sandbox's Docker engine");
                None
            }
        }
    }
}

/// What [`Manager::keep_front`] does with a sandbox's front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FrontAction {
    /// It serves as it should.
    Keep,
    /// None runs: start one.
    Start(DockerServe),
    /// One runs in another mode: stop it, then start this one.
    Replace(DockerServe),
}

/// The front decision (SPEC-196 DKL-003) from the current front (its record, whether it still
/// runs) and the wanted mode. The wanted mode needs no cgroup write, so a failed one never cuts
/// a running engine.
///
/// `moved` ([`engine_group_moved`]): the group an engine front starts its engine in is gone,
/// because the sandbox's groups moved to another unit (SPEC-458 LAY-005). The move ended the
/// engine, so replacing its front cuts nothing, and the next engine starts in the group of now.
pub(super) fn front_action(
    current: Option<(&DockerFrontRecord, bool)>,
    wanted: DockerServe,
    moved: bool,
) -> FrontAction {
    let running = current
        .filter(|(_, running)| *running)
        .map(|(front, _)| front);
    let reason = match &wanted {
        DockerServe::Refuse { reason } => Some(reason),
        _ => None,
    };
    match running {
        Some(front) if moved && front.mode == "engine" && wanted.kind() == "engine" => {
            FrontAction::Replace(wanted)
        }
        Some(front) if front.mode == wanted.kind() && front.reason.as_ref() == reason => {
            FrontAction::Keep
        }
        // `dockerd` gone from the sandbox's system for a moment (a package swap) never cuts a
        // running engine and its containers (SPEC-199); a new holder decides again.
        Some(front) if front.mode == "engine" && reason.is_some_and(|r| r == DOCKER_NO_DOCKERD) => {
            FrontAction::Keep
        }
        Some(_) => FrontAction::Replace(wanted),
        None => FrontAction::Start(wanted),
    }
}

/// The group the front of `paths` starts its engine in, from the spec it was started with;
/// `None` for a front that serves no engine of its own, or without a readable spec.
pub(super) fn served_engine_group(paths: &StatePaths) -> Option<String> {
    match read_json::<DockerFrontSpec>(&paths.docker_spec())
        .ok()?
        .serve
    {
        DockerServe::Engine { engine } => Some(engine.cgroup),
        _ => None,
    }
}

/// Whether a front that starts its engine in `served` does so in a group that is gone while the
/// sandbox's engine group is another one now (`wanted`): the sandbox's groups moved to another
/// unit (ADR-0298). A group that is still there (`exists`) may hold a running engine and is
/// never given up.
pub(super) fn engine_group_moved(
    served: Option<&str>,
    wanted: &DockerServe,
    exists: impl Fn(&Path) -> bool,
) -> bool {
    match (served, wanted) {
        (Some(served), DockerServe::Engine { engine }) => {
            served != engine.cgroup && !exists(Path::new(served))
        }
        _ => false,
    }
}

/// Which runs `bind-docker` should get now (SPEC-196 DKL-004), from `found` (one pid per mount
/// namespace, with its [`Plan`](crate::docker_bind::Plan)), updating `known`: namespaces that
/// ended are forgotten (their inode numbers are reused), those needing nothing are done, and one
/// is given up after [`BIND_TRIES`] attempts.
pub(super) fn bind_plan(
    known: &mut HashMap<u64, u8>,
    found: &[(u32, u64, crate::docker_bind::Plan)],
) -> Vec<(u32, u64)> {
    known.retain(|mnt, _| found.iter().any(|(_, m, _)| m == mnt));
    let mut pids = Vec::new();
    for (pid, mnt, plan) in found {
        if known.get(mnt).is_some_and(|tries| *tries >= BIND_TRIES) {
            continue;
        }
        if *plan == crate::docker_bind::Plan::Bind {
            pids.push((*pid, *mnt));
        } else {
            known.insert(*mnt, BIND_DONE);
        }
    }
    pids
}

/// Records the helper's `output` for the runs it was given: bound or skipped ones are done,
/// others (failed, or missing from the output) took one more attempt.
pub(super) fn bind_record(known: &mut HashMap<u64, u8>, tried: &[(u32, u64)], output: &str) {
    let outcomes: HashMap<u32, bool> = output
        .lines()
        .filter_map(crate::docker_bind::Outcome::parse)
        .collect();
    for (pid, mnt) in tried {
        let tries = known.entry(*mnt).or_insert(0);
        if outcomes.get(pid) == Some(&true) {
            *tries = BIND_DONE;
        } else {
            *tries = tries.saturating_add(1);
        }
    }
}
