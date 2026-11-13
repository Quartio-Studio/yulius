//! The Yulius `pre-push` guard (SPEC-002 §8, DSC-007).
//!
//! Design: the guard lives in a Yulius-owned hooks directory inside the worktree's own Git
//! directory (`<git-dir>/yulius-hooks`), activated with a worktree-scoped
//! `core.hooksPath` (`extensions.worktreeConfig`). Nothing in the working tree changes (a
//! tracked `.githooks/` directory is never written), other worktrees and the primary
//! checkout are unaffected, and `git worktree remove` deletes it with the worktree.
//!
//! Chaining: at run time each Yulius hook resolves the hooks directory Git would use without
//! Yulius — the last `core.hooksPath` outside worktree scope (relative values resolve
//! against the worktree root, as Git does), else `<common-dir>/hooks` — and runs the
//! repository's hook of the same name there, so existing hooks (including `pre-push`) are
//! preserved untouched and `core.hooksPath` set later by a preparation task is honored.
//!
//! The guard is a guardrail for cooperative agents, not a security boundary: `--no-verify`
//! skips it. GitHub rulesets and token scoping are the enforcement layer.

use std::path::{Path, PathBuf};

use crate::Git;
use crate::error::GitError;
use crate::runner::Invocation;
use crate::validate::validate_branch_name;

/// Directory name of the Yulius hooks inside a worktree's Git directory.
pub const HOOKS_DIR_NAME: &str = "yulius-hooks";
const LIB_NAME: &str = "yulius-hooks-lib.sh";

/// Client-side hooks forwarded to the repository's own hooks. Hooks whose mere presence
/// changes Git behavior (`reference-transaction`, `fsmonitor-watchman`, `push-to-checkout`,
/// `post-index-change`) are deliberately not shimmed.
const FORWARDED_HOOKS: &[&str] = &[
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "post-rewrite",
    "pre-auto-gc",
];

const LIB_SCRIPT: &str = r#"# Sourced by Yulius hook shims; Yulius rewrites this file.
# Resolves the hooks directory Git would use without Yulius: the last core.hooksPath outside
# worktree scope (relative to the worktree root, like Git), else <common-dir>/hooks.
yulius_self_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
yulius_prev_hooks=$(git config --show-scope --get-all core.hooksPath 2>/dev/null | awk '{ scope = $0; sub(/\t.*/, "", scope); value = $0; sub(/^[^\t]*\t/, "", value); if (scope != "worktree") last = value } END { print last }')
if [ -z "$yulius_prev_hooks" ]; then
  yulius_prev_hooks="$(git rev-parse --git-common-dir)/hooks"
fi
case $yulius_prev_hooks in
  /*) ;;
  "~/"*) yulius_prev_hooks="$HOME/${yulius_prev_hooks#"~/"}" ;;
  *) yulius_prev_hooks="$PWD/$yulius_prev_hooks" ;;
esac
yulius_prev_real=$(CDPATH= cd -- "$yulius_prev_hooks" 2>/dev/null && pwd -P)
yulius_has_prev() {
  [ -x "$yulius_prev_hooks/$1" ] && [ "$yulius_prev_real" != "$yulius_self_dir" ]
}
"#;

fn shell_single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn pre_push_script(protected: &[String]) -> String {
    let branches: Vec<String> = protected.iter().map(|b| shell_single_quote(b)).collect();
    format!(
        r#"#!/bin/sh
# Installed by Yulius: refuses pushes to protected branches, then runs the repository's own
# pre-push hook. Yulius rewrites this file; do not edit.
. "$(dirname -- "$0")/{LIB_NAME}"
yulius_input=$(cat)
yulius_refused=0
while read -r yulius_local_ref yulius_local_oid yulius_remote_ref yulius_remote_oid; do
  [ -n "$yulius_remote_ref" ] || continue
  for yulius_branch in {branches}; do
    if [ "$yulius_remote_ref" = "refs/heads/$yulius_branch" ]; then
      printf 'yulius: push to protected branch %s refused; push a task branch and open a pull request\n' "$yulius_branch" >&2
      yulius_refused=1
    fi
  done
done <<YULIUS_EOF
$yulius_input
YULIUS_EOF
[ "$yulius_refused" -eq 0 ] || exit 1
if yulius_has_prev pre-push; then
  if [ -n "$yulius_input" ]; then
    printf '%s\n' "$yulius_input" | "$yulius_prev_hooks/pre-push" "$@"
  else
    "$yulius_prev_hooks/pre-push" "$@" </dev/null
  fi
  exit $?
fi
exit 0
"#,
        branches = branches.join(" ")
    )
}

fn forwarder_script(name: &str) -> String {
    format!(
        r#"#!/bin/sh
# Installed by Yulius: runs the repository's own {name} hook, if any.
. "$(dirname -- "$0")/{LIB_NAME}"
if yulius_has_prev {name}; then
  exec "$yulius_prev_hooks/{name}" "$@"
fi
exit 0
"#
    )
}

impl Git {
    /// Installs (or rewrites) the pre-push guard for `worktree`, a worktree of the clone `repo`
    /// (or the clone itself), refusing pushes whose remote ref is `refs/heads/<b>` for any `b`
    /// in `protected_branches`. Existing repository hooks are preserved and chained (see the
    /// module docs). Returns the path of the installed `pre-push` script.
    ///
    /// The hooks are written below `repo`'s own `.git` through handles that follow no link
    /// (ADR-0314): the agents of the sandbox write there, and a new worktree's Git directory
    /// exists before its checkout ends. Git says which of the clone's Git directories is the
    /// worktree's; only that name is taken from it, never a path to write to.
    ///
    /// # Errors
    /// Invalid branch names; a worktree of another repository; a link or a file where a folder
    /// of the Git directory is; Git or file failures.
    #[cfg(unix)]
    pub fn install_pre_push_guard(
        &self,
        repo: &Path,
        worktree: &Path,
        protected_branches: &[String],
    ) -> Result<PathBuf, GitError> {
        use crate::handles::{open_dir_in, replace_in};
        use nix::sys::stat::Mode;
        if protected_branches.is_empty() {
            return Err(GitError::InvalidInput {
                what: "protected branches",
                reason: "empty".to_owned(),
            });
        }
        for branch in protected_branches {
            validate_branch_name(branch)?;
        }
        let (git_dir_path, git_dir) = self.git_dir_below(repo, worktree)?;
        let hooks = open_dir_in(&git_dir, HOOKS_DIR_NAME, true)?.ok_or(GitError::InvalidInput {
            what: "repository",
            reason: "its hooks folder could not be made".to_owned(),
        })?;
        let (plain, executable) = (
            Mode::from_bits_truncate(0o644),
            Mode::from_bits_truncate(0o755),
        );
        replace_in(&hooks, LIB_NAME, LIB_SCRIPT.as_bytes(), plain)?;
        for name in FORWARDED_HOOKS {
            replace_in(&hooks, name, forwarder_script(name).as_bytes(), executable)?;
        }
        let script = pre_push_script(protected_branches);
        replace_in(&hooks, "pre-push", script.as_bytes(), executable)?;
        let hooks_dir = git_dir_path.join(HOOKS_DIR_NAME);
        self.enable_worktree_config(worktree)?;
        self.run_ok(
            Invocation::new(self, "config", Some(worktree))
                .args(["config", "--worktree", "core.hooksPath"])
                .arg(&hooks_dir),
        )?;
        Ok(hooks_dir.join("pre-push"))
    }

    /// Off Unix there are no no-follow handles, and no contained agents.
    #[cfg(not(unix))]
    pub fn install_pre_push_guard(
        &self,
        _: &Path,
        _: &Path,
        _: &[String],
    ) -> Result<PathBuf, GitError> {
        Err(GitError::InvalidInput {
            what: "pre-push guard",
            reason: "the guard needs a Unix host".to_owned(),
        })
    }

    /// The Git directory of `worktree` as Git names it, and a handle of it opened below the
    /// clone `repo`'s `.git` without following a link: `.git` itself for the clone, else
    /// `.git/worktrees/<name>`. A worktree whose Git directory is elsewhere is refused.
    #[cfg(unix)]
    fn git_dir_below(
        &self,
        repo: &Path,
        worktree: &Path,
    ) -> Result<(PathBuf, std::os::fd::OwnedFd), GitError> {
        use crate::handles::{open_dir_in, open_git_dir};
        let elsewhere = || GitError::InvalidInput {
            what: "worktree",
            reason: "its Git directory is not in the repository".to_owned(),
        };
        let told = self.run_ok(Invocation::new(self, "rev-parse", Some(worktree)).args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ]))?;
        let text = told.stdout_trimmed();
        let (git_dir, common) = text.split_once('\n').ok_or_else(elsewhere)?;
        let git_dir = PathBuf::from(git_dir);
        let below = git_dir
            .strip_prefix(common)
            .map_err(|_| elsewhere())?
            .to_path_buf();
        let names: Vec<&str> = below
            .components()
            .map(|part| match part {
                std::path::Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .collect::<Option<_>>()
            .ok_or_else(elsewhere)?;
        if !matches!(names.as_slice(), [] | ["worktrees", _]) {
            return Err(elsewhere());
        }
        let mut dir = open_git_dir(repo)?;
        for name in names {
            dir = open_dir_in(&dir, name, false)?.ok_or_else(elsewhere)?;
        }
        Ok((git_dir, dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_names_are_single_quoted() {
        let script = pre_push_script(&["main".to_owned(), "it's".to_owned()]);
        assert!(script.contains(r"for yulius_branch in 'main' 'it'\''s'; do"));
    }
}
