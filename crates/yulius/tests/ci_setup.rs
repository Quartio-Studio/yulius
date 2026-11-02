//! `ci_setup` reads a task worktree on the host only through handles opened from the sandbox
//! directory (SPEC-391 VCT-001, #829, ADR-0174): what the agent's worktree holds is summarized,
//! and a folder there swapped for a link to a host folder reads nothing from it.

use crate::support;

use std::sync::Arc;

use serde_json::json;
use support::{
    TestResult, harness_with_git, mcp_tool, seed_github_user, seed_live_attempt, seed_sandbox,
    seed_task, tool_text,
};
use yulius::wiring::git::GitAdapter;
use yulius_domain::task::TaskStatus;

const TOKEN: &str = "ci-setup-task-token-s1-0123456789abcdef";

#[tokio::test]
async fn ci_setup_reads_the_worktree_and_follows_no_link_out_of_it() -> TestResult {
    let h = harness_with_git(Arc::new(GitAdapter::new("git")))?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    let worktree = h.app.config.data_dir.join("sandboxes/s1/worktrees/t1");
    std::fs::create_dir_all(worktree.join(".github/workflows"))?;
    std::fs::write(
        worktree.join(".github/workflows/ci.yml"),
        "name: own\njobs:\n  test:\n    runs-on: x\n",
    )?;
    std::fs::write(
        worktree.join("yulius.toml"),
        "[commands]\ngate = \"cargo test\"\n",
    )?;
    seed_task(
        &h,
        "t1",
        "s1",
        "u-alice",
        TaskStatus::Running,
        Some(&worktree),
    )
    .await?;
    seed_live_attempt(&h, "t1", TOKEN).await?;

    let own = mcp_tool(&h, TOKEN, "ci_setup", json!({})).await?;
    assert_ne!(own["isError"], true, "{own}");
    let own = tool_text(&own);
    assert_eq!(
        own["github_actions_workflows"][0]["file"], ".github/workflows/ci.yml",
        "{own}"
    );
    assert_eq!(
        own["worktree_ci"]["jobs"][0]["command"], "cargo test",
        "{own}"
    );

    // A host folder outside the sandbox, and `.github` swapped for a link to it.
    let host = tempfile::tempdir()?;
    std::fs::create_dir_all(host.path().join("workflows"))?;
    std::fs::write(
        host.path().join("workflows/leak.yml"),
        "name: host\njobs:\n  leak:\n    runs-on: x\n",
    )?;
    std::fs::remove_dir_all(worktree.join(".github"))?;
    std::os::unix::fs::symlink(host.path(), worktree.join(".github"))?;
    let swapped = mcp_tool(&h, TOKEN, "ci_setup", json!({})).await?;
    assert_ne!(swapped["isError"], true, "{swapped}");
    assert!(!swapped.to_string().contains("leak"), "{swapped}");
    let swapped = tool_text(&swapped);
    assert_eq!(swapped["github_actions_workflows"], json!([]), "{swapped}");
    assert_eq!(
        swapped["worktree_ci"]["jobs"][0]["command"], "cargo test",
        "the worktree's own yulius.toml is still read: {swapped}"
    );
    Ok(())
}
