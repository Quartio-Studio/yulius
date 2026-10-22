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
    assert_eq!(attachments(&w).len(), 2, "{:?}", attachments(&w));
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
            .any(|c| c.contains("secret") || c.contains("private")),
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
