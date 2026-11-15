//! Merging the integration branch into a task's branch costs only what it must (SPEC-083).
//!
//! - **MRG-001.** Yulius has every task add an entry at the top of the changes log and record
//!   its decisions, so every pair of parallel tasks touches the same lines there. Those logs merge
//!   by union in every repository: Yulius keeps `merge=union` for them in the clone's
//!   `info/attributes`, which every task worktree reads, so a repository needs no committed
//!   `.gitattributes` for it. A host's server-side merge ignores merge drivers, so Yulius always
//!   merges the integration branch locally first (SPEC-050 DLV-203). A changes folder, one note
//!   per change, needs no union merge (SPEC-363).
//! - **REV-001.** A branch that moved past its reviewed head only through Git's own merges of the
//!   integration branch keeps its independent review: such a merge brings in only commits that
//!   were reviewed and merged, and resolves nothing by hand. 33 of 57 merges on 2026-10-12/13
//!   spent a review round on such merges or on renumbering (F 2026-10-13).

use std::path::{Component, Path};

use yulius_domain::ids::SandboxId;
use yulius_domain::spec_format::{self, DecisionsLocation, DocsLayout, MAX_CONFIG_BYTES};

use crate::context::App;
use crate::model::TaskRecord;

/// Most integration merges a review carries over (one per integration merge the agent or Yulius
/// made since the review).
pub const MAX_CARRIED_MERGES: usize = 16;

/// The `info/attributes` lines that make the logs every task appends to merge by union: the
/// changes log (not a changes folder, where each task adds its own note, SPEC-363), and the
/// decisions file or the index (`README.md`) of the decisions directory.
/// A path a gitattributes pattern would read as more than a path (a glob, an escape, a space) is
/// left out.
#[must_use]
pub fn union_lines(layout: &DocsLayout) -> Vec<String> {
    let decisions = match &layout.decisions {
        DecisionsLocation::Directory(dir) => format!("{}/README.md", dir.trim_end_matches('/')),
        DecisionsLocation::File(file) => file.clone(),
    };
    let mut lines: Vec<String> = Vec::new();
    let changes = if layout.changes_folder().is_some() {
        ""
    } else {
        layout.changes.as_str()
    };
    for path in [changes, decisions.as_str()] {
        let line = format!("/{path} merge=union");
        if plain_path(path) && !lines.contains(&line) {
            lines.push(line);
        }
    }
    lines
}

/// The documentation layout of the checkout at `root`, from its `yulius.toml` text `config`,
/// with a changes path that is a directory there read as a changes folder (SPEC-363 CHN-001).
#[must_use]
pub fn checkout_layout(root: &Path, config: Option<&str>) -> DocsLayout {
    let mut layout = spec_format::project_layout(config);
    layout.resolve_changes_folder(|rel| {
        std::fs::symlink_metadata(root.join(rel)).is_ok_and(|meta| meta.is_dir())
    });
    layout
}

/// A repository-relative path of letters, digits, `.`, `_`, `-` and `/` only, with normal
/// components.
fn plain_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= spec_format::MAX_PATH_BYTES
        && !path.contains("//")
        && !path.ends_with('/')
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

impl App {
    /// MRG-001: keeps the union merge of the project's shared logs in `sandbox`'s clone, which
    /// every worktree of it reads, from the `yulius.toml` in `worktree` (a task's worktree at
    /// its start). The clone is named by the sandbox, never asked of the worktree, whose Git
    /// files the agent writes (ADR-0314). Failures are logged: the task starts anyway.
    pub async fn keep_union_merges(&self, sandbox: &SandboxId, worktree: &Path) {
        let config = crate::delivery::read_repo_file(
            worktree,
            "yulius.toml",
            u64::try_from(MAX_CONFIG_BYTES).unwrap_or(u64::MAX),
        )
        .await
        .ok();
        let lines = union_lines(&checkout_layout(worktree, config.as_deref()));
        let repo = self.sandbox_repo(sandbox);
        match self.deps.git.set_merge_attributes(&repo, &lines).await {
            Ok(true) => tracing::info!(lines = ?lines, "the shared logs merge by union"),
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(error = %err, "the union merge of the shared logs could not be set");
            }
        }
    }

    /// REV-001: whether the independent review of `reviewed` (a commit id of at least 7 hex
    /// digits) covers `head` of `task`'s worktree: `head` is `reviewed` plus only Git's own
    /// merges of commits the integration branch already contains, at most
    /// [`MAX_CARRIED_MERGES`].
    pub(crate) async fn review_covers(
        &self,
        task: &TaskRecord,
        worktree: &Path,
        reviewed: &str,
        head: &str,
    ) -> bool {
        self.review_covers_against(task, worktree, reviewed, head, None)
            .await
    }

    /// [`Self::review_covers`], with the merges' second parents judged against `integration`
    /// (a commit or ref) instead of the integration branch's tip: after a merge-commit merge
    /// the merged head itself is in the integration branch, so a merge judged then is judged
    /// against the branch as it was before that merge (SPEC-493 REV-005).
    pub(crate) async fn review_covers_against(
        &self,
        task: &TaskRecord,
        worktree: &Path,
        reviewed: &str,
        head: &str,
        integration: Option<&str>,
    ) -> bool {
        let reviewed = reviewed.trim();
        if reviewed.len() < 7
            || reviewed.len() > 64
            || !reviewed.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return false;
        }
        let Ok(Some(sandbox)) = self.deps.store.sandbox(&task.sandbox_id).await else {
            return false;
        };
        // What the integration branch contains is read from a fresh fetch, not from a
        // remote-tracking ref a worktree could have moved.
        if let Err(err) = self.dlv_refresh_clone(&sandbox).await {
            tracing::warn!(task = %task.id, error = %err, "the clone could not be refreshed before comparing a review");
        }
        let integration = integration.map_or_else(
            || {
                format!(
                    "origin/{}",
                    crate::delivery::delivery_profile(&sandbox).integration_branch
                )
            },
            str::to_owned,
        );
        match self
            .deps
            .git
            .only_clean_merges_since(worktree, reviewed, head, &integration, MAX_CARRIED_MERGES)
            .await
        {
            Ok(covered) => covered,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "the review could not be compared with the head");
                false
            }
        }
    }
}

impl App {
    /// What the commits after the reviewed one changed (SPEC-493 APR-001 to APR-003): `Ok` with
    /// the count when the approval carries (at most [`FOLLOW_UP_LINES`] product lines, beside
    /// clean merges of the integration branch), `Err` with why not: the reviewed commit is not
    /// on the head's way, a merge after it resolved a conflict, a file of the reviewed change left
    /// the pull request's change, more product lines, or what Git said cannot be read.
    ///
    /// [`FOLLOW_UP_LINES`]: yulius_domain::follow_up::FOLLOW_UP_LINES
    pub(crate) async fn follow_up_since_review(
        &self,
        task: &TaskRecord,
        worktree: &Path,
        reviewed: &str,
        head: &str,
    ) -> Result<yulius_domain::follow_up::FollowUp, String> {
        use yulius_domain::follow_up::{
            FOLLOW_UP_LINES, FollowUp, ProductRule, first_test_module_line,
        };
        let git = &self.deps.git;
        let sandbox = self
            .deps
            .store
            .sandbox(&task.sandbox_id)
            .await
            .ok()
            .flatten()
            .ok_or("the sandbox cannot be read")?;
        if let Err(err) = self.dlv_refresh_clone(&sandbox).await {
            tracing::warn!(task = %task.id, error = %err, "the clone could not be refreshed before counting a follow-up");
        }
        let integration = format!(
            "origin/{}",
            crate::delivery::delivery_profile(&sandbox).integration_branch
        );
        let commits = git
            .follow_up_commits(worktree, reviewed, head, &integration)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| {
                "the reviewed commit is not on the head's way, or a merge after it resolved a \
                 conflict or merged something other than the integration branch"
                    .to_owned()
            })?;
        // A file of the reviewed change that is no longer in the pull request's change: what was
        // approved is not what merges (a pull request split anew).
        let changed = |rev: String| {
            let integration = integration.clone();
            async move {
                git.changed_paths(worktree, &integration, &rev)
                    .await
                    .ok()
                    .flatten()
            }
        };
        let (Some(was), Some(now)) = (
            changed(reviewed.to_owned()).await,
            changed(head.to_owned()).await,
        ) else {
            return Err("the files of the change cannot be read".to_owned());
        };
        let left: Vec<&String> = was.iter().filter(|file| !now.contains(file)).collect();
        if !left.is_empty() {
            let named: Vec<String> = left.iter().take(5).map(|f| format!("`{f}`")).collect();
            return Err(format!(
                "{} of the reviewed change left the pull request's change",
                named.join(", ")
            ));
        }
        let mut lines = Vec::new();
        for commit in &commits {
            let files = git
                .commit_lines(worktree, commit)
                .await
                .ok()
                .flatten()
                .ok_or_else(|| format!("what {} changed cannot be read", short(commit)))?;
            for mut file in files {
                if Path::new(&file.path)
                    .extension()
                    .is_some_and(|ext| ext == "rs")
                {
                    let test_line = |rev: String| {
                        let path = file.path.clone();
                        async move {
                            git.show_file(worktree, &rev, &path)
                                .await
                                .ok()
                                .flatten()
                                .and_then(|text| first_test_module_line(&text))
                        }
                    };
                    file.tests_from_new = test_line(commit.clone()).await;
                    file.tests_from_old = test_line(format!("{commit}^")).await;
                }
                lines.push(file);
            }
        }
        let follow = FollowUp::count(&lines, &ProductRule::default());
        if follow.within_bound() {
            Ok(follow)
        } else {
            Err(format!(
                "{}, more than the {FOLLOW_UP_LINES} a follow-up may change",
                follow.describe()
            ))
        }
    }
}

fn short(sha: &str) -> &str {
    sha.get(..9).unwrap_or(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-363 CHN-004: a changes folder (each task adds its own note) has no union line,
    /// named with a trailing `/` or existing as a directory of the checkout.
    #[test]
    fn a_changes_folder_has_no_union_line() -> Result<(), Box<dyn std::error::Error>> {
        let checkout = tempfile::tempdir()?;
        let decisions = vec!["/docs/decisions/README.md merge=union".to_owned()];
        let slash = Some("[docs]\nchanges = \"docs/changes/\"\n");
        assert_eq!(
            union_lines(&checkout_layout(checkout.path(), slash)),
            decisions
        );
        let bare = Some("[docs]\nchanges = \"docs/changes\"\n");
        assert_eq!(
            union_lines(&checkout_layout(checkout.path(), bare)),
            vec![
                "/docs/changes merge=union".to_owned(),
                "/docs/decisions/README.md merge=union".to_owned(),
            ],
            "a file of that name"
        );
        std::fs::create_dir_all(checkout.path().join("docs/changes"))?;
        assert_eq!(
            union_lines(&checkout_layout(checkout.path(), bare)),
            decisions
        );
        Ok(())
    }

    #[test]
    fn the_changes_log_and_the_decision_index_merge_by_union() {
        let layout = DocsLayout::default();
        assert_eq!(
            union_lines(&layout),
            vec![
                "/docs/changes.md merge=union".to_owned(),
                "/docs/decisions/README.md merge=union".to_owned(),
            ]
        );
        let file = DocsLayout {
            decisions: DecisionsLocation::File("knowledge/decisions.md".into()),
            changes: "CHANGELOG.md".into(),
            ..DocsLayout::default()
        };
        assert_eq!(
            union_lines(&file),
            vec![
                "/CHANGELOG.md merge=union".to_owned(),
                "/knowledge/decisions.md merge=union".to_owned(),
            ]
        );
        let odd = DocsLayout {
            decisions: DecisionsLocation::File("CHANGELOG.md".into()),
            changes: "docs/my changes*.md".into(),
            ..DocsLayout::default()
        };
        assert_eq!(
            union_lines(&odd),
            vec!["/CHANGELOG.md merge=union".to_owned()]
        );
        assert!(!plain_path("../x.md") && !plain_path("/x.md") && !plain_path("a//b"));
    }
}
