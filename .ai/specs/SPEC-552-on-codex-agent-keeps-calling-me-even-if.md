---
yulius-task: cae3b3f1-1b1c-42bf-ab5e-7a7d32dd30eb
status: in-progress
---

# SPEC-552 Codex reply alerts during pull request work

## Goal

Keep a Codex task that is waiting on Yulius out of the person's reply queue. Show a terminal reply prompt only when that is the action the task needs.

## Context

The screenshot shows a failing PR check and a terminal reply banner. The task view draws that banner for any idle task in the Needs you group, including one grouped there by the pull request alone. Separately, the process probe recognizes a live `yulius wait` only inside Claude's Bash wrapper. Codex launches commands under its own process without that wrapper. A missed wait makes failed checks appear to need the person while the agent is waiting for Yulius. The screenshot does not expose the process tree of the reported run.

## Requirements

- CWA-001 Count a live wait owned by the agent, including direct and shell-launched waits. The next probe after it exits removes the wait. Exclude adopted or unrelated processes and preserve existing shell accounting.
- CWA-002 When failing checks explain the task's status, show the PR status without a generic terminal reply prompt. Explicit requests, terminal questions and an ordinary person's turn keep their reply controls.

## Plan

- [x] Trace the false Codex reply alert
- [x] Reproduce active work raising a reply alert
- [ ] Keep Codex work out of the reply queue
- [ ] Verify and review the fix

## Acceptance criteria

- AC-1 A wait in the Codex process tree counts as a wait and consumes no build-shell count. Ending it clears the count. A wait in an adopted subtree stays excluded.
- AC-2 A Codex task with failed checks and a live wait appears under Working without a reply banner. Afterward the strip can show failed checks without asking for a terminal reply. An explicit question on the same task offers Reply.

## Test plan

Use synthetic process trees modeled on the sandbox and a browser test with a failed PR. Run the affected package tests, end-to-end tests, quick checks and the changed-file security command. Deliver through independent review and Yulius CI.

## Decisions

Apply the existing ownership and wait rules from SPEC-085 and SPEC-130. Use the status that the task view already derives to distinguish a failed PR from a request for a reply.
