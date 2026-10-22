//! `reference_repo` (SPEC-267, ADR-0197): a task reads another repository of its project's owner
//! through a bundle Yulius fetches as the task's owner, instead of asking a person for access.

use crate::support;

use std::path::{Path, PathBuf};

use axum::http::Method;
use serde_json::{Value, json};
use support::repos::{World, git};
use support::{Creds, TestResult, call};
use yulius_app::ports::store::UserStore as _;
use yulius_domain::task::TaskStatus;

type BoxError = Box<dyn std::error::Error>;

const SANDBOX: &str = "sb-ref";

/// A sandbox of `acme/shop` owned by alice, with a live task of hers (`tok-ref`) and her read
/// access to `acme/lib`, a remote with one commit on `main`.
async fn world() -> Result<(World, yulius_app::model::UserRecord, PathBuf), BoxError> {
    let w = World::new("org").await?;
    let (alice, _) = w.user("alice", false).await?;
    support::seed_sandbox(&w.h, SANDBOX, "shop", "u-alice").await?;
    std::fs::create_dir_all(w.data_dir().join("sandboxes").join(SANDBOX))?;
    support::seed_task(&w.h, "t-ref", SANDBOX, "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&w.h, "t-ref", "tok-ref").await?;
    let lib = w.remote("acme", "lib", &[("README.md", "# Lib v1\n")], &[])?;
    w.grant("alice", "acme/lib", "pull");
    Ok((w, alice, lib))
}

fn refused(result: &Value, wanted: &str) -> TestResult {
    assert_eq!(result["isError"], true, "{result}");
    let text = support::tool_text(result).to_string();
    assert!(text.contains(wanted), "{text}");
    assert!(text.contains("needs_you"), "{text}");
    Ok(())
}

fn attachments(w: &World) -> Vec<String> {
    let dir = w
        .data_dir()
        .join("sandboxes")
        .join(SANDBOX)
        .join("attachments/tasks/t-ref");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn tmp_entries(w: &World) -> usize {
    std::fs::read_dir(w.data_dir().join("tmp")).map_or(0, Iterator::count)
}

/// Clones the bundle the tool answered, as the agent would, and returns the clone.
fn clone_bundle(record: &Value, into: &Path) -> Result<PathBuf, BoxError> {
    let bundle = record["bundle"].as_str().ok_or("bundle path")?;
    let branch = record["branch"].as_str().ok_or("branch")?;
    let dest = into.join(format!(
        "clone-{}",
        record["commit"].as_str().unwrap_or("x")
    ));
    git(
        into,
        &[
            "clone",
            "--quiet",
            "--branch",
            branch,
            bundle,
            &dest.to_string_lossy(),
        ],
    )?;
    Ok(dest)
}

/// AC1, AC2, AC4: the owner's read access is enough; the agent gets a bundle among its
/// attachments that clones to the repository's files, fetched with the owner's token, and a
/// later call brings the repository's new commit. Nothing stays in the server's own folder.
#[tokio::test]
async fn a_task_gets_a_bundle_of_another_repository_of_its_owner() -> TestResult {
    let (w, _alice, lib) = world().await?;
    let first = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    assert_ne!(first["isError"], true, "{first}");
    let record = support::tool_text(&first);
    let bundle = record["bundle"].as_str().ok_or("bundle")?;
    let attachments_dir = w
        .data_dir()
        .join("sandboxes")
        .join(SANDBOX)
        .join("attachments/tasks/t-ref");
    assert!(
        Path::new(bundle).starts_with(std::path::absolute(&attachments_dir)?),
        "{record}"
    );
    assert_eq!(record["branch"], "main", "{record}");
    let head = git(&lib, &["rev-parse", "refs/heads/main"])?;
    assert_eq!(record["commit"], head.trim(), "{record}");
    assert!(
        record["clone_into"]
            .as_str()
            .is_some_and(|p| p.ends_with("/reference/acme/lib")),
        "{record}"
    );
    let instructions = record["message"].as_str().unwrap_or_default();
    assert!(instructions.contains("git clone"), "{record}");
    let clone = clone_bundle(&record, w.scratch.path())?;
    assert_eq!(
        std::fs::read_to_string(clone.join("README.md"))?,
        "# Lib v1\n"
    );
    assert!(
        w.git
            .calls()
            .iter()
            .any(|c| c.contains("acme/lib") && c.contains("token=token-alice")),
        "fetched as the task's owner: {:?}",
        w.git.calls()
    );
    assert_eq!(tmp_entries(&w), 0, "the fetch folder is removed");

    // AC2: the repository moved; a second call brings its new commit.
    let seed = w.scratch.path().join("seed/acme/lib");
    std::fs::write(seed.join("README.md"), "# Lib v2\n")?;
    git(&seed, &["commit", "--quiet", "-am", "v2"])?;
    git(
        &seed,
        &["push", "--quiet", &lib.to_string_lossy(), "HEAD:main"],
    )?;
    let second = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    let record = support::tool_text(&second);
    let moved = git(&lib, &["rev-parse", "refs/heads/main"])?;
    assert_eq!(record["commit"], moved.trim(), "{record}");
    let clone = clone_bundle(&record, w.scratch.path())?;
    assert_eq!(
        std::fs::read_to_string(clone.join("README.md"))?,
        "# Lib v2\n"
    );
    // A fresh copy replaces the earlier bundle of the same repository instead of piling up.
    assert_eq!(
        attachments(&w),
        ["acme-lib.bundle"],
        "{:?}",
        attachments(&w)
    );
    assert_eq!(tmp_entries(&w), 0);
    Ok(())
}

/// AC3: another owner's repository, the sandbox's own, one the owner cannot read and a disabled
/// owner are refused and sent to `needs_you`; a malformed name is invalid; nothing is written.
#[tokio::test]
async fn reading_beyond_the_owner_s_reach_is_refused() -> TestResult {
    let (w, alice, _lib) = world().await?;
    w.remote("other-org", "secret", &[("a.txt", "a\n")], &[])?;
    w.grant("alice", "other-org/secret", "pull");
    let other = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "other-org/secret"}),
    )
    .await?;
    refused(&other, "acme")?;

    let own = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/shop"}),
    )
    .await?;
    assert_eq!(own["isError"], true, "{own}");
    assert!(
        support::tool_text(&own).to_string().contains("worktree"),
        "{own}"
    );

    w.remote("acme", "private", &[("a.txt", "a\n")], &[])?;
    let unreadable = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/private"}),
    )
    .await?;
    refused(&unreadable, "cannot read")?;

    // A repository GitHub moved to another owner is not that owner's any more (its redirect).
    w.remote("other-org", "old", &[("a.txt", "a\n")], &[])?;
    w.grant("alice", "other-org/old", "pull");
    w.github
        .transferred
        .lock()
        .map_err(|_| "lock")?
        .insert("acme/old".into(), "other-org/old".into());
    let moved = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/old"}),
    )
    .await?;
    refused(&moved, "other-org")?;

    // AC4: a fetch that fails leaves no folder of the server's behind.
    w.grant("alice", "acme/ghost", "pull");
    let failed = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/ghost"}),
    )
    .await?;
    refused(&failed, "Fetching acme/ghost")?;
    assert_eq!(tmp_entries(&w), 0);

    w.h.store
        .update_user_flags(&alice.id, Some(Some(w.h.app.now())), None)
        .await?;
    let disabled = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    refused(&disabled, "disabled")?;
    w.h.store
        .update_user_flags(&alice.id, Some(None), None)
        .await?;

    for bad in ["lib", "acme/lib/x", "../lib", "acme/.git", "-x/lib", ""] {
        let invalid =
            support::mcp_tool(&w.h, "tok-ref", "reference_repo", json!({"repo": bad})).await?;
        assert_eq!(invalid["isError"], true, "{bad}: {invalid}");
    }
    assert!(attachments(&w).is_empty(), "{:?}", attachments(&w));
    assert!(
        !w.git
            .calls()
            .iter()
            .any(|c| c.contains("secret") || c.contains("private") || c.contains("old")),
        "nothing fetched: {:?}",
        w.git.calls()
    );
    assert_eq!(tmp_entries(&w), 0);
    Ok(())
}

/// AC5: agents are sent to the tool before a person.
#[tokio::test]
async fn needs_you_sends_agents_to_reference_repo_first() -> TestResult {
    let (w, _alice, _lib) = world().await?;
    let listed = call(
        &w.h.internal,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})),
        &Creds {
            bearer: Some("tok-ref".into()),
            ..Creds::default()
        },
    )
    .await?;
    let tools = listed.body["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .clone();
    let description = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .and_then(|t| t["description"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    assert!(!description("reference_repo").is_empty(), "{tools:?}");
    assert!(
        description("needs_you").contains("reference_repo"),
        "{}",
        description("needs_you")
    );
    Ok(())
}

/// REF-003: a playground has no GitHub owner, and an owner without a usable GitHub sign-in cannot
/// be acted for; both are refused before anything is fetched.
#[tokio::test]
async fn a_playground_and_an_owner_without_github_are_refused() -> TestResult {
    use yulius_app::ports::store::SandboxStore as _;
    use yulius_domain::ids::{SandboxId, UserId};
    let (w, _alice, _lib) = world().await?;
    let mut playground =
        w.h.store
            .sandbox(&SandboxId::parse(SANDBOX)?)
            .await?
            .ok_or("sandbox")?;
    playground.id = SandboxId::parse("sb-play")?;
    playground.slug = "play".into();
    playground.repo_owner = String::new();
    playground.repo_name = "play".into();
    playground.github_repo_id = None;
    playground.port_base = w.h.store.next_port_base(44000, 20).await?;
    w.h.store.insert_sandbox(&playground).await?;
    support::seed_task(
        &w.h,
        "t-play",
        "sb-play",
        "u-alice",
        TaskStatus::Running,
        None,
    )
    .await?;
    support::seed_live_attempt(&w.h, "t-play", "tok-play").await?;
    let played = support::mcp_tool(
        &w.h,
        "tok-play",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    assert_eq!(played["isError"], true, "{played}");
    assert!(
        support::tool_text(&played)
            .to_string()
            .contains("playground"),
        "{played}"
    );

    // Carol has no GitHub token stored: Yulius cannot act as her.
    let carol = yulius_app::model::UserRecord {
        id: UserId::parse("u-carol")?,
        github_login: Some("carol".into()),
        github_id: Some(4242),
        display_name: "CAROL".into(),
        avatar_url: None,
        is_superadmin: false,
        is_instance_admin: false,
        password_hash: None,
        created_at: yulius_domain::time::Timestamp(1),
        disabled_at: None,
    };
    w.h.store.insert_user(&carol).await?;
    w.grant("carol", "acme/lib", "pull");
    support::seed_task(
        &w.h,
        "t-carol",
        SANDBOX,
        "u-carol",
        TaskStatus::Running,
        None,
    )
    .await?;
    support::seed_live_attempt(&w.h, "t-carol", "tok-carol").await?;
    let unsigned = support::mcp_tool(
        &w.h,
        "tok-carol",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    refused(&unsigned, "cannot act as this task's owner")?;
    assert!(
        w.git.calls().iter().all(|c| !c.contains("bundle")),
        "{:?}",
        w.git.calls()
    );
    Ok(())
}

/// REF-001: a fetch folder left by a restart in the middle of a fetch is swept by the next call;
/// a recent one, maybe still in use, stays.
#[tokio::test]
async fn an_old_fetch_folder_is_swept() -> TestResult {
    let (w, _alice, _lib) = world().await?;
    let tmp = w.data_dir().join("tmp");
    let (old, recent) = (tmp.join("reference-old"), tmp.join("reference-recent"));
    std::fs::create_dir_all(old.join("repo.git"))?;
    std::fs::write(old.join("repo.bundle"), b"partial")?;
    std::fs::create_dir_all(&recent)?;
    let aged = std::process::Command::new("touch")
        .args(["-d", "3 hours ago"])
        .arg(&old)
        .status()?;
    assert!(aged.success());
    let done = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    assert_ne!(done["isError"], true, "{done}");
    assert!(!old.exists(), "the old fetch folder is swept");
    assert!(recent.exists(), "a recent one may be a running fetch");
    Ok(())
}

/// The real Git adapter: `bundle_branch` fetches one branch with its history into a new bare
/// repository and writes a bundle that clones to it; a branch Git could take as an option or a
/// refspec is refused before Git runs.
#[tokio::test]
async fn the_git_adapter_bundles_one_branch() -> TestResult {
    use yulius_app::ports::git::GitPort as _;
    let w = World::new("org").await?;
    let lib = w.remote("acme", "lib", &[("README.md", "# Lib\n")], &["other"])?;
    let adapter = yulius::wiring::git::GitAdapter::new("git")
        .with_github_base(&w.scratch.path().join("remotes"));
    let token = yulius_app::model::Secret("token-alice".into());
    let out = w.scratch.path().join("out");
    std::fs::create_dir_all(&out)?;
    let (work, bundle) = (out.join("repo.git"), out.join("repo.bundle"));
    let commit = adapter
        .bundle_branch(
            "https://github.com/acme/lib.git",
            "main",
            &work,
            &bundle,
            &token,
        )
        .await?;
    assert_eq!(commit, git(&lib, &["rev-parse", "main"])?.trim());
    let heads = git(&out, &["bundle", "list-heads", &bundle.to_string_lossy()])?;
    assert_eq!(
        heads.trim(),
        format!("{commit} refs/heads/main"),
        "only the branch"
    );
    let clone = out.join("clone");
    git(
        &out,
        &[
            "clone",
            "--quiet",
            "--branch",
            "main",
            &bundle.to_string_lossy(),
            &clone.to_string_lossy(),
        ],
    )?;
    assert_eq!(std::fs::read_to_string(clone.join("README.md"))?, "# Lib\n");
    for bad in ["-x", "a:b", "main..x"] {
        let fresh = out.join(format!("bad-{}", bad.len()));
        let refused = adapter
            .bundle_branch(
                "https://github.com/acme/lib.git",
                bad,
                &fresh,
                &out.join("b"),
                &token,
            )
            .await;
        assert!(refused.is_err(), "{bad}");
        assert!(!fresh.exists(), "{bad}: nothing created");
    }
    Ok(())
}
