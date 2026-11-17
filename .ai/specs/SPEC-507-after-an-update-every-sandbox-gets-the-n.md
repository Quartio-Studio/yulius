---
yulius-task: 6b9db65e-20f8-4698-9efb-de305674b21d
status: in-progress
---

# SPEC-507: After an update, every sandbox gets the new skills at once

Status: in-progress
Date: 2026-11-17
Owner: Yulius task #489
Scope authorization: task #489, the person's request of 2026-11-17 ("Make the skills reach every
sandbox when the instance updates, not when its next task happens to start"). Builds on SPEC-220
(the safe skill install, ADR-0174), SPEC-493 (the reviewer agents, ADR-0340) and SPEC-506 (whose
review rules this update failed to deliver).

## Goal

When the instance updates, every sandbox that has a home gets the instance's current skills at the
server's start, the same safe install a run's start does, and the update says which sandboxes it
refreshed. What an update cannot reach in a session already running is named, and nothing a
running session cannot see changes there.

## Context

Measured on production by the person's session (17 Nov 2026): the update to ffa950b0c at
22:18:56 UTC wrote the instance's skill catalog at 22:18:58. Half an hour later only quartio-v2,
where a task had started at 22:26, had the new skills; yulius, autoresearch, VistulaValley and
product kept 6 of 13 skills of the build before (`delivery-pipeline`, `infra-bootstrap`,
`test-driven-development`, `om-code-review`, `om-check-and-commit`, `om-fix`), and their running
sessions kept reading the rules SPEC-506 had replaced.

What the code does (read on `dev` at 06289c6ee):

- `App::prepare_run_extras` (`crates/yulius-app/src/supervisor.rs`) is the one caller of
  `HarnessPort::install_skills`: skills reach a sandbox's home (`<sandbox>/home/.claude/skills`)
  only when a run starts there. `serve.rs` extracts the embedded packs to `<data>/skills` at every
  start (`install_skill_catalog`) and touches no sandbox.
- `yulius_harness::skills::install_skills` stages each skill next to its target and swaps it in
  by rename, reached from the sandbox directory through folder handles without following a link
  (ADR-0174); managed copies (`.yulius-managed`) whose skill left the set are removed, folders the
  agent made are left alone.
- `set_skill_enabled` only writes the switch: the home changes at the sandbox's next run start.
- The reviewer agents (`--agents`, `yulius_harness::launch::reviewer_agents`) are given at a
  session's launch with `REVIEWER_PROMPT` (`crates/yulius-domain/src/review.rs`). SPEC-506 FST-002
  added its CI rule to that prompt as well as to `om-code-review`; a session launched before the
  update kept the old prompt. The prompt also tells the reviewer to load `om-code-review`, which a
  subagent reads from the home when it uses it.
- What a session asks Yulius (hooks, MCP tools, the review round's message from the PreToolUse
  hook) is answered by the running server, so it changes with the update. The briefing
  (SessionStart), the launch settings and `--agents` are fixed at the launch.

## Requirements

- **SKR-001** At the server's start, after the catalog is extracted and before tasks are resumed
  or queued runs start, Yulius compares each sandbox's installed skills with the set a run there
  would get (`skills_for_run`) and, where any differs (a skill missing or with other content, a
  managed copy of a skill that left the set), installs the set into that sandbox's home with the
  run start's install. Running tasks do not hold it back. A sandbox with no home (no run yet) or a
  removed sandbox is skipped. Copies that already match are not rewritten.
- **SKR-002** The comparison reads the home without following a link and within the install's
  bounds (size, entries, depth, folder listing); a link or anything not a regular file or folder
  in an installed copy counts as different, so the install replaces it. A link on the way to the
  skills folder fails that sandbox's refresh, as it fails a run's install.
- **SKR-003** Switching a skill on or off for a sandbox installs that sandbox's set into its home
  at once (when it has one), the same way.
- **SKR-004** Each refreshed sandbox gets an audit entry `skills.refreshed` (actor `system`,
  target the sandbox's slug, detail the commit of the server and the skills installed and
  removed); the server log names it too. A failure is logged (`warn`) with the sandbox and never
  fails the server's start, the switch or a run.
- **SKR-005** Installs into one home never run at once: a refresh and a run's install of the same
  sandbox are serialized.
- **SKR-006** The reviewer agents' prompt carries no review rule an update can change: who the
  reviewer is, that it loads `om-code-review` (and the repository's review skill) and works
  read-only, that it marks findings own code or pre-existing, and the verdict line the code parses.
  The rules (CI alone is no request for changes, severity, the verdict's mechanics) live in the
  skill, which the refresh brings to every sandbox and the reviewer reads at each use. A test
  holds the prompt to that contract. Working sessions are not relaunched.
- **SKR-007** `docs/operations.md` says what an update refreshes (the skills in every sandbox's
  home, what hooks and tools answer) and what it does not (a running session's briefing, launch
  settings and reviewer agents' prompt, until the session is launched again).

## Plan

- [x] Spec SPEC-507 and ADR-0364 written
- [ ] Harness: a refresh installs only where the installed copies differ, read without following links
- [ ] A server start refreshes every sandbox's home, audited as skills.refreshed
- [ ] Switching a skill on or off installs the sandbox's skills at once
- [ ] The reviewer agents' prompt holds only a contract no update changes
- [ ] Operations, architecture and changes documents say what an update refreshes
- [ ] Independent review approved and its findings answered

## Acceptance criteria

- **AC-1** (SKR-001) A test: a sandbox with a running task (a live attempt) holds a copy of a
  skill installed from the build before; the server's start routine with the changed catalog
  replaces that copy, leaves a skill the agent added alone, writes one `skills.refreshed` entry
  naming the sandbox and the skill, and a second start writes nothing and rewrites nothing.
- **AC-2** (SKR-001) A sandbox without a home is left without one.
- **AC-3** (SKR-002) Harness tests: matching copies are not rewritten; changed content, a missing
  skill, an extra file, a changed mode, a link or a FIFO in a copy, a copy without the marker and a
  managed copy of a skill that left the set each count as different; a link on the way fails; a
  FIFO is never opened for reading.
- **AC-4** (SKR-003) A test: switching a skill off removes its managed copy from the home at once,
  switching it on installs it.
- **AC-5** (SKR-004) A failing refresh (a link as `home/.claude`) is logged and the start goes on;
  the other sandboxes are refreshed.
- **AC-6** (SKR-006) A test holds `REVIEWER_PROMPT` to the contract and the CI rule to
  `om-code-review`.

## Test plan

- `cargo nextest run -p yulius-harness -E 'binary_id(yulius-harness::skills)'` and the module
  tests of `yulius_harness::skills`.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^skills_mcp::/)'`.
- `cargo nextest run -p yulius-domain review` and `-p yulius-app skills`.
- Yulius CI runs the whole gate on the pushed head.

## Decisions

- **Refresh only what differs, at the start, before tasks resume** (ADR-0364). Reinstalling every
  skill everywhere at each start would swap 13 folders under every running session for nothing
  and would make "refreshed" mean nothing in the audit. The comparison is a content digest of the
  catalog folder (with the install's rules for links inside a skill) against one of the installed
  copy read through folder handles; it costs one read of each copy (13 skills, a few hundred KiB).
  The start runs it before `start_background` and the update loop, so a task the update froze is
  thawed onto the new skills. Alternative rejected: a version stamp in the marker file, which an
  agent's edit of the copy would leave untrue.
- **The reviewer's prompt is a contract, the rules are in the skill** (ADR-0364). The person
  offered two ways: a prompt that carries nothing an update changes, or a new prompt for the next
  round another way. The second would need the PreToolUse hook to rewrite every reviewer's task
  prompt, which leaves the session's launch prompt saying the old thing beside the new one. The
  CI rule SPEC-506 added to the prompt is already in `om-code-review`, which the prompt sends the
  reviewer to; so it leaves the prompt, and SPEC-506 AC-2's clause "the reviewer agents' prompt
  says the CI rule" is replaced by AC-6 here. The round's own message from the hook is the place for
  anything a round must be told by the current server.
- **The audit entry, not the Update page** (SKR-004). A refresh happens at every start (a restart
  too, a skill switched), not only an update; one entry per sandbox, targeted by slug as the
  sandbox's other entries are, is found the same way for all of them. The Update page's run is
  unchanged.
