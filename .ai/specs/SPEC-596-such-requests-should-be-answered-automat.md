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

- **CLR-001** Answer a contained Claude Bash permission request for a variable-parent/variable-child removal locally and immediately, including a removal within a reviewer loop. Give the agent a retry with quoted, nonempty components and the run's ownership-checked cleanup tool. The agent continues its turn.
- **CLR-002** Apply this policy only when Claude requests permission. Ordinary commands, other tools and uncontained runs keep their existing permission handling. Treat malformed or oversized events as unrecognized.
- **CLR-003** The agent executes the retry in its run. The existing cleanup tool can remove a whole scratch directory and refuses shared paths. An unrelated action embedded in the original Bash command receives no automatic approval.

## Plan

- [x] Specify automatic handling of Claude removal requests
- [x] Route task and reviewer cleanup through the run’s removal policy
- [x] Verify scratch cleanup and other permission decisions
- [ ] Deliver the independently reviewed change on green CI

## Acceptance criteria

- **AC-1** The attached loop produces a non-interrupting denial of the original command with a separate checked retry that needs no person. The same decision reaches the main agent and a worker.
- **AC-2** Guarded components, unrelated requests, command text printed as data, and invalid or oversized inputs fall through to Claude’s existing permission handler. An uncontained launch has no automatic cleanup hook.
- **AC-3** Executing the checked retry removes populated scratch directories and handles an already removed directory. Empty components stop the shell before deletion. The helper refuses shared targets.

## Test plan

Exercise the generated Claude launch settings and local hook entry point with synthetic main-agent and worker events. Check mixed Bash scripts, unrelated permission decisions and missing or malformed input. Run a scratch cleanup in this contained run and reuse the existing helper's ownership tests.

## Decisions

Use Claude's structured PermissionRequest event. Its decision belongs to the pending tool call and can return the cleanup instruction directly to the agent. The existing helper determines ownership at execution time. [ADR-0435](../../docs/decisions/0435-claude-returns-variable-path-cleanup-to-the-calling-agent.md) [ADR-0155](../../docs/decisions/0155-a-task-removes-its-own-files-with-yulius-remove.md)


## Evidence

- Before the change, the hook regression received zero bytes and the contained launch snapshot found no PermissionRequest hook. Both tests failed. The interactive-removal negative case also failed before restricting the recognized flags.
- `cargo test --locked -p yulius-harness --test hook_client --test launch` passed all 51 tests. `cargo clippy --locked -p yulius-harness --all-targets -- -D warnings` passed. Rust 1.99 emits a future-compatibility notice for macro syntax in conch-parser 0.1.1.
- Real Claude Code 2.1.330, using a loopback model stub, ran the test as the main agent and as an asynchronous reviewer. Both received the checked retry, removed two populated scratch directories, rebuilt the test folders and preserved the sibling marker. The stub issued the retry only after it received the hook's message. This exercises Claude's real hook and Bash execution without a paid model or an operator response.
- In this contained run, an empty child stopped Bash before removal, a checked retry deleted a populated directory, repeating it reported already gone, and the helper refused a tracked worktree file.
- `python3 scripts/check_repository.py` passed.
