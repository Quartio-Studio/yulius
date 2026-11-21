---
yulius-task: 1f417829-d430-4e66-9a47-52c44c4ed39c
status: in-progress
---

# SPEC-596 Automatic answers for Claude scratch cleanup

## Goal

A contained Claude task or reviewer can remove its scratch copies and continue testing without asking the person to answer a Bash removal dialog.

## Context

Task #586 shows a background reviewer resetting two scratch directories in a loop. Claude asks about `rm -rf $S/$d` because an empty variable could change the target. Task #468 recorded a timeout for the same command shape. A reviewer has a fresh context and may never see the task briefing's cleanup advice. The run has its own temporary filesystem and shares persistent sandbox directories with other tasks. [SPEC-192](SPEC-192-claude-sessions-asks-me-to-delete-someth.md)

## Requirements

- **CLR-001** Answer a contained Claude Bash permission request for ordinary variable-path or root-level scratch removal locally and immediately, including removals inside a reviewer loop or conditional. Give the agent an in-place retry that preserves control flow, quotes each operand and requires nonempty variables. The agent continues its turn.
- **CLR-002** Apply this policy only when Claude requests permission. Ordinary commands, other tools and uncontained runs keep their existing permission handling. Treat malformed or oversized events as unrecognized.
- **CLR-003** The agent executes the retry in its run. The existing cleanup tool can remove a whole scratch directory and refuses shared paths. Preserve non-recursive removal by using a file-only operation for those requests. An expanded operand ending in a slash requires a directory. A trailing slash on a regular file preserves the file with the original forced or unforced status. A directory symlink used this way is refused. Without an explicit option terminator, an operand that expands to an option stops the entire retry before removal. Root-level operands retain their trailing slashes. A removal with a command-local redirection falls through to the existing handler so Bash keeps its prerequisites and effects. A retry only runs for the system removal command. Preserve shell functions and Bash combined output redirects. A redirect on a later command in a foreground conditional list belongs to that command. Classify redirections, quotes, comments and here-document bodies from the Bash syntax tree. An unrelated action embedded in the original Bash command receives no automatic approval.

## Plan

- [x] Specify automatic handling of Claude removal requests
- [x] Route task and reviewer cleanup through the run’s removal policy
- [x] Verify scratch cleanup and other permission decisions
- [x] Preserve conditions and file-only semantics for equivalent removals
- [x] Preserve directory requirements on expanded cleanup paths
- [x] Refuse ambiguous expanded options before removing any operand
- [x] Recognize root-level scratch directories with trailing slashes
- [x] Preserve command-local redirections during cleanup decisions
- [x] Keep shell functions and asynchronous commands out of deletion retries
- [x] Recognize foreground cleanup before another command’s Bash redirect
- [x] Distinguish Bash redirects from quoted and here-document text
- [ ] Independent review approves the verified change for delivery

## Acceptance criteria

- **AC-1** The attached loop produces a non-interrupting denial of the original command with a separate checked retry that needs no person. The same decision reaches the main agent and a worker.
- **AC-2** Guarded components, unrelated requests, command text printed as data, and invalid or oversized inputs fall through to Claude’s existing permission handler. An uncontained launch has no automatic cleanup hook.
- **AC-3** Executing the checked retry removes populated scratch directories and handles an already removed directory. Empty components stop the shell before deletion. The helper refuses shared targets and a file-only operation preserves a populated directory. An in-place retry preserves variable bindings and the original conditional, including when the guard is false.

## Test plan

Exercise the generated Claude launch settings and local hook entry point with synthetic main-agent and worker events. Check mixed Bash scripts, unrelated permission decisions and missing or malformed input. Run a scratch cleanup in this contained run and reuse the existing helper's ownership tests.

## Decisions

Use Claude's structured PermissionRequest event. Its decision belongs to the pending tool call and can return the cleanup instruction directly to the agent. The existing helper determines ownership at execution time. [ADR-0435](../../docs/decisions/0435-claude-returns-variable-path-cleanup-to-the-calling-agent.md) [ADR-0155](../../docs/decisions/0155-a-task-removes-its-own-files-with-yulius-remove.md)


## Evidence

- Before the change, the hook regression received zero bytes and the contained launch snapshot found no PermissionRequest hook. Both tests failed. The interactive-removal negative case also failed before restricting the recognized flags.
- `cargo test --locked -p yulius-harness --test hook_client --test launch` passed all 51 tests. `cargo clippy --locked -p yulius-harness --all-targets -- -D warnings` passed. That first implementation used conch-parser 0.1.1. The delivered parser uses the Tree-sitter Bash grammar.
- Real Claude Code 2.1.330, using a loopback model stub, ran the test as the main agent and as an asynchronous reviewer. Both received the checked retry, removed two populated scratch directories, rebuilt the test folders and preserved the sibling marker. The stub issued the retry only after it received the hook's message. This exercises Claude's real hook and Bash execution without a paid model or an operator response.
- In this contained run, an empty child stopped Bash before removal, a checked retry deleted a populated directory, repeating it reported already gone, and the helper refused a tracked worktree file.
- `python3 scripts/check_repository.py` passed.

- The first independent review requested coverage for single-variable, variable-plus-literal and root-level scratch operands, and an explicit in-place retry for conditional commands. Both findings were in the change. Before the fixes, equivalent cleanup requests had no hook decision, the message omitted the in-place instruction and the CLI rejected both new options. The regression now exercises these shapes and the helper's recursive and missing-target behavior.
- After the review fixes, the 51 hook and launch tests and the 12 `remove_cli` tests passed. In a real Claude Code 2.1.330 asynchronous reviewer with a loopback model stub, a conditional inside the original loop selected one scratch copy. The checked retry rebuilt that copy, preserved the other copy and left the sibling marker intact.
- `scripts/dev quick` passed. `cargo deny check bans licenses` passed. `scripts/dev security` reported Vidoc disabled for this project and scanned no code.

- The second review found that trimming a trailing slash could unlink a regular file. Before the correction, both forced and unforced retries removed the disposable marker. Tests now cover expanded operands ending in one or several slashes, regular files, dangling links, links to files, directory symlinks and actual directories. The original helper's default trailing-slash test still covers its existing callers.
- After the trailing-slash correction, all 51 hook and launch tests and all 13 removal tests passed.
- The third review found that a variable expanding to `--help` could remove that filename and another operand. A binary fixture reproduced both deletions on the prior head while the original rm kept both markers. The checked retry now verifies all expanded arguments first. Original explicit option termination still permits that filename.
- The third review also reproduced a prompt for a root-level directory ending in a slash, and a retry that dropped a required input redirect. Both new hook regressions failed on the old implementation. The hook recognizes root-level paths with trailing slashes and preserves their spelling. A redirect or environment assignment attached to the removal goes to the normal permission handler.
- A further independent review reproduced two cases where a deletion retry could change Bash behavior. The hook tests failed before excluding shell jobs (including Bash combined redirects) and defined removal functions. A native Bash test executes the complete returned retry against a recording helper. It calls the helper only for the system executable and preserves the function and alias cases. An absolute system executable is still recognized in a script defining a bare rm function.
- An independent review distinguished a combined redirect on the removal from one on a later command in a foreground conditional list. The old hook returned no answer for `rm -rf "$scratch" && echo rebuilt &>result`. The regression exercises `&&`, `||`, append, an actual asynchronous list, and a here-document with a redirect-like delimiter. The agent resubmits the original script with its redirections intact.
- The quoted/comment `<<` regression failed with the earlier POSIX parser. With the Bash grammar, the same redirect is recognized beside quoted text, comments and a real heredoc containing a deletion as data. During the change a test also caught a dropped literal dollar suffix. An operand with an unrecognized fragment now falls through without generating a replacement. The Bash grammar compiled with Clippy for aarch64 macOS. The dependency license and ban checks passed.
- The Bash grammar represents positional parameters as variable-name nodes. An independent review reproduced that `"$10"` could be rewritten to `"${10:?}"`, which Bash resolves differently. The hook now uses the harness’s shell-identifier check for named variables. Tests for quoted, unquoted and braced positional parameters failed before the fix.
- A mutation that replaced the binary command dispatch with an immediate successful exit survived the workspace test filter. The product integration suite now calls the real cleanup CLI, checks both exit codes and output, and verifies that a rejection preserves both disposable markers.
- A subsequent mutation replaced the existing public removal entry point with a constant exit code. A compatibility test now invokes that entry point and checks both its usage error and the successful already-gone result, including the output. The same calls exercise the options-aware implementation it delegates to.
