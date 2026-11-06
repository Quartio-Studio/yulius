---
yulius-task: 746c60a2-d83f-41d3-befb-9f0908241417
status: in-progress
---

# SPEC-433: An image pasted into the Global Superagent reaches it

Status: in-progress
Date: 2026-11-06
Owner: Yulius task #415
Scope authorization: task #415 (`/goal`): "I can't paste to the global superadmin. Fix." (the
Global Superagent, the All sandboxes tab of the Superagent panel). Extends SPEC-430 GKY-003;
decided in [ADR-0277](../../docs/decisions/0277-the-global-superagent-keeps-pasted-images-in-its-own-folder.md).

## Goal

A person pastes a screenshot (or drops an image) into the Global Superagent's terminal and Claude
Code takes it as an image, `[Image #1]`, as it does in a sandbox's Superagent and a task's
terminal. Text pastes keep working as they do.

## Context

- Reproduced against the mock (e2e, Chromium) at `d6642cb9`, the commit the instance runs
  (`dd628f26` promotes it): a text paste into the All sandboxes terminal reaches the session; a
  clipboard image does nothing at all, no upload, no path, no message.
- In the sandbox Superagent and a task's terminal a pasted image is uploaded as an attachment and
  its path typed into the terminal as a bracketed paste (`filedrop.js` `attachTerminalDrop`,
  UI-048); Claude Code turns a pasted image path into an image. A browser cannot hand the image to
  Claude Code otherwise: its own Ctrl+V image paste reads the server's clipboard.
- SPEC-430 GKY-003 left the Global Superagent without that ("Files dropped on it are not uploaded
  (it has no sandbox to upload to)"): `agent.js` attaches no drop for it. Uploading to its host
  sandbox's shell attachments would not do: on a contained server its bubblewrap root is only its
  own folder `users/<id>/superagent` (ADR-0271), so it cannot open a path in a sandbox, and its
  files do not belong in a sandbox its person may lose.
- It has no built-in tools (`--tools ""`), so a path to a PDF or a text file is only text to it;
  Claude Code reads an image path itself when it is pasted. Probed in a PTY on Claude Code 2.1.311
  with the Global Superagent's flags (`--setting-sources "" --strict-mcp-config --tools ""
  --dangerously-skip-permissions`, a synthetic token, no model called): a bracketed paste of a PNG's
  path shows `[Image #1]`. Its binary takes `png`, `jpg`, `jpeg`, `gif` and `webp` as images.

## Requirements

- GIM-001: `POST /api/superagent/attachments?name=` stores a raw body (streamed, the upload limit,
  the disk check, the concurrent-upload bound, audited `attachment.uploaded`) in the caller's
  Global Superagent's own folder, `users/<id>/superagent/attachments/`, which is inside its root at
  the same path, and answers 201 with the stored file's absolute path as other attachments do
  (`{path, name, size, sha256, replaced}`, a free name, never replacing a file).
- GIM-002: only images Claude Code takes (`png`, `jpg`, `jpeg`, `gif`, `webp`, any case) are
  stored; any other name is refused before the body is read (422, saying it takes images only, it
  has no file tools). Only the caller's own running Global Superagent takes files (404 without one,
  another person never reaches it); a disabled account is refused.
- GIM-003: the folder is bounded: at most `MAX_ATTACHMENTS_PER_FOLDER` (500) files, and images
  older than a week are removed at the next upload (Claude Code reads the image when it is pasted).
  Every folder on the way is opened without following links, as other attachments are.
- GIM-004: in the web UI, an image pasted into the All sandboxes terminal, or files dropped on it,
  are uploaded there and their paths pasted into the terminal (bracketed when Claude Code asked for
  it), as in the sandbox Superagent; the drop overlay says "Drop an image to send to Claude"; a file
  that is not an image is not sent and the upload status says why. Text pastes stay with the
  terminal; file links in its output still do not open the Editor.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] An image pasted or dropped for the Global Superagent is stored in its own folder
- [x] The All sandboxes terminal uploads pasted and dropped images and types their paths
- [x] ADR, architecture, API and product docs describe the Global Superagent's images
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-GIM-1: alice, whose Global Superagent runs, uploads `shot.png`: 201, the path is
  `<data>/users/u-alice/superagent/attachments/shot.png` (canonical) and holds the bytes; a second
  `shot.png` gets a free name.
- AC-GIM-2: `notes.pdf` is refused with 422 and nothing is written; carol (no running Global
  Superagent) gets 404 and writes nothing into alice's folder.
- AC-GIM-3: an image older than a week in the folder is removed by the next upload; a newer one
  stays.
- AC-GIM-4 (e2e): in the All sandboxes tab, a pasted clipboard image is typed into the terminal as
  `…/superagent/attachments/pasted-<stamp>.png`; a text paste stays text; a dropped `.pdf` is not
  sent and the status says images only.

## Test plan

- Integration (`crates/yulius/tests/global_superagent.rs`, the router and the real ptyhost):
  AC-GIM-1 to AC-GIM-3.
- e2e (`e2e/tests/global-superagent.e2e.ts`) against the mock, which gets the route: AC-GIM-4.

## Decisions

- **Its own folder, not the host sandbox's shell attachments.** The contained Global Superagent
  sees only `users/<id>/superagent`; a path in a sandbox would not open, and its person's files
  would sit in a sandbox they may lose. Alternatives: binding the host's shell attachments folder
  into its root widens a root ADR-0271 keeps to its own folder; sending the image through the MCP
  needs a tool round trip and is not how a pasted image reaches Claude Code.
- **Images only.** It has no file tools: a PDF's path would be text it cannot open, so refusing it
  says so at once instead of a reply that it cannot read the file.
- **A week, then removed at the next upload**, like the composer's drafts: Claude Code reads the
  image when it is pasted, and the folder needs a bound without a sweep of its own.
- **A new route under `/api/superagent`**, beside `POST /api/superagent`, rather than the shell's
  `…/shells/{id}/attachments` with a special case: the folder belongs to the person, not to a
  sandbox or a shell id.
