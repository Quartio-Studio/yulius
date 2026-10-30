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

/// Gives `acme/lib` a `dev` branch with a commit of its own and a tag `v1` on `main`; returns
/// `dev`'s commit.
fn add_dev_and_tag(w: &World, lib: &Path) -> Result<String, BoxError> {
    let seed = w.scratch.path().join("seed/acme/lib");
    git(&seed, &["tag", "v1"])?;
    git(&seed, &["checkout", "--quiet", "-b", "dev"])?;
    std::fs::write(seed.join("model.rs"), "pub struct Model;\n")?;
    git(&seed, &["add", "model.rs"])?;
    git(&seed, &["commit", "--quiet", "-m", "model on dev"])?;
    git(
        &seed,
        &[
            "push",
            "--quiet",
            &lib.to_string_lossy(),
            "dev:dev",
            "refs/tags/v1:refs/tags/v1",
        ],
    )?;
    Ok(git(lib, &["rev-parse", "refs/heads/dev"])?
        .trim()
        .to_owned())
}

/// SPEC-330 AC1, AC2 (#915): the task's owner can read every branch, so the task gets every
/// branch and tag: it asks for `dev` and clones `dev`, and the default copy also carries `dev`
/// and the tags.
#[tokio::test]
async fn a_task_gets_every_branch_and_tag_its_owner_can_read() -> TestResult {
    let (w, _alice, lib) = world().await?;
    let dev = add_dev_and_tag(&w, &lib)?;
    let main = git(&lib, &["rev-parse", "refs/heads/main"])?
        .trim()
        .to_owned();

    let asked = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib", "branch": "dev"}),
    )
    .await?;
    assert_ne!(asked["isError"], true, "{asked}");
    let record = support::tool_text(&asked);
    assert_eq!(record["branch"], "dev", "{record}");
    assert_eq!(record["commit"], dev.as_str(), "{record}");
    assert_eq!(record["default_branch"], "main", "{record}");
    let clone = clone_bundle(&record, w.scratch.path())?;
    assert_eq!(
        std::fs::read_to_string(clone.join("model.rs"))?,
        "pub struct Model;\n"
    );

    let default = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    assert_ne!(default["isError"], true, "{default}");
    let record = support::tool_text(&default);
    assert_eq!(record["branch"], "main", "{record}");
    assert_eq!(record["commit"], main.as_str(), "{record}");
    assert_eq!(
        record["branches"],
        json!([{"name": "dev", "commit": dev}, {"name": "main", "commit": main}]),
        "{record}"
    );
    assert_eq!(record["branch_count"], 2, "{record}");
    assert_eq!(record["tag_count"], 1, "{record}");
    let message = record["message"].as_str().unwrap_or_default();
    for command in ["clone_command", "update_command"] {
        let text = record[command].as_str().ok_or(command)?;
        assert!(message.contains(text), "{command}: {record}");
    }
    let clone = clone_bundle(&record, w.scratch.path())?;
    assert!(!clone.join("model.rs").exists());
    assert_eq!(git(&clone, &["rev-parse", "origin/dev"])?.trim(), dev);
    assert_eq!(git(&clone, &["rev-parse", "v1^{commit}"])?.trim(), main);
    assert_eq!(attachments(&w), ["acme-lib.bundle"]);
    assert_eq!(tmp_entries(&w), 0);
    Ok(())
}

/// Runs a command the tool answered through `sh`, as an agent pasting it would; `Err` with its
/// output when it fails.
fn run_answered(record: &Value, command: &str, cwd: &Path) -> Result<(), BoxError> {
    let text = record[command].as_str().ok_or(command)?;
    let out = std::process::Command::new("sh")
        .args(["-c", text])
        .current_dir(cwd)
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "{command} `{text}` failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(())
}

/// SPEC-330 REF-103 (review round 1): the answered commands work as given: clone a branch whose
/// name the shell would expand, then update that copy after the branch moved, a tag was re-pointed
/// and another was added.
#[tokio::test]
async fn the_answered_commands_clone_and_update_a_copy() -> TestResult {
    let (w, _alice, lib) = world().await?;
    let dev = add_dev_and_tag(&w, &lib)?;
    let seed = w.scratch.path().join("seed/acme/lib");
    let odd = "odd$(touch${IFS}pwned)'q";
    git(&seed, &["branch", odd, "dev"])?;
    git(
        &seed,
        &[
            "push",
            "--quiet",
            &lib.to_string_lossy(),
            &format!("refs/heads/{odd}:refs/heads/{odd}"),
        ],
    )?;
    let first = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib", "branch": odd}),
    )
    .await?;
    assert_ne!(first["isError"], true, "{first}");
    let record = support::tool_text(&first);
    let copy = PathBuf::from(record["clone_into"].as_str().ok_or("clone_into")?);
    std::fs::create_dir_all(copy.parent().ok_or("parent")?)?;
    run_answered(&record, "clone_command", w.scratch.path())?;
    assert_eq!(git(&copy, &["rev-parse", "HEAD"])?.trim(), dev);
    assert!(
        !w.scratch.path().join("pwned").exists() && !copy.join("pwned").exists(),
        "the branch name is not run"
    );

    // The owner's repository moves on: the branch gets a commit, v1 is re-pointed, v2 is new.
    git(&seed, &["checkout", "--quiet", odd])?;
    std::fs::write(seed.join("model.rs"), "pub struct Model2;\n")?;
    git(&seed, &["commit", "--quiet", "-am", "model 2"])?;
    git(&seed, &["tag", "-f", "v1", "dev"])?;
    git(&seed, &["tag", "v2"])?;
    git(
        &seed,
        &[
            "push",
            "--quiet",
            "--force",
            &lib.to_string_lossy(),
            &format!("refs/heads/{odd}:refs/heads/{odd}"),
            "refs/tags/v1:refs/tags/v1",
            "refs/tags/v2:refs/tags/v2",
        ],
    )?;
    let moved = git(&lib, &["rev-parse", &format!("refs/heads/{odd}")])?;
    let second = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib", "branch": odd}),
    )
    .await?;
    assert_ne!(second["isError"], true, "{second}");
    let record = support::tool_text(&second);
    run_answered(&record, "update_command", w.scratch.path())?;
    assert_eq!(git(&copy, &["rev-parse", "HEAD"])?.trim(), moved.trim());
    assert_eq!(git(&copy, &["rev-parse", "v1^{commit}"])?.trim(), dev);
    assert_eq!(
        git(&copy, &["rev-parse", "v2^{commit}"])?.trim(),
        moved.trim()
    );
    assert_eq!(
        std::fs::read_to_string(copy.join("model.rs"))?,
        "pub struct Model2;\n"
    );
    assert!(
        !w.scratch.path().join("pwned").exists() && !copy.join("pwned").exists(),
        "the branch name is not run by the update either"
    );
    Ok(())
}

/// SPEC-330 REF-103 (review round 2): an answer lists at most 100 branches by name and counts
/// them all; a missing branch's error says how many it left out.
#[tokio::test]
async fn an_answer_lists_at_most_a_hundred_branches_and_counts_them_all() -> TestResult {
    let (w, _alice, lib) = world().await?;
    let main = git(&lib, &["rev-parse", "refs/heads/main"])?
        .trim()
        .to_owned();
    for n in 0..101 {
        git(&lib, &["update-ref", &format!("refs/heads/b{n:03}"), &main])?;
    }
    let done = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    assert_ne!(done["isError"], true, "{done}");
    let record = support::tool_text(&done);
    assert_eq!(record["branch_count"], 102, "{record}");
    let listed = record["branches"].as_array().ok_or("branches")?;
    assert_eq!(listed.len(), 100);
    assert_eq!(listed[0]["name"], "b000");
    assert!(
        listed.iter().all(|b| b["name"] != "main"),
        "main is the 102nd"
    );

    let missing = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib", "branch": "nope"}),
    )
    .await?;
    assert_eq!(missing["isError"], true, "{missing}");
    let text = support::tool_text(&missing).to_string();
    assert!(
        text.contains("b099") && text.contains("and 2 more"),
        "{text}"
    );
    assert!(!text.contains("b100"), "{text}");
    Ok(())
}

/// SPEC-330 AC3: a branch the repository lacks is the agent's own mistake: invalid, naming the
/// branches, with no person to ask and nothing left behind; a branch Git could read as an option
/// is refused before anything is fetched.
#[tokio::test]
async fn a_branch_the_repository_lacks_is_invalid_and_names_the_branches() -> TestResult {
    let (w, _alice, lib) = world().await?;
    add_dev_and_tag(&w, &lib)?;
    let missing = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib", "branch": "nope"}),
    )
    .await?;
    assert_eq!(missing["isError"], true, "{missing}");
    let text = support::tool_text(&missing).to_string();
    assert!(
        text.contains("nope") && text.contains("dev") && text.contains("main"),
        "{text}"
    );
    assert!(!text.contains("needs_you"), "{text}");
    assert!(attachments(&w).is_empty(), "{:?}", attachments(&w));
    assert_eq!(tmp_entries(&w), 0);

    let fetches = w.git.calls().len();
    for bad in ["-x", "a:b", "main..x", ""] {
        let invalid = support::mcp_tool(
            &w.h,
            "tok-ref",
            "reference_repo",
            json!({"repo": "acme/lib", "branch": bad}),
        )
        .await?;
        assert_eq!(invalid["isError"], true, "{bad}: {invalid}");
    }
    assert_eq!(
        w.git.calls().len(),
        fetches,
        "nothing fetched: {:?}",
        w.git.calls()
    );
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

/// SPEC-330 AC4, the real Git adapter: `bundle_repository` fetches every branch and tag with
/// their history into a new bare repository and writes a bundle of `HEAD` (the default branch),
/// every branch and every tag; a default branch Git could take as an option is refused before Git
/// runs.
#[tokio::test]
async fn the_git_adapter_bundles_every_branch_and_tag() -> TestResult {
    use yulius_app::ports::git::GitPort as _;
    let w = World::new("org").await?;
    let lib = w.remote("acme", "lib", &[("README.md", "# Lib\n")], &["other"])?;
    let dev = add_dev_and_tag(&w, &lib)?;
    let main = git(&lib, &["rev-parse", "main"])?.trim().to_owned();
    // Longer than Yulius's own branch names may be, but Git's: still in the bundle, so counted.
    let long = format!("{}x", "long/".repeat(60));
    git(&lib, &["update-ref", &format!("refs/heads/{long}"), &main])?;
    let adapter = yulius::wiring::git::GitAdapter::new("git")
        .with_github_base(&w.scratch.path().join("remotes"));
    let token = yulius_app::model::Secret("token-alice".into());
    let out = w.scratch.path().join("out");
    std::fs::create_dir_all(&out)?;
    let (work, bundle) = (out.join("repo.git"), out.join("repo.bundle"));
    let carried = adapter
        .bundle_repository(
            "https://github.com/acme/lib.git",
            "main",
            (&work, &bundle),
            yulius_app::reference_repo::MAX_REFERENCE_BYTES,
            &token,
        )
        .await?;
    assert_eq!(
        carried.branches,
        [
            ("dev".to_owned(), dev.clone()),
            (long.clone(), main.clone()),
            ("main".to_owned(), main.clone()),
            ("other".to_owned(), main.clone()),
        ]
    );
    assert_eq!(carried.tags, 1);
    let heads = git(&out, &["bundle", "list-heads", &bundle.to_string_lossy()])?;
    let mut heads: Vec<&str> = heads.lines().collect();
    heads.sort_unstable();
    let tag = git(&lib, &["rev-parse", "refs/tags/v1"])?;
    let mut wanted = [
        format!("{dev} refs/heads/dev"),
        format!("{main} HEAD"),
        format!("{main} refs/heads/main"),
        format!("{main} refs/heads/other"),
        format!("{main} refs/heads/{long}"),
        format!("{} refs/tags/v1", tag.trim()),
    ];
    wanted.sort_unstable();
    assert_eq!(heads, wanted, "HEAD, every branch and the tag");
    let clone = out.join("clone");
    git(
        &out,
        &[
            "clone",
            "--quiet",
            &bundle.to_string_lossy(),
            &clone.to_string_lossy(),
        ],
    )?;
    assert_eq!(std::fs::read_to_string(clone.join("README.md"))?, "# Lib\n");
    assert_eq!(git(&clone, &["rev-parse", "origin/dev"])?.trim(), dev);
    // SPEC-349 AC3: past its byte limit the fetch is stopped as a capacity bound.
    let small = out.join("small");
    std::fs::create_dir_all(&small)?;
    let bounded = adapter
        .bundle_repository(
            "https://github.com/acme/lib.git",
            "main",
            (&small.join("repo.git"), &small.join("repo.bundle")),
            1,
            &token,
        )
        .await;
    assert!(
        matches!(bounded, Err(yulius_app::error::AppError::Capacity(_))),
        "{bounded:?}"
    );
    assert!(
        !small.join("repo.bundle").exists(),
        "stopped before the bundle"
    );
    for bad in ["-x", "a:b", "main..x"] {
        let fresh = out.join(format!("bad-{}", bad.len()));
        let refused = adapter
            .bundle_repository(
                "https://github.com/acme/lib.git",
                bad,
                (&fresh, &out.join("b")),
                yulius_app::reference_repo::MAX_REFERENCE_BYTES,
                &token,
            )
            .await;
        assert!(refused.is_err(), "{bad}");
        assert!(!fresh.exists(), "{bad}: nothing created");
    }
    Ok(())
}

/// A task whose attachments folder is full still refreshes its bundle: replacing it adds no
/// file (review round 2).
#[tokio::test]
async fn a_full_attachments_folder_still_takes_a_fresh_bundle() -> TestResult {
    let (w, _alice, _lib) = world().await?;
    let dir = w
        .data_dir()
        .join("sandboxes")
        .join(SANDBOX)
        .join("attachments/tasks/t-ref");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("acme-lib.bundle"), b"older bundle")?;
    for n in 1..yulius_app::uploads::MAX_ATTACHMENTS_PER_FOLDER {
        std::fs::write(dir.join(format!("shot-{n}.png")), b"x")?;
    }
    let done = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    assert_ne!(done["isError"], true, "{done}");
    let record = support::tool_text(&done);
    let clone = clone_bundle(&record, w.scratch.path())?;
    assert_eq!(
        std::fs::read_to_string(clone.join("README.md"))?,
        "# Lib v1\n"
    );
    assert_eq!(
        attachments(&w).len(),
        yulius_app::uploads::MAX_ATTACHMENTS_PER_FOLDER
    );
    Ok(())
}

/// Whether the Git fake fetched `repo`.
fn fetched(w: &World, repo: &str) -> bool {
    w.git
        .calls()
        .iter()
        .any(|c| c.starts_with("bundle ") && c.contains(repo))
}

/// SPEC-349 AC1 (#920): a repository GitHub reports above the 1 GiB cap is refused before any
/// fetch, saying its size and the cap; one GitHub reports under it is fetched.
#[tokio::test]
async fn a_repository_github_reports_above_the_cap_is_refused_before_any_fetch() -> TestResult {
    let (w, _alice, _lib) = world().await?;
    let sizes = |kib: u64| -> TestResult {
        w.github
            .sizes
            .lock()
            .map_err(|_| "lock")?
            .insert("acme/lib".into(), kib);
        Ok(())
    };
    sizes(2 * 1024 * 1024)?;
    let large = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    refused(&large, "2 GB")?;
    refused(&large, "1 GB")?;
    assert!(!fetched(&w, "acme/lib"), "{:?}", w.git.calls());
    assert_eq!(tmp_entries(&w), 0, "no fetch folder");

    sizes(1024)?;
    let small = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    assert_ne!(small["isError"], true, "{small}");
    assert!(fetched(&w, "acme/lib"), "{:?}", w.git.calls());
    Ok(())
}

/// SPEC-349 AC2 (#920): a fetch that would bring the data disk to its critical level (95 %) is
/// refused before it starts, counting three times GitHub's size (the fetched copy, its bundle and
/// the attached bundle), or three times the cap when GitHub gives none; an unknown disk or one
/// with room refuses nothing.
#[tokio::test]
async fn a_fetch_the_data_disk_has_no_room_for_is_refused_before_it_starts() -> TestResult {
    const GB: u64 = 1_000_000_000;
    let (w, _alice, _lib) = world().await?;
    let call = || {
        support::mcp_tool(
            &w.h,
            "tok-ref",
            "reference_repo",
            json!({"repo": "acme/lib"}),
        )
    };
    let size_mb = |mb: u64| -> TestResult {
        w.github
            .sizes
            .lock()
            .map_err(|_| "lock")?
            .insert("acme/lib".into(), mb * 1_000_000 / 1024);
        Ok(())
    };

    w.h.system.set_disk(100 * GB, 96 * GB);
    let full = call().await?;
    refused(&full, "disk")?;
    assert!(!fetched(&w, "acme/lib"), "{:?}", w.git.calls());
    assert_eq!(tmp_entries(&w), 0, "no fetch folder");

    // No size from GitHub counts the cap: 92 GB and three times 1 GiB pass 95 of 100 GB.
    w.h.system.set_disk(100 * GB, 92 * GB);
    refused(&call().await?, "disk")?;
    assert!(!fetched(&w, "acme/lib"), "{:?}", w.git.calls());

    // 600 MB three times on 8 of 10 GB is 9.8 GB: past the critical level.
    w.h.system.set_disk(10 * GB, 8 * GB);
    size_mb(600)?;
    refused(&call().await?, "disk")?;
    assert!(!fetched(&w, "acme/lib"), "{:?}", w.git.calls());

    // 200 MB three times is 8.6 GB: below it.
    size_mb(200)?;
    let fits = call().await?;
    assert_ne!(fits["isError"], true, "{fits}");
    assert!(fetched(&w, "acme/lib"), "{:?}", w.git.calls());
    Ok(())
}

/// SPEC-349 AC3 (#920): a fetch that grows past the cap while it runs (GitHub reported less) is
/// stopped; the tool says the repository is too large, nothing is attached and the fetch folder
/// is removed.
#[tokio::test]
async fn a_fetch_that_grows_past_the_cap_is_stopped_and_its_folder_removed() -> TestResult {
    let (w, _alice, _lib) = world().await?;
    *w.git.reference_limit.lock().map_err(|_| "lock")? = Some(1);
    let stopped = support::mcp_tool(
        &w.h,
        "tok-ref",
        "reference_repo",
        json!({"repo": "acme/lib"}),
    )
    .await?;
    refused(&stopped, "too large")?;
    refused(&stopped, "1 GB")?;
    assert!(fetched(&w, "acme/lib"), "{:?}", w.git.calls());
    assert_eq!(tmp_entries(&w), 0, "the fetch folder is removed");
    assert!(attachments(&w).is_empty(), "{:?}", attachments(&w));
    Ok(())
}
