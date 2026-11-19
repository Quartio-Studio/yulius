---
yulius-task: bad1fa89-4834-4383-9dc9-6d9a976477d4
status: done
---

# SPEC-529 — Scrolling a task terminal keeps the prompt untouched

## Goal

A person looking for earlier output can scroll a task without making Yulius hold messages for a possible draft. Fixes #1446.

## Context

Task #511 shows the held-message warning on the Codex session for PR #1439. The screenshot omits the prompt box and the preceding gesture. One reproducible trigger is [#1446](https://github.com/Quartio-Studio/yulius/issues/1446). In Chromium the shipped xterm emits an Up arrow on a wheel turn on its alternate screen when mouse reporting is off. The task view forwards that arrow, which can recall a real draft. The server records it as typing until the next prompt submission. The older tests cover a program with the mouse and a normal buffer with scrollback.

## Requirements

- WHL-511-1. A wheel gesture in the task's alternate buffer sends no keyboard input when the program has not enabled mouse reporting. The existing earlier-messages offer remains available.
- WHL-511-2. Real keyboard input reaches that task. Normal scrollback, native mouse reporting and independent shell terminals keep their scrolling behavior.

## Plan

- [x] Reproduce wheel-generated arrow keys in a task terminal
- [x] Specify the scroll behavior and commit the failing browser regression
- [x] Keep passive scrolling out of the task prompt and verify real keyboard input
- [x] Independent review and verification for delivery to dev

## Acceptance criteria

- With a writable task on its alternate screen and mouse tracking off, wheel up and down generate zero task input frames. Wheel up offers the session history.
- A physical Up key still sends one arrow. A program that requests mouse reports still receives wheel reports. A normal terminal buffer scrolls through its own history.

## Test plan

Use the actual xterm renderer and the task WebSocket mock in the terminal browser suite. Observe outgoing input frames and the history offer, then run the web suite. Keep the server's existing draft-protection test as a separate control.

## Decisions

Prevent xterm's synthesized history key at the task view. Labelling that arrow as a scroll on the wire would still let the program recall someone’s previous input. Previous issue #1260 and PR #1266 concern identifying an empty Claude Code composer after actual input. This fix also works before a viewer knows which agent screen they are looking at.

## Evidence

- On the original task view, the Chromium regression sent three frames for two wheel gestures and one physical arrow key. The expected frame list contained only the physical Up. The observed list contained Down, Up, Up. The same xterm and the shipped classifier in isolation classified a wheel-generated Up as typing.
- After the fix, the new Chromium test and the existing native-wheel, classic-renderer history and normal-scrollback controls passed (4/4). The test observed one physical Up after both wheel gestures.
- The real-process control `cargo test --locked -p yulius --test main task_message::a_message_of_yulius_waits_for_a_persons_draft_and_tells_them -- --exact` passed (1/1).
- Review round 1 requested coverage for independent shells. A Chromium shell test passed with the task-only guard. In an isolated copy with that guard removed, the same test failed because the shell received neither wheel arrow.
- `scripts/dev quick` passed. `scripts/dev mutants` found no Rust changes to mutate. The browser regressions caught the original behavior and the removal of the task-only restriction.
- The local Vidoc scan was unrun (exit 2, HTTP 401).
- Independent review approved `3a0580a4709bfa092cb955ee02bc88fc53bc7955` after the reviewer ran five Chromium controls covering the task, shell, native mouse tracking and scrollback. The screenshot does not establish the original gesture or buffer mode.
- Full browser suite on the fixed product code: 1,119 passed, one model-backed test skipped, zero failures. The independent-shell control added during review also passed separately.
