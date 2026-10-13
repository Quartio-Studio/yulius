//! Merging the integration branch into a task's branch costs only what it must (SPEC-083).
//!
//! - **MRG-001.** Yulius has every task add an entry at the top of the changes log and record
//!   its decisions, so every pair of parallel tasks touches the same lines there. Those logs merge
//!   by union in every repository: Yulius keeps `merge=union` for them in the clone's
//!   `info/attributes`, which every task worktree reads, so a repository needs no committed
//!   `.gitattributes` for it. A host's server-side merge ignores merge drivers, so Yulius always
//!   merges the integration branch locally first (SPEC-050 DLV-203).
//! - **REV-001.** A branch that moved past its reviewed head only through Git's own merges of the
//!   integration branch keeps its independent review: such a merge brings in only commits that
//!   were reviewed and merged, and resolves nothing by hand. 33 of 57 merges on 2026-10-12/13
//!   spent a review round on such merges or on renumbering (F 2026-10-13).

use std::path::{Component, Path};

use yulius_domain::spec_format::{self, DecisionsLocation, DocsLayout, MAX_CONFIG_BYTES};

use crate::context::App;
use crate::model::TaskRecord;

/// Most integration merges a review carries over (one per integration merge the agent or Yulius
/// made since the review).
pub const MAX_CARRIED_MERGES: usize = 16;

/// The `info/attributes` lines that make the logs every task appends to merge by union: the
/// changes log, and the decisions file or the index (`README.md`) of the decisions directory.
/// A path a gitattributes pattern would read as more than a path (a glob, an escape, a space) is
/// left out.
#[must_use]
pub fn union_lines(layout: &DocsLayout) -> Vec<String> {
    let decisions = match &layout.decisions {
        DecisionsLocation::Directory(dir) => format!("{}/README.md", dir.trim_end_matches('/')),
        DecisionsLocation::File(file) => file.clone(),
    };
    let mut lines: Vec<String> = Vec::new();
    for path in [layout.changes.as_str(), decisions.as_str()] {
        let line = format!("/{path} merge=union");
        if plain_path(path) && !lines.contains(&line) {
            lines.push(line);
        }
    }
    lines
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
    /// MRG-001: keeps the union merge of the project's shared logs in the repository behind
    /// `worktree` (a task's worktree at its start), from the `yulius.toml` there. Failures are
    /// logged: the task starts anyway.
    pub async fn keep_union_merges(&self, worktree: &Path) {
        let config = crate::delivery::read_repo_file(
            worktree,
            "yulius.toml",
            u64::try_from(MAX_CONFIG_BYTES).unwrap_or(u64::MAX),
        )
        .await
        .ok();
        let lines = union_lines(&spec_format::project_layout(config.as_deref()));
        match self.deps.git.set_merge_attributes(worktree, &lines).await {
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
        let integration = format!(
            "origin/{}",
            crate::delivery::delivery_profile(&sandbox).integration_branch
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

#[cfg(test)]
mod tests {
    use super::*;

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
