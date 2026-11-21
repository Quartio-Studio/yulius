# 0435. Claude returns variable-path cleanup to the calling agent

## Status

Accepted for [SPEC-596](../../.ai/specs/SPEC-596-such-requests-should-be-answered-automat.md).

## Context

In task #575 a reviewer wanted to replace scratch copies before rerunning tests. Claude's Bash tool asked a person to approve a script containing a recursive deletion with two possibly empty variables. The reviewer starts with a fresh context. It can miss the parent's briefing about the run's cleanup tool. A Bash permission decision applies to the whole script, including commands after the deletion.

## Decision

Register a local Bash PermissionRequest hook for contained Claude tasks. When the event is in bypassPermissions mode, has an empty suggestion list and contains a forced recursive removal of one variable-parent/variable-child path, return a non-interrupting denial to the calling agent. Give it the quoted, guarded operand for the existing cleanup command so it can issue a fresh tool call. Parse shell commands and arguments to distinguish the removal from a string or here-document containing its spelling. The hook sends no request to the server.

The cleanup tool resolves the path where the shell expanded it and enforces the ownership rules in [ADR-0155](0155-a-task-removes-its-own-files-with-yulius-remove.md).

## Alternatives

Typing Yes into the terminal could approve another dialog if the task advanced. Approving the compound tool call would authorize every command in it. A briefing alone did not reach the reviewer in the reported incident.

## Consequences

The agent spends another tool call on the checked retry. A failed removal reports its reason in the run. Unrecognized syntax and other permission requests follow Claude's usual handling. Existing sessions acquire the hook when Yulius next launches them.
