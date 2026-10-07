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

use std::fs;
use std::io::Write;
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

fn write_atomic(path: &Path, contents: &str, executable: bool) -> Result<(), GitError> {
    let io = |source| GitError::Io {
        context: "writing hook",
        source,
    };
    let mut temp_name = path.as_os_str().to_owned();
    temp_name.push(".yulius-tmp");
    let temp = PathBuf::from(temp_name);
    {
        let mut file = fs::File::create(&temp).map_err(io)?;
        file.write_all(contents.as_bytes()).map_err(io)?;
        file.sync_all().map_err(io)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        fs::set_permissions(&temp, fs::Permissions::from_mode(mode)).map_err(io)?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    fs::rename(&temp, path).map_err(io)
}

impl Git {
    /// Installs (or rewrites) the pre-push guard for `worktree`, refusing pushes whose
    /// remote ref is `refs/heads/<b>` for any `b` in `protected_branches`. Existing
    /// repository hooks are preserved and chained (see the module docs). Returns the path
    /// of the installed `pre-push` script.
    ///
    /// `worktree` is the worktree directory (or the primary checkout); its Git directory and
    /// the effective hooks location are discovered with Git itself.
    pub fn install_pre_push_guard(
        &self,
        worktree: &Path,
        protected_branches: &[String],
    ) -> Result<PathBuf, GitError> {
        if protected_branches.is_empty() {
            return Err(GitError::InvalidInput {
                what: "protected branches",
                reason: "empty".to_owned(),
            });
        }
        for branch in protected_branches {
            validate_branch_name(branch)?;
        }
        let git_dir = self.run_ok(
            Invocation::new(self, "rev-parse", Some(worktree))
                .args(["rev-parse", "--absolute-git-dir"]),
        )?;
        let git_dir = PathBuf::from(git_dir.stdout_trimmed());
        let hooks_dir = git_dir.join(HOOKS_DIR_NAME);
        fs::create_dir_all(&hooks_dir).map_err(|source| GitError::Io {
            context: "creating the Yulius hooks directory",
            source,
        })?;
        write_atomic(&hooks_dir.join(LIB_NAME), LIB_SCRIPT, false)?;
        for name in FORWARDED_HOOKS {
            write_atomic(&hooks_dir.join(name), &forwarder_script(name), true)?;
        }
        let pre_push = hooks_dir.join("pre-push");
        write_atomic(&pre_push, &pre_push_script(protected_branches), true)?;
        self.enable_worktree_config(worktree)?;
        self.run_ok(
            Invocation::new(self, "config", Some(worktree))
                .args(["config", "--worktree", "core.hooksPath"])
                .arg(&hooks_dir),
        )?;
        Ok(pre_push)
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
