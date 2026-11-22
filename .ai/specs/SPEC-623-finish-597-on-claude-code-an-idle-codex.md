---
yulius-task: fbb3f169-44ed-404c-ae2d-de7992b135ef
status: done
---

# SPEC-623. Finish #597: an idle Codex session takes messages after a restart

## Goal

Deliver task #597's fix ([SPEC-613](SPEC-613-an-idle-codex-session-takes-messages-aft.md)) into `dev`: an idle Codex session that survived a server restart takes messages. Yulius repaints a terminal it cannot read before typing. A terminal that stays unreadable raises Needs you within the bound. Yulius retries the messages it owes the task (PR results, the wake after an outage) until it delivers them or raises them.

## Context

Task #597 ran on Codex and finished steps 1–5 of SPEC-613 on branch `feat/597-an-idle-codex-session-takes-messages-aft`. Its owner stopped it on 22 November 2026 and committed the unfinished step 6 as `9f3298fbf` ("server-replacement size recovery and wake"). This task continues on Claude Code from that branch, merged with current `dev`. SPEC-613 keeps the requirements CID-001 to CID-003, the acceptance criteria and the evidence of steps 1–5. This spec covers what was left and the delivery.

At the handover, step 6 had two parts:

- The intended terminal size survives a server replacement during a repaint probe. Its run receipt `codex-repaint-size` was written and read, and its test passed.
- The automatic wake after an outage resumes once a refused wake's screen is readable. This part was unfinished. A refused wake raised the private-message action ("retry the message"), which no retry releases. The open request then made every later wake pass skip the task (`WakeFacts::needs_you`). The handover's own test `a_refused_codex_outage_wake_resumes_on_repaint_without_an_owner_resend` failed.

`dev` meanwhile gained SPEC-614's screen lock (`on_screen_of`, `show_screen_of`). The merge keeps that lock around every restore of the reattached screen.

## Requirements

- FIN-001. An automatic prompt from Yulius (`Sender::Yulius` or `Sender::YuliusLastTry` with `Typed::Prompt` or `Typed::Command`) refused for an unreadable screen or an open picker raises Needs you with the automatic action. The next send of the same text releases that request once the screen is readable and the Codex paste hold is free, then types it. A private message keeps its owner action and stays open. A PR notice keeps its notice action.
- FIN-002. A failure to save the intended size does not refuse a viewer's resize. The viewer's size still applies to the PTY and the hub. Only the probe, which needs the record before its temporary width, refuses.
- FIN-003. Keep SPEC-613 CID-001 to CID-003 and the picker protection: nothing is pasted and no Enter is sent while the picker is known to be open.

## Plan

- [x] Take over #597's branch onto current dev
- [x] Resume a refused automatic wake once the Codex screen is readable
- [x] Keep viewer resizes working when the size record cannot be saved
- [x] Re-run SPEC-613's evidence on current dev
- [x] Update the spec, ADR, architecture and changes
- [x] Fresh independent review of the finished change

## Acceptance criteria

1. An outage wake that is refused on an unreadable Codex screen raises Needs you. A later wake pass on a readable screen ends that request and delivers the wake without a person resending it.
2. An outage wake whose request was raised by a private message is not released by the wake.
3. A server replaced during a repaint probe reattaches the child at the intended size (SPEC-613, step 6).
4. A viewer resize succeeds and reaches the child when the size record cannot be written.
5. SPEC-613's tests pass on the merged head, and the five affected binaries pass.

## Test plan

Integration tests in `crates/yulius/tests/task_message.rs` with the fake Codex agent and a real store: refused and resumed outage wake, a private request the wake does not release, server replacement mid-probe, and an unwritable size record. Unit tests in `supervisor.rs` for the size record. Run the Codex filter of `task_message`, then the whole `yulius::main` binary and the `yulius-app`, `yulius-store` and `yulius` libraries and bin. Yulius CI runs the gate on the reviewed head.

## Decisions

Continue SPEC-613's [Decision 0451](../../docs/decisions/0451-repaint-an-unreadable-codex-terminal-before-typing.md). An automatic prompt is matched to its request by its redacted text, so a wake releases only its own request and never a private message's or another sender's. Saving the size is a best effort for a viewer resize: the viewer's choice is authoritative, and a stale record is overwritten at the next probe.

## Evidence

- Takeover: `feat/597-an-idle-codex-session-takes-messages-aft` (9f3298fbf) merged into this branch, then `origin/dev` at 2a6e07423 and fb01043e6. The three conflicts in `supervisor.rs` kept SPEC-614's screen lock (`show_screen_of`, `on_screen_of`) around the restores that now use the recovered size.
- At the handover, `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^task_message::.*codex/)'` ran 16 tests: 15 passed and `a_refused_codex_outage_wake_resumes_on_repaint_without_an_owner_resend` failed. No wake was delivered (`left: []`) because the refused wake had raised the private-message action, which no retry releases.
- FIN-001: an automatic prompt now raises `CODEX_AUTOMATIC_ACTION`. The wake test passes. `an_outage_wake_leaves_a_private_messages_codex_request_open` passes too. A mutant that let any automatic send release any request (`matches || prompt.is_some()`) made it fail. A mutant that skipped only the action check survived, because the redacted-text comparison also guards the release.
- FIN-002: `a_viewer_resize_reaches_codex_when_its_size_record_cannot_be_saved` failed before the fix with `Conflict("Codex's terminal could not confirm a safe input screen after a repaint")`. After the fix the resize returns Ok, the hub is at 100×30 and the child reports `SIZE 30 100`.
- The handover's own size test, `restarting_the_server_during_a_codex_repaint_restores_the_childs_size`, passes. It stops the first server while the host is at the probe width, and the replacement server and the child report the intended size.
- SPEC-613 re-run on current dev: `scripts/cargo nextest run --no-fail-fast -p yulius -p yulius-app -p yulius-store --lib --bin yulius --test main` ran 4,124 tests, and all passed. The default profile skipped 24.
- The same Codex and outage filter (19 tests) was run against dev's versions of the eight changed source files. 11 failed: reattached repaint, unreadable screen to Needs you, PR-news escalation, picker refusal raising PR news, wake resume, private request, and size recovery on cancel, reconnect, server replacement and unsaved record. The 8 tests from SPEC-603's picker protection passed there as on the branch.
- `scripts/dev quick` and `scripts/dev lint` passed. `scripts/dev security` printed "Security scan disabled: Vidoc is turned off in Yulius". Nothing was scanned; this is disabled, not passed.
- Not tested: the real screen condition of task #596 on 22 November remains unknown (SPEC-613 Evidence). The fixtures use the fake Codex agent's synthetic grids, not a live Codex CLI.
