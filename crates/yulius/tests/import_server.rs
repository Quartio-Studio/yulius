//! Server side of `yulius import` over HTTP with real Git: guarded job creation, resumable
//! idempotent chunks, bundle/snapshot/session application, sessions stored as attributed,
//! deduplicated session documents, idempotent re-runs and the native continuation of imported
//! sessions (AC-IMP-1, AC-IMP-2, AC-IMP-4..6, AC-SES-1).

use crate::support;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::repos::{OWNER, World, call_raw, git};
use support::{Creds, Resp, TestResult, call};
use yulius_app::ports::store::{SandboxStore, TaskStore};
use yulius_contracts::import::{
    CreateImportRequest, FileRef, IMPORT_MANIFEST_VERSION, ImportHarness, ImportManifest,
    RepoDescriptor, SessionRef,
};
use yulius_domain::ids::{SandboxId, TaskId};

type Error = Box<dyn std::error::Error>;

const CLAUDE_SESSION: &str = "4f1c2d3e-0000-4000-8000-000000000001";
const MAIN_SESSION: &str = "4f1c2d3e-0000-4000-8000-000000000002";
const LAPTOP_DIR: &str = "/Users/alice/shop";
/// Chunk size used by the test client: larger than the 1 MiB JSON body limit.
const CHUNK: usize = 2 * 1024 * 1024;

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// An import as the CLI would offer it: manifest plus file contents by name.
struct Offer {
    manifest: ImportManifest,
    contents: BTreeMap<String, Vec<u8>>,
}

impl Offer {
    fn body(&self) -> Result<Value, Error> {
        Ok(serde_json::to_value(CreateImportRequest {
            manifest: self.manifest.clone(),
        })?)
    }
}

fn file_ref(w: &World, name: &str, bytes: &[u8]) -> FileRef {
    FileRef {
        name: name.to_owned(),
        size: bytes.len() as u64,
        sha256: w.h.app.deps.crypto.sha256_hex(bytes),
    }
}

/// A laptop checkout of the remote with an unpushed commit on `feature-x` and uncommitted
/// changes, captured as a bundle and a snapshot archive, plus two Claude sessions (one on
/// `feature-x`, one on `main`).
fn laptop_offer(w: &World, remote: &Path) -> Result<Offer, Error> {
    let laptop = w.scratch.path().join("laptop");
    std::fs::create_dir_all(&laptop)?;
    git(
        &laptop,
        &["clone", "--quiet", &remote.to_string_lossy(), "shop"],
    )?;
    let checkout = laptop.join("shop");
    git(&checkout, &["checkout", "--quiet", "-b", "feature-x"])?;
    std::fs::write(checkout.join("src/app.txt"), "v2\n")?;
    git(&checkout, &["commit", "--quiet", "-am", "local work"])?;
    let head = git(&checkout, &["rev-parse", "HEAD"])?.trim().to_owned();

    let out = w.scratch.path().join("upload");
    std::fs::create_dir_all(&out)?;
    let bundle_path = out.join("repo.bundle");
    git(
        &checkout,
        &[
            "bundle",
            "create",
            "--quiet",
            &bundle_path.to_string_lossy(),
            "feature-x",
            "--not",
            "--remotes",
        ],
    )?;
    // Snapshot layout (contracts): files/<path> entries and a NUL-separated DELETED list.
    let stage = out.join("stage");
    std::fs::create_dir_all(stage.join("files/src"))?;
    std::fs::create_dir_all(stage.join("files/notes"))?;
    std::fs::write(stage.join("files/src/app.txt"), "v3-uncommitted\n")?;
    std::fs::write(stage.join("files/notes/new.md"), "# Notes\n")?;
    std::fs::write(stage.join("DELETED"), b"old.txt\0")?;
    let snapshot_path = out.join("snapshot.tar.gz");
    let status = Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .arg("--format=ustar")
        .arg("-czf")
        .arg(&snapshot_path)
        .arg("-C")
        .arg(&stage)
        .args(["files", "DELETED"])
        .status()?;
    assert!(status.success(), "tar failed");

    let padding = "x".repeat(1_500_000);
    let claude = format!(
        concat!(
            "{{\"type\":\"user\",\"cwd\":\"{dir}\",\"sessionId\":\"{id}\",\"gitBranch\":\"feature-x\",\"message\":{{\"role\":\"user\",\"content\":\"Add a cart\"}}}}\n",
            "{{\"type\":\"assistant\",\"cwd\":\"{dir}\",\"sessionId\":\"{id}\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{pad}\"}}]}}}}\n"
        ),
        dir = LAPTOP_DIR,
        id = CLAUDE_SESSION,
        pad = padding
    );
    let subagent =
        format!("{{\"type\":\"user\",\"cwd\":\"{LAPTOP_DIR}/src\",\"isSidechain\":true}}\n");
    let on_main = format!(
        "{{\"type\":\"user\",\"cwd\":\"{LAPTOP_DIR}\",\"sessionId\":\"{MAIN_SESSION}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"Fix login\"}}}}\n"
    );

    let mut contents = BTreeMap::new();
    let bundle = std::fs::read(&bundle_path)?;
    let snapshot = std::fs::read(&snapshot_path)?;
    let claude_name = format!("sessions/claude/{CLAUDE_SESSION}.jsonl");
    let subagent_name = format!("sessions/claude/{CLAUDE_SESSION}/subagents/agent-1.jsonl");
    let main_name = format!("sessions/claude/{MAIN_SESSION}.jsonl");
    let manifest = ImportManifest {
        version: IMPORT_MANIFEST_VERSION,
        repo: RepoDescriptor {
            remote_url: format!("https://github.com/{OWNER}/shop"),
            owner: OWNER.to_owned(),
            name: "shop".to_owned(),
            head_branch: Some("feature-x".to_owned()),
            head_sha: head,
        },
        bundle: Some(file_ref(w, "repo.bundle", &bundle)),
        snapshot: Some(file_ref(w, "snapshot.tar.gz", &snapshot)),
        sessions: vec![
            SessionRef {
                harness: ImportHarness::Claude,
                session_id: CLAUDE_SESSION.to_owned(),
                title: "Add a cart".to_owned(),
                cwd_relative: None,
                started_at: "2026-10-01T10:00:00.000Z".to_owned(),
                ended_at: "2026-10-01T11:00:00.000Z".to_owned(),
                git_branch: Some("feature-x".to_owned()),
                files: vec![
                    file_ref(w, &claude_name, claude.as_bytes()),
                    file_ref(w, &subagent_name, subagent.as_bytes()),
                ],
                redactions: 0,
            },
            SessionRef {
                harness: ImportHarness::Claude,
                session_id: MAIN_SESSION.to_owned(),
                title: "Fix login".to_owned(),
                cwd_relative: None,
                started_at: "2026-10-02T09:00:00.000Z".to_owned(),
                ended_at: "2026-10-02T09:30:00.000Z".to_owned(),
                git_branch: Some("main".to_owned()),
                files: vec![file_ref(w, &main_name, on_main.as_bytes())],
                redactions: 0,
            },
        ],
        since: "2026-09-23T00:00:00.000Z".to_owned(),
    };
    contents.insert("repo.bundle".to_owned(), bundle);
    contents.insert("snapshot.tar.gz".to_owned(), snapshot);
    contents.insert(claude_name, claude.into_bytes());
    contents.insert(subagent_name, subagent.into_bytes());
    contents.insert(main_name, on_main.into_bytes());
    Ok(Offer { manifest, contents })
}

async fn create_job(w: &World, creds: &Creds, body: Value) -> Result<Resp, Error> {
    call(&w.h.router, Method::POST, "/api/import", Some(body), creds).await
}

async fn put_chunk(
    w: &World,
    creds: &Creds,
    job: &str,
    name: &str,
    offset: u64,
    bytes: &[u8],
    digest: Option<&str>,
) -> Result<Resp, Error> {
    let real = w.h.app.deps.crypto.sha256_hex(bytes);
    let path = format!(
        "/api/import/{job}/files/{}?offset={offset}",
        name.replace('/', "%2F")
    );
    call_raw(
        &w.h.router,
        Method::PUT,
        &path,
        bytes.to_vec(),
        &[
            ("x-chunk-sha256", digest.unwrap_or(&real)),
            ("content-type", "application/octet-stream"),
        ],
        creds,
    )
    .await
}

/// Uploads every missing file from its reported offset, as the CLI does.
async fn upload_missing(w: &World, creds: &Creds, offer: &Offer, created: &Value) -> TestResult {
    let job = text(&created["job_id"]);
    for missing in created["missing"].as_array().ok_or("missing")? {
        let name = text(&missing["name"]);
        let bytes = offer.contents.get(name).ok_or("unknown file")?;
        let mut offset = usize::try_from(missing["received_bytes"].as_u64().ok_or("offset")?)?;
        while offset < bytes.len() {
            let end = (offset + CHUNK).min(bytes.len());
            let resp = put_chunk(
                w,
                creds,
                job,
                name,
                offset as u64,
                &bytes[offset..end],
                None,
            )
            .await?;
            assert_eq!(
                resp.status,
                StatusCode::OK,
                "{name}@{offset}: {:?}",
                resp.body
            );
            offset = usize::try_from(resp.body["received_bytes"].as_u64().ok_or("received")?)?;
        }
    }
    Ok(())
}

async fn complete(w: &World, creds: &Creds, job: &str) -> Result<Resp, Error> {
    call(
        &w.h.router,
        Method::POST,
        &format!("/api/import/{job}/complete"),
        None,
        creds,
    )
    .await
}

fn missing_names(created: &Value) -> Vec<(String, u64)> {
    created["missing"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|m| {
                    (
                        text(&m["name"]).to_owned(),
                        m["received_bytes"].as_u64().unwrap_or(0),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn foreign_and_non_github_repositories_are_refused() -> TestResult {
    let w = World::new("org").await?;
    let (_, alice) = w.user("alice", false).await?;
    let (_, carol) = w.user("carol", false).await?;
    let remote = w.remote(
        OWNER,
        "shop",
        &[
            ("README.md", "# Shop\n"),
            ("src/app.txt", "v1\n"),
            ("old.txt", "x\n"),
        ],
        &[],
    )?;
    w.grant("alice", "shop", "admin");
    w.grant("carol", "shop", "push");
    let offer = laptop_offer(&w, &remote)?;

    let mut foreign = offer.manifest.clone();
    foreign.repo.owner = "someone-else".into();
    foreign.repo.remote_url = "https://github.com/someone-else/shop".into();
    let refused = create_job(
        &w,
        &alice,
        serde_json::to_value(CreateImportRequest { manifest: foreign })?,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        text(&refused.body["error"]["message"]).contains("only repositories of Quartio-Studio")
    );

    let mut gitlab = offer.manifest.clone();
    gitlab.repo.remote_url = "https://gitlab.com/Quartio-Studio/shop".into();
    let refused = create_job(
        &w,
        &alice,
        serde_json::to_value(CreateImportRequest { manifest: gitlab })?,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text(&refused.body["error"]["message"]).contains("GitHub repository"));

    let mut bad = offer.manifest.clone();
    bad.version = 2;
    let refused = create_job(
        &w,
        &alice,
        serde_json::to_value(CreateImportRequest { manifest: bad })?,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Adding the project needs admin or maintain (IMP-002).
    let refused = create_job(&w, &carol, offer.body()?).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let unauthenticated = create_job(&w, &Creds::default(), offer.body()?).await?;
    assert_eq!(unauthenticated.status, StatusCode::UNAUTHORIZED);
    assert!(w.h.store.sandboxes().await?.is_empty(), "nothing linked");
    Ok(())
}

#[tokio::test]
async fn import_resumes_applies_and_reruns_idempotently() -> TestResult {
    let w = World::new("org").await?;
    let (_, alice) = w.user("alice", false).await?;
    let (_, carol) = w.user("carol", false).await?;
    let remote = w.remote(
        OWNER,
        "shop",
        &[
            ("README.md", "# Shop\n"),
            ("src/app.txt", "v1\n"),
            ("old.txt", "remove me\n"),
        ],
        &[],
    )?;
    w.grant("alice", "shop", "admin");
    w.grant("carol", "shop", "push");
    let offer = laptop_offer(&w, &remote)?;

    // Job creation links the repository and lists every file.
    let created = create_job(&w, &alice, offer.body()?).await?;
    assert_eq!(created.status, StatusCode::OK, "{:?}", created.body);
    let job = text(&created.body["job_id"]).to_owned();
    let mut expected: Vec<(String, u64)> = offer.contents.keys().map(|k| (k.clone(), 0)).collect();
    expected.sort();
    assert_eq!(missing_names(&created.body), expected);
    let sandboxes = w.h.store.sandboxes().await?;
    assert_eq!(sandboxes.len(), 1);
    let sandbox = sandboxes[0].clone();
    assert_eq!(sandbox.onboarding_state, "pending");

    // AC-IMP-1: an interrupted upload resumes at the received length.
    let bundle = offer.contents.get("repo.bundle").ok_or("bundle")?;
    let half = bundle.len() / 2;
    let first = put_chunk(&w, &alice, &job, "repo.bundle", 0, &bundle[..half], None).await?;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    assert_eq!(first.body["received_bytes"], half as u64);
    let resumed = create_job(&w, &alice, offer.body()?).await?;
    assert_eq!(
        resumed.body["job_id"],
        job.as_str(),
        "the same job is reused"
    );
    assert!(missing_names(&resumed.body).contains(&("repo.bundle".to_owned(), half as u64)));

    // Chunk protocol: idempotent repeats, offset gaps, bad digests, foreign jobs, early completion.
    let repeat = put_chunk(&w, &alice, &job, "repo.bundle", 0, &bundle[..half], None).await?;
    assert_eq!(repeat.status, StatusCode::OK);
    assert_eq!(repeat.body["received_bytes"], half as u64);
    let gap = put_chunk(
        &w,
        &alice,
        &job,
        "repo.bundle",
        half as u64 + 1,
        &bundle[half + 1..],
        None,
    )
    .await?;
    assert_eq!(gap.status, StatusCode::CONFLICT);
    let digest = put_chunk(
        &w,
        &alice,
        &job,
        "repo.bundle",
        half as u64,
        &bundle[half..],
        Some(&"0".repeat(64)),
    )
    .await?;
    assert_eq!(digest.status, StatusCode::UNPROCESSABLE_ENTITY);
    let unknown = put_chunk(&w, &alice, &job, "nope.bin", 0, b"x", None).await?;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let foreign = put_chunk(
        &w,
        &carol,
        &job,
        "repo.bundle",
        half as u64,
        &bundle[half..],
        None,
    )
    .await?;
    assert_eq!(
        foreign.status,
        StatusCode::NOT_FOUND,
        "jobs belong to their creator"
    );
    let early = complete(&w, &alice, &job).await?;
    assert_eq!(early.status, StatusCode::CONFLICT);

    upload_missing(&w, &alice, &offer, &resumed.body).await?;
    let result = complete(&w, &alice, &job).await?;
    assert_eq!(result.status, StatusCode::OK, "{:?}", result.body);
    assert_eq!(
        result.body["tasks_created"], 0,
        "sessions are documents, not tasks"
    );
    assert_eq!(result.body["sessions_imported"], 2);
    assert_eq!(result.body["sessions_skipped_existing"], 0);
    // OBD-001: the upload never starts onboarding; a person starts it in the browser.
    assert_eq!(
        result.body["onboarding"], "pending",
        "the CLI uploads; onboarding waits for Start onboarding on the sandbox page"
    );
    let stored = w.h.store.sandbox(&sandbox.id).await?.ok_or("sandbox")?;
    assert_eq!(stored.onboarding_state, "pending");
    let prep = stored.onboarding_task_id.clone().ok_or("onboarding task")?;
    assert!(
        w.h.store.attempts(&prep).await?.is_empty(),
        "the onboarding task has not run"
    );
    assert_eq!(result.body["onboarding_status"]["ready"], false);
    let pending = result.body["onboarding_status"]["pending"].to_string();
    assert!(
        pending.contains("Start onboarding") && pending.contains("https://yulius.test/s/shop"),
        "{:?}",
        result.body["onboarding_status"]
    );
    assert_eq!(result.body["sandbox_url"], "https://yulius.test/s/shop");
    assert_eq!(
        result.body["sessions_url"],
        "https://yulius.test/s/shop/sessions"
    );
    assert_eq!(result.body["sandbox_id"], sandbox.id.as_str());
    let again = complete(&w, &alice, &job).await?;
    assert_eq!(
        again.body, result.body,
        "completing twice returns the stored result"
    );

    // Bundle → import/<login>/<branch>; snapshot → a committed snapshot branch from HEAD.
    let repo = w
        .data_dir()
        .join("sandboxes")
        .join(sandbox.id.as_str())
        .join("repo");
    let local = git(
        &repo,
        &["log", "-1", "--format=%s", "import/alice/feature-x"],
    )?;
    assert_eq!(local.trim(), "local work");
    let snapshot = "import/alice/snapshot-2026-10-07";
    assert_eq!(
        git(&repo, &["show", &format!("{snapshot}:src/app.txt")])?,
        "v3-uncommitted\n"
    );
    assert_eq!(
        git(&repo, &["show", &format!("{snapshot}:notes/new.md")])?,
        "# Notes\n"
    );
    let tree = git(&repo, &["ls-tree", "-r", "--name-only", snapshot])?;
    assert!(
        !tree.lines().any(|l| l == "old.txt"),
        "deleted path removed: {tree}"
    );
    let parent = git(&repo, &["rev-parse", &format!("{snapshot}~1")])?;
    assert_eq!(
        parent.trim(),
        offer.manifest.repo.head_sha,
        "created from the client's HEAD"
    );
    let worktrees = repo.parent().ok_or("parent")?.join("worktrees");
    assert!(
        std::fs::read_dir(&worktrees)?
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().starts_with("import-")),
        "snapshot worktree removed (the preparation task has its own)"
    );

    // Sessions → read-only session documents attributed to the uploader, no tasks (SES-001).
    let archived = call(
        &w.h.router,
        Method::GET,
        "/api/archived-tasks",
        None,
        &alice,
    )
    .await?;
    assert_eq!(archived.body.as_array().map(Vec::len), Some(0));
    let docs = sessions_of(&w, &alice, &sandbox.id).await?;
    assert_eq!(docs.len(), 2);
    assert!(
        docs.iter()
            .all(|d| d["kind"] == "imported" && d["author"]["login"] == "alice")
    );
    let doc_of = |title: &str| -> Result<String, Error> {
        let doc = docs.iter().find(|d| d["title"] == title).ok_or("doc")?;
        Ok(text(&doc["id"]).to_owned())
    };
    let claude_doc = doc_of("Add a cart")?;
    let main_doc = doc_of("Fix login")?;
    let stored = w
        .data_dir()
        .join("sandboxes")
        .join(sandbox.id.as_str())
        .join("sessions")
        .join(&claude_doc)
        .join(format!("{CLAUDE_SESSION}.jsonl"));
    let claude_name = format!("sessions/claude/{CLAUDE_SESSION}.jsonl");
    assert_eq!(
        &std::fs::read(&stored)?,
        offer.contents.get(&claude_name).ok_or("claude")?
    );
    assert_eq!(
        std::fs::metadata(&stored)?.permissions().mode() & 0o222,
        0,
        "read-only copy"
    );
    assert!(
        stored
            .with_extension("")
            .join("subagents/agent-1.jsonl")
            .exists()
    );
    let page = call(
        &w.h.router,
        Method::GET,
        &format!("/api/sessions/{main_doc}/transcript"),
        None,
        &carol,
    )
    .await?;
    assert_eq!(page.status, StatusCode::OK, "{:?}", page.body);
    assert_eq!(page.body["entries"][0]["blocks"][0]["text"], "Fix login");
    assert_eq!(page.body["session"]["can_continue"], true);
    assert_eq!(
        page.body["session"]["can_delete"], false,
        "only the uploader or an admin deletes"
    );
    // Ordinary tasks wait while the sandbox is being prepared: created, not started.
    let waiting = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{}/tasks", sandbox.id),
        Some(json!({"prompt": "x", "harness": "claude"})),
        &alice,
    )
    .await?;
    assert_eq!(waiting.status, StatusCode::CREATED, "{:?}", waiting.body);
    assert_eq!(waiting.body["status"], "chained");
    let early = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sessions/{claude_doc}/continue"),
        None,
        &carol,
    )
    .await?;
    assert_eq!(
        early.status,
        StatusCode::CONFLICT,
        "continuing a session starts a task: gated while preparing"
    );

    // AC-IMP-6: a re-run uploads nothing and stores nothing twice.
    let rerun = create_job(&w, &alice, offer.body()?).await?;
    assert_eq!(rerun.status, StatusCode::OK);
    assert_ne!(
        rerun.body["job_id"],
        job.as_str(),
        "a completed job is not reused"
    );
    assert_eq!(
        missing_names(&rerun.body),
        Vec::new(),
        "content is already stored"
    );
    let rerun_result = complete(&w, &alice, text(&rerun.body["job_id"])).await?;
    assert_eq!(
        rerun_result.status,
        StatusCode::OK,
        "{:?}",
        rerun_result.body
    );
    assert_eq!(rerun_result.body["sessions_imported"], 0);
    assert_eq!(rerun_result.body["sessions_unchanged"], 2);
    assert_eq!(rerun_result.body["sessions_skipped_existing"], 2);
    assert!(
        git(
            &repo,
            &["rev-parse", "--verify", "--quiet", &format!("{snapshot}-2")]
        )
        .is_err(),
        "an applied snapshot is not applied again"
    );

    // A new session only uploads its own file; a grown session replaces its earlier copy.
    let mut grown = Offer {
        manifest: offer.manifest.clone(),
        contents: offer.contents.clone(),
    };
    let extra_id = "5a5a5a5a-0000-4000-8000-000000000002";
    let extra_name = format!("sessions/claude/{extra_id}.jsonl");
    let extra =
        format!("{{\"type\":\"user\",\"cwd\":\"{LAPTOP_DIR}\",\"sessionId\":\"{extra_id}\"}}\n");
    grown.manifest.sessions.push(SessionRef {
        harness: ImportHarness::Claude,
        session_id: extra_id.to_owned(),
        title: "Write docs".to_owned(),
        cwd_relative: None,
        started_at: "2026-10-03T08:00:00.000Z".to_owned(),
        ended_at: "2026-10-03T08:10:00.000Z".to_owned(),
        git_branch: None,
        files: vec![file_ref(&w, &extra_name, extra.as_bytes())],
        redactions: 3,
    });
    grown
        .contents
        .insert(extra_name.clone(), extra.into_bytes());
    let main_name = format!("sessions/claude/{MAIN_SESSION}.jsonl");
    let longer = format!(
        "{}{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"Fixed.\"}}]}}}}\n",
        String::from_utf8_lossy(offer.contents.get(&main_name).ok_or("main")?)
    );
    let main_index = grown
        .manifest
        .sessions
        .iter()
        .position(|s| s.session_id == MAIN_SESSION)
        .ok_or("main session")?;
    grown.manifest.sessions[main_index].files = vec![file_ref(&w, &main_name, longer.as_bytes())];
    grown
        .contents
        .insert(main_name.clone(), longer.into_bytes());
    let created = create_job(&w, &alice, grown.body()?).await?;
    assert_eq!(
        missing_names(&created.body),
        vec![(main_name.clone(), 0), (extra_name, 0)]
    );
    upload_missing(&w, &alice, &grown, &created.body).await?;
    let grown_result = complete(&w, &alice, text(&created.body["job_id"])).await?;
    assert_eq!(grown_result.body["sessions_imported"], 1);
    assert_eq!(grown_result.body["sessions_updated"], 1);
    assert_eq!(grown_result.body["sessions_unchanged"], 1);
    let docs = sessions_of(&w, &alice, &sandbox.id).await?;
    assert_eq!(docs.len(), 3, "one document per session");
    let written = docs
        .iter()
        .find(|d| d["title"] == "Write docs")
        .ok_or("docs")?;
    assert_eq!(written["redactions"], 3);
    let page = call(
        &w.h.router,
        Method::GET,
        &format!("/api/sessions/{main_doc}/transcript"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(
        page.body["entries"][1]["blocks"][0]["text"], "Fixed.",
        "the replaced copy is the longer transcript"
    );

    // Another person's upload of the same session id keeps the first uploader's document.
    let mut theirs = grown.manifest.clone();
    theirs.bundle = None;
    theirs.snapshot = None;
    let changed = format!("{{\"type\":\"user\",\"sessionId\":\"{MAIN_SESSION}\"}}\n");
    theirs.sessions = vec![SessionRef {
        files: vec![file_ref(&w, &main_name, changed.as_bytes())],
        ..grown.manifest.sessions[main_index].clone()
    }];
    let carol_offer = Offer {
        manifest: theirs,
        contents: [(main_name.clone(), changed.into_bytes())]
            .into_iter()
            .collect(),
    };
    let created = create_job(&w, &carol, carol_offer.body()?).await?;
    assert_eq!(created.status, StatusCode::OK, "{:?}", created.body);
    upload_missing(&w, &carol, &carol_offer, &created.body).await?;
    let taken = complete(&w, &carol, text(&created.body["job_id"])).await?;
    assert_eq!(taken.body["sessions_updated"], 0);
    assert_eq!(taken.body["sessions_unchanged"], 1);
    let docs = sessions_of(&w, &alice, &sandbox.id).await?;
    let main = docs
        .iter()
        .find(|d| d["id"] == main_doc.as_str())
        .ok_or("main")?;
    assert_eq!(main["author"]["login"], "alice");

    // AC-IMP-5: once the sandbox is ready, another member continues imported sessions natively.
    let skip = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{}/onboarding/skip", sandbox.id),
        None,
        &alice,
    )
    .await?;
    assert_eq!(skip.status, StatusCode::OK, "{:?}", skip.body);
    let claude_task = continue_doc(&w, &carol, &claude_doc).await?;
    let main_task = continue_doc(&w, &carol, &main_doc).await?;
    assert_eq!(
        continue_doc(&w, &carol, &claude_doc).await?,
        claude_task,
        "a continued session opens its task"
    );
    let docs = sessions_of(&w, &alice, &sandbox.id).await?;
    let claude = docs
        .iter()
        .find(|d| d["id"] == claude_doc.as_str())
        .ok_or("claude")?;
    assert_eq!(claude["continued_in"]["id"], claude_task.as_str());
    let lineage = w.h.store.provider_sessions(&claude_task).await?;
    assert_eq!(lineage.len(), 1);
    assert_eq!(lineage[0].origin, "imported");
    assert_eq!(lineage[0].session_id, CLAUDE_SESSION);
    assert_imported_resume(&w, &sandbox.id, &claude_task, &main_task).await?;
    Ok(())
}

async fn continue_doc(w: &World, creds: &Creds, doc: &str) -> Result<TaskId, Error> {
    let resp = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sessions/{doc}/continue"),
        None,
        creds,
    )
    .await?;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.body);
    assert_eq!(resp.body["origin"], "import");
    Ok(TaskId::parse(text(&resp.body["id"]))?)
}

async fn sessions_of(w: &World, creds: &Creds, sandbox: &SandboxId) -> Result<Vec<Value>, Error> {
    let list = call(
        &w.h.router,
        Method::GET,
        &format!("/api/sandboxes/{sandbox}/sessions"),
        None,
        creds,
    )
    .await?;
    assert_eq!(list.status, StatusCode::OK, "{:?}", list.body);
    Ok(list.body["sessions"]
        .as_array()
        .ok_or("sessions")?
        .iter()
        .filter(|s| s["kind"] == "imported")
        .cloned()
        .collect())
}

async fn assert_imported_resume(
    w: &World,
    sandbox: &SandboxId,
    claude_task: &TaskId,
    main_task: &TaskId,
) -> TestResult {
    let app = &w.h.app;
    let sandbox_dir = w.data_dir().join("sandboxes").join(sandbox.as_str());
    let session = app.prepare_imported_resume(claude_task).await?;
    assert_eq!(session.as_deref(), Some(CLAUDE_SESSION));
    let task = w.h.store.task(claude_task).await?.ok_or("task")?;
    let worktree = PathBuf::from(task.worktree_path.clone().ok_or("worktree")?);
    assert_eq!(
        worktree,
        sandbox_dir.join("worktrees").join(claude_task.as_str())
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("src/app.txt"))?,
        "v3-uncommitted\n",
        "the worktree starts from the imported snapshot"
    );
    let branch = git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    assert_eq!(Some(branch.trim().to_owned()), task.branch);
    let native =
        app.deps
            .harness
            .claude_transcript(&sandbox_dir.join("home"), &worktree, CLAUDE_SESSION);
    let copied = std::fs::read_to_string(&native)?;
    let cwd = worktree.to_string_lossy().into_owned();
    for line in copied.lines() {
        let record: Value = serde_json::from_str(line)?;
        assert_eq!(record["cwd"], cwd.as_str());
        assert_eq!(record["sessionId"], CLAUDE_SESSION);
    }
    let subagent = native.with_extension("").join("subagents/agent-1.jsonl");
    let record: Value = serde_json::from_str(std::fs::read_to_string(&subagent)?.trim())?;
    assert_eq!(record["cwd"], cwd.as_str());
    // A native continuation is never overwritten by a later preparation.
    let continued = format!("{copied}{{\"type\":\"user\",\"cwd\":\"{cwd}\"}}\n");
    std::fs::write(&native, &continued)?;
    app.prepare_imported_resume(claude_task).await?;
    assert_eq!(std::fs::read_to_string(&native)?, continued);

    let session = app.prepare_imported_resume(main_task).await?;
    assert_eq!(session.as_deref(), Some(MAIN_SESSION));
    let task = w.h.store.task(main_task).await?.ok_or("task")?;
    let main_worktree = PathBuf::from(task.worktree_path.clone().ok_or("worktree")?);
    assert_eq!(
        std::fs::read_to_string(main_worktree.join("src/app.txt"))?,
        "v1\n",
        "the second session ran on main"
    );
    let native =
        app.deps
            .harness
            .claude_transcript(&sandbox_dir.join("home"), &main_worktree, MAIN_SESSION);
    let native_text = std::fs::read_to_string(&native)?;
    let record: Value = serde_json::from_str(native_text.lines().next().ok_or("line")?)?;
    assert_eq!(record["cwd"], main_worktree.to_string_lossy().as_ref());
    assert_eq!(record["message"]["content"], "Fix login");

    // Tasks without an imported session need no preparation.
    let removed_or_other = w.h.store.sandbox(sandbox).await?.ok_or("sandbox")?;
    let prep = removed_or_other.onboarding_task_id.ok_or("prep task")?;
    assert_eq!(app.prepare_imported_resume(&prep).await?, None);
    Ok(())
}
