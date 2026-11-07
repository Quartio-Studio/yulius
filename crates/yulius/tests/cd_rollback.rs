//! Rollback in CD end to end (SPEC-415, ADR-0263): a test project whose `yulius.toml` defines
//! `rollback` and `migrations`, deployed and rolled back with the real Git adapter and the real
//! detached ptyhost running its scripts. This is the live drill on a test project: deploys record
//! releases, the dev drill runs by itself, a person rolls back in one request, production waits
//! for a proven rollback, agents roll back dev only, and barriers block.

use crate::cd::{BoxResult, Fixture, TOKEN, TestResult, VALUE, fixture};
use crate::support;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{Creds, call};
use yulius_app::ports::store_ext::AutomationStore as _;

/// Deploys `v<version file>` and names it; one part.
const DEPLOY: &str = r#"#!/bin/sh
if [ -f "$MARKS/hold" ]; then touch "$MARKS/holding"; fi
while [ -f "$MARKS/hold" ]; do sleep 0.1; done
v="v$(cat version)"
echo "building $v"
echo "::yulius-release $v"
if [ -f barrier ]; then echo "::yulius-rollback-barrier $(cat barrier)"; fi
echo "$v" > "$MARKS/$YULIUS_CD_ENV"
"#;

/// Checks that the environment runs the release a rollback asked for, else the one deployed.
const VERIFY: &str = r#"#!/bin/sh
want="${YULIUS_CD_RELEASE:-v$(cat version)}"
test "$(cat "$MARKS/$YULIUS_CD_ENV")" = "$want" && echo "verified $want"
"#;

/// Redeploys the release it is given, never building; logs what it got.
const ROLLBACK: &str = r#"#!/bin/sh
test -n "$YULIUS_CD_RELEASE" || { echo "no release given"; exit 64; }
echo "${YULIUS_CD_ROLLBACK:-0} ${YULIUS_CD_DRILL:-0} $YULIUS_CD_ENV ${YULIUS_CD_PART:--} $YULIUS_CD_RELEASE $YULIUS_CD_RELEASE_SHA" >> "$MARKS/rollbacks"
if [ -f "$MARKS/fail-rollback" ] || [ -f "$MARKS/fail-to-$YULIUS_CD_RELEASE" ]; then echo "the artifact store is down"; exit 5; fi
echo "switching to $YULIUS_CD_RELEASE"
echo "$YULIUS_CD_RELEASE" > "$MARKS/$YULIUS_CD_ENV"
"#;

/// Two parts: ships each part whose file exists, with a barrier for infra when asked.
const PARTS_DEPLOY: &str = r#"#!/bin/sh
for part in engine infra; do
  if [ -f "$part" ]; then
    echo "::yulius-release $part=$(cat "$part")"
    echo "$(cat "$part")" > "$MARKS/$YULIUS_CD_ENV-$part"
  fi
done
if [ -f barrier ]; then echo "::yulius-rollback-barrier infra: $(cat barrier)"; fi
"#;

const PARTS_VERIFY: &str = r#"#!/bin/sh
if [ -n "$YULIUS_CD_PART" ]; then
  test "$(cat "$MARKS/$YULIUS_CD_ENV-$YULIUS_CD_PART")" = "$YULIUS_CD_RELEASE" || exit 1
fi
echo verified
"#;

const PARTS_ROLLBACK: &str = r#"#!/bin/sh
echo "${YULIUS_CD_DRILL:-0} $YULIUS_CD_PART $YULIUS_CD_RELEASE" >> "$MARKS/rollbacks"
echo "$YULIUS_CD_RELEASE" > "$MARKS/$YULIUS_CD_ENV-$YULIUS_CD_PART"
"#;

const MIGRATIONS: &str = "none: the test app keeps no data";

fn table(env: &str, prod: bool, rollback: bool) -> String {
    let mut text = format!(
        "[cd.{env}]\n\
         deploy = \"MARKS={{marks}} sh scripts/cd/rb-deploy.sh\"\n\
         verify = \"MARKS={{marks}} sh scripts/cd/rb-verify.sh\"\n"
    );
    if prod {
        text.push_str("promote = true\n");
    }
    if rollback {
        text.push_str(&format!(
            "rollback = \"MARKS={{marks}} sh scripts/cd/rb-rollback.sh\"\nmigrations = \"{MIGRATIONS}\"\n"
        ));
    }
    text
}

/// Dev and production deploying `v<version>`, both with rollback.
fn rollback_cd() -> String {
    format!(
        "{}\n{}",
        table("dev", false, true),
        table("prod", true, true)
    )
}

/// The scripts and the first version, pushed to dev.
fn scripts(version: &str) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("scripts/cd/rb-deploy.sh", Some(DEPLOY.to_owned())),
        ("scripts/cd/rb-verify.sh", Some(VERIFY.to_owned())),
        ("scripts/cd/rb-rollback.sh", Some(ROLLBACK.to_owned())),
        ("version", Some(format!("{version}\n"))),
    ]
}

fn push(f: &Fixture, branch: &str, files: &[(&str, Option<String>)]) -> BoxResult<String> {
    let files: Vec<(&str, Option<&str>)> = files
        .iter()
        .map(|(path, text)| (*path, text.as_deref()))
        .collect();
    f.push(branch, &files)
}

impl Fixture {
    async fn post_as(&self, who: &Creds, path: &str, body: Value) -> BoxResult<support::Resp> {
        call(&self.h.router, Method::POST, path, Some(body), who).await
    }

    /// Deploys `env` as `who` and waits for the run to pass.
    async fn deployed(&self, who: &Creds, env: &str, confirm: Option<&str>) -> BoxResult<Value> {
        let started = self.deploy(who, env, confirm).await?;
        assert_eq!(started.status, StatusCode::OK, "{}", started.body);
        let run = self
            .ended(started.body["run_id"].as_str().ok_or("id")?)
            .await?;
        assert_eq!(run["state"], "passed", "{}", run["output"]);
        Ok(run)
    }

    /// Rolls `env` back as `who` with `body` and waits for the run's end.
    async fn rolled_back(&self, who: &Creds, env: &str, body: Value) -> BoxResult<Value> {
        let started = self
            .post_as(who, &format!("/api/sandboxes/s1/cd/{env}/rollback"), body)
            .await?;
        assert_eq!(started.status, StatusCode::OK, "{}", started.body);
        assert_eq!(started.body["kind"], "rollback", "{}", started.body);
        self.ended(started.body["run_id"].as_str().ok_or("id")?)
            .await
    }

    /// Waits until the sandbox has `count` ended drills and returns the newest with its output.
    async fn drill(&self, count: usize) -> BoxResult<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let overview = self.get("/api/sandboxes/s1/cd").await?;
            let drills: Vec<&Value> = overview["runs"]
                .as_array()
                .ok_or("runs")?
                .iter()
                .filter(|r| r["kind"] == "drill")
                .collect();
            if drills.len() >= count && drills.iter().all(|r| r["state"] != "running") {
                let id = drills[0]["run_id"].as_str().ok_or("id")?;
                return self.ended(id).await;
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!("no drill #{count}: {}", overview["runs"]).into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Waits until the sandbox has `count` production runs, all ended, and returns the newest
    /// with its output.
    async fn production_run(&self, count: usize) -> BoxResult<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let overview = self.get("/api/sandboxes/s1/cd").await?;
            let runs: Vec<&Value> = overview["runs"]
                .as_array()
                .ok_or("runs")?
                .iter()
                .filter(|r| r["environment"] == "prod")
                .collect();
            if runs.len() >= count && runs.iter().all(|r| r["state"] != "running") {
                let id = runs[0]["run_id"].as_str().ok_or("id")?;
                return self.ended(id).await;
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!("no production run #{count}: {}", overview["runs"]).into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Waits until the deploy holds on its `hold` mark.
    async fn holding(&self) -> TestResult {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while !self.marks.join("holding").exists() {
            if tokio::time::Instant::now() > deadline {
                return Err("the deploy never started".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    }

    /// The environment `name`'s rollback on the CD page.
    async fn rollback_of(&self, name: &str) -> BoxResult<Value> {
        let overview = self.get("/api/sandboxes/s1/cd").await?;
        let env = overview["environments"]
            .as_array()
            .ok_or("environments")?
            .iter()
            .find(|e| e["name"] == name)
            .ok_or("environment")?;
        Ok(env["rollback"].clone())
    }

    fn mark(&self, name: &str) -> BoxResult<String> {
        Ok(std::fs::read_to_string(self.marks.join(name))?
            .trim()
            .to_owned())
    }

    fn rollbacks(&self) -> Vec<String> {
        std::fs::read_to_string(self.marks.join("rollbacks"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn releases(part: &Value) -> Vec<String> {
    part["releases"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|r| r["release"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// AC1, AC2 (RBK-010..013, RBK-020, RBK-024, RBK-030): two dev deploys record two releases; the
/// second is drilled by itself (back to v1, verify, forward to v2, verify); a person rolls back to
/// the previous release in one request and forward again; each is audited.
#[tokio::test]
async fn dev_rolls_back_to_a_kept_release_and_its_drill_runs_by_itself() -> TestResult {
    let f = fixture(&rollback_cd()).await?;
    let v1 = push(&f, "dev", &scripts("1"))?;
    f.deployed(&f.alice, "dev", None).await?;
    assert_eq!(f.mark("dev")?, "v1");
    let rollback = f.rollback_of("dev").await?;
    assert_eq!(
        rollback["command"],
        "MARKS={marks} sh scripts/cd/rb-rollback.sh"
            .replace("{marks}", f.marks.to_str().ok_or("path")?)
    );
    assert_eq!(rollback["migrations"], MIGRATIONS);
    let part = &rollback["parts"][0];
    assert_eq!(
        (part["name"].as_str(), part["current"].as_str()),
        (Some(""), Some("v1"))
    );
    assert_eq!(part["previous"], Value::Null, "nothing to go back to yet");
    assert_eq!(
        rollback["drill"],
        Value::Null,
        "a first release is not drilled"
    );
    let nothing = f
        .post_as(&f.alice, "/api/sandboxes/s1/cd/dev/rollback", json!({}))
        .await?;
    assert_eq!(nothing.status, StatusCode::CONFLICT, "{}", nothing.body);
    assert!(
        nothing.body.to_string().contains("no earlier release"),
        "{}",
        nothing.body
    );

    let v2 = push(&f, "dev", &[("version", Some("2\n".to_owned()))])?;
    f.deployed(&f.alice, "dev", None).await?;
    let drill = f.drill(1).await?;
    let output = drill["output"].as_str().unwrap_or_default();
    assert_eq!(drill["state"], "passed", "{output}");
    assert_eq!(
        (drill["requested_by"].as_str(), drill["stage"].as_str()),
        (Some("yulius"), Some("dev"))
    );
    assert_eq!(drill["commit"], v2, "it drills the deployed commit");
    let back = output.find("[yulius] rollback: v1").ok_or(output)?;
    let forward = output.find("[yulius] rollback: v2").ok_or(output)?;
    assert!(
        back < forward && output.contains("verified v1") && output.contains("verified v2"),
        "{output}"
    );
    assert_eq!(
        f.rollbacks(),
        [format!("1 1 dev - v1 {v1}"), format!("1 1 dev - v2 {v2}")],
        "the release, its commit and the drill reach the command"
    );
    assert_eq!(
        f.mark("dev")?,
        "v2",
        "the drill leaves the new release deployed"
    );
    let rollback = f.rollback_of("dev").await?;
    let part = &rollback["parts"][0];
    assert_eq!(releases(part), ["v2", "v1"]);
    assert_eq!(
        (part["current"].as_str(), part["previous"].as_str()),
        (Some("v2"), Some("v1"))
    );
    assert_eq!(part["releases"][1]["commit"], v1);
    assert_eq!(part["releases"][1]["requested_by"], "alice");
    assert_eq!(rollback["drill"]["state"], "passed");

    // One click: back to the previous release, with no rebuild.
    let run = f.rolled_back(&f.alice, "dev", json!({})).await?;
    let output = run["output"].as_str().unwrap_or_default();
    assert_eq!(run["state"], "passed", "{output}");
    assert_eq!(run["commit"], v1, "what is deployed now");
    assert!(!output.contains("building"), "never rebuilds: {output}");
    assert!(output.contains("verified v1"), "{output}");
    assert_eq!(f.mark("dev")?, "v1");
    assert_eq!(
        f.rollbacks().last().map(String::as_str),
        Some(format!("1 0 dev - v1 {v1}").as_str())
    );
    let part = &f.rollback_of("dev").await?["parts"][0];
    assert_eq!(
        (part["current"].as_str(), part["previous"].as_str()),
        (Some("v1"), None)
    );
    assert_eq!(
        f.get("/api/sandboxes/s1/cd").await?["environments"][0]["deployed"],
        v1
    );
    // A rollback is no deploy: no drill follows it.
    assert_eq!(f.drill(1).await?["run_id"], drill["run_id"]);

    // Forward again to a chosen release.
    let run = f
        .rolled_back(&f.alice, "dev", json!({"release": "v2"}))
        .await?;
    assert_eq!(run["state"], "passed", "{}", run["output"]);
    assert_eq!(f.mark("dev")?, "v2");

    for (body, says) in [
        (json!({"release": "v9"}), "not one of the releases"),
        (json!({"release": "v2"}), "already deployed"),
        (json!({"part": "engine"}), "has no part"),
    ] {
        let refused = f
            .post_as(&f.alice, "/api/sandboxes/s1/cd/dev/rollback", body)
            .await?;
        assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
        assert!(
            refused.body.to_string().contains(says),
            "{says}: {}",
            refused.body
        );
    }
    let entries = f.h.store.automation_audit("app/dev", 50).await?;
    for (action, actor) in [
        ("cd.rollback", "alice"),
        ("cd.drill", "yulius"),
        ("cd.finished", "yulius"),
    ] {
        assert!(
            entries
                .iter()
                .any(|e| e.action == action && e.actor == actor),
            "{action}: {entries:?}"
        );
    }
    Ok(())
}

/// AC3, AC5 (RBK-031): production is refused until rollback is configured and the drill of the
/// commit it would deploy passed; it never drills itself.
#[tokio::test]
async fn production_deploys_wait_for_a_proven_rollback() -> TestResult {
    let unconfigured = format!(
        "{}\n{}",
        table("dev", false, false),
        table("prod", true, false)
    );
    let f = fixture(&unconfigured).await?;
    push(&f, "dev", &scripts("1"))?;
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    assert!(
        refused
            .body
            .to_string()
            .contains("rollback is not configured"),
        "{}",
        refused.body
    );
    let before = f.head("main")?;
    let page = f.rollback_of("prod").await?;
    assert_eq!(page["command"], Value::Null);
    assert!(
        page["blocked"]
            .as_str()
            .is_some_and(|b| b.contains("not configured")),
        "{page}"
    );

    let config = rollback_cd().replace("{marks}", f.marks.to_str().ok_or("path")?);
    push(&f, "dev", &[("yulius.toml", Some(config))])?;
    // Nothing drilled yet: production deploys dev first (SPEC-438), but one release cannot be
    // drilled, so production does not deploy and says why.
    let started = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(started.body["environment"], "dev", "{}", started.body);
    let dev = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    assert_eq!(dev["state"], "passed", "{}", dev["output"]);
    let prod = f.production_run(1).await?;
    assert_eq!(prod["state"], "error", "{prod}");
    assert!(
        prod["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("production did not deploy:")
                && r.contains("no rollback drill")),
        "{prod}"
    );
    assert_eq!(f.head("main")?, before, "nothing was promoted");

    // A failing drill does not prove it.
    std::fs::write(f.marks.join("fail-rollback"), "")?;
    push(&f, "dev", &[("version", Some("2\n".to_owned()))])?;
    f.deployed(&f.alice, "dev", None).await?;
    assert_eq!(f.drill(1).await?["state"], "failed");
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(
        refused.body.to_string().contains("did not pass"),
        "{}",
        refused.body
    );
    assert_eq!(f.head("main")?, before, "nothing was promoted by a refusal");
    let entries = f.h.store.automation_audit("app/prod", 50).await?;
    assert!(
        entries.iter().any(|e| e.action == "cd.refused"
            && e.detail.as_deref().is_some_and(|d| d.contains("rollback"))),
        "{entries:?}"
    );

    std::fs::remove_file(f.marks.join("fail-rollback"))?;
    push(&f, "dev", &[("version", Some("3\n".to_owned()))])?;
    f.deployed(&f.alice, "dev", None).await?;
    assert_eq!(f.drill(2).await?["state"], "passed");
    let page = f.rollback_of("prod").await?;
    assert_eq!(page["blocked"], Value::Null, "{page}");
    let run = f.deployed(&f.bob, "prod", Some("app")).await?;
    assert!(
        run["output"]
            .as_str()
            .is_some_and(|o| o.contains("verified v3"))
    );
    // Production is never drilled.
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    assert!(
        overview["runs"]
            .as_array()
            .ok_or("runs")?
            .iter()
            .all(|r| r["kind"] != "drill" || r["stage"] == "dev"),
        "{}",
        overview["runs"]
    );
    let prod = &f.rollback_of("prod").await?["parts"][0];
    assert_eq!(prod["current"], "v3");
    Ok(())
}

/// SPEC-438 AC1, AC4 (PDF-001, PDF-002, PDF-004): dev moved on since its drill; a person's
/// production deploy deploys dev at that commit, waits for its drill (shown, and a second
/// production deploy is refused meanwhile), then deploys production by itself.
#[tokio::test]
async fn a_production_deploy_proves_its_rollback_on_dev_first() -> TestResult {
    let f = fixture(&rollback_cd()).await?;
    push(&f, "dev", &scripts("1"))?;
    f.deployed(&f.alice, "dev", None).await?;
    push(&f, "dev", &[("version", Some("2\n".to_owned()))])?;
    f.deployed(&f.alice, "dev", None).await?;
    assert_eq!(f.drill(1).await?["state"], "passed");
    // Agents merged into dev since: its drill is at an older commit.
    let v3 = push(&f, "dev", &[("version", Some("3\n".to_owned()))])?;

    std::fs::write(f.marks.join("hold"), "")?;
    let started = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(
        (
            started.body["environment"].as_str(),
            started.body["commit"].as_str(),
            started.body["requested_by"].as_str()
        ),
        (Some("dev"), Some(v3.as_str()), Some("bob")),
        "{}",
        started.body
    );
    assert!(
        started.body["note"]
            .as_str()
            .is_some_and(|n| n.contains(&format!("production deploys at {}", &v3[..7]))),
        "{}",
        started.body
    );
    f.holding().await?;
    let waiting = format!(
        "Production deploys at {} once the rollback drill of dev passes (asked by bob)",
        &v3[..7]
    );
    assert_eq!(f.rollback_of("prod").await?["waiting"], waiting);
    let again = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.body);
    assert!(again.body.to_string().contains(&waiting), "{}", again.body);
    std::fs::remove_file(f.marks.join("hold"))?;

    let dev = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    assert_eq!(dev["state"], "passed", "{}", dev["output"]);
    let drill = f.drill(2).await?;
    assert_eq!(
        (drill["state"].as_str(), drill["commit"].as_str()),
        (Some("passed"), Some(v3.as_str())),
        "{drill}"
    );
    let prod = f.production_run(1).await?;
    let output = prod["output"].as_str().unwrap_or_default();
    assert_eq!(prod["state"], "passed", "{prod}");
    assert_eq!(prod["requested_by"], "bob");
    assert!(output.contains("verified v3"), "{output}");
    assert_eq!(f.mark("prod")?, "v3");
    git_contains(&f, "main", &v3)?;
    assert_eq!(f.rollback_of("prod").await?["waiting"], Value::Null);
    Ok(())
}

/// SPEC-438 AC2, AC3 (PDF-003, PDF-005): the drill production waited for fails, so production
/// does not deploy and its run says why; a failed drill at the same tree is then refused as before.
#[tokio::test]
async fn production_does_not_deploy_when_the_drill_it_waited_for_fails() -> TestResult {
    let f = fixture(&rollback_cd()).await?;
    push(&f, "dev", &scripts("1"))?;
    f.deployed(&f.alice, "dev", None).await?;
    push(&f, "dev", &[("version", Some("2\n".to_owned()))])?;
    f.deployed(&f.alice, "dev", None).await?;
    assert_eq!(f.drill(1).await?["state"], "passed");
    let before = f.head("main")?;
    std::fs::write(f.marks.join("fail-rollback"), "")?;
    let v3 = push(&f, "dev", &[("version", Some("3\n".to_owned()))])?;
    let started = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(started.body["environment"], "dev");
    assert_eq!(f.drill(2).await?["state"], "failed");
    let prod = f.production_run(1).await?;
    assert_eq!(prod["state"], "error", "{prod}");
    assert_eq!(prod["commit"], v3);
    assert_eq!(
        prod["reason"],
        format!(
            "production did not deploy: the rollback drill of dev at {} failed",
            &v3[..7]
        ),
        "{prod}"
    );
    assert_eq!(f.head("main")?, before, "nothing was promoted");
    let entries = f.h.store.automation_audit("app/prod", 50).await?;
    assert!(
        entries.iter().any(|e| e.action == "cd.refused"
            && e.detail
                .as_deref()
                .is_some_and(|d| d.starts_with("production did not deploy"))),
        "{entries:?}"
    );
    // The same tree failed its drill: a dev deploy cannot fix that, so it is refused.
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    assert!(
        refused.body.to_string().contains("did not pass"),
        "{}",
        refused.body
    );
    assert_eq!(f.rollback_of("prod").await?["waiting"], Value::Null);
    Ok(())
}

/// `branch` on the origin contains `sha`.
fn git_contains(f: &Fixture, branch: &str, sha: &str) -> TestResult {
    crate::cd::git(&f.origin, &["merge-base", "--is-ancestor", sha, branch])?;
    Ok(())
}

/// AC4 (RBK-021, RBK-041): an agent rolls dev back with `cd_rollback` and is refused production;
/// production is an administrator's with the slug typed, and never promotes.
#[tokio::test]
async fn agents_roll_back_dev_and_people_roll_back_production() -> TestResult {
    use yulius_domain::task::TaskStatus;
    let f = fixture(&rollback_cd()).await?;
    support::seed_task(&f.h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&f.h, "t1", TOKEN).await?;
    let tool = |name: &'static str, args: Value| support::mcp_tool(&f.h, TOKEN, name, args);
    push(&f, "dev", &scripts("1"))?;
    f.deployed(&f.alice, "dev", None).await?;
    push(&f, "dev", &[("version", Some("2\n".to_owned()))])?;
    f.deployed(&f.alice, "dev", None).await?;
    f.drill(1).await?;

    let status = support::tool_text(&tool("cd_status", json!({})).await?);
    let shown = status["text"].as_str().unwrap_or_default();
    assert!(shown.contains("rollback: MARKS="), "{shown}");
    assert!(shown.contains("releases: v2 (current), v1"), "{shown}");
    assert!(shown.contains("drill: passed"), "{shown}");
    for env in ["prod", "production"] {
        let refused = tool("cd_rollback", json!({"environment": env})).await?;
        assert_eq!(refused["isError"], true, "{refused}");
        assert!(refused.to_string().contains("only a person"), "{refused}");
    }
    let started = tool("cd_rollback", json!({"environment": "dev"})).await?;
    assert_eq!(started["isError"], false, "{started}");
    let run = &support::tool_text(&started)["run"];
    assert_eq!(run["kind"], "rollback");
    let ended = f.ended(run["run_id"].as_str().ok_or("id")?).await?;
    assert_eq!(ended["state"], "passed", "{}", ended["output"]);
    assert_eq!(ended["requested_by"], "task #1");
    assert_eq!(f.mark("dev")?, "v1");

    // Production: two releases, then a person rolls back with the slug typed.
    f.rolled_back(&f.alice, "dev", json!({"release": "v2"}))
        .await?;
    f.deployed(&f.alice, "dev", None).await?;
    f.drill(2).await?;
    f.deployed(&f.bob, "prod", Some("app")).await?;
    push(&f, "dev", &[("version", Some("3\n".to_owned()))])?;
    f.deployed(&f.alice, "dev", None).await?;
    f.drill(3).await?;
    f.deployed(&f.bob, "prod", Some("app")).await?;
    assert_eq!(f.mark("prod")?, "v3");
    let main = f.head("main")?;
    let writer = f
        .post_as(
            &f.alice,
            "/api/sandboxes/s1/cd/prod/rollback",
            json!({"confirm": "app"}),
        )
        .await?;
    assert_eq!(writer.status, StatusCode::FORBIDDEN, "{}", writer.body);
    let untyped = f
        .post_as(&f.bob, "/api/sandboxes/s1/cd/prod/rollback", json!({}))
        .await?;
    assert_eq!(
        untyped.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        untyped.body
    );
    let run = f
        .rolled_back(&f.bob, "prod", json!({"confirm": "app"}))
        .await?;
    assert_eq!(run["state"], "passed", "{}", run["output"]);
    assert_eq!(run["branch"], "main");
    assert_eq!(f.mark("prod")?, "v2");
    assert_eq!(f.head("main")?, main, "a rollback never promotes");
    let entries = f.h.store.automation_audit("app/prod", 50).await?;
    assert!(
        entries
            .iter()
            .any(|e| e.action == "cd.rollback" && e.actor == "bob"),
        "{entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|e| e.action == "cd.refused" && e.actor == "task:1"),
        "{entries:?}"
    );
    Ok(())
}

/// AC5 (RBK-003, RBK-011, RBK-022, RBK-030): parts keep their own releases; a barrier makes the
/// drill only redeploy the new release, and a rollback past it is refused with its reason.
#[tokio::test]
async fn parts_roll_back_on_their_own_and_barriers_block() -> TestResult {
    let cd = "[cd.dev]\n\
              deploy = \"MARKS={marks} sh scripts/cd/parts-deploy.sh\"\n\
              verify = \"MARKS={marks} sh scripts/cd/parts-verify.sh\"\n\
              rollback = \"MARKS={marks} sh scripts/cd/parts-rollback.sh\"\n\
              migrations = \"expand and contract; a destructive step prints a barrier\"\n\
              parts = [\"engine\", \"infra\"]\n\n[cd.prod]\n";
    let f = fixture(cd).await?;
    push(
        &f,
        "dev",
        &[
            ("scripts/cd/parts-deploy.sh", Some(PARTS_DEPLOY.to_owned())),
            ("scripts/cd/parts-verify.sh", Some(PARTS_VERIFY.to_owned())),
            (
                "scripts/cd/parts-rollback.sh",
                Some(PARTS_ROLLBACK.to_owned()),
            ),
            ("engine", Some("e1\n".to_owned())),
            ("infra", Some("i1\n".to_owned())),
        ],
    )?;
    f.deployed(&f.alice, "dev", None).await?;
    push(
        &f,
        "dev",
        &[
            ("engine", Some("e2\n".to_owned())),
            ("infra", Some("i2\n".to_owned())),
            (
                "barrier",
                Some(format!("users.email is now NOT NULL ({VALUE})\n")),
            ),
        ],
    )?;
    f.deployed(&f.alice, "dev", None).await?;
    let drill = f.drill(1).await?;
    assert_eq!(drill["state"], "passed", "{}", drill["output"]);
    assert_eq!(
        f.rollbacks(),
        ["1 engine e1", "1 engine e2", "1 infra i2"],
        "infra behind its barrier is only redeployed"
    );
    let rollback = f.rollback_of("dev").await?;
    let parts = rollback["parts"].as_array().ok_or("parts")?;
    assert_eq!(
        parts
            .iter()
            .map(|p| p["name"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["engine", "infra"]
    );
    assert_eq!(releases(&parts[1]), ["i2", "i1"]);
    assert!(
        parts[1]["releases"][1]["blocked"]
            .as_str()
            .is_some_and(|b| b.contains("users.email is now NOT NULL")),
        "the page says why before the button: {}",
        parts[1]
    );
    // Review M4: a secret printed in a barrier is redacted before it is kept.
    assert!(
        parts[1]["releases"][0]["barrier"]
            .as_str()
            .is_some_and(|b| b.contains("[secret:DEV_TOKEN]") && !b.contains(VALUE)),
        "{}",
        parts[1]
    );
    let named = f
        .post_as(&f.alice, "/api/sandboxes/s1/cd/dev/rollback", json!({}))
        .await?;
    assert_eq!(named.status, StatusCode::CONFLICT, "{}", named.body);
    assert!(
        named.body.to_string().contains("engine, infra"),
        "a part must be named: {}",
        named.body
    );
    let blocked = f
        .post_as(
            &f.alice,
            "/api/sandboxes/s1/cd/dev/rollback",
            json!({"part": "infra"}),
        )
        .await?;
    assert_eq!(blocked.status, StatusCode::CONFLICT, "{}", blocked.body);
    assert!(
        blocked
            .body
            .to_string()
            .contains("users.email is now NOT NULL"),
        "{}",
        blocked.body
    );
    let run = f
        .rolled_back(&f.alice, "dev", json!({"part": "engine"}))
        .await?;
    assert_eq!(run["state"], "passed", "{}", run["output"]);
    assert_eq!(f.mark("dev-engine")?, "e1");
    assert_eq!(f.mark("dev-infra")?, "i2", "the other part stays");

    // A deploy that ships only infra keeps engine's releases as they are.
    push(
        &f,
        "dev",
        &[
            ("engine", None),
            ("barrier", None),
            ("infra", Some("i3\n".to_owned())),
        ],
    )?;
    f.deployed(&f.alice, "dev", None).await?;
    f.drill(2).await?;
    let parts = f.rollback_of("dev").await?["parts"].clone();
    assert_eq!(parts[0]["current"], "e1");
    assert_eq!(releases(&parts[1]), ["i3", "i2", "i1"]);

    // Review M2: a deploy that ships no part still drills the parts with an earlier release, so
    // production at its commit can be proven.
    let before = f.rollbacks().len();
    push(
        &f,
        "dev",
        &[("infra", None), ("notes.txt", Some("x\n".to_owned()))],
    )?;
    f.deployed(&f.alice, "dev", None).await?;
    let drill = f.drill(3).await?;
    assert_eq!(drill["state"], "passed", "{}", drill["output"]);
    assert_eq!(f.rollbacks()[before..], ["1 infra i2", "1 infra i3"]);
    Ok(())
}

/// Review M1: a drill that fails after going back leaves dev on the earlier release, which the
/// page says, and rolling forward to the new release is offered and works.
#[tokio::test]
async fn a_failed_drill_says_where_it_left_dev() -> TestResult {
    let f = fixture(&rollback_cd()).await?;
    push(&f, "dev", &scripts("1"))?;
    f.deployed(&f.alice, "dev", None).await?;
    std::fs::write(f.marks.join("fail-to-v2"), "")?;
    push(&f, "dev", &[("version", Some("2\n".to_owned()))])?;
    f.deployed(&f.alice, "dev", None).await?;
    let drill = f.drill(1).await?;
    assert_eq!(drill["state"], "failed", "{}", drill["output"]);
    assert_eq!(f.mark("dev")?, "v1", "the drill got back to v1 only");
    let part = &f.rollback_of("dev").await?["parts"][0];
    assert_eq!(part["current"], "v1", "{part}");
    std::fs::remove_file(f.marks.join("fail-to-v2"))?;
    let run = f
        .rolled_back(&f.alice, "dev", json!({"release": "v2"}))
        .await?;
    assert_eq!(run["state"], "passed", "{}", run["output"]);
    assert_eq!(f.mark("dev")?, "v2");
    assert_eq!(f.rollback_of("dev").await?["parts"][0]["current"], "v2");
    Ok(())
}

/// Review (low): a drill that cannot start (here its secret was no longer allowed when the deploy
/// ended) is recorded as an error saying why, audited, and production stays refused.
#[tokio::test]
async fn a_drill_that_cannot_start_is_recorded() -> TestResult {
    use yulius_app::ports::vault::{EnvFiles, VaultStore as _};
    let cd = rollback_cd().replacen("[cd.dev]\n", "[cd.dev]\nsecrets = \"DEV_TOKEN\"\n", 1);
    let f = fixture(&cd).await?;
    f.allow_secret().await?;
    push(&f, "dev", &scripts("1"))?;
    f.deployed(&f.alice, "dev", None).await?;
    push(&f, "dev", &[("version", Some("2\n".to_owned()))])?;
    std::fs::write(f.marks.join("hold"), "")?;
    let started = f.deploy(&f.alice, "dev", None).await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while !f.marks.join("holding").exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the deploy never started"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let now = f.h.store.vault_get(&f.secret).await?.ok_or("secret")?;
    f.h.store
        .vault_set_env_files(
            &f.secret,
            (now.env_files, now.env_requested_by.as_deref()),
            (EnvFiles::No, None),
        )
        .await?;
    std::fs::remove_file(f.marks.join("hold"))?;
    let run = f
        .ended(started.body["run_id"].as_str().ok_or("id")?)
        .await?;
    assert_eq!(run["state"], "passed", "{}", run["output"]);
    let drill = f.drill(1).await?;
    assert_eq!(drill["state"], "error");
    assert!(
        drill["reason"]
            .as_str()
            .is_some_and(|r| r.contains("could not start") && r.contains("DEV_TOKEN")),
        "{drill}"
    );
    let entries = f.h.store.automation_audit("app/dev", 50).await?;
    assert!(
        entries.iter().any(|e| e.action == "cd.drill"
            && e.detail
                .as_deref()
                .is_some_and(|d| d.contains("could not start"))),
        "{entries:?}"
    );
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(
        refused.body.to_string().contains("did not pass"),
        "{}",
        refused.body
    );
    Ok(())
}

/// Review round 2: a release deployed again without its barrier (the migration ran the first
/// time) keeps the barrier, so the release before it stays blocked.
#[tokio::test]
async fn a_redeployed_release_keeps_its_barrier() -> TestResult {
    let f = fixture(&rollback_cd()).await?;
    push(&f, "dev", &scripts("1"))?;
    f.deployed(&f.alice, "dev", None).await?;
    push(
        &f,
        "dev",
        &[
            ("version", Some("2\n".to_owned())),
            ("barrier", Some("orders.total split\n".to_owned())),
        ],
    )?;
    f.deployed(&f.alice, "dev", None).await?;
    f.drill(1).await?;
    push(&f, "dev", &[("barrier", None)])?;
    f.deployed(&f.alice, "dev", None).await?;
    f.drill(2).await?;
    let part = &f.rollback_of("dev").await?["parts"][0];
    assert_eq!(
        part["releases"][0]["barrier"], "orders.total split",
        "{part}"
    );
    assert!(
        part["releases"][1]["blocked"]
            .as_str()
            .is_some_and(|b| b.contains("orders.total split")),
        "{part}"
    );
    let refused = f
        .post_as(
            &f.alice,
            "/api/sandboxes/s1/cd/dev/rollback",
            json!({"release": "v1"}),
        )
        .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    Ok(())
}
