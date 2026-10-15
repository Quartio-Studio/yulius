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
use yulius_app::ports::store_ext::CdStore as _;
use yulius_app::ports::vault::{EnvFiles, VaultStore as _};
use yulius_domain::cd::CdEnv;
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
    // A short base: run sockets live at <data>/runs/cd-<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("ycd").tempdir_in("/tmp")?;
    let root = dir.path().canonicalize()?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let _ = Command::new(&exe).arg("version").output();
    let h = harness_with(HarnessOptions {
        session_host: Some(Arc::new(PtySessionHost::new(exe))),
        git: Some(Arc::new(GitAdapter::new("git"))),
        dir: Some(dir),
        configure: Some(Box::new(|config| {
            config.ci.job_timeout = Duration::from_secs(20);
            config.contained = false;
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
        Some(json!({"name": "DEV_TOKEN", "description": "dev machine", "value": VALUE})),
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
    let unknown = f.deploy(&f.alice, "staging", None).await?;
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
            environment: CdEnv::Dev,
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
    assert!(
        shown.contains("production is deployed by a person"),
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
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    assert!(
        refused.body.to_string().contains("for production deploys"),
        "{}",
        refused.body
    );
    let overview = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1/cd",
        None,
        &f.bob,
    )
    .await?;
    assert_eq!(
        overview.body["environments"][1]["secrets"][0]["state"],
        "not_allowed"
    );

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
    // Someone who manages the secret (its maker) allows it on the Secrets page.
    let allowed = cd_prod(true).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    assert_eq!(allowed.body["cd_prod"], true);
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
    f.h.store
        .insert_cd_run(&CdRunRecord {
            id: "lost2".into(),
            sandbox_id: SandboxId::parse("s1")?,
            environment: CdEnv::Prod,
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
    Ok(())
}
