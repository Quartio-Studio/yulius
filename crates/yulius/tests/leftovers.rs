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
        std::fs::remove_dir_all(path).map_err(|e| e.to_string())
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
    assert!(
        w.holders.mapped().contains(&w.at("sysroots/old")),
        "the system layer goes through the isolation's mapped removal"
    );
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
