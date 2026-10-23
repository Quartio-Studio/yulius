//! CD (deployment) end to end (SPEC-134, ADR-0109/0110/0111): a real repository whose
//! `yulius.toml` defines `[cd.dev]` and `[cd.prod]` with scripts, the real Git adapter and the real
//! detached ptyhost running the deploys, the vault giving allowed secrets, and the fake GitHub
//! promoting.

use crate::support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, FakeRemote, HarnessOptions, call, configure_github_app, harness_with, perms,
    seed_github_user, seed_sandbox,
};
use yulius::wiring::git::GitAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::ci::cd::CdRunRecord;
use yulius_app::ports::store_ext::{AutomationStore as _, CdStore as _};
use yulius_app::ports::vault::{EnvFiles, VaultStore as _};
use yulius_domain::cd::CdStage;
use yulius_domain::ci::RunState;
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = BoxResult<()>;

const WAIT: Duration = Duration::from_secs(60);
const VALUE: &str = "dev-deploy-token-9q2w7e5r";

fn git(dir: &Path, args: &[&str]) -> BoxResult<String> {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Yulius Test",
            "-c",
            "user.email=test@yulius.test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The bare `origin` as the fake GitHub's repository side: promotions merge and fast-forward it.
struct Origin(PathBuf);

impl FakeRemote for Origin {
    fn branch_sha(&self, branch: &str) -> Option<String> {
        git(
            &self.0,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .ok()
    }
    fn merge(&self, base: &str, head_sha: &str, _: &str, message: &str) -> Result<String, String> {
        let run = |args: &[&str]| git(&self.0, args).map_err(|e| e.to_string());
        let base_sha = self.branch_sha(base).ok_or("base branch not found")?;
        let tree = run(&["merge-tree", "--write-tree", &base_sha, head_sha])?;
        let commit = run(&[
            "commit-tree",
            &tree,
            "-p",
            &base_sha,
            "-p",
            head_sha,
            "-m",
            message,
        ])?;
        run(&[
            "update-ref",
            &format!("refs/heads/{base}"),
            &commit,
            &base_sha,
        ])?;
        Ok(commit)
    }
    fn fast_forward(&self, branch: &str, sha: &str) -> Result<(), String> {
        git(
            &self.0,
            &["update-ref", &format!("refs/heads/{branch}"), sha],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }
    fn delete_branch(&self, _: &str) -> Result<bool, String> {
        Ok(false)
    }
}

/// The deploy script: proves what it got (the secret's value, `YULIUS_CD_*`, no task token) and
/// leaves a mark the verify script checks. It prints the value, which the stored output redacts.
const DEPLOY: &str = r#"#!/bin/sh
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

struct Fixture {
    h: support::Harness,
    alice: Creds,
    bob: Creds,
    origin: PathBuf,
    work: PathBuf,
    marks: PathBuf,
    secret: String,
}

/// `yulius.toml` with CD tables `cd` (the marks directory replaces `{marks}`).
async fn fixture(cd: &str) -> BoxResult<Fixture> {
    fixture_with(cd, None).await
}

/// [`fixture`]; with `host`, runs are contained (`config.contained`) and go to that process host.
async fn fixture_with(
    cd: &str,
    host: Option<Arc<dyn yulius_app::ports::session::SessionHost>>,
) -> BoxResult<Fixture> {
    let contained = host.is_some();
    // A short base: run sockets live at <data>/runs/cd-<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("ycd").tempdir_in("/tmp")?;
    let root = dir.path().canonicalize()?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let _ = Command::new(&exe).arg("version").output();
    let h = harness_with(HarnessOptions {
        session_host: Some(host.unwrap_or_else(|| Arc::new(PtySessionHost::new(exe)))),
        git: Some(Arc::new(GitAdapter::new("git"))),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.ci.job_timeout = Duration::from_secs(20);
            config.contained = contained;
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
    h.github.set_remote(Arc::new(Origin(origin.clone())));
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

/// Dev deploys the scripts with the secret; production promotes and deploys the same scripts.
fn full_cd() -> String {
    "[cd.dev]\n\
     about = \"the team's dev machine\"\n\
     deploy = \"MARKS={marks} sh scripts/cd/deploy.sh\"\n\
     verify = \"MARKS={marks} sh scripts/cd/verify.sh\"\n\
     secrets = \"DEV_TOKEN\"\n\n\
     [cd.prod]\n\
     promote = true\n\
     deploy = \"MARKS={marks} DEV_TOKEN=none sh scripts/cd/deploy.sh\"\n\
     verify = \"MARKS={marks} sh scripts/cd/verify.sh\"\n"
        .to_owned()
}

impl Fixture {
    async fn get(&self, path: &str) -> BoxResult<Value> {
        let r = call(&self.h.router, Method::GET, path, None, &self.alice).await?;
        assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.body);
        Ok(r.body)
    }

    async fn deploy(
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

    async fn allow_secret(&self) -> TestResult {
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
                )
                .await?
        );
        Ok(())
    }

    /// The state production's first secret has on the CD page.
    async fn prod_secret_state(&self) -> BoxResult<Value> {
        let overview = self.get("/api/sandboxes/s1/cd").await?;
        Ok(overview["environments"][1]["secrets"][0]["state"].clone())
    }

    /// Waits until run `id` ended and returns it with its output.
    async fn ended(&self, id: &str) -> BoxResult<Value> {
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
    fn push(&self, branch: &str, files: &[(&str, Option<&str>)]) -> BoxResult<String> {
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

    fn head(&self, branch: &str) -> BoxResult<String> {
        git(&self.origin, &["rev-parse", branch])
    }
}

/// AC2 (CDR-001..003, CDA-001): a person deploys dev from the integration head in a fresh
/// checkout; the scripts see the allowed secret and `YULIUS_CD_*`, never a task token; the stored
/// output never holds the value; the run's home is gone afterwards.
#[tokio::test]
async fn a_person_deploys_dev_from_the_integration_head_with_its_allowed_secret() -> TestResult {
    let f = fixture(&full_cd()).await?;
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    assert_eq!(overview["environments"][0]["state"], "ready", "{overview}");
    assert_eq!(
        overview["environments"][0]["secrets"][0]["state"],
        "not_allowed"
    );
    assert_eq!(overview["can_deploy_dev"], true);
    assert_eq!(
        overview["can_deploy_prod"], false,
        "alice writes, bob administers"
    );

    // Not allowed outside the proxy yet: nothing runs.
    let refused = f.deploy(&f.alice, "dev", None).await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    let message = refused.body["error"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains("DEV_TOKEN") && message.contains("Secrets page"),
        "{message}"
    );
    assert!(
        f.get("/api/sandboxes/s1/cd").await?["runs"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );

    f.allow_secret().await?;
    let started = f.deploy(&f.alice, "dev", None).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(started.body["state"], "running");
    let dev = f.head("dev")?;
    assert_eq!(started.body["commit"], dev);
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert!(
        output.contains(&format!("deploying dev dev {dev}")),
        "{output}"
    );
    assert!(
        output.contains("token-ok") && output.contains("clean-env"),
        "{output}"
    );
    assert!(output.contains("verified"), "{output}");
    assert!(
        output.contains("the token is [secret:DEV_TOKEN]"),
        "{output}"
    );
    assert!(
        !output.contains(VALUE),
        "the value never reaches the stored output"
    );
    assert_eq!(run["requested_by"], "alice");
    let homes = f.h.app.config.data_dir.join("sandboxes/s1/cd/homes");
    assert!(
        std::fs::read_dir(&homes).map(|d| d.count()).unwrap_or(0) == 0,
        "the run's home is removed"
    );
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    assert_eq!(overview["environments"][0]["deployed"], dev);
    assert_eq!(overview["environments"][0]["last_run"]["state"], "passed");
    Ok(())
}

/// AC1 (CDC-003): a task branch that rewrites `[cd.dev]` changes nothing a deploy runs until it
/// is merged into the integration branch.
#[tokio::test]
async fn a_task_branch_never_changes_what_dev_deploys() -> TestResult {
    let f = fixture(&full_cd()).await?;
    f.allow_secret().await?;
    let cheat = "[cd.dev]\ndeploy = \"echo CHEATED\"\n\n[cd.prod]\n";
    f.push("feat/cheat", &[("yulius.toml", Some(cheat))])?;
    let started = f.deploy(&f.alice, "dev", None).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert!(!output.contains("CHEATED"), "{output}");
    assert_eq!(
        run["deploy"].as_str().map(|d| d.contains("deploy.sh")),
        Some(true)
    );

    // Merged into dev, the change is what dev deploys.
    f.push("dev", &[("yulius.toml", Some(cheat))])?;
    let started = f.deploy(&f.alice, "dev", None).await?;
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    assert!(
        run["output"]
            .as_str()
            .unwrap_or_default()
            .contains("CHEATED")
    );
    Ok(())
}

/// AC3 (CDR-001): a failing deploy skips verify and the run fails with its status; a second
/// request while one runs is refused, and cancelling ends the running one.
#[tokio::test]
async fn a_failing_deploy_skips_verify_and_one_run_at_a_time() -> TestResult {
    let f = fixture(&full_cd()).await?;
    f.allow_secret().await?;
    f.push("dev", &[("fail", Some("x"))])?;
    let started = f.deploy(&f.alice, "dev", None).await?;
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(
        (run["state"].as_str(), run["exit_code"].as_i64()),
        (Some("failed"), Some(3))
    );
    assert!(
        output.contains("deploy failed with status 3; verify skipped"),
        "{output}"
    );
    assert!(!output.contains("verified"), "{output}");

    f.push("dev", &[("fail", None), ("slow", Some("x"))])?;
    let started = f.deploy(&f.alice, "dev", None).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let id = started.body["run_id"].as_str().ok_or("id")?.to_owned();
    let again = f.deploy(&f.alice, "dev", None).await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.body);
    let cancelled = call(
        &f.h.router,
        Method::POST,
        &format!("/api/sandboxes/s1/cd/runs/{id}/cancel"),
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(cancelled.status, StatusCode::OK, "{}", cancelled.body);
    let run = f.ended(&id).await?;
    assert_eq!(run["state"], "cancelled", "{run}");
    assert_eq!(run["reason"], "cancelled by alice");
    Ok(())
}

/// AC3 (CDC-002): an empty environment and an undefined one run nothing, and say what to add.
#[tokio::test]
async fn empty_and_undefined_environments_run_nothing() -> TestResult {
    let f = fixture("[cd.dev]\nabout = \"no dev machine yet\"\n").await?;
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    assert_eq!(overview["environments"][0]["state"], "empty");
    assert_eq!(overview["environments"][0]["about"], "no dev machine yet");
    assert_eq!(overview["environments"][1]["state"], "not_defined");
    let dev = f.deploy(&f.alice, "dev", None).await?;
    assert_eq!(dev.status, StatusCode::CONFLICT, "{}", dev.body);
    assert!(dev.body.to_string().contains("[cd.dev] has no deploy"));
    let prod = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(prod.status, StatusCode::CONFLICT, "{}", prod.body);
    assert!(prod.body.to_string().contains("add [cd.prod]"));
    // SPEC-257: any name may be an environment; one no table defines says what to add.
    let undefined = f.deploy(&f.alice, "staging", None).await?;
    assert_eq!(undefined.status, StatusCode::CONFLICT, "{}", undefined.body);
    assert!(undefined.body.to_string().contains("add [cd.staging]"));
    let unknown = f.deploy(&f.alice, "Staging", None).await?;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(f.marks.read_dir()?.next().is_none(), "nothing ran");
    Ok(())
}

/// AC5 (CDR-004): a run left running by a restart is recorded as an error, never run again.
#[tokio::test]
async fn a_run_left_by_a_restart_is_an_error() -> TestResult {
    let f = fixture(&full_cd()).await?;
    f.h.store
        .insert_cd_run(&CdRunRecord {
            id: "lost".into(),
            sandbox_id: SandboxId::parse("s1")?,
            environment: "dev".into(),
            stage: CdStage::Dev,
            branch: "dev".into(),
            sha: f.head("dev")?,
            state: RunState::Running,
            reason: None,
            requested_by: "alice".into(),
            task_id: None,
            deploy: Some("sh scripts/cd/deploy.sh".into()),
            verify: None,
            note: None,
            output: "[yulius] deploying\n".into(),
            exit_code: None,
            started_at: Timestamp(1),
            ended_at: None,
        })
        .await?;
    assert_eq!(f.h.app.reconcile_cd_runs().await?, 1);
    let run = f.get("/api/sandboxes/s1/cd/runs/lost").await?;
    assert_eq!(run["state"], "error");
    assert!(
        run["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("not run again")
    );
    assert!(f.marks.read_dir()?.next().is_none(), "nothing ran again");
    assert_eq!(f.h.app.reconcile_cd_runs().await?, 0);
    Ok(())
}

/// AC4 (CDA-002): production is a person's with administrator access who typed the project's
/// slug; the deploy promotes the integration branch (the promotion pull request, merged at once
/// when its checks passed) and runs the scripts at the new production head.
#[tokio::test]
async fn production_is_deployed_by_an_administrator_who_types_the_slug() -> TestResult {
    let f = fixture(&full_cd()).await?;
    let dev = f.push("dev", &[("feature.txt", Some("new\n"))])?;
    let main_before = f.head("main")?;
    let writer = f.deploy(&f.alice, "prod", Some("app")).await?;
    assert_eq!(writer.status, StatusCode::FORBIDDEN, "{}", writer.body);
    for typed in [None, Some("ap"), Some("yes")] {
        let refused = f.deploy(&f.bob, "prod", typed).await?;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            refused.body
        );
        assert!(
            refused.body.to_string().contains("'app'"),
            "{}",
            refused.body
        );
    }
    assert_eq!(
        f.head("main")?,
        main_before,
        "nothing was promoted by a refusal"
    );

    // The person confirmed the dev commit they saw; dev moved since: refused (round 2, M-C).
    let stale = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/cd/prod/deploy",
        Some(json!({"confirm": "app", "expect": main_before})),
        &f.bob,
    )
    .await?;
    assert_eq!(stale.status, StatusCode::CONFLICT, "{}", stale.body);
    assert!(
        stale.body.to_string().contains("moved since you looked"),
        "{}",
        stale.body
    );
    assert_eq!(f.head("main")?, main_before, "nothing was promoted");

    let started = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/cd/prod/deploy",
        Some(json!({"confirm": " app ", "expect": &dev[..7]})),
        &f.bob,
    )
    .await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let main = f.head("main")?;
    assert_ne!(main, main_before, "promoted");
    assert!(git(&f.origin, &["merge-base", "--is-ancestor", &dev, &main]).is_ok());
    assert_eq!(started.body["commit"], main);
    assert_eq!(started.body["branch"], "main");
    assert!(
        started.body["note"]
            .as_str()
            .is_some_and(|n| n.contains("promoted 'dev' to 'main'")),
        "{}",
        started.body
    );
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert!(
        output.contains(&format!("deploying prod main {main}")),
        "{output}"
    );
    assert!(output.contains("verified"), "{output}");
    assert_eq!(run["requested_by"], "bob");
    let overview = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1/cd",
        None,
        &f.bob,
    )
    .await?;
    assert_eq!(overview.body["can_deploy_prod"], true);
    assert_eq!(overview.body["environments"][1]["deployed"], main);

    // Deployed again with nothing new: no promotion, the same head.
    let again = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    assert!(
        again.body["note"]
            .as_str()
            .is_some_and(|n| n.contains("nothing to promote"))
    );
    f.ended(again.body["run_id"].as_str().ok_or("id")?).await?;
    Ok(())
}

const TOKEN: &str = "cd-task-token-s1-0123456789abcdef";

/// AC2, AC4, AC6 (CDA-001/003, CDT-001..004): an agent reads CD, asks a person through
/// `needs_you`, deploys dev (its secret asked for first), waits for the run with `yulius wait`,
/// and is refused production whatever it passes.
#[tokio::test]
async fn agents_deploy_dev_ask_a_person_and_never_deploy_production() -> TestResult {
    use yulius_domain::task::TaskStatus;
    let f = fixture(&full_cd()).await?;
    support::seed_task(&f.h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&f.h, "t1", TOKEN).await?;
    let agent = Creds {
        bearer: Some(TOKEN.into()),
        ..Creds::default()
    };
    let tool = |name: &'static str, args: Value| support::mcp_tool(&f.h, TOKEN, name, args);

    let status = tool("cd_status", json!({})).await?;
    let text = support::tool_text(&status);
    let shown = text["text"].as_str().unwrap_or_default();
    assert!(shown.contains("dev ([cd.dev] from dev): ready"), "{shown}");
    assert!(shown.contains("secrets: DEV_TOKEN not allowed"), "{shown}");
    // The CI/CD page's CD tab, its canonical address (SPEC-151).
    assert!(
        shown.contains("production-stage ones are deployed by a person on ")
            && shown.ends_with("/ci?section=cd."),
        "{shown}"
    );

    // Production is never an agent's: refused before anything is read, and audited.
    for env in ["prod", "production"] {
        let refused = tool("cd_deploy", json!({"environment": env})).await?;
        assert_eq!(refused["isError"], true, "{refused}");
        assert!(
            refused
                .to_string()
                .contains("only a person deploys production")
        );
    }
    let main_before = f.head("main")?;

    // The agent asks a person explicitly; the task is in Needs you until it resolves it.
    let link = format!("{}/s/app/secrets?section=agents", f.h.app.config.public_url);
    let outside = tool(
        "needs_you",
        json!({"action": "Allow DEV_TOKEN", "link": "https://evil.test/x"}),
    )
    .await?;
    assert_eq!(
        outside["isError"], true,
        "links stay inside Yulius: {outside}"
    );
    let needs = tool(
        "needs_you",
        json!({"action": "Allow DEV_TOKEN for CD on the Secrets page", "why": "dev deploys need it", "link": link}),
    )
    .await?;
    assert_eq!(needs["isError"], false, "{needs}");
    let task = call(&f.h.router, Method::GET, "/api/tasks/t1", None, &f.alice).await?;
    let summary = if task.body["task"].is_object() {
        &task.body["task"]
    } else {
        &task.body
    };
    assert_eq!(
        summary["needs_you"]["action"], "Allow DEV_TOKEN for CD on the Secrets page",
        "{}",
        task.body
    );
    let sandbox = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(
        sandbox.body["counts"]["needs_attention"], 1,
        "{}",
        sandbox.body
    );
    let resolved = tool("needs_you", json!({"resolved": true})).await?;
    assert_eq!(support::tool_text(&resolved)["resolved"], true);
    let sandbox = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(
        sandbox.body["counts"]["needs_attention"], 0,
        "{}",
        sandbox.body
    );

    // The secret is not allowed yet: nothing runs, it is asked for, the task needs a person.
    let asked = tool("cd_deploy", json!({"environment": "dev"})).await?;
    assert_eq!(asked["isError"], true, "{asked}");
    assert!(
        asked
            .to_string()
            .contains("allows using it outside the proxy"),
        "{asked}"
    );
    let secret = f.h.store.vault_get(&f.secret).await?.ok_or("secret")?;
    assert_eq!(secret.env_files, EnvFiles::Asked);
    assert_eq!(secret.env_requested_by.as_deref(), Some("#1"));

    // Allowed: the agent deploys dev, and `yulius wait` follows the run to its end.
    f.allow_secret().await?;
    let head = f.head("dev")?;
    let started = tool("cd_deploy", json!({"environment": "dev"})).await?;
    assert_eq!(started["isError"], false, "{started}");
    let run = support::tool_text(&started)["run"].clone();
    let id = run["run_id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(run["requested_by"], "task #1");
    let ended = f.ended(&id).await?;
    assert_eq!(ended["state"], "passed", "{ended}");
    let wait = call(
        &f.h.internal,
        Method::POST,
        "/internal/agent/wait",
        Some(json!({"head": head, "since": Value::Null, "timeout_secs": 1})),
        &agent,
    )
    .await?;
    assert_eq!(wait.status, StatusCode::OK, "{}", wait.body);
    assert_eq!(wait.body["cd_run"], id.as_str());
    assert_eq!(wait.body["cd_state"], "passed");
    let read = tool("cd_status", json!({"run_id": id})).await?;
    assert!(
        support::tool_text(&read)["text"]
            .as_str()
            .is_some_and(|t| t.contains("verified") && !t.contains(VALUE)),
        "{read}"
    );
    assert_eq!(f.head("main")?, main_before, "an agent never promotes");
    // Nor through a production environment of the delivery profile (ADR-0110).
    let profile = call(
        &f.h.router,
        Method::PATCH,
        "/api/sandboxes/s1",
        Some(json!({"profile": {
            "preset": "custom", "integration_branch": "dev", "production_branch": "main",
            "merge": "squash", "promote": "merge_commit", "auto_promote": false,
            "require_checks": true,
            "environments": [{"name": "live", "after": "production", "command": "true", "verify": "command"}]
        }})),
        &f.bob,
    )
    .await?;
    assert_eq!(profile.status, StatusCode::OK, "{}", profile.body);
    let live = tool("delivery_deploy", json!({"environment": "live"})).await?;
    assert_eq!(live["isError"], true, "{live}");
    assert!(
        live.to_string()
            .contains("only a person deploys production"),
        "{live}"
    );
    let promote = tool("delivery_promote", json!({})).await?;
    assert_eq!(promote["isError"], true, "no such tool any more: {promote}");

    use yulius_app::ports::store_ext::AutomationStore as _;
    for (target, action) in [
        ("app/prod", "cd.refused"),
        ("app/dev", "cd.deploy"),
        ("t1", "task.needs_you"),
    ] {
        let entries = f.h.store.automation_audit(target, 20).await?;
        assert!(
            entries
                .iter()
                .any(|e| e.action == action && e.actor == "task:1"),
            "{target} {action}: {entries:?}"
        );
    }
    Ok(())
}

/// CDT-004: a prompt typed into the task ends its request: a person answered.
#[tokio::test]
async fn a_typed_prompt_ends_a_needs_you_request() -> TestResult {
    use yulius_domain::task::TaskStatus;
    let f = fixture(&full_cd()).await?;
    support::seed_task(&f.h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&f.h, "t1", TOKEN).await?;
    let needs = support::mcp_tool(
        &f.h,
        TOKEN,
        "needs_you",
        json!({"action": "Add DEV_SSH_KEY on the Secrets page"}),
    )
    .await?;
    assert_eq!(needs["isError"], false, "{needs}");
    let agent = Creds {
        bearer: Some(TOKEN.into()),
        ..Creds::default()
    };
    let sandbox = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(
        sandbox.body["counts"]["needs_attention"], 1,
        "{}",
        sandbox.body
    );
    // A prompt Yulius types (nobody typed into the terminal) answers nothing (review M5).
    let typed_by_yulius = call(
        &f.h.internal,
        Method::POST,
        "/hooks/claude/UserPromptSubmit",
        Some(json!({"prompt": "Go on with the plan", "hook_event_name": "UserPromptSubmit"})),
        &agent,
    )
    .await?;
    assert!(
        typed_by_yulius.status.is_success(),
        "{}",
        typed_by_yulius.body
    );
    let still = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(still.body["counts"]["needs_attention"], 1, "{}", still.body);
    f.h.app.record_typing(
        &yulius_domain::ids::TaskId::parse("t1")?,
        &yulius_domain::ids::UserId::parse("u-alice")?,
    );
    let prompt = call(
        &f.h.internal,
        Method::POST,
        "/hooks/claude/UserPromptSubmit",
        Some(json!({"prompt": "Done, I added it", "hook_event_name": "UserPromptSubmit"})),
        &agent,
    )
    .await?;
    assert!(prompt.status.is_success(), "{}", prompt.body);
    let sandbox = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(
        sandbox.body["counts"]["needs_attention"], 0,
        "{}",
        sandbox.body
    );
    Ok(())
}

/// Review C1 (CDC-003): nothing an agent can write in the sandbox changes what a deploy runs. A
/// replacement planted in the sandbox clone (`git replace`) and files planted where checkouts
/// used to live are ignored: the run executes the commit GitHub reports, from Yulius's own mirror.
#[tokio::test]
async fn nothing_an_agent_writes_in_the_sandbox_changes_what_a_deploy_runs() -> TestResult {
    let f = fixture(&full_cd()).await?;
    f.allow_secret().await?;
    let repo = f.h.app.config.data_dir.join("sandboxes/s1/repo");
    let dev = f.head("dev")?;
    let honest = git(
        &repo,
        &["rev-parse", &format!("{dev}:scripts/cd/deploy.sh")],
    )?;
    let evil_file = f.marks.join("evil.sh");
    std::fs::write(&evil_file, "#!/bin/sh\necho REPLACED\n")?;
    let evil = git(
        &repo,
        &["hash-object", "-w", evil_file.to_str().ok_or("path")?],
    )?;
    git(&repo, &["replace", &honest, &evil])?;
    let planted =
        f.h.app
            .config
            .data_dir
            .join("sandboxes/s1/cd/checkouts/dev/scripts/cd");
    std::fs::create_dir_all(&planted)?;
    std::fs::write(planted.join("deploy.sh"), "#!/bin/sh\necho PLANTED\n")?;

    let started = f.deploy(&f.alice, "dev", None).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert!(
        output.contains("token-ok") && output.contains("verified"),
        "{output}"
    );
    assert!(
        !output.contains("REPLACED") && !output.contains("PLANTED"),
        "{output}"
    );
    // Its files were private to the run and are gone.
    let runs = f.h.app.config.data_dir.join("runs");
    assert!(
        std::fs::read_dir(&runs).map_or(true, |d| d
            .filter_map(Result::ok)
            .all(|e| !e.file_name().to_string_lossy().starts_with("cd-"))),
        "the run's directory is removed"
    );
    Ok(())
}

/// SPEC-216 SSA-003/004, AC-3/AC-4: production runs get a secret agents set up with no person's
/// allowance; the production-only allowance and agents setting it up exclude each other.
#[tokio::test]
async fn production_gets_a_secret_agents_set_up() -> TestResult {
    let f = fixture(
        "[cd.dev]\n\n[cd.prod]\n\
         deploy = \"MARKS={marks} sh scripts/cd/deploy.sh\"\n\
         verify = \"MARKS={marks} sh scripts/cd/verify.sh\"\n\
         secrets = \"DEV_TOKEN\"\n",
    )
    .await?;
    let secret_path = format!("/api/agent-secrets/{}", f.secret);
    let auto = |on: bool| {
        call(
            &f.h.router,
            Method::PATCH,
            &secret_path,
            Some(json!({"auto_allow": on})),
            &f.alice,
        )
    };
    let on = auto(true).await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(on.body["auto_allow"], true);
    let overview = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1/cd",
        None,
        &f.bob,
    )
    .await?;
    assert_eq!(
        overview.body["environments"][1]["secrets"][0]["state"], "ready",
        "{}",
        overview.body
    );
    let started = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert!(
        output.contains("token-ok") && !output.contains(VALUE),
        "{output}"
    );

    // Production already gets it: no production-only allowance on top.
    let cd_prod_path = format!("/api/agent-secrets/{}/cd-prod", f.secret);
    let cd_prod = |allow: bool| {
        call(
            &f.h.router,
            Method::POST,
            &cd_prod_path,
            Some(json!({"allow": allow})),
            &f.alice,
        )
    };
    let both = cd_prod(true).await?;
    assert_eq!(both.status, StatusCode::CONFLICT, "{}", both.body);
    // Turned off, production waits for a person again; a production secret is never set up by
    // agents.
    assert_eq!(auto(false).await?.status, StatusCode::OK);
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    let allowed = cd_prod(true).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    let again = auto(true).await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.body);
    assert!(
        again.body.to_string().contains("production secret"),
        "{}",
        again.body
    );
    Ok(())
}

/// #704 (SPEC-237 CAU-001, CAU-002): a secret the proxy sends to a host is one agents use, so
/// production never gets it and the page does not ask a person to allow it; dev is unchanged, and
/// the refusal still names a plain secret waiting beside it.
#[tokio::test]
async fn production_never_gets_a_secret_bound_to_hosts() -> TestResult {
    let f = fixture(
        "[cd.dev]\n\
         deploy = \"MARKS={marks} sh scripts/cd/deploy.sh\"\n\
         secrets = \"DEV_TOKEN\"\n\n\
         [cd.prod]\n\
         deploy = \"MARKS={marks} sh scripts/cd/deploy.sh\"\n\
         verify = \"MARKS={marks} sh scripts/cd/verify.sh\"\n\
         secrets = \"DEV_TOKEN PROD_TOKEN\"\n",
    )
    .await?;
    let created = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "PROD_TOKEN", "value": "prod-value-0123456789", "auto_allow": false})),
        &f.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(f.prod_secret_state().await?, "not_allowed");
    let hosts = call(
        &f.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{}", f.secret),
        Some(json!({"hosts": ["api.example.com"]})),
        &f.alice,
    )
    .await?;
    assert_eq!(hosts.status, StatusCode::OK, "{}", hosts.body);
    assert_eq!(f.prod_secret_state().await?, "agents_use");
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    assert_eq!(
        overview["environments"][0]["secrets"][0]["state"],
        "not_allowed"
    );
    assert_eq!(
        overview["environments"][1]["secrets"][1]["state"],
        "not_allowed"
    );
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    let said = refused.body.to_string();
    assert!(said.contains("agents use DEV_TOKEN"), "{said}");
    assert!(
        said.contains("a person who manages PROD_TOKEN allows it for production deploys"),
        "{said}"
    );
    Ok(())
}

/// Review H1 (CDR-002): production runs get only secrets a person allowed for production; an
/// allowance for `.env` files (which any agent of the project could use) is not enough.
#[tokio::test]
async fn production_secrets_need_their_own_allowance() -> TestResult {
    let f = fixture(
        "[cd.dev]\n\n[cd.prod]\n\
         deploy = \"MARKS={marks} sh scripts/cd/deploy.sh\"\n\
         verify = \"MARKS={marks} sh scripts/cd/verify.sh\"\n\
         secrets = \"DEV_TOKEN\"\n",
    )
    .await?;
    f.allow_secret().await?;
    // Agents use it (its .env copies), so no person can allow it for production (round 2, M-B):
    // the page and the refusal say so instead of asking a person to allow it (#704, SPEC-237).
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    let said = refused.body.to_string();
    assert!(
        said.contains("agents use DEV_TOKEN") && said.contains("separate secret"),
        "{said}"
    );
    assert!(!said.contains("allows it for production"), "{said}");
    assert_eq!(f.prod_secret_state().await?, "agents_use");

    // A secret agents can use (its .env copies) is no production secret (round 2, M-B).
    let cd_prod_path = format!("/api/agent-secrets/{}/cd-prod", f.secret);
    let cd_prod = |allow: bool| {
        call(
            &f.h.router,
            Method::POST,
            &cd_prod_path,
            Some(json!({"allow": allow})),
            &f.alice,
        )
    };
    let shared = cd_prod(true).await?;
    assert_eq!(shared.status, StatusCode::CONFLICT, "{}", shared.body);
    assert!(
        shared
            .body
            .to_string()
            .contains("agents can use this secret"),
        "{}",
        shared.body
    );
    let now = f.h.store.vault_get(&f.secret).await?.ok_or("secret")?;
    assert!(
        f.h.store
            .vault_set_env_files(
                &f.secret,
                (now.env_files, now.env_requested_by.as_deref()),
                (EnvFiles::No, None),
            )
            .await?
    );
    // Nobody uses it any more: now a person can allow it.
    assert_eq!(f.prod_secret_state().await?, "not_allowed");
    let waiting = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(waiting.status, StatusCode::CONFLICT, "{}", waiting.body);
    assert!(
        waiting.body.to_string().contains("for production deploys"),
        "{}",
        waiting.body
    );
    // Someone who manages the secret (its maker) allows it on the Secrets page.
    let allowed = cd_prod(true).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    assert_eq!(allowed.body["cd_prod"], true);
    assert_eq!(f.prod_secret_state().await?, "ready");
    let started = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert!(
        output.contains("token-ok") && !output.contains(VALUE),
        "{output}"
    );
    // A production secret is never given to agents: no .env copy, no hosts (round 2, M-B) and no
    // MCP `${secret:NAME}` (round 3, M2).
    let env = call(
        &f.h.router,
        Method::POST,
        &format!("/api/agent-secrets/{}/env", f.secret),
        Some(json!({"allow": true, "env_files": "no", "env_requested_by": null})),
        &f.alice,
    )
    .await?;
    assert_eq!(env.status, StatusCode::CONFLICT, "{}", env.body);
    let hosts = call(
        &f.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{}", f.secret),
        Some(json!({"hosts": ["api.example.com"]})),
        &f.alice,
    )
    .await?;
    assert_eq!(hosts.status, StatusCode::CONFLICT, "{}", hosts.body);
    assert!(
        hosts.body.to_string().contains("production secret"),
        "{}",
        hosts.body
    );
    use yulius_app::ports::store::SandboxStore as _;
    let sandbox =
        f.h.store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("sandbox")?;
    assert_eq!(
        f.h.app.resolve_secret_ref(&sandbox, "DEV_TOKEN").await?,
        None
    );
    // Agents cannot even ask for it (round 3, L3).
    support::seed_task(
        &f.h,
        "t1",
        "s1",
        "u-alice",
        yulius_domain::task::TaskStatus::Running,
        None,
    )
    .await?;
    support::seed_live_attempt(&f.h, "t1", TOKEN).await?;
    for (tool, args) in [
        ("secret_copy_to_env", json!({"secrets": ["DEV_TOKEN"]})),
        (
            "secret_request_hosts",
            json!({"secret": "DEV_TOKEN", "hosts": ["api.example.com"]}),
        ),
    ] {
        let asked = support::mcp_tool(&f.h, TOKEN, tool, args).await?;
        assert_eq!(asked["isError"], true, "{tool}: {asked}");
        assert!(
            asked.to_string().contains("production secret"),
            "{tool}: {asked}"
        );
    }
    Ok(())
}

/// Review H2 (CDR-004): a run left by a restart has its process host stopped and its files
/// removed, not only its row closed.
#[tokio::test]
async fn a_lost_run_leaves_no_files_behind() -> TestResult {
    let f = fixture(&full_cd()).await?;
    let run_dir = f.h.app.config.data_dir.join("runs/cd-lost2");
    std::fs::create_dir_all(run_dir.join("work/home"))?;
    std::fs::write(run_dir.join("work/home/key"), "secret material")?;
    // A production run's files (SPEC-279 PDE-004).
    let files = f.h.app.config.data_dir.join("cd/s1/runs/cd-lost2");
    std::fs::create_dir_all(files.join("work/home"))?;
    std::fs::write(files.join("work/home/key"), "secret material")?;
    f.h.store
        .insert_cd_run(&CdRunRecord {
            id: "lost2".into(),
            sandbox_id: SandboxId::parse("s1")?,
            environment: "prod".into(),
            stage: CdStage::Prod,
            branch: "main".into(),
            sha: f.head("main")?,
            state: RunState::Running,
            reason: None,
            requested_by: "bob".into(),
            task_id: None,
            deploy: None,
            verify: None,
            note: None,
            output: String::new(),
            exit_code: None,
            started_at: Timestamp(1),
            ended_at: None,
        })
        .await?;
    assert_eq!(f.h.app.reconcile_cd_runs().await?, 1);
    assert!(!run_dir.exists(), "the lost run's files are removed");
    assert!(
        !files.exists(),
        "the lost production run's files are removed"
    );
    Ok(())
}

/// SPEC-137 STG-301/302: a production deploy is measured stage by stage: the person's
/// confirmation (as the page measured it, up to an hour), the checks, the promotion, the run with
/// its commit and trigger, its deploy with the stages its output names, and its verify.
#[tokio::test]
async fn a_deploy_records_its_confirmation_checks_promotion_phases_and_stages() -> TestResult {
    use std::sync::atomic::Ordering;
    use yulius_app::ports::store_ext::SpanStore as _;
    use yulius_domain::ids::SandboxId;
    use yulius_domain::stages::SubjectKind;
    let f = fixture(
        "[cd.dev]\n\
         about = \"no dev machine yet\"\n\n\
         [cd.prod]\n\
         promote = true\n\
         deploy = \"echo '::yulius-stage build'; sleep 1; echo '::yulius-stage upload'; MARKS={marks} DEV_TOKEN=none sh scripts/cd/deploy.sh\"\n\
         verify = \"MARKS={marks} sh scripts/cd/verify.sh\"\n",
    )
    .await?;
    let clock = Arc::clone(&f.h.clock);
    let follow = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        loop {
            tick.tick().await;
            clock.0.fetch_add(20, Ordering::SeqCst);
        }
    });
    f.push("dev", &[("feature.txt", Some("new\n"))])?;
    let deploy = |confirm_ms: u64| {
        call(
            &f.h.router,
            Method::POST,
            "/api/sandboxes/s1/cd/prod/deploy",
            Some(json!({"confirm": "app", "confirm_ms": confirm_ms})),
            &f.bob,
        )
    };
    let started = deploy(4_200).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let id = started.body["run_id"].as_str().ok_or("id")?.to_owned();
    let run = f.ended(&id).await?;
    assert_eq!(run["state"], "passed", "{run}");
    let spans =
        f.h.store
            .subject_spans(&SandboxId::parse("s1")?, SubjectKind::CdRun, &id, 1_000)
            .await?;
    let find = |key: &str| {
        spans
            .iter()
            .find(|s| s.key == key)
            .ok_or_else(|| format!("no {key} in {spans:?}"))
    };
    let confirm = find("confirm")?;
    assert_eq!(
        (confirm.kind.as_str(), confirm.name.as_str()),
        ("wait", "confirmation")
    );
    assert_eq!(confirm.duration_ms(), Some(4_200));
    assert!(confirm.detail.as_deref().unwrap_or("").contains("page"));
    for key in ["prepare", "promote"] {
        let span = find(key)?;
        assert_eq!(
            (span.outcome.as_deref(), span.environment.as_deref()),
            (Some("passed"), Some("prod")),
            "{span:?}"
        );
    }
    assert!(find("promote")?.started_at >= find("prepare")?.started_at);
    let whole = find("run")?;
    assert_eq!(whole.outcome.as_deref(), Some("passed"));
    let detail: Value = serde_json::from_str(whole.detail.as_deref().unwrap_or("{}"))?;
    assert_eq!(
        (detail["requested_by"].as_str(), detail["sha"].as_str()),
        (Some("bob"), Some(f.head("main")?.as_str()))
    );
    // SPEC-154 USE-003: what the deploy's processes used, as their host measured it.
    assert!(
        detail["cpu_ms"].as_i64().is_some() && detail["memory_peak_mb"].as_i64() >= Some(1),
        "{detail}"
    );
    let deploy_span = find("run/deploy")?;
    let verify_span = find("run/verify")?;
    assert_eq!(
        (
            deploy_span.outcome.as_deref(),
            verify_span.outcome.as_deref()
        ),
        (Some("passed"), Some("passed"))
    );
    let build = find("run/deploy/stage:1")?;
    let upload = find("run/deploy/stage:2")?;
    assert_eq!(
        (build.name.as_str(), upload.name.as_str()),
        ("build", "upload")
    );
    assert_eq!(build.parent_key.as_deref(), Some("run/deploy"));
    assert!(build.duration_ms().unwrap_or(0) >= 500, "{build:?}");
    assert!(
        deploy_span.ended_at <= verify_span.started_at.into()
            && verify_span.ended_at <= whole.ended_at
    );

    // A confirmation held over an hour is not a measurement.
    let again = deploy(2 * 60 * 60 * 1000).await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    let again_id = again.body["run_id"].as_str().ok_or("id")?.to_owned();
    f.ended(&again_id).await?;
    follow.abort();
    let again_spans =
        f.h.store
            .subject_spans(
                &SandboxId::parse("s1")?,
                SubjectKind::CdRun,
                &again_id,
                1_000,
            )
            .await?;
    assert!(
        again_spans
            .iter()
            .all(|s| s.key != "confirm" && s.key != "promote"),
        "{again_spans:?}"
    );
    Ok(())
}

/// Dev and production plus a named environment of each stage (SPEC-257): staging next to dev,
/// the hosted platform and the self-hosted release next to production.
fn named_cd() -> String {
    "[cd.dev]\n\
     about = \"the team's dev machine\"\n\n\
     [cd.prod]\n\
     about = \"instances update themselves from main\"\n\n\
     [cd.staging]\n\
     stage = \"dev\"\n\
     deploy = \"MARKS={marks} sh scripts/cd/deploy.sh\"\n\
     verify = \"MARKS={marks} sh scripts/cd/verify.sh\"\n\n\
     [cd.platform]\n\
     stage = \"prod\"\n\
     deploy = \"test ! -e $YULIUS_CD_CACHE/planted && MARKS={marks} sh scripts/cd/deploy.sh\"\n\
     verify = \"MARKS={marks} sh scripts/cd/verify.sh\"\n\n\
     [cd.selfserve]\n\
     stage = \"prod\"\n\
     about = \"released from main by its tags\"\n"
        .to_owned()
}

/// SPEC-257 AC2, AC5: every environment is listed with its stage, dev-stage first; a named
/// dev-stage environment deploys the integration head for a person with write access and an
/// agent; a named production-stage one deploys the production head only for an administrator who
/// typed the slug, never for an agent, and its run keeps name and stage.
#[tokio::test]
async fn named_environments_deploy_by_their_stage() -> TestResult {
    use yulius_domain::task::TaskStatus;
    let f = fixture(&named_cd()).await?;
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    let listed: Vec<(String, String, String)> = overview["environments"]
        .as_array()
        .ok_or("environments")?
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap_or_default().to_owned(),
                e["stage"].as_str().unwrap_or_default().to_owned(),
                e["state"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let expected = [
        ("dev", "dev", "empty"),
        ("staging", "dev", "ready"),
        ("prod", "prod", "empty"),
        ("platform", "prod", "ready"),
        ("selfserve", "prod", "empty"),
    ]
    .map(|(n, s, st)| (n.to_owned(), s.to_owned(), st.to_owned()));
    assert_eq!(listed, expected, "{overview}");
    assert_eq!(overview["environments"][3]["branch"], "main");
    assert_eq!(overview["environments"][1]["branch"], "dev");

    // A dev-stage environment: a person with write access deploys the integration head.
    let dev = f.head("dev")?;
    let started = f.deploy(&f.alice, "staging", None).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(
        (&started.body["environment"], &started.body["stage"]),
        (&json!("staging"), &json!("dev"))
    );
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert!(
        output.contains(&format!("deploying staging dev {dev}")),
        "{output}"
    );

    // A production-stage one: not for write access, not without the slug.
    let refused = f.deploy(&f.alice, "platform", Some("app")).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let refused = f.deploy(&f.bob, "platform", None).await?;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.body
    );
    let main = f.head("main")?;
    let started = f.deploy(&f.bob, "platform", Some("app")).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(started.body["stage"], "prod");
    assert_eq!(started.body["branch"], "main");
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert!(
        output.contains(&format!("deploying platform main {main}")),
        "{output}"
    );
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    assert_eq!(overview["environments"][3]["deployed"], main);
    assert!(
        overview["environments"][2]["deployed"].is_null(),
        "prod is another environment"
    );

    // An agent: the dev-stage one is its own, a production-stage one never.
    support::seed_task(&f.h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&f.h, "t1", TOKEN).await?;
    let tool = |name: &'static str, args: Value| support::mcp_tool(&f.h, TOKEN, name, args);
    let shown = support::tool_text(&tool("cd_status", json!({})).await?)["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        shown.contains("staging ([cd.staging] from dev): ready"),
        "{shown}"
    );
    assert!(
        shown.contains("platform ([cd.platform], deploys main): ready"),
        "{shown}"
    );
    for env in ["platform", "selfserve"] {
        let refused = tool("cd_deploy", json!({"environment": env})).await?;
        assert_eq!(refused["isError"], true, "{refused}");
        assert!(
            refused
                .to_string()
                .contains("only a person deploys production"),
            "{refused}"
        );
    }
    let started = tool("cd_deploy", json!({"environment": "staging"})).await?;
    assert_eq!(started["isError"], false, "{started}");
    let run = support::tool_text(&started)["run"].clone();
    assert_eq!(
        (&run["environment"], &run["stage"]),
        (&json!("staging"), &json!("dev"))
    );
    f.ended(run["run_id"].as_str().ok_or("id")?).await?;
    // Refused production-stage deploys are audited as production's are; dev-stage ones are not.
    use yulius_app::ports::store_ext::AutomationStore as _;
    let refused = f.h.store.automation_audit("app/platform", 20).await?;
    assert!(
        refused
            .iter()
            .any(|e| e.action == "cd.refused" && e.actor == "task:1"),
        "{refused:?}"
    );
    let staging = f.h.store.automation_audit("app/staging", 20).await?;
    assert!(
        !staging.iter().any(|e| e.action == "cd.refused"),
        "{staging:?}"
    );
    Ok(())
}

/// SPEC-257 CDN-006: a production-stage run of a named environment is cancelled only with
/// administrator access, as production's.
#[tokio::test]
async fn a_named_production_run_is_cancelled_by_an_administrator() -> TestResult {
    let f = fixture(&named_cd()).await?;
    f.push("main", &[("slow", Some("x"))])?;
    let started = f.deploy(&f.bob, "platform", Some("app")).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let id = started.body["run_id"].as_str().ok_or("id")?.to_owned();
    let path = format!("/api/sandboxes/s1/cd/runs/{id}/cancel");
    let refused = call(&f.h.router, Method::POST, &path, None, &f.alice).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let cancelled = call(&f.h.router, Method::POST, &path, None, &f.bob).await?;
    assert_eq!(cancelled.status, StatusCode::OK, "{}", cancelled.body);
    assert_eq!(f.ended(&id).await?["state"], "cancelled");
    Ok(())
}

/// SPEC-257 CDN-004: relabelling a production environment as dev on the integration branch only
/// makes a dev environment of that name: it runs the integration head's table as dev, and nothing
/// of production comes with the name.
#[tokio::test]
async fn a_relabelled_environment_takes_the_integration_heads_stage() -> TestResult {
    let f = fixture(&named_cd()).await?;
    let relabelled = named_cd().replace(
        "[cd.platform]\nstage = \"prod\"",
        "[cd.platform]\nstage = \"dev\"",
    );
    let marks = f.marks.to_str().ok_or("path")?;
    f.push(
        "dev",
        &[("yulius.toml", Some(&relabelled.replace("{marks}", marks)))],
    )?;
    let started = f.deploy(&f.alice, "platform", None).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(
        (&started.body["stage"], &started.body["branch"]),
        (&json!("dev"), &json!("dev"))
    );
    f.ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    // The page agrees once the clone has the new head (the deploy fetched it).
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    let platform = overview["environments"]
        .as_array()
        .and_then(|all| all.iter().find(|e| e["name"] == "platform"))
        .ok_or("platform")?;
    assert_eq!(platform["stage"], "dev", "{overview}");

    // Review H1: what a dev-stage run leaves in its cache never reaches the production-stage
    // environment of the same name, nor does its commit count as deployed there.
    let planting = relabelled.replace(
        "deploy = \"test ! -e",
        "deploy = \"touch $YULIUS_CD_CACHE/planted && test ! -e",
    );
    assert_ne!(planting, relabelled);
    f.push(
        "dev",
        &[("yulius.toml", Some(&planting.replace("{marks}", marks)))],
    )?;
    let planted = f.deploy(&f.alice, "platform", None).await?;
    assert_eq!(planted.status, StatusCode::OK, "{}", planted.body);
    let planted = f
        .ended(planted.body["run_id"].as_str().ok_or("id")?)
        .await?;
    assert_eq!(
        planted["state"], "failed",
        "it sees its own file: {planted}"
    );
    f.push(
        "dev",
        &[("yulius.toml", Some(&named_cd().replace("{marks}", marks)))],
    )?;
    let sid = SandboxId::parse("s1")?;
    let deployed = |stage| f.h.store.last_passed_cd_run(&sid, "platform", stage);
    assert!(
        deployed(CdStage::Dev).await?.is_some(),
        "the first, clean run"
    );
    assert!(
        deployed(CdStage::Prod).await?.is_none(),
        "a dev-stage run is not what production-stage platform has deployed"
    );
    let started = f.deploy(&f.bob, "platform", Some("app")).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(started.body["stage"], "prod");
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    assert_eq!(run["state"], "passed", "a cache of its own: {run}");
    Ok(())
}

/// The deploy of SPEC-278's tests: it reads the restored state where `YULIUS_STATE_OPERATOR`
/// says (absolute, outside `HOME`), prints its inventory and the variables it got, appends a line
/// and adds a receipt, waits while `$MARKS/hold` exists (after saying so with `$MARKS/holding`)
/// and fails when the commit has a `fail` file; with `noop` it leaves the state alone, with `break`
/// it replaces the folder with a link to `/`.
const STATE_SCRIPT: &str = r#"#!/bin/sh
d="$YULIUS_STATE_OPERATOR"
case "$d" in /*) ;; *) echo "no state path"; exit 7;; esac
case "$d" in "$HOME"*) echo "the state is inside HOME"; exit 6;; esac
if [ -f noop ]; then echo "left the state alone"; exit 0; fi
if [ -f break ]; then rm -rf "$d" && ln -s / "$d" && echo "replaced the state with a link"; exit 0; fi
echo "state at $d"
echo "inventory: $(tr '\n' '|' < "$d/inventory.db")"
echo "$YULIUS_CD_ENV $YULIUS_CD_RUN" >> "$d/inventory.db"
mkdir -p "$d/receipts" && echo ok > "$d/receipts/$YULIUS_CD_RUN.json"
echo "region ${OVH_ENDPOINT:-unset} python ${PYTHONPATH:-unset}"
if [ -f "$MARKS/hold" ]; then : > "$MARKS/holding"; while [ -f "$MARKS/hold" ]; do sleep 0.1; done; fi
if [ -f fail ]; then exit 3; fi
"#;

fn states_cd() -> String {
    "[cd.dev]\n\
     deploy = \"MARKS={marks} sh scripts/cd/state.sh\"\n\
     states = [\"OPERATOR\"]\n\n\
     [cd.prod]\n\
     deploy = \"MARKS={marks} sh scripts/cd/state.sh\"\n\
     states = [\"OPERATOR\"]\n\
     variables = [\"OVH_ENDPOINT\"]\n\n\
     [cd.second]\n\
     stage = \"dev\"\n\
     deploy = \"MARKS={marks} sh scripts/cd/state.sh\"\n\
     states = [\"OPERATOR\"]\n\n\
     [cd.third]\n\
     stage = \"dev\"\n\
     deploy = \"true\"\n\
     variables = [\"NOT_SET\"]\n\n\
     [cd.staging]\n\
     stage = \"dev\"\n\
     deploy = \"true\"\n\
     states = \"MISSING_STATE\"\n"
        .to_owned()
}

/// A packed state of `(path, mode, content)` files, as a laptop sends it.
fn state_archive(files: &[(&str, u32, &[u8])]) -> BoxResult<Vec<u8>> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, mode, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(*mode);
        header.set_size(content.len() as u64);
        builder.append_data(&mut header, path, *content)?;
    }
    Ok(builder.into_inner()?.finish()?)
}

/// The files of a packed state, by path.
fn state_files(bytes: &[u8]) -> BoxResult<Vec<(String, String)>> {
    use std::io::Read as _;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut out = Vec::new();
    for entry in tar.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_file() {
            let mut content = String::new();
            entry.read_to_string(&mut content)?;
            out.push((entry.path()?.display().to_string(), content));
        }
    }
    out.sort();
    Ok(out)
}

/// A served instance, called over HTTP as a browser and a laptop do.
struct Web {
    client: reqwest::Client,
    base: String,
    origin: String,
}

impl Web {
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<(Vec<u8>, &str)>,
        who: &Creds,
    ) -> BoxResult<(u16, Vec<u8>)> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header("origin", &self.origin);
        if let Some(token) = &who.bearer {
            request = request.bearer_auth(token);
        }
        if let Some((bytes, kind)) = body {
            request = request.header("content-type", kind).body(bytes);
        }
        let response = request.send().await?;
        Ok((response.status().as_u16(), response.bytes().await?.to_vec()))
    }

    async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        who: &Creds,
    ) -> BoxResult<(u16, Value)> {
        let body = body.map(|json| (json.to_string().into_bytes(), "application/json"));
        let (status, bytes) = self.send(method, path, body, who).await?;
        Ok((
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        ))
    }

    /// Starts a deploy; its run id.
    async fn deploy(&self, env: &str, confirm: Option<&str>, who: &Creds) -> BoxResult<String> {
        let (status, body) = self
            .json(
                reqwest::Method::POST,
                &format!("/api/sandboxes/s1/cd/{env}/deploy"),
                Some(json!({ "confirm": confirm })),
                who,
            )
            .await?;
        assert_eq!(status, 200, "{body}");
        Ok(body["run_id"].as_str().ok_or("run id")?.to_owned())
    }

    /// Waits until run `id` ended; it with its output.
    async fn ended(&self, id: &str, who: &Creds) -> BoxResult<Value> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let (_, run) = self
                .json(
                    reqwest::Method::GET,
                    &format!("/api/sandboxes/s1/cd/runs/{id}"),
                    None,
                    who,
                )
                .await?;
            if run["state"] != "running" {
                return Ok(run);
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!("the CD run did not end: {run}").into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The vault's record of the state `OPERATOR`.
    async fn operator(&self, who: &Creds) -> BoxResult<Value> {
        let (status, states) = self
            .json(
                reqwest::Method::GET,
                "/api/sandboxes/s1/agent-states",
                None,
                who,
            )
            .await?;
        assert_eq!(status, 200, "{states}");
        Ok(states
            .as_array()
            .and_then(|list| list.iter().find(|s| s["name"] == "OPERATOR"))
            .cloned()
            .unwrap_or(Value::Null))
    }
}

/// Waits until `path` exists.
async fn exists(path: &Path) -> TestResult {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !path.exists() {
        if tokio::time::Instant::now() > deadline {
            return Err(format!("{} never appeared", path.display()).into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

/// SPEC-278 AC1–AC7 over a served instance: a laptop sends the state; a dev deploy restores the
/// vault's version outside `HOME`, writes to it and saves it (audited, the laptop's next send
/// refused); while a deploy holds it a task's save and a send are refused and a production deploy
/// waits; a failing deploy still saves; production restores the vault's newest version, never the
/// task's unsaved copy, and withholds a code-loading variable; the task's copy is then stale until
/// it restores; a deploy naming a missing state runs nothing.
#[tokio::test]
async fn deploys_restore_write_and_save_an_agent_state_with_one_writer() -> TestResult {
    use reqwest::Method as M;
    use yulius_domain::task::TaskStatus;
    let f = fixture(&states_cd()).await?;
    for branch in ["dev", "main"] {
        f.push(branch, &[("scripts/cd/state.sh", Some(STATE_SCRIPT))])?;
    }
    let served = support::serve(f.h.router.clone()).await?;
    // As `yulius` does before its first client (plain HTTP here; reqwest still asks for one).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let web = Web {
        client: reqwest::Client::new(),
        base: format!("http://{}", served.addr),
        origin: f.h.app.config.public_url.clone(),
    };
    let data = f.h.app.config.data_dir.clone();
    // Alice (write) and Bob (admin) with their CLI tokens, over HTTP.
    let (alice, bob) = (&f.alice, &f.bob);

    // The laptop sends version 1; the person sets two variables.
    let laptop = state_archive(&[
        ("inventory.db", 0o600, b"v1\n"),
        ("rollouts/r0/bundle.json", 0o644, b"{}\n"),
    ])?;
    let (status, sent) = web
        .send(
            M::PUT,
            "/api/sandboxes/s1/agent-states/OPERATOR",
            Some((laptop.clone(), "application/gzip")),
            alice,
        )
        .await?;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&sent));
    let (status, set) = web
        .json(
            M::PUT,
            "/api/sandboxes/s1/variables",
            Some(json!({"variables": [
                {"name": "OVH_ENDPOINT", "value": "ovh-ca"},
                {"name": "PYTHONPATH", "value": "/tmp/elsewhere"},
            ]})),
            alice,
        )
        .await?;
    assert_eq!(status, 200, "{set}");

    // AC6: a state the vault does not hold: nothing runs.
    let (status, refused) = web
        .json(
            M::POST,
            "/api/sandboxes/s1/cd/staging/deploy",
            Some(json!({})),
            alice,
        )
        .await?;
    assert_eq!(status, 409, "{refused}");
    let said = refused.to_string();
    assert!(
        said.contains("MISSING_STATE") && said.contains("nothing ran"),
        "{said}"
    );

    // A project variable the table names but the project lacks: nothing runs.
    let (status, refused) = web
        .json(
            M::POST,
            "/api/sandboxes/s1/cd/third/deploy",
            Some(json!({})),
            alice,
        )
        .await?;
    assert_eq!(status, 409, "{refused}");
    let said = refused.to_string();
    assert!(
        said.contains("NOT_SET") && said.contains("nothing ran"),
        "{said}"
    );

    // AC1: dev restores version 1 outside HOME, writes, and saves version 2.
    let (_, overview) = web
        .json(M::GET, "/api/sandboxes/s1/cd", None, alice)
        .await?;
    assert_eq!(
        overview["environments"][0]["states"],
        json!([{"name": "OPERATOR", "version": 1, "held_by": null}]),
        "{overview}"
    );
    let first = web.deploy("dev", None, alice).await?;
    let run = web.ended(&first, alice).await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    let folder = data.join(format!("runs/cd-{first}/work/states/OPERATOR"));
    for line in [
        format!("state at {}", folder.display()),
        "inventory: v1|".to_owned(),
        "agent state OPERATOR version 1 restored into $YULIUS_STATE_OPERATOR".to_owned(),
        "region ovh-ca python /tmp/elsewhere".to_owned(),
        "agent state OPERATOR saved as version 2".to_owned(),
    ] {
        assert!(output.contains(&line), "{line}: {output}");
    }
    assert!(!folder.exists(), "the run's copy goes with the run");
    let stored = web.operator(alice).await?;
    assert_eq!(stored["version"], 2, "{stored}");
    assert_eq!(stored["updated_by"], "cd:dev", "{stored}");
    let (status, archive) = web
        .send(
            M::GET,
            "/api/sandboxes/s1/agent-states/OPERATOR/archive",
            None,
            alice,
        )
        .await?;
    assert_eq!(status, 200);
    assert_eq!(
        state_files(&archive)?,
        [
            ("inventory.db".to_owned(), format!("v1\ndev {first}\n")),
            (format!("receipts/{first}.json"), "ok\n".to_owned()),
            ("rollouts/r0/bundle.json".to_owned(), "{}\n".to_owned()),
        ]
    );
    let audit: Vec<(String, String, String)> =
        f.h.store
            .automation_audit("s1", 50)
            .await?
            .into_iter()
            .map(|e| (e.action, e.actor, e.detail.unwrap_or_default()))
            .collect();
    for action in ["vault.state.restore", "vault.state.save"] {
        assert!(
            audit
                .iter()
                .any(|(a, by, detail)| a == action && by == "cd:dev" && detail.contains(&first)),
            "{action}: {audit:?}"
        );
    }
    // The laptop's copy is no longer the one written to.
    let (status, again) = web
        .send(
            M::PUT,
            "/api/sandboxes/s1/agent-states/OPERATOR",
            Some((laptop.clone(), "application/gzip")),
            alice,
        )
        .await?;
    let again = String::from_utf8_lossy(&again).into_owned();
    assert_eq!(status, 409, "{again}");
    assert!(again.contains("written by a CD run of dev"), "{again}");

    // A task restores version 2 into its home.
    support::seed_task(&f.h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&f.h, "t1", TOKEN).await?;
    let home = data.join("sandboxes/s1/home");
    std::fs::create_dir_all(&home)?;
    let tool = |name: &'static str, args: Value| {
        let h = &f.h;
        async move {
            let result = support::mcp_tool(h, TOKEN, name, args).await?;
            Ok::<_, Box<dyn std::error::Error>>((
                result["isError"].as_bool().unwrap_or(false),
                support::tool_text(&result),
            ))
        }
    };
    let (error, restored) = tool(
        "secret_state_restore",
        json!({"name": "OPERATOR", "path": "~/.ops"}),
    )
    .await?;
    assert!(!error, "{restored}");
    assert_eq!(restored["version"], 2, "{restored}");

    // AC2, AC3: a failing deploy holds the state while it runs.
    f.push("dev", &[("fail", Some("x\n"))])?;
    std::fs::write(f.marks.join("hold"), "")?;
    let second = web.deploy("dev", None, alice).await?;
    exists(&f.marks.join("holding")).await?;
    let (error, said) = tool("secret_state_save", json!({"name": "OPERATOR"})).await?;
    let said = said.to_string();
    assert!(
        error && said.contains(&format!("held by dev run {second}")),
        "{said}"
    );
    let (error, listed) = tool("secrets_list", json!({})).await?;
    assert!(!error, "{listed}");
    assert_eq!(
        listed["states"][0]["held_by"],
        format!("dev run {second}"),
        "{listed}"
    );
    let (status, held) = web
        .send(
            M::PUT,
            "/api/sandboxes/s1/agent-states/OPERATOR?replace=true",
            Some((laptop.clone(), "application/gzip")),
            alice,
        )
        .await?;
    let held = String::from_utf8_lossy(&held).into_owned();
    assert_eq!(status, 409, "{held}");
    assert!(held.contains("held by dev run"), "{held}");
    // AC4: production waits for it; the task changes its copy without saving.
    let third = web.deploy("prod", Some("app"), bob).await?;
    let copy = home.join(".ops/inventory.db");
    let mut unsaved = std::fs::read_to_string(&copy)?;
    unsaved.push_str("task-unsaved\n");
    std::fs::write(&copy, unsaved)?;
    std::fs::remove_file(f.marks.join("hold"))?;

    let run = web.ended(&second, alice).await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "failed", "{output}");
    assert!(
        output.contains("agent state OPERATOR saved as version 3"),
        "{output}"
    );
    let run = web.ended(&third, bob).await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    for line in [
        format!("waiting for the agent state OPERATOR: dev run {second} holds it"),
        format!("inventory: v1|dev {first}|dev {second}|"),
        "region ovh-ca python unset".to_owned(),
        "production runs get only the variables [cd.prod] names; left out: PYTHONPATH".to_owned(),
        "agent state OPERATOR saved as version 4".to_owned(),
    ] {
        assert!(output.contains(&line), "{line}: {output}");
    }
    assert!(!output.contains("task-unsaved"), "{output}");
    assert_eq!(web.operator(bob).await?["updated_by"], "cd:prod");

    // AC5: the task's copy is stale: its save is refused until a restore replaces it.
    let (_, listed) = tool("secrets_list", json!({})).await?;
    let state = &listed["states"][0];
    assert_eq!(state["stale"], true, "{listed}");
    assert_eq!(state["version"], 4, "{listed}");
    assert!(state.get("held_by").is_none(), "{listed}");
    let (error, said) = tool("secret_state_save", json!({"name": "OPERATOR"})).await?;
    let said = said.to_string();
    assert!(
        error && said.contains("stale") && said.contains("a CD run of prod saved version 4"),
        "{said}"
    );
    let (error, restored) = tool(
        "secret_state_restore",
        json!({"name": "OPERATOR", "path": "~/.ops"}),
    )
    .await?;
    assert!(!error, "{restored}");
    assert_eq!(
        std::fs::read_to_string(&copy)?,
        format!("v1\ndev {first}\ndev {second}\nprod {third}\n")
    );
    Ok(())
}

/// SPEC-278 failure paths (review of SPEC-278, M2): a deploy that leaves its state alone keeps
/// the version; one that replaces its folder with a link ends as an error naming the state, the
/// link is not followed and the version stays; a deploy cancelled while it waits for the hold
/// ends cancelled and the holder keeps it.
#[tokio::test]
async fn a_state_left_alone_broken_or_waited_for_keeps_its_version() -> TestResult {
    use reqwest::Method as M;
    let f = fixture(&states_cd()).await?;
    f.push(
        "dev",
        &[
            ("scripts/cd/state.sh", Some(STATE_SCRIPT)),
            ("noop", Some("x\n")),
        ],
    )?;
    let served = support::serve(f.h.router.clone()).await?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let web = Web {
        client: reqwest::Client::new(),
        base: format!("http://{}", served.addr),
        origin: f.h.app.config.public_url.clone(),
    };
    let alice = &f.alice;
    let laptop = state_archive(&[("inventory.db", 0o600, b"v1\n")])?;
    let (status, _) = web
        .send(
            M::PUT,
            "/api/sandboxes/s1/agent-states/OPERATOR",
            Some((laptop, "application/gzip")),
            alice,
        )
        .await?;
    assert_eq!(status, 200);

    // Left alone: nothing is stored, the version stays.
    let run = web.deploy("dev", None, alice).await?;
    let run = web.ended(&run, alice).await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert!(
        output.contains("agent state OPERATOR unchanged: version 1 kept"),
        "{output}"
    );
    let stored = web.operator(alice).await?;
    assert_eq!(stored["version"], 1, "{stored}");
    assert_ne!(stored["updated_by"], "cd:dev", "{stored}");

    // Replaced with a link: the save follows no link, the run is an error naming the state.
    f.push("dev", &[("noop", None), ("break", Some("x\n"))])?;
    let run = web.deploy("dev", None, alice).await?;
    let run = web.ended(&run, alice).await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "error", "{output}");
    let reason = run["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("the deploy passed") && reason.contains("OPERATOR"),
        "{reason}"
    );
    assert!(
        output.contains("replaced its folder of OPERATOR with a link or a file"),
        "{output}"
    );
    assert_eq!(web.operator(alice).await?["version"], 1);

    // Cancelled while it waits for the hold: the holder keeps it and saves it.
    f.push("dev", &[("break", None)])?;
    std::fs::write(f.marks.join("hold"), "")?;
    let holder = web.deploy("dev", None, alice).await?;
    exists(&f.marks.join("holding")).await?;
    let waiting = web.deploy("second", None, alice).await?;
    let (status, cancelled) = web
        .json(
            M::POST,
            &format!("/api/sandboxes/s1/cd/runs/{waiting}/cancel"),
            Some(json!({})),
            alice,
        )
        .await?;
    assert_eq!(status, 200, "{cancelled}");
    let run = web.ended(&waiting, alice).await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "cancelled", "{output}");
    assert!(!output.contains("state at"), "it never ran: {output}");
    let (_, overview) = web
        .json(M::GET, "/api/sandboxes/s1/cd", None, alice)
        .await?;
    assert_eq!(
        overview["environments"][0]["states"][0]["held_by"],
        format!("dev run {holder}"),
        "{overview}"
    );
    std::fs::remove_file(f.marks.join("hold"))?;
    let run = web.ended(&holder, alice).await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert_eq!(web.operator(alice).await?["version"], 2);
    Ok(())
}

/// The holders of a recording isolation adapter: the sandbox's and its production holder's
/// joins, with the calls made.
#[derive(Default)]
struct Holders {
    calls: std::sync::Mutex<Vec<&'static str>>,
    /// The sandbox is under disk pressure.
    over: bool,
}

impl Holders {
    fn calls(&self) -> Vec<&'static str> {
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
            extra_readonly: vec!["/opt/agent-cli".into()],
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
struct Recording {
    inner: PtySessionHost,
    seen: std::sync::Mutex<Vec<yulius_app::ports::session::Containment>>,
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

/// SPEC-279 PDE-003, PDE-004 (AC2, AC3): a production deploy prepares the sandbox's production
/// holder, never the sandbox's, joins it on the host's system with that holder's Docker
/// directory, keeps its files in `<data>/cd/<sandbox>/runs` and removes them, and releases the
/// production holder; a dev deploy keeps the sandbox's holder and `<data>/runs`.
#[tokio::test]
async fn production_runs_join_the_production_holder_with_its_docker_engine() -> TestResult {
    use yulius_app::ports::session::Containment;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let recording = Arc::new(Recording {
        inner: PtySessionHost::new(exe),
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let f = fixture_with(&full_cd(), Some(recording.clone())).await?;
    let holders = Arc::new(Holders::default());
    assert!(f.h.app.set_isolation(holders.clone()));
    let data = f.h.app.config.data_dir.clone();
    let started = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let id = started.body["run_id"].as_str().ok_or("id")?.to_owned();
    let run = f.ended(&id).await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert_eq!(
        holders.calls(),
        ["prepare_production", "release_production"],
        "only the production holder"
    );
    let seen = recording.seen.lock().map(|s| s.clone()).unwrap_or_default();
    let Some(Containment::Bubblewrap {
        sandbox_root,
        writable,
        join: Some(join),
        ..
    }) = seen.first()
    else {
        return Err(format!("a joined containment: {seen:?}").into());
    };
    let files = data.join("cd/s1/runs").join(format!("cd-{id}"));
    assert_eq!(sandbox_root, &files.join("work"));
    assert_eq!(writable, &[data.join("cd/s1/cache/prod")]);
    assert_eq!(join.net_ns, 20, "the production holder's network");
    assert_eq!(
        join.docker_dir.as_deref(),
        Some(Path::new("/fake/runtime/sandboxes/s1.prod/docker")),
        "the production holder's engine"
    );
    assert!(join.host_system.is_some(), "the host's read-only system");
    assert!(join.extra_readonly.is_empty());
    assert_eq!(join.cgroup.as_deref(), Some(Path::new("/cg/s1.prod/ci")));
    assert!(!files.exists(), "the run's files are removed");
    assert!(!data.join("runs").join(format!("cd-{id}")).exists());

    // A dev deploy keeps the sandbox's holder, its engine and `<data>/runs`.
    f.allow_secret().await?;
    let dev = f.deploy(&f.alice, "dev", None).await?;
    assert_eq!(dev.status, StatusCode::OK, "{}", dev.body);
    let dev_id = dev.body["run_id"].as_str().ok_or("id")?.to_owned();
    let run = f.ended(&dev_id).await?;
    assert_eq!(run["state"], "passed", "{}", run["output"]);
    assert_eq!(
        holders.calls()[2..],
        ["prepare", "release"],
        "the sandbox's holder for dev"
    );
    let seen = recording.seen.lock().map(|s| s.clone()).unwrap_or_default();
    let Some(Containment::Bubblewrap {
        writable,
        join: Some(join),
        ..
    }) = seen.get(1)
    else {
        return Err(format!("a joined containment: {seen:?}").into());
    };
    assert_eq!(join.net_ns, 2);
    assert!(join.host_system.is_none());
    assert!(
        writable.contains(&data.join("runs").join(format!("cd-{dev_id}")).join("work")),
        "{writable:?}"
    );
    Ok(())
}

/// SPEC-279 PDE-006 (review finding 1, round 2): after a production run, a sandbox under disk
/// pressure has its engines pruned while the production engine still runs, so production's
/// images and build cache never stay out of the upkeep's reach.
#[tokio::test]
async fn a_production_run_over_the_quota_prunes_the_engines_after_it() -> TestResult {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let recording = Arc::new(Recording {
        inner: PtySessionHost::new(exe),
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let f = fixture_with(&full_cd(), Some(recording)).await?;
    let holders = Arc::new(Holders {
        over: true,
        ..Holders::default()
    });
    assert!(f.h.app.set_isolation(holders.clone()));
    let started = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    assert_eq!(run["state"], "passed", "{}", run["output"]);
    let deadline = tokio::time::Instant::now() + WAIT;
    while !holders.calls().contains(&"prune_engine") && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let calls = holders.calls();
    assert_eq!(
        calls.iter().filter(|c| **c == "prepare_production").count(),
        1,
        "{calls:?}"
    );
    assert!(calls.contains(&"prune_engine"), "{calls:?}");
    Ok(())
}
