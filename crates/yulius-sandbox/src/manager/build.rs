//! Building a new sandbox base when the host's packages change.

use super::*;

impl Manager {
    pub(super) fn maybe_build(self: &Arc<Self>, in_use: Vec<String>) {
        if !self.config.auto_build || self.building.load(Ordering::SeqCst) {
            return;
        }
        let current = self.bases.current();
        let base_lacks_dockerd = current.as_ref().is_some_and(|base| {
            crate::docker_status::has_dockerd(Path::new("/"))
                && !crate::docker_status::has_dockerd(&self.bases.path(&base.id))
        });
        let fingerprint = {
            let mut build = lock(&self.build);
            let due = fingerprint_due(
                build.checked.map(|at| at.elapsed()),
                current.is_some(),
                base_lacks_dockerd,
            );
            if !due {
                return;
            }
            build.checked = Some(Instant::now());
            crate::base::fingerprint(Path::new("/"), &self.config.sudo_shim)
        };
        let wanted = match &current {
            None => true,
            Some(base) if base.fingerprint == fingerprint => {
                let mut build = lock(&self.build);
                build.pending = None;
                build.pending_at_ms = None;
                false
            }
            Some(_) => {
                let mut build = lock(&self.build);
                match &build.pending {
                    Some((seen, since)) if *seen == fingerprint => {
                        since.elapsed() >= REBUILD_DEBOUNCE
                    }
                    _ => {
                        build.pending = Some((fingerprint, Instant::now()));
                        build.pending_at_ms = Some(epoch_ms());
                        // Check again after the debounce instead of waiting a full interval.
                        build.checked =
                            Instant::now().checked_sub(FINGERPRINT_EVERY - REBUILD_DEBOUNCE);
                        false
                    }
                }
            }
        };
        if wanted {
            self.spawn_build(in_use);
        }
    }

    /// Starts a base build as a child process (when none runs).
    pub fn spawn_build(self: &Arc<Self>, in_use: Vec<String>) {
        if self.building.swap(true, Ordering::SeqCst) {
            return;
        }
        lock(&self.build).building_since_ms = Some(epoch_ms());
        let started = Instant::now();
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            tracing::info!("building the sandbox base (a copy of the host system)");
            let mut command = tokio::process::Command::new(&manager.config.exe);
            command
                .args(["sandbox-base", "build", "--data-dir"])
                .arg(&manager.config.data_dir);
            if !in_use.is_empty() {
                command.arg("--keep").arg(in_use.join(","));
            }
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .process_group(0)
                .kill_on_drop(true);
            let result = match command.spawn() {
                Ok(child) => {
                    match tokio::time::timeout(BUILD_TIMEOUT, child.wait_with_output()).await {
                        Ok(Ok(output)) if output.status.success() => Ok(()),
                        Ok(Ok(output)) => Err(String::from_utf8_lossy(&output.stderr)
                            .trim()
                            .chars()
                            .take(2000)
                            .collect()),
                        Ok(Err(error)) => Err(error.to_string()),
                        Err(_) => Err("the base build timed out".to_owned()),
                    }
                }
                Err(error) => Err(error.to_string()),
            };
            match &result {
                Ok(()) => {
                    tracing::info!(base = ?manager.bases.current().map(|b| b.id), "sandbox base ready")
                }
                Err(error) => tracing::warn!(%error, "sandbox base build failed"),
            }
            let mut build = lock(&manager.build);
            if result.is_ok() {
                build.last_build = Some(started.elapsed());
            }
            build.last_error = result.err();
            build.pending = None;
            build.pending_at_ms = None;
            build.building_since_ms = None;
            drop(build);
            manager.building.store(false, Ordering::SeqCst);
        });
    }
}
