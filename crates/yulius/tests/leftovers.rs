//! Leftovers end to end (SPEC-215, ADR-0170): a real store, the real Git adapter, a data
//! directory laid out as the server writes it and a fake isolation whose holders the test starts
//! and stops. A removed sandbox's disposable parts go once nothing of it runs (#625), its system
//! layer once its holder has stopped (#582), a partly cleaned one is finished, what PRJ-009 keeps
//! stays, and every rule is checked again right before a deletion.

use crate::support;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, HarnessOptions, admit_and_sign_in, call, configure_github_app, harness_with,
    seed_github_user, seed_sandbox, seed_task, setup_superadmin_creds,
};
use yulius::wiring::git::GitAdapter;
use yulius_app::error::AppResult;
use yulius_app::leftovers::Leftover;
use yulius_app::ports::isolation::{SandboxIsolation, SandboxJoin, SandboxRef};
use yulius_app::ports::store::SandboxStore as _;
use yulius_app::ports::store_ext::AutomationStore as _;
use yulius_domain::ids::SandboxId;
use yulius_domain::leftovers::{Kind, REMOVED_KEEP_MS};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

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
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Holders the test starts and stops; mapped removals are recorded (and done plainly).
#[derive(Default)]
struct Holders {
    live: Mutex<HashSet<String>>,
    mapped: Mutex<Vec<PathBuf>>,
}

impl Holders {
    fn set_live(&self, sandbox: &str, live: bool) {
        if let Ok(mut set) = self.live.lock() {
            if live {
                set.insert(sandbox.to_owned());
            } else {
                set.remove(sandbox);
            }
        }
    }

    fn mapped(&self) -> Vec<PathBuf> {
        self.mapped.lock().map(|m| m.clone()).unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl SandboxIsolation for Holders {
    async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        Ok(None)
    }
    fn release(&self, _: &SandboxId) {}
    async fn holder_live(&self, name: &str) -> bool {
        self.live.lock().is_ok_and(|set| set.contains(name))
    }
    async fn remove_owned_tree(&self, path: &Path) -> Result<(), String> {
        if let Ok(mut mapped) = self.mapped.lock() {
            mapped.push(path.to_path_buf());
        }
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
            std::fs::remove_dir_all(path).map_err(|e| e.to_string())
        } else {
            std::fs::remove_file(path).map_err(|e| e.to_string())
        }
    }
}

struct World {
    h: support::Harness,
    holders: Arc<Holders>,
    data: PathBuf,
}

fn write(path: &Path, text: &str) -> BoxResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)?;
    Ok(())
}

async fn world() -> BoxResult<World> {
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        ..HarnessOptions::default()
    })?;
    let holders = Arc::new(Holders::default());
    assert!(h.app.set_isolation(holders.clone()));
    seed_github_user(&h, "u-alice", "alice").await?;
    let data = h.app.config.data_dir.clone();
    Ok(World { h, holders, data })
}

impl World {
    fn at(&self, relative: &str) -> PathBuf {
        self.data.join(relative)
    }

    /// A sandbox laid out as the server writes it: a clone with a worktree of task `t-<id>`, CI
    /// checkouts, a browser profile, a home, its system layer, holder state, CD files,
    /// transcripts and (when removed) an export.
    async fn sandbox(&self, id: &str) -> BoxResult<PathBuf> {
        seed_sandbox(&self.h, id, id, "u-alice").await?;
        let dir = self.at(&format!("sandboxes/{id}"));
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo)?;
        git(&repo, &["init", "-q", "-b", "main"])?;
        write(&repo.join("README.md"), "hello\n")?;
        git(&repo, &["add", "-A"])?;
        git(&repo, &["commit", "-q", "-m", "init"])?;
        let task = format!("t-{id}");
        let worktree = dir.join("worktrees").join(&task);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("feat/{task}"),
                worktree.to_str().ok_or("path")?,
            ],
        )?;
        seed_task(
            &self.h,
            &task,
            id,
            "u-alice",
            TaskStatus::Closed,
            Some(&worktree),
        )
        .await?;
        write(&worktree.join("notes.txt"), "uncommitted")?;
        write(&dir.join("ci/jobs/gate/build.log"), "ok")?;
        write(&dir.join("browser/Default/Preferences"), "{}")?;
        write(&dir.join("home/.claude/projects/x.jsonl"), "{}")?;
        write(
            &self.at(&format!("sysroots/{id}/upper/usr/bin/tool")),
            "bin",
        )?;
        write(
            &self.at(&format!("runtime/sandboxes/{id}/holder.log")),
            "log",
        )?;
        write(&self.at(&format!("cd/{id}/source.git/HEAD")), "ref")?;
        write(&self.at(&format!("transcripts/{task}/main.jsonl")), "{}")?;
        Ok(dir)
    }

    async fn remove(&self, id: &str) -> BoxResult<()> {
        self.h
            .store
            .remove_sandbox(&SandboxId::parse(id)?, self.h.app.now())
            .await?;
        write(&self.at(&format!("exports/sandbox-{id}-1.json")), "{}")?;
        Ok(())
    }

    fn later(&self, ms: i64) {
        self.h.clock.0.fetch_add(ms, Ordering::SeqCst);
    }

    async fn scan(&self) -> BoxResult<Vec<Leftover>> {
        Ok(self.h.app.leftovers_scan().await?)
    }

    async fn audit(&self, target: &str, action: &str) -> BoxResult<Vec<String>> {
        Ok(self
            .h
            .store
            .automation_audit(target, 100)
            .await?
            .into_iter()
            .filter(|e| e.action == action)
            .map(|e| format!("{} {}", e.actor, e.detail.unwrap_or_default()))
            .collect())
    }
}

fn kinds(items: &[Leftover]) -> Vec<(Kind, String)> {
    let mut kinds: Vec<(Kind, String)> = items
        .iter()
        .map(|i| (i.kind, i.path.to_string_lossy().into_owned()))
        .collect();
    kinds.sort_by_key(|(_, path)| path.clone());
    kinds
}

/// #625 and PRJ-009: a removed sandbox loses its worktrees, CI checkouts, browser, CD files,
/// system layer and holder state in one automatic round once 10 minutes have passed and nothing
/// of it runs, each audited; its clone, home, transcripts and export stay; a live sandbox keeps
/// everything.
#[tokio::test]
async fn a_removed_sandbox_loses_its_disposable_parts_and_keeps_what_prj_009_keeps() -> BoxResult<()>
{
    let w = world().await?;
    let old = w.sandbox("old").await?;
    let live = w.sandbox("live").await?;
    w.remove("old").await?;

    let early = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert_eq!(early.deleted, 0, "removed a moment ago: {early:?}");
    assert!(old.join("worktrees").exists());

    w.later(REMOVED_KEEP_MS + 1_000);
    let report = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert!(report.failed.is_empty(), "{report:?}");
    for gone in ["worktrees", "ci", "browser"] {
        assert!(
            !old.join(gone).exists(),
            "{gone} of the removed sandbox is gone"
        );
    }
    for gone in ["sysroots/old", "runtime/sandboxes/old", "cd/old"] {
        assert!(!w.at(gone).exists(), "{gone} is gone");
    }
    let mapped = w.holders.mapped();
    assert!(
        mapped.contains(&w.at("sysroots/old")),
        "the system layer goes through the isolation's mapped removal"
    );
    // #675: CI jobs and agents write files there as the sandbox's mapped ids.
    for sandbox_written in ["worktrees", "ci", "browser"] {
        assert!(
            mapped.contains(&old.join(sandbox_written)),
            "{sandbox_written} goes through the mapped removal: {mapped:?}"
        );
    }
    for kept in [
        old.join("repo/README.md"),
        old.join("home/.claude/projects/x.jsonl"),
        w.at("transcripts/t-old/main.jsonl"),
        w.at("exports/sandbox-old-1.json"),
    ] {
        assert!(kept.exists(), "PRJ-009 keeps {}", kept.display());
    }
    assert!(
        !old.join("repo/.git/worktrees/t-old").exists(),
        "Git forgets the removed worktree"
    );
    for kept in [
        "worktrees/t-live/notes.txt",
        "ci/jobs/gate/build.log",
        "browser",
    ] {
        assert!(live.join(kept).exists(), "a live sandbox keeps {kept}");
    }
    assert!(w.at("sysroots/live").exists() && w.at("cd/live").exists());
    let audited = w.audit("old", "leftovers.delete").await?;
    assert_eq!(audited.len(), report.deleted, "{audited:?}");
    assert!(
        audited.iter().any(|a| a.starts_with("yulius ")
            && a.contains("sysroots/old")
            && a.contains("removed-system")),
        "{audited:?}"
    );

    // Everything else stays listed for a person, and only a person deletes it.
    let items = w.scan().await?;
    let everything: Vec<&Leftover> = items
        .iter()
        .filter(|i| i.kind == Kind::RemovedSandbox)
        .collect();
    assert_eq!(everything.len(), 1, "{:?}", kinds(&items));
    let again = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert_eq!(again.deleted, 0, "{again:?}");
    assert!(old.join("repo").exists());
    let manual = w.h.app.delete_leftovers(&items, "root").await;
    assert!(manual.failed.is_empty(), "{manual:?}");
    assert!(!old.exists(), "the person deleted everything of it");
    assert!(!w.at("transcripts/t-old").exists() && !w.at("exports/sandbox-old-1.json").exists());
    assert!(live.join("repo").exists() && w.at("transcripts/t-live").exists());
    Ok(())
}

/// #582: the system layer and holder state stay while the holder runs and go once it stopped.
#[tokio::test]
async fn the_system_layer_waits_for_the_holder_to_stop() -> BoxResult<()> {
    let w = world().await?;
    let old = w.sandbox("old").await?;
    w.remove("old").await?;
    w.later(REMOVED_KEEP_MS + 1_000);
    w.holders.set_live("old", true);

    let report = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(
        !old.join("worktrees").exists(),
        "worktrees do not wait for the holder"
    );
    assert!(
        w.at("sysroots/old/upper/usr/bin/tool").exists() && w.at("runtime/sandboxes/old").exists(),
        "kept while its holder runs"
    );

    w.holders.set_live("old", false);
    let report = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(!w.at("sysroots/old").exists() && !w.at("runtime/sandboxes/old").exists());
    Ok(())
}

/// A sandbox a person half cleaned by hand (worktrees and sysroot gone): the round finishes the
/// rest without failures, and Git's records of the vanished worktrees are pruned.
#[tokio::test]
async fn a_partly_cleaned_sandbox_is_finished() -> BoxResult<()> {
    let w = world().await?;
    let old = w.sandbox("old").await?;
    w.remove("old").await?;
    std::fs::remove_dir_all(old.join("worktrees"))?;
    std::fs::remove_dir_all(w.at("sysroots/old"))?;
    assert!(old.join("repo/.git/worktrees/t-old").exists());
    w.later(REMOVED_KEEP_MS + 1_000);

    let items = w.scan().await?;
    assert!(
        items
            .iter()
            .any(|i| i.kind == Kind::GitMetadata && i.path.ends_with(".git/worktrees/t-old")),
        "{:?}",
        kinds(&items)
    );
    let report = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(!old.join("repo/.git/worktrees/t-old").exists());
    assert!(!old.join("ci").exists() && !w.at("runtime/sandboxes/old").exists());
    // Run again: nothing left to do, nothing fails.
    let again = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert_eq!((again.deleted, again.failed.len()), (0, 0), "{again:?}");

    // A live sandbox's stale record goes too, its live worktree's stays.
    let live = w.sandbox("live").await?;
    git(
        &live.join("repo"),
        &["worktree", "add", "-q", "-b", "wt", "../worktrees/wt"],
    )?;
    std::fs::remove_dir_all(live.join("worktrees/wt"))?;
    w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert!(!live.join("repo/.git/worktrees/wt").exists());
    assert!(live.join("repo/.git/worktrees/t-live").exists());
    Ok(())
}

/// A running CI job (its lane claimed) or a live holder keeps a removed sandbox's files.
#[tokio::test]
async fn a_running_job_or_a_live_holder_blocks_deletion() -> BoxResult<()> {
    let w = world().await?;
    let old = w.sandbox("old").await?;
    w.remove("old").await?;
    w.later(REMOVED_KEEP_MS + 1_000);
    let id = SandboxId::parse("old")?;
    assert!(w.h.app.ci_claim(&id), "a CI run of it executes");

    let report = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert_eq!(report.deleted, 0, "{report:?}");
    assert!(old.join("ci/jobs/gate").exists() && old.join("worktrees").exists());
    w.h.app.ci_release(&id);

    w.holders.set_live("old", true);
    let report = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert!(!old.join("ci").exists(), "{report:?}");
    assert!(
        w.at("sysroots/old").exists(),
        "the live holder keeps its system"
    );
    Ok(())
}

/// LFT-007: the rule is checked again right before each deletion. A CI run or a holder that
/// started after the scan keeps the files the scan listed; the item is reported as kept.
#[tokio::test]
async fn the_rule_is_checked_again_at_deletion() -> BoxResult<()> {
    let w = world().await?;
    let old = w.sandbox("old").await?;
    w.remove("old").await?;
    w.later(REMOVED_KEEP_MS + 1_000);
    let items = w.scan().await?;
    assert!(
        items.iter().any(|i| i.kind == Kind::RemovedSystem)
            && items.iter().any(|i| i.kind == Kind::RemovedCi),
        "{:?}",
        kinds(&items)
    );

    w.holders.set_live("old", true);
    let id = SandboxId::parse("old")?;
    assert!(w.h.app.ci_claim(&id));
    let report = w.h.app.delete_leftovers(&items, "root").await;
    w.h.app.ci_release(&id);
    assert_eq!(report.deleted, 0, "{report:?}");
    assert!(
        report
            .kept
            .iter()
            .any(|k| k.contains("sysroots/old") && k.contains("still running: a CI run")),
        "{report:?}"
    );
    assert!(w.at("sysroots/old").exists() && old.join("ci").exists());

    // Once the run and the holder have ended, the same items go.
    w.holders.set_live("old", false);
    let report = w.h.app.delete_leftovers(&items, "root").await;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(!w.at("sysroots/old").exists() && !old.join("ci").exists());
    Ok(())
}

/// LFT-006: instance leftovers. A smoke test's sandbox whose process is gone, old run
/// directories and transcripts of deleted tasks go by themselves; a directory no sandbox owns
/// and a worktree no task owns wait for a person.
#[tokio::test]
async fn instance_leftovers_are_found_and_only_safe_ones_go_by_themselves() -> BoxResult<()> {
    let w = world().await?;
    let live = w.sandbox("live").await?;
    let smoke = "docker-smoke-4194303";
    write(&w.at(&format!("sandboxes/{smoke}/home/x")), "x")?;
    write(&w.at(&format!("sysroots/{smoke}/upper/x")), "x")?;
    write(&w.at("runs/a-ended/launch.json"), "{}")?;
    write(&w.at("transcripts/t-gone/main.jsonl"), "{}")?;
    write(&w.at("sandboxes/stray/file"), "x")?;
    write(&live.join("worktrees/wt-206/work.txt"), "maybe work")?;
    write(&live.join("attachments/drafts/u-alice/old.png"), "png")?;
    // Real files are dated now; the clock moves two days past them.
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    w.h.clock
        .0
        .store(now_ms + 8 * 24 * 60 * 60 * 1000, Ordering::SeqCst);

    let items = w.scan().await?;
    let found = kinds(&items);
    for (kind, path) in [
        (Kind::StaleSmoke, format!("sandboxes/{smoke}")),
        (Kind::StaleSmoke, format!("sysroots/{smoke}")),
        (Kind::EndedRun, "runs/a-ended".to_owned()),
        (Kind::OrphanTranscripts, "transcripts/t-gone".to_owned()),
        (Kind::Unowned, "sandboxes/stray".to_owned()),
        (
            Kind::OrphanWorktree,
            "sandboxes/live/worktrees/wt-206".to_owned(),
        ),
        (
            Kind::OldDraft,
            "sandboxes/live/attachments/drafts/u-alice/old.png".to_owned(),
        ),
    ] {
        let path = w.at(&path).to_string_lossy().into_owned();
        assert!(
            found.contains(&(kind, path.clone())),
            "{kind:?} {path}: {found:?}"
        );
    }
    let report = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert!(report.failed.is_empty(), "{report:?}");
    for gone in [
        format!("sandboxes/{smoke}"),
        format!("sysroots/{smoke}"),
        "runs/a-ended".to_owned(),
        "transcripts/t-gone".to_owned(),
        "sandboxes/live/attachments/drafts/u-alice/old.png".to_owned(),
    ] {
        assert!(!w.at(&gone).exists(), "{gone} went by itself");
    }
    assert!(
        w.at("sandboxes/stray/file").exists(),
        "no record owns it: a person decides"
    );
    assert!(
        live.join("worktrees/wt-206/work.txt").exists(),
        "it may hold work"
    );
    assert!(
        w.at("transcripts/t-live").exists() && live.join("worktrees/t-live").exists(),
        "a task's files stay"
    );
    Ok(())
}

/// SPEC-219 (review of 5d72af32): a delivery environment run's directory (`runs/env-<run>`) is
/// its own while the run is `running`, however old; once it ended, it is an ended run.
#[tokio::test]
async fn a_running_environment_run_keeps_its_directory() -> BoxResult<()> {
    use yulius_app::delivery::{EnvironmentRunKind, EnvironmentRunRecord, EnvironmentRunState};
    use yulius_app::ports::store_ext::DeliveryStore as _;
    let w = world().await?;
    w.sandbox("live").await?;
    let run = EnvironmentRunRecord {
        id: "r-live".into(),
        sandbox_id: SandboxId::parse("live")?,
        environment: "dev-vm".into(),
        kind: EnvironmentRunKind::Command,
        requested_by: yulius_domain::ids::UserId::parse("u-alice")?,
        command: Some("./deploy.sh".into()),
        commit: None,
        state: EnvironmentRunState::Running,
        output: String::new(),
        exit_code: None,
        started_at: Timestamp(1),
        ended_at: None,
    };
    w.h.store.insert_environment_run(&run).await?;
    write(&w.at("runs/env-r-live/work/log"), "deploying")?;
    write(&w.at("runs/env-r-gone/work/log"), "deployed")?;
    w.h.clock
        .0
        .store(real_now_ms()? + 8 * 24 * 60 * 60 * 1000, Ordering::SeqCst);

    let found = kinds(&w.scan().await?);
    let path = |name: &str| w.at(&format!("runs/{name}")).to_string_lossy().into_owned();
    assert!(
        !found.iter().any(|(_, p)| *p == path("env-r-live")),
        "a running run's directory: {found:?}"
    );
    assert!(
        found.contains(&(Kind::EndedRun, path("env-r-gone"))),
        "{found:?}"
    );
    Ok(())
}

async fn view(h: &support::Harness, creds: &Creds) -> BoxResult<Value> {
    let r = call(&h.router, Method::GET, "/api/admin/leftovers", None, creds).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body)
}

/// Waits (bounded) until the scan or deletion has ended.
async fn settled(h: &support::Harness, creds: &Creds) -> BoxResult<Value> {
    for _ in 0..1_000 {
        let v = view(h, creds).await?;
        if v["job"]["running"] == false {
            return Ok(v);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("the job never ended".into())
}

/// LFT-010: the superadmin lists, rescans and deletes; everyone else is refused; Delete all
/// safe never takes an item that needs a person; unknown ids are refused.
#[tokio::test]
async fn the_superadmin_lists_rescans_and_deletes_through_the_api() -> BoxResult<()> {
    let w = world().await?;
    let admin = setup_superadmin_creds(&w.h).await?;
    configure_github_app(&w.h).await?;
    let member = admit_and_sign_in(&w.h, &admin, "bob", 12, true).await?;
    let old = w.sandbox("old").await?;
    w.remove("old").await?;
    w.later(REMOVED_KEEP_MS + 1_000);

    for (method, path, body) in [
        (Method::GET, "/api/admin/leftovers", None),
        (Method::POST, "/api/admin/leftovers/scan", None),
        (
            Method::POST,
            "/api/admin/leftovers/delete",
            Some(json!({"all_safe": true})),
        ),
    ] {
        let r = call(&w.h.router, method, path, body, &member).await?;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}: {}", r.body);
    }
    assert!(old.join("ci").exists(), "a refused request deletes nothing");

    let before = view(&w.h, &admin).await?;
    assert_eq!(before["items"], json!([]), "nothing scanned yet");
    assert!(before["disk_total"].is_u64() || before["disk_total"].is_null());
    let r = call(
        &w.h.router,
        Method::POST,
        "/api/admin/leftovers/scan",
        None,
        &admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let v = settled(&w.h, &admin).await?;
    let items = v["items"].as_array().ok_or("items")?;
    let of = |kind: &str| {
        items
            .iter()
            .find(|i| i["kind"] == kind)
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(of("removed-ci")["auto"], true, "{v}");
    assert_eq!(of("removed-ci")["sandbox"], "old");
    assert!(of("removed-ci")["bytes"].as_u64().unwrap_or(0) > 0);
    assert_eq!(of("removed-sandbox")["auto"], false, "{v}");
    assert!(v["safe_bytes"].as_u64().unwrap_or(0) > 0);
    assert!(v["total_bytes"].as_u64() > v["safe_bytes"].as_u64());

    let r = call(
        &w.h.router,
        Method::POST,
        "/api/admin/leftovers/delete",
        Some(json!({"ids": ["0000000000000000"]})),
        &admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);

    let r = call(
        &w.h.router,
        Method::POST,
        "/api/admin/leftovers/delete",
        Some(json!({"all_safe": true})),
        &admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let v = settled(&w.h, &admin).await?;
    assert!(!old.join("ci").exists() && !w.at("sysroots/old").exists());
    assert!(
        old.join("repo").exists(),
        "all safe never takes a person's item"
    );
    assert_eq!(v["job"]["action"], "delete");
    assert!(v["job"]["deleted"].as_u64().unwrap_or(0) >= 5, "{v}");
    let left: Vec<&str> = v["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .filter_map(|i| i["kind"].as_str())
        .filter(|kind| *kind != "host-docker")
        .collect();
    assert_eq!(left, ["removed-sandbox"], "the list updates");
    let audited = w.audit("old", "leftovers.delete").await?;
    assert!(
        audited.iter().all(|a| a.starts_with("superadmin ")),
        "{audited:?}"
    );

    let id = of("removed-sandbox")["id"].as_str().ok_or("id")?.to_owned();
    let r = call(
        &w.h.router,
        Method::POST,
        "/api/admin/leftovers/delete",
        Some(json!({"ids": [id]})),
        &admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    settled(&w.h, &admin).await?;
    assert!(!old.exists(), "the person's explicit delete");
    Ok(())
}

/// Waits (bounded) until `path` is gone; `false` after `limit`.
async fn gone_within(path: &Path, limit: Duration) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while path.exists() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    true
}

/// LFT-009: the automatic round runs at start (two minutes in, so after every update) and
/// again every period (an hour); starting writes the CLI's key (0600). The test shortens the
/// periods to 100 ms and 600 ms.
#[tokio::test]
async fn the_round_runs_at_start_and_every_period() -> BoxResult<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let w = world().await?;
    let old = w.sandbox("old").await?;
    w.remove("old").await?;
    w.later(REMOVED_KEEP_MS + 1_000);

    let started = std::time::Instant::now();
    w.h.app
        .start_leftovers_rounds_with(Duration::from_millis(100), Duration::from_millis(600));
    let key = w.at(yulius_app::leftovers::CLI_KEY_FILE);
    assert_eq!(std::fs::metadata(&key)?.permissions().mode() & 0o777, 0o600);
    let next = w.h.app.leftovers_overview().await.next_auto_at;
    assert_eq!(
        next.as_deref(),
        Some(yulius_app::auth::rfc3339(Timestamp(w.h.app.now().0 + 100)).as_str())
    );
    assert!(
        gone_within(&old.join("ci"), Duration::from_secs(10)).await,
        "the round at start"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(100),
        "not before its delay"
    );

    // A leftover made after that round waits for the next one.
    let stray = w.at("runs/a-ended");
    write(&stray.join("launch.json"), "{}")?;
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    w.h.clock
        .0
        .store(now_ms + 2 * 24 * 60 * 60 * 1000, Ordering::SeqCst);
    assert!(
        gone_within(&stray, Duration::from_secs(10)).await,
        "the next round"
    );
    Ok(())
}

/// LFT-012: `yulius leftovers` lists and deletes through the loopback listener with the key the
/// server wrote; a refused confirmation deletes nothing; a wrong key or no server is reported
/// with exit 2.
#[tokio::test]
async fn the_cli_lists_and_deletes_with_the_server_s_key() -> BoxResult<()> {
    use yulius::leftovers_cli::{Action, run_async};
    let w = world().await?;
    let old = w.sandbox("old").await?;
    w.remove("old").await?;
    w.later(REMOVED_KEEP_MS + 1_000);
    w.h.app.write_leftovers_key()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let router = w.h.internal.clone();
    tokio::spawn(async move { axum::serve(listener, router).await });

    let mut out = Vec::new();
    let code = run_async(
        &mut out,
        &w.data,
        &url,
        Action::List { json: false },
        &mut |_| false,
    )
    .await;
    let text = String::from_utf8_lossy(&out).into_owned();
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("removed-ci") && text.contains("safe"),
        "{text}"
    );
    assert!(
        text.contains("removed-sandbox") && text.contains("needs you"),
        "{text}"
    );
    assert!(text.contains("Reclaimable:"), "{text}");

    let delete = |yes| Action::Delete {
        all_safe: true,
        ids: Vec::new(),
        yes,
    };
    let mut out = Vec::new();
    let mut asked = Vec::new();
    let code = run_async(&mut out, &w.data, &url, delete(false), &mut |q| {
        asked.push(q.to_owned());
        false
    })
    .await;
    assert_eq!(code, 2, "{}", String::from_utf8_lossy(&out));
    assert!(
        asked
            .iter()
            .any(|q| q.starts_with("Delete ") && q.contains("Type yes")),
        "{asked:?}"
    );
    assert!(
        old.join("ci").exists(),
        "a refused confirmation deletes nothing"
    );

    let mut out = Vec::new();
    let code = run_async(&mut out, &w.data, &url, delete(true), &mut |_| false).await;
    let text = String::from_utf8_lossy(&out).into_owned();
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("Deleted "), "{text}");
    assert!(!old.join("ci").exists() && !w.at("sysroots/old").exists());
    assert!(old.join("repo").exists(), "the person's item stays");
    let audited = w.audit("old", "leftovers.delete").await?;
    assert!(audited.iter().all(|a| a.starts_with("cli ")), "{audited:?}");

    write(&w.at(yulius_app::leftovers::CLI_KEY_FILE), "00ff")?;
    let mut out = Vec::new();
    let code = run_async(
        &mut out,
        &w.data,
        &url,
        Action::List { json: true },
        &mut |_| false,
    )
    .await;
    let text = String::from_utf8_lossy(&out).into_owned();
    assert_eq!(code, 2, "{text}");
    assert!(text.contains("refused the key"), "{text}");

    let mut out = Vec::new();
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let nowhere = format!("http://{}", closed.local_addr()?);
    drop(closed);
    let code = run_async(
        &mut out,
        &w.data,
        &nowhere,
        Action::List { json: false },
        &mut |_| false,
    )
    .await;
    assert_eq!(code, 2);
    assert!(String::from_utf8_lossy(&out).contains("cannot reach"));
    Ok(())
}

/// LFT-012: the loopback routes refuse a request without the key, with an empty one, a wrong
/// one, and every request before the server wrote its key.
#[tokio::test]
async fn the_loopback_routes_need_the_server_s_key() -> BoxResult<()> {
    let w = world().await?;
    let get = |auth: Option<String>| {
        let router = w.h.internal.clone();
        async move {
            let mut request = axum::http::Request::builder().uri("/internal/leftovers");
            if let Some(auth) = auth {
                request = request.header("authorization", auth);
            }
            let response =
                tower::ServiceExt::oneshot(router, request.body(axum::body::Body::empty())?)
                    .await?;
            Ok::<_, Box<dyn std::error::Error>>(response.status())
        }
    };
    assert_eq!(
        get(Some("Bearer ".into())).await?,
        StatusCode::UNAUTHORIZED,
        "no key written yet"
    );
    w.h.app.write_leftovers_key()?;
    let key = std::fs::read_to_string(w.at(yulius_app::leftovers::CLI_KEY_FILE))?;
    assert!(
        !w.at(".leftovers-cli.key.new").exists(),
        "no staged copy is left"
    );
    for refused in [
        None,
        Some("Bearer ".to_owned()),
        Some(format!("Bearer {}", &key[1..])),
        Some(format!("Bearer {key}0")),
        Some(key.clone()),
    ] {
        assert_eq!(
            get(refused.clone()).await?,
            StatusCode::UNAUTHORIZED,
            "{refused:?}"
        );
    }
    assert_eq!(get(Some(format!("Bearer {key}"))).await?, StatusCode::OK);
    Ok(())
}

fn real_now_ms() -> BoxResult<i64> {
    Ok(i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?)
}

/// A CI run of `sandbox` queued at `at` with jobs of `keys`.
async fn ci_run(w: &World, sandbox: &str, id: &str, at: i64, keys: &[&str]) -> BoxResult<()> {
    use yulius_app::ci::{CiJobRecord, CiRunRecord};
    use yulius_app::ports::store_ext::CiStore as _;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    let run = CiRunRecord {
        id: id.into(),
        sandbox_id: SandboxId::parse(sandbox)?,
        reference: CiRef::PullRequest(1),
        sha: "abc1234".into(),
        source: GateSource::ProjectConfig,
        state: RunState::Passed,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: Timestamp(at),
        started_at: None,
        ended_at: None,
    };
    let jobs: Vec<CiJobRecord> = keys
        .iter()
        .enumerate()
        .map(|(i, key)| CiJobRecord {
            id: format!("{id}-{key}"),
            run_id: id.into(),
            ordinal: u32::try_from(i).unwrap_or(0),
            name: (*key).into(),
            key: (*key).into(),
            command: "true".into(),
            matrix: None,
            state: RunState::Passed,
            reason: None,
            exit_code: Some(0),
            output: String::new(),
            failures: None,
            started_at: None,
            ended_at: None,
            need: yulius_domain::resources::Need::default(),
        })
        .collect();
    w.h.store.insert_ci_run(&run, &jobs).await?;
    Ok(())
}

/// Review blocker: a sandbox's agents and shells can put links into their sandbox directory. A
/// linked `attachments/drafts/<user>` or `ci/jobs` never steers a deletion outside the sandbox:
/// neither the data directory's files nor another sandbox's.
#[tokio::test]
async fn a_link_in_a_sandbox_never_steers_a_deletion_outside_it() -> BoxResult<()> {
    let w = world().await?;
    let live = w.sandbox("live").await?;
    let victim = w.sandbox("victim").await?;
    write(&w.at("master.key"), "the key")?;
    std::fs::create_dir_all(live.join("attachments/drafts"))?;
    std::os::unix::fs::symlink(&w.data, live.join("attachments/drafts/x"))?;
    std::fs::remove_dir_all(live.join("ci/jobs"))?;
    std::os::unix::fs::symlink(&victim, live.join("ci/jobs"))?;
    let now = real_now_ms()? + 8 * 24 * 60 * 60 * 1000;
    w.h.clock.0.store(now, Ordering::SeqCst);
    ci_run(&w, "live", "r1", now - 60_000, &["gate"]).await?;

    let items = w.scan().await?;
    assert!(
        !items
            .iter()
            .any(|i| matches!(i.kind, Kind::OldDraft | Kind::StaleCiCheckout)),
        "nothing reached through a link is listed: {:?}",
        kinds(&items)
    );
    let report = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert!(w.at("master.key").exists(), "{report:?}");
    for kept in [
        "repo/README.md",
        "home/.claude/projects/x.jsonl",
        "worktrees/t-victim/notes.txt",
    ] {
        assert!(victim.join(kept).exists(), "the victim keeps {kept}");
    }
    assert!(w.at("sandboxes").exists() && w.at("transcripts").exists());

    // Even an item listed before the link appeared is refused at deletion.
    let link_free = w.sandbox("other").await?;
    write(&link_free.join("attachments/drafts/u-alice/old.png"), "png")?;
    let items: Vec<Leftover> = w
        .scan()
        .await?
        .into_iter()
        .filter(|i| i.kind == Kind::OldDraft)
        .collect();
    assert_eq!(items.len(), 1, "{:?}", kinds(&items));
    std::fs::rename(
        link_free.join("attachments/drafts/u-alice"),
        link_free.join("attachments/moved"),
    )?;
    std::fs::create_dir_all(w.at("elsewhere"))?;
    write(&w.at("elsewhere/old.png"), "not a draft")?;
    std::os::unix::fs::symlink(
        w.at("elsewhere"),
        link_free.join("attachments/drafts/u-alice"),
    )?;
    let report = w.h.app.delete_leftovers(&items, "root").await;
    assert_eq!(report.deleted, 0, "{report:?}");
    assert!(w.at("elsewhere/old.png").exists());
    Ok(())
}

/// Review major: a worktree an agent made outside the sandbox directory (its own `/tmp`, which
/// the host does not see) keeps its Git record; a record under the sandbox whose directory is gone
/// goes, and only that one.
#[tokio::test]
async fn git_records_of_worktrees_outside_the_sandbox_are_kept() -> BoxResult<()> {
    let w = world().await?;
    let live = w.sandbox("live").await?;
    let outside = tempfile::tempdir()?;
    let review = outside.path().join("review");
    git(
        &live.join("repo"),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "review",
            review.to_str().ok_or("path")?,
        ],
    )?;
    std::fs::remove_dir_all(&review)?;
    git(
        &live.join("repo"),
        &["worktree", "add", "-q", "-b", "gone", "../worktrees/gone"],
    )?;
    std::fs::remove_dir_all(live.join("worktrees/gone"))?;

    let report = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(
        live.join("repo/.git/worktrees/review").exists(),
        "outside the sandbox: kept"
    );
    assert!(
        !live.join("repo/.git/worktrees/gone").exists(),
        "under the sandbox and gone"
    );
    assert!(live.join("repo/.git/worktrees/t-live").exists());
    Ok(())
}

/// Review major: a task's run directories stay while its attempt records exist (the last one
/// holds the terminal's final screen, shown after a restart); one no attempt owns goes after a
/// day.
#[tokio::test]
async fn an_existing_task_keeps_its_run_directories() -> BoxResult<()> {
    use yulius_app::model::{AttemptRecord, LaunchKind};
    use yulius_app::ports::store::TaskStore as _;
    use yulius_domain::ids::{AttemptId, TaskId};
    use yulius_domain::task::AttemptState;
    let w = world().await?;
    w.sandbox("live").await?;
    for (attempt, state) in [
        ("a-last", AttemptState::Exited),
        ("a-live", AttemptState::Live),
    ] {
        let run = w.at(&format!("runs/{attempt}"));
        write(&run.join("final-screen"), "how it ended")?;
        w.h.store
            .insert_attempt(&AttemptRecord {
                id: AttemptId::parse(attempt)?,
                task_id: TaskId::parse("t-live")?,
                seq: if attempt == "a-last" { 1 } else { 2 },
                harness: yulius_domain::harness::Harness::ClaudeCode,
                account_id: None,
                provider_session_id: None,
                launch_kind: LaunchKind::New,
                state,
                run_dir: run.to_string_lossy().into_owned(),
                token_hash: format!("hash-{attempt}"),
                host_token: w.h.app.deps.crypto.seal(&[1u8; 32])?,
                ptyhost_pid: None,
                child_pid: None,
                started_at: w.h.app.now(),
                ended_at: None,
                exit_code: None,
                exit_reason: None,
            })
            .await?;
    }
    write(&w.at("runs/a-orphan/launch.json"), "{}")?;
    w.h.clock
        .0
        .store(real_now_ms()? + 2 * 24 * 60 * 60 * 1000, Ordering::SeqCst);

    let report = w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(
        w.at("runs/a-last/final-screen").exists(),
        "its attempt record owns it"
    );
    assert!(w.at("runs/a-live").exists(), "a live run");
    assert!(!w.at("runs/a-orphan").exists(), "no attempt owns it");
    Ok(())
}

/// Review major: CI checkouts of jobs no run used for 14 days, and of lanes past the configured
/// ones, go while no CI run of the sandbox runs; the jobs still used stay; a sandbox with no
/// recent run keeps every checkout; a run that started after the scan keeps them.
#[tokio::test]
async fn unused_ci_checkouts_go_while_no_run_runs() -> BoxResult<()> {
    let w = world().await?;
    let live = w.sandbox("live").await?;
    let idle = w.sandbox("idle").await?;
    for name in ["gate.2", "old", "gate@7"] {
        write(&live.join(format!("ci/jobs/{name}/f")), "x")?;
    }
    let now = w.h.app.now().0;
    ci_run(&w, "live", "r-new", now - 60_000, &["gate"]).await?;
    ci_run(
        &w,
        "live",
        "r-old",
        now - 20 * 24 * 60 * 60 * 1000,
        &["old"],
    )
    .await?;

    let items = w.scan().await?;
    let stale: Vec<String> = items
        .iter()
        .filter(|i| i.kind == Kind::StaleCiCheckout)
        .filter_map(|i| i.path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    assert_eq!(stale.len(), 2, "{stale:?}");
    assert!(stale.contains(&"old".to_owned()) && stale.contains(&"gate@7".to_owned()));
    assert!(
        !items.iter().any(|i| i.path.starts_with(&idle)),
        "no recent run: idle checkouts are not stale"
    );

    let id = SandboxId::parse("live")?;
    assert!(w.h.app.ci_claim(&id), "a run starts after the scan");
    let report = w.h.app.delete_leftovers(&items, "root").await;
    w.h.app.ci_release(&id);
    assert!(live.join("ci/jobs/old").exists(), "{report:?}");

    w.h.app.run_leftovers_round().await.ok_or("round")?;
    assert!(!live.join("ci/jobs/old").exists() && !live.join("ci/jobs/gate@7").exists());
    assert!(
        w.holders.mapped().contains(&live.join("ci/jobs/old")),
        "#675: a stale checkout goes through the mapped removal"
    );
    assert!(live.join("ci/jobs/gate.2/f").exists(), "a job still used");
    assert!(idle.join("ci/jobs/gate/build.log").exists());
    Ok(())
}

/// LFT-007: a done task's cache listed by a scan stays when the task works again before the
/// deletion.
#[tokio::test]
async fn a_cache_whose_task_works_again_is_kept() -> BoxResult<()> {
    use yulius_app::ports::store::TaskStore as _;
    use yulius_domain::ids::TaskId;
    use yulius_domain::task::Activity;
    let w = world().await?;
    let live = w.sandbox("live").await?;
    let worktree = live.join("worktrees/t-done");
    git(
        &live.join("repo"),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/t-done",
            "../worktrees/t-done",
        ],
    )?;
    write(&worktree.join("Cargo.toml"), "[package]\n")?;
    write(&worktree.join(".gitignore"), "target\n")?;
    write(&worktree.join("target/debug/blob"), "built")?;
    seed_task(
        &w.h,
        "t-done",
        "live",
        "u-alice",
        TaskStatus::Running,
        Some(&worktree),
    )
    .await?;
    let task = TaskId::parse("t-done")?;
    w.h.store
        .set_activity(&task, Some(Activity::Done), w.h.app.now())
        .await?;
    w.later(31 * 60 * 1000);

    let items: Vec<Leftover> = w
        .scan()
        .await?
        .into_iter()
        .filter(|i| i.kind == Kind::TaskCache)
        .collect();
    assert_eq!(items.len(), 1, "{:?}", kinds(&items));
    w.h.store
        .set_activity(&task, Some(Activity::Working), w.h.app.now())
        .await?;
    let report = w.h.app.delete_leftovers(&items, "root").await;
    assert!(
        report.kept.iter().any(|k| k.contains("works again")),
        "{report:?}"
    );
    assert!(worktree.join("target/debug/blob").exists());
    Ok(())
}
