---
yulius-task: 1f417829-d430-4e66-9a47-52c44c4ed39c
status: done
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
- [x] Independent review approves the verified change for delivery

## Acceptance criteria

- **AC-1** The attached loop produces a non-interrupting denial of the original command with a separate checked retry that needs no person. The same decision reaches the main agent and a worker.
- **AC-2** Guarded components, unrelated requests, command text printed as data, and invalid or oversized inputs fall through to Claude’s existing permission handler. An uncontained launch has no automatic cleanup hook.
- **AC-3** Executing the checked retry removes populated scratch directories and handles an already removed directory. Empty components stop the shell before deletion. The helper refuses shared targets and a file-only operation preserves a populated directory. An in-place retry preserves variable bindings and the original conditional, including when the guard is false.

## Test plan

Exercise the generated Claude launch settings and local hook entry point with synthetic main-agent and worker events. Check mixed Bash scripts, unrelated permission decisions and missing or malformed input. Run a scratch cleanup in this contained run and reuse the existing helper's ownership tests.

## Decisions

Use Claude's structured PermissionRequest event. Its decision belongs to the pending tool call and can return the cleanup instruction directly to the agent. The existing helper determines ownership at execution time. [ADR-0435](../../docs/decisions/0435-claude-returns-variable-path-cleanup-to-the-calling-agent.md) [ADR-0155](../../docs/decisions/0155-a-task-removes-its-own-files-with-yulius-remove.md)

## Evidence

- The hook test initially returned zero bytes for the pictured removal. The launch test found no PermissionRequest registration. Both regressions now pass for contained Claude runs. Separate cases exercise other tools, harnesses, hook events, permission modes and malformed input.
- Claude Code 2.1.330 ran the loop as a main agent and as an asynchronous reviewer using a loopback model stub. Both received the retry, deleted populated scratch copies, rebuilt them and preserved a sibling marker without operator input. A conditional reviewer run rebuilt only its selected copy and retained its Bash logging redirect. The stub waited for the hook decision before issuing the retry.
- Tests initially reproduced incomplete retries for conditional commands, missing support for file-only retries, trailing slashes removing regular files and option-like expansions removing two files. The corrected helper preserves the force and recursion flags, checks all operands for ambiguous options before touching a file and preserves trailing-slash requirements. The CLI integration asserts exit status, output and both filesystem markers. A contained run also verified an empty child variable stops Bash, an already-removed path succeeds under force and a tracked file is refused.
- Review regressions initially reproduced lost redirects, function and asynchronous command misclassification, a quoted or commented `<<` hiding a cleanup, a lost literal dollar suffix and positional parameters being rewritten as named variables. The corrected tests exercise actual system-command resolution in Bash, conditional lists, aliases, redirects, here-documents, path fragments and positional parameters. Only named variables receive the nonempty guard.
- Independent review approved `fbad865a8` after inspecting the Bash parser, permission routing, command resolution and removal semantics and rerunning the hook and product integration tests. Subsequent regression tests cover redirect ownership, literal operands with spaces, identifier-length boundaries, hook dispatch and the existing public removal entry point.
- All 51 hook and launch tests, all 15 removal unit tests and the binary CLI integration test passed. `scripts/dev quick` verified repository contracts and formatting. Harness Clippy passed on Linux and for the aarch64 macOS target. The macOS check was a cross-compilation. Dependency bans and licenses passed. `scripts/dev security` reported Vidoc disabled and scanned no code.

### Mutation evidence

The first two workspace runs listed 53 and 54 mutations respectively. Both passed their unmutated baseline and were interrupted before a summary. Later bounded workspace runs produced the following summaries.

- Full branch at `0272ee2d1` — `mutants: 98 listed, 32 caught, 7 missed, 0 timed out, 1 unviable, 58 not reached (bound 30 min, took 1815 s)`
- Trailing-slash handling — `mutants: 19 listed, 17 caught, 0 missed, 0 timed out, 1 unviable, 1 not reached (bound 30 min, took 1806 s)`
- Expanded operands — `mutants: 10 listed, 10 caught, 0 missed, 0 timed out, 0 unviable, 0 not reached (bound 30 min, took 1512 s)`
- System command resolution — `mutants: 14 listed, 12 caught, 0 missed, 0 timed out, 2 unviable, 0 not reached (bound 30 min, took 1617 s)`
- Foreground redirects — `mutants: 7 listed, 7 caught, 0 missed, 0 timed out, 0 unviable, 0 not reached (bound 30 min, took 1167 s)`
- Bash grammar — `mutants: 52 listed, 28 caught, 1 missed, 0 timed out, 2 unviable, 21 not reached (bound 30 min, took 1801 s)`
- Named variables — `mutants: 15 listed, 15 caught, 0 missed, 0 timed out, 0 unviable, 0 not reached (bound 30 min, took 1470 s)`

Across the delivered Rust changes, pinned cargo-mutants listed 135 mutations. Focused runs and replays detected 131 and classified four as unviable. In the harness, seven focused replays caught both previously missed mutations and five previously timed-out mutations. In the CLI, the regression for dispatch caught the no-op binary and the public entry-point regression caught the six constant-return mutations. The full changed removal module recorded 27 caught and one unviable. The focused outcomes were matched to the complete mutation list for the delivered product diff.
