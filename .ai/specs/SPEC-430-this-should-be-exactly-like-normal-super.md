---
yulius-task: 550602c1-f7ec-4f6b-8542-02a76331b504
status: in-progress
---

# SPEC-430: The Global Superagent takes its person's keys like the sandbox Superagent

Status: in-progress
Date: 2026-11-06
Owner: Yulius task #412
Scope authorization: task #412 (`/goal`, with a screenshot of the All sandboxes tab stuck on
Claude Code's Bypass Permissions warning): "this should be exactly like normal superagent without
some other input. Global superagent is per user (other users cant access) but it has access to all
sandboxes", and "i cant click things there, like i cant select 'yes'".
Changes SPEC-427 GSA-006 and GSA-007 (and the message box of SPEC-428 SAT-001/SAT-004); decided in
[ADR-0274](../../docs/decisions/0274-the-global-superagent-takes-its-persons-keys.md).

## Goal

The All sandboxes tab works exactly like This sandbox: the person types into the Global
Superagent's terminal and clicks in it (menus, options, "Yes"), with no separate message box. It
stays the person's own session (nobody else attaches to it) that reaches every sandbox of theirs
through Yulius. Its start never stops on Claude Code's Bypass Permissions warning.

## Context

- SPEC-427 GSA-007 made the Global Superagent's terminal read-only (`ShellAttach::takes_keys`
  false, `can_write: false`) and gave it a message box (`POST /api/superagent/message`,
  `…/interrupt`) because Claude Code's keys reach an external editor, `!` commands, plugin installs
  and `@` file mentions read as the server. The web terminal forwards mouse clicks only while it may
  write (`terminal-core.js`, `setAppMouse(state.can_write)`), so nothing in it can be clicked.
- The screenshot shows the session waiting on the Bypass Permissions warning ("No, exit / Yes, I
  accept"), which neither a click nor a message can answer: the session is unusable.
- Claude Code 2.1.311 shows that warning when bypass is asked for and no settings source holds
  `skipDangerousModePermissionPrompt` (`Y6()`: user, local, flag or policy settings). Yulius writes
  `bypassPermissionsModeAccepted` into `.claude.json`, which Claude Code migrates into the user
  settings file; the Global Superagent deliberately reads no settings file (`--setting-sources ""`),
  so that acceptance is not dependable for it. Probed in a PTY with a synthetic token
  (Decisions): with the key in `--settings` (flag settings) the warning never shows, also with no
  acceptance stored anywhere; without it and without a stored acceptance it shows.
- Every layer that keeps keys harmless already exists independently of the read-only terminal
  (GSA-002): no built-in tools, only the Yulius MCP, no settings files (so no hooks, no enabled
  plugins, no permission rules), hooks off, `CLAUDE_CODE_SHELL_PREFIX`/`SHELL` and `EDITOR`/`VISUAL`
  naming a refusing script (2.1.311 takes `VISUAL`, then `EDITOR`, and falls back to `code`/`vi`/
  `nano` only when both are unset), and on a contained server a bubblewrap root whose only writable
  path is its own folder, with the host's system folders read-only, `/etc/shadow` and
  `/etc/gshadow` covered, and no Yulius data directory, repository or home.
- The sandbox Superagent takes keys, holds the same subscription token in its environment and runs
  shell commands; on an uncontained server any agent runs as the service user (operations.md).
- The terminal is private already: `shell_open`/attach of a Cockpit shell admits only its creator
  (GSA-005), and every key is checked again (`shell_may_write`).

## Requirements

- GKY-001: the Global Superagent's terminal takes its person's keys, pastes, resizes and mouse
  clicks as the sandbox Superagent's does (`can_write: true`, input reaches its PTY); only its person
  attaches to it, as before (anyone else: not found / refused).
- GKY-002: `POST /api/superagent/message` and `POST /api/superagent/interrupt` are removed (404),
  with the message checks, the label and the interrupt gap that existed only for them.
- GKY-003: the web UI's All sandboxes tab is the sandbox Superagent's view: its note and the
  terminal, focused when shown, no message box, no "Write to it below" banner. Files dropped on it
  are not uploaded (it has no sandbox to upload to), and paths in its output do not open the Editor,
  as before.
- GKY-004: a contained Global Superagent never shows Claude Code's Bypass Permissions warning: its
  `--settings` carry `skipDangerousModePermissionPrompt: true` with its approval bypass. Its launch
  revision is bumped, so a running one restarts once idle with it (SPEC-186 SAV-006).
- GKY-005: everything else of GSA-001 to GSA-005 holds: no built-in tools, only the Yulius MCP,
  no settings files, hooks off, every shell command and the external editor refused, its own
  contained root, its withheld tools, its private seat.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The Global Superagent's terminal takes its person's keys and clicks
- [ ] Its start never stops on the Bypass Permissions warning
- [ ] The All sandboxes tab is the sandbox Superagent's view, without a message box
- [ ] The message and interrupt endpoints are gone
- [ ] ADR, architecture, product and API docs describe the keys
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-GKY-1: alice attaches to her Global Superagent: the state says `can_write: true`, and a typed
  line reaches its PTY (the fake echoes it); carol cannot attach to it.
- AC-GKY-2: `POST /api/superagent/message` and `/interrupt` answer 404.
- AC-GKY-3: the contained launch plan's `--settings` holds `skipDangerousModePermissionPrompt:
  true`, `disableAllHooks: true`; a sandbox Superagent's plan is unchanged.
- AC-GKY-4 (e2e): the All sandboxes tab shows the note and a writable terminal and no message box;
  typing reaches the session.

## Test plan

- Integration (`crates/yulius/tests/global_superagent.rs`): AC-GKY-1, AC-GKY-2 through the router,
  the real ptyhost and the fake `claude`; replaces the message-only tests.
- Unit (`yulius-harness` launch tests): AC-GKY-3.
- e2e (`e2e/tests/global-superagent.e2e.ts`, `superagent-requests.e2e.ts` where they used the box)
  against the mock: AC-GKY-4.

## Decisions

- **Keys rather than a better message box.** The person asked for the same Superagent as in a
  sandbox. The read-only terminal was a second safety layer over refusals that already stand on
  their own (no tools, no settings files, refused shell and editor, its own root); what keys add is
  Claude Code's own UI: menus, `/` commands, `@` mentions of files in its root (system folders and
  its own folder, nothing of Yulius's data on a contained server), `/plugin` installs that no
  settings source enables, `/login`. The sandbox Superagent already gives the same person a shell
  with the same subscription token; on an uncontained server every agent runs as the service user
  anyway. Alternatives: answering the warning by a message ("yes" typed by Yulius) keeps the box
  the person asked to drop; a holder of its own (ADR-0271's next step) is the stronger containment
  but a separate isolation adapter change not needed for what keys reach.
- **Consent in `--settings`.** Flag settings are read whatever `--setting-sources` says, and nothing
  the session writes can remove them, unlike the migrated user settings file it does not read.
  Probed on 2.1.311: with no stored acceptance the warning shows; with the key in `--settings` it
  does not.
- **Remove, not keep, the message endpoints.** Nothing else calls them; keeping them would keep
  ~300 lines of message checks for a path the UI no longer has.
