//! A sandbox with CD environments, its container holders and a process host that records containment.

use crate::support;
use crate::support::BareRemote;
use crate::support::git;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use support::{
    Creds, HarnessOptions, call, configure_github_app, harness_with, perms, seed_github_user,
    seed_sandbox,
};
use yulius::wiring::session::PtySessionHost;
use yulius_app::ci::cd::CdRunRecord;
use yulius_app::ports::store_ext::CdStore as _;
use yulius_app::ports::vault::{EnvFiles, VaultStore as _};
use yulius_domain::cd::CdStage;
use yulius_domain::ci::RunState;
use yulius_domain::ids::SandboxId;

pub(crate) type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

pub(crate) type TestResult = BoxResult<()>;

pub(crate) const WAIT: Duration = Duration::from_secs(60);

pub(crate) const VALUE: &str = "dev-deploy-token-9q2w7e5r";

/// The deploy script: proves what it got (the secret's value, `YULIUS_CD_*`, no task token) and
/// leaves a mark the verify script checks. It prints the value, which the stored output redacts.
pub(crate) const DEPLOY: &str = r#"#!/bin/sh
echo "deploying $YULIUS_CD_ENV $YULIUS_CD_BRANCH $YULIUS_CD_SHA"
if [ "$DEV_TOKEN" = "dev-deploy-token-9q2w7e5r" ]; then echo token-ok; fi
echo "the token is $DEV_TOKEN"
if env | grep -q -E "YULIUS_TASK_TOKEN|GITHUB_TOKEN|CLAUDE_CODE_OAUTH_TOKEN|ANTHROPIC"; then exit 9; fi
echo clean-env
if [ -f fail ]; then exit 3; fi
if [ -f slow ]; then sleep 30; fi
echo "$YULIUS_CD_SHA" > "$MARKS/$YULIUS_CD_ENV"
echo "home $HOME"
"#;

const VERIFY: &str = r#"#!/bin/sh
test "$(cat "$MARKS/$YULIUS_CD_ENV")" = "$YULIUS_CD_SHA" && echo verified
"#;

pub(crate) struct Fixture {
    pub(crate) h: support::Harness,
    pub(crate) alice: Creds,
    pub(crate) bob: Creds,
    pub(crate) origin: PathBuf,
    pub(crate) work: PathBuf,
    pub(crate) marks: PathBuf,
    pub(crate) secret: String,
}

/// `yulius.toml` with CD tables `cd` (the marks directory replaces `{marks}`).
pub(crate) async fn fixture(cd: &str) -> BoxResult<Fixture> {
    fixture_with(cd, None).await
}

/// [`fixture`]; with `host`, runs are contained (`config.contained`) and go to that process host.
pub(crate) async fn fixture_with(
    cd: &str,
    host: Option<Arc<dyn yulius_app::ports::session::SessionHost>>,
) -> BoxResult<Fixture> {
    fixture_configured(cd, host, Box::new(|_| {})).await
}

/// [`fixture_with`], then `configure` adjusts the configuration.
pub(crate) async fn fixture_configured(
    cd: &str,
    host: Option<Arc<dyn yulius_app::ports::session::SessionHost>>,
    configure: support::ConfigureFn,
) -> BoxResult<Fixture> {
    let contained = host.is_some();
    // A short base: run sockets live at <data>/runs/cd-<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("ycd").tempdir_in("/tmp")?;
    let root = dir.path().canonicalize()?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let _ = Command::new(&exe).arg("version").output();
    let h = harness_with(HarnessOptions {
        session_host: Some(host.unwrap_or_else(|| Arc::new(PtySessionHost::new(exe)))),
        git: Some(Arc::new(crate::support::confined_git())),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.ci.job_timeout = Duration::from_secs(20);
            config.contained = contained;
            configure(config);
        })),
        ..HarnessOptions::default()
    })?;
    let data = h.app.config.data_dir.clone();
    support::setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    h.github.set_perm("bob", "acme/app", perms("admin"));
    seed_sandbox(&h, "s1", "app", "u-alice").await?;

    let marks = root.join("marks");
    std::fs::create_dir_all(&marks)?;
    let origin = root.join("origin.git");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q", "-b", "main"])?;
    let work = root.join("work");
    git(
        &root,
        &["clone", "-q", origin.to_str().ok_or("path")?, "work"],
    )?;
    std::fs::create_dir_all(work.join("scripts/cd"))?;
    std::fs::write(work.join("scripts/cd/deploy.sh"), DEPLOY)?;
    std::fs::write(work.join("scripts/cd/verify.sh"), VERIFY)?;
    let marks_text = marks.to_str().ok_or("path")?;
    std::fs::write(work.join("yulius.toml"), cd.replace("{marks}", marks_text))?;
    git(&work, &["add", "-A"])?;
    git(&work, &["commit", "-q", "-m", "init"])?;
    git(&work, &["push", "-q", "origin", "HEAD:main", "HEAD:dev"])?;
    h.github.set_remote(Arc::new(BareRemote(origin.clone())));
    let repo = data.join("sandboxes/s1/repo");
    std::fs::create_dir_all(repo.parent().ok_or("parent")?)?;
    git(
        &root,
        &[
            "clone",
            "-q",
            origin.to_str().ok_or("path")?,
            repo.to_str().ok_or("path")?,
        ],
    )?;
    let created = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        // A person confirms its hosts and production (SPEC-216: agents do not set it up).
        Some(
            json!({"name": "DEV_TOKEN", "description": "dev machine", "value": VALUE, "auto_allow": false}),
        ),
        &alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let secret = created.body["id"].as_str().ok_or("id")?.to_owned();
    Ok(Fixture {
        h,
        alice,
        bob,
        origin,
        work,
        marks,
        secret,
    })
}

impl Fixture {
    pub(crate) async fn get(&self, path: &str) -> BoxResult<Value> {
        let r = call(&self.h.router, Method::GET, path, None, &self.alice).await?;
        assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.body);
        Ok(r.body)
    }

    pub(crate) async fn deploy(
        &self,
        who: &Creds,
        env: &str,
        confirm: Option<&str>,
    ) -> BoxResult<support::Resp> {
        call(
            &self.h.router,
            Method::POST,
            &format!("/api/sandboxes/s1/cd/{env}/deploy"),
            Some(json!({ "confirm": confirm })),
            who,
        )
        .await
    }

    pub(crate) async fn allow_secret(&self) -> TestResult {
        let now = self
            .h
            .store
            .vault_get(&self.secret)
            .await?
            .ok_or("secret")?;
        assert!(
            self.h
                .store
                .vault_set_env_files(
                    &self.secret,
                    (now.env_files, now.env_requested_by.as_deref()),
                    (EnvFiles::Allowed, None),
                    None,
                )
                .await?
        );
        Ok(())
    }

    /// The state production's first secret has on the CD page.
    pub(crate) async fn prod_secret_state(&self) -> BoxResult<Value> {
        let overview = self.get("/api/sandboxes/s1/cd").await?;
        Ok(overview["environments"][1]["secrets"][0]["state"].clone())
    }

    /// Waits until run `id` ended and returns it with its output.
    pub(crate) async fn ended(&self, id: &str) -> BoxResult<Value> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let run = self.get(&format!("/api/sandboxes/s1/cd/runs/{id}")).await?;
            if run["state"] != "running" {
                return Ok(run);
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!("the CD run did not end: {run}").into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Commits `files` (path, contents; `None` removes) on `branch` and pushes it.
    pub(crate) fn push(&self, branch: &str, files: &[(&str, Option<&str>)]) -> BoxResult<String> {
        git(
            &self.work,
            &["checkout", "-q", "-B", branch, &format!("origin/{branch}")],
        )
        .or_else(|_| git(&self.work, &["checkout", "-q", "-B", branch, "origin/dev"]))?;
        for (path, contents) in files {
            match contents {
                Some(text) => std::fs::write(self.work.join(path), text)?,
                None => std::fs::remove_file(self.work.join(path))?,
            }
        }
        git(&self.work, &["add", "-A"])?;
        git(&self.work, &["commit", "-q", "-m", "change"])?;
        git(
            &self.work,
            &["push", "-q", "origin", &format!("HEAD:{branch}")],
        )?;
        git(&self.work, &["fetch", "-q", "origin"])?;
        git(&self.work, &["rev-parse", "HEAD"])
    }

    pub(crate) fn head(&self, branch: &str) -> BoxResult<String> {
        git(&self.origin, &["rev-parse", branch])
    }

    /// Records a passed rollback drill of `dev` at `branch`'s head, so a production deploy of
    /// that tree is not refused for its rollback: for tests about something else.
    pub(crate) async fn prove(&self, branch: &str) -> TestResult {
        let sha = self.head(branch)?;
        let id = format!("drill-{}", &sha[..12]);
        if self.h.store.cd_run(&id, false).await?.is_some() {
            return Ok(());
        }
        let at = self.h.app.now();
        self.h
            .store
            .insert_cd_run(&CdRunRecord {
                id: id.clone(),
                sandbox_id: SandboxId::parse("s1")?,
                environment: "dev".into(),
                stage: CdStage::Dev,
                branch: "dev".into(),
                sha,
                state: RunState::Running,
                reason: None,
                requested_by: "yulius".into(),
                task_id: None,
                deploy: Some("true".into()),
                verify: None,
                note: None,
                output: String::new(),
                exit_code: None,
                started_at: at,
                ended_at: None,
                kind: yulius_domain::rollback::CdRunKind::Drill,
                source: None,
                steps: Vec::new(),
            })
            .await?;
        let end = yulius_app::ci::cd::CdRunEnd {
            state: RunState::Passed,
            reason: None,
            exit_code: Some(0),
            output: String::new(),
            ended_at: at,
            usage: None,
        };
        self.h.store.finish_cd_run(&id, &end).await?;
        Ok(())
    }
}

pub(crate) const TOKEN: &str = "cd-task-token-s1-0123456789abcdef";

/// The holders of a recording isolation adapter: the sandbox's and its production holder's
/// joins, with the calls made.
#[derive(Default)]
pub(crate) struct Holders {
    pub(crate) calls: std::sync::Mutex<Vec<&'static str>>,
    /// The sandbox is under disk pressure.
    pub(crate) over: bool,
}

impl Holders {
    pub(crate) fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    fn join(net_ns: u64, name: &str) -> yulius_app::ports::isolation::SandboxJoin {
        yulius_app::ports::isolation::SandboxJoin {
            holder_pid: 4242,
            user_ns: net_ns + 1,
            net_ns,
            mnt_ns: net_ns + 2,
            cgroup: Some(format!("/cg/{name}/agents").into()),
            ci_cgroup: Some(format!("/cg/{name}/ci").into()),
            system_root: format!("/fake/runtime/sandboxes/{name}/root").into(),
            hostname: name.replace('.', "-"),
            exec_shim: None,
            extra_binds: vec![yulius_app::ports::session::Bind {
                source: "/data/claude-code/versions/2.1.311".into(),
                dest: "/opt/agent-cli".into(),
                writable: false,
            }],
            private_dirs: Vec::new(),
            system_passwd: None,
            system_group: None,
            host_system: None,
            docker_dir: Some(format!("/fake/runtime/sandboxes/{name}/docker").into()),
        }
    }
}

#[async_trait::async_trait]
impl yulius_app::ports::isolation::SandboxIsolation for Holders {
    async fn prepare(
        &self,
        _: &yulius_app::ports::isolation::SandboxRef,
    ) -> yulius_app::error::AppResult<Option<yulius_app::ports::isolation::SandboxJoin>> {
        self.calls
            .lock()
            .map_err(|_| yulius_app::error::AppError::internal("poisoned"))?
            .push("prepare");
        Ok(Some(Self::join(2, "s1")))
    }
    fn release(&self, _: &SandboxId) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push("release");
        }
    }
    async fn prepare_production(
        &self,
        _: &yulius_app::ports::isolation::SandboxRef,
    ) -> yulius_app::error::AppResult<Option<yulius_app::ports::isolation::SandboxJoin>> {
        self.calls
            .lock()
            .map_err(|_| yulius_app::error::AppError::internal("poisoned"))?
            .push("prepare_production");
        Ok(Some(Self::join(20, "s1.prod")))
    }
    fn release_production(&self, _: &SandboxId) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push("release_production");
        }
    }
    async fn disk(
        &self,
        _: &yulius_app::ports::isolation::SandboxRef,
    ) -> Option<yulius_app::ports::isolation::DiskUsage> {
        // Under the quota but past the upkeep's pressure point (review round 2).
        self.over
            .then_some(yulius_app::ports::isolation::DiskUsage {
                used: 990_000_000,
                limit: 1_000_000_000,
            })
    }
    async fn measure_disk(
        &self,
        sandbox: &yulius_app::ports::isolation::SandboxRef,
    ) -> Option<yulius_app::ports::isolation::DiskUsage> {
        self.disk(sandbox).await
    }
    async fn prune_engine(&self, _: &yulius_app::ports::isolation::SandboxRef) -> Option<u64> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push("prune_engine");
        }
        Some(0)
    }
}

/// A process host that records each run's containment and runs it uncontained (the joins are
/// fakes), with the real ptyhost.
pub(crate) struct Recording {
    pub(crate) inner: PtySessionHost,
    pub(crate) seen: std::sync::Mutex<Vec<yulius_app::ports::session::Containment>>,
}

#[async_trait::async_trait]
impl yulius_app::ports::session::SessionHost for Recording {
    async fn spawn(
        &self,
        run_dir: &Path,
        attempt_id: &str,
        task_id: &str,
        plan: &yulius_app::ports::session::LaunchPlan,
        token: &[u8; 32],
        size: (u16, u16),
    ) -> yulius_app::error::AppResult<yulius_app::ports::session::SpawnedRun> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(plan.containment.clone());
        }
        let mut plan = plan.clone();
        plan.containment = yulius_app::ports::session::Containment::None;
        self.inner
            .spawn(run_dir, attempt_id, task_id, &plan, token, size)
            .await
    }
    async fn attach(
        &self,
        run_dir: &Path,
        token: &[u8; 32],
    ) -> yulius_app::error::AppResult<yulius_app::ports::session::AttachedRun> {
        self.inner.attach(run_dir, token).await
    }
    fn inspect(&self, run_dir: &Path) -> yulius_app::ports::session::RunInspection {
        self.inner.inspect(run_dir)
    }
    fn terminate(&self, run_dir: &Path) -> yulius_app::error::AppResult<()> {
        self.inner.terminate(run_dir)
    }
}
