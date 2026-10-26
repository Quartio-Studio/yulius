---
yulius-task: 2c4d04f3-a7fc-4b6e-bb9c-078fe1a8c70b
status: in-progress
---

# SPEC-325: Files up to 1 GB can be uploaded, so an agent can analyse a screen recording

Status: in-progress
Date: 2026-10-26
Owner: Yulius task #309
Scope authorization: task #309 (`/goal`): "fix that I can upload videos up to 1gb, because I want
claude to analyze the video and based on that it should understand what the issue is and fix it
[…] now in Yulius you can upload only 25MB". Amends EDT-006 and UI-048 of
[SPEC-001](SPEC-001-yulius-product.md) (the "25 MB by default" size) and the upload row of
SPEC-002's bounds table.

## Goal

A screen recording of up to 1 GB dropped or pasted onto the new-task composer, a task's terminal
or the editor uploads on a default Yulius instance, and the agent gets a path it can open, so it
can extract frames from the video (ffmpeg) and look at them, as the user does with Claude Code on
their computer.

## Context

- **Symptom.** Dropping a video larger than 25 MB is refused: "the file is larger than the upload
  limit of 25 MB" (the server) or "larger than the server accepts" (the proxy's 413).
- **Cause.** Two limits, both deliberate (EDT-006, F 2026-10-10): the server's
  `YULIUS_UPLOAD_MAX_MB` defaults to 25 (`crates/yulius/src/serve.rs`; it already accepts 1 to
  1024), and the documented reverse proxy caps request bodies at 32 MB
  (`docs/operations.md` §4). This instance's Caddy (`/etc/caddy/Caddyfile`, outside the
  repository) has `max_size 32MB` on both sites, and its unit sets no `YULIUS_UPLOAD_MAX_MB`.
- **What already scales.** Uploads stream to a temporary file through a bounded buffer
  (`yulius_app::uploads`, 8 at once, 60 s idle bound, never held in memory); the browser streams
  a `File` body with progress, and its timeout counts from the last progress (`api.js`). The
  prompt attachment endpoint streams in 64 KiB chunks (SPEC-080, ADR-0064), independent of the
  file's size.
- **What does not.** (1) The data disk: 500 attachments per task at 1 GiB each can fill the disk
  that also holds the database; today only a full disk is caught (`Capacity`), after the fact.
  (2) The original-prompt dialog and the queued request card download a non-image attachment
  into memory as a `Blob` with a 120 s total timeout (`actions.js`, `taskview.js`): a 1 GiB video
  would need 1 GiB of browser memory and about 9 MB/s.
- **Video for the agent.** Claude Code reads images, not videos; on a computer the user's Claude
  extracts frames with `ffmpeg` and reads them (imported session of 2026-10-07). `ffmpeg` is a
  prerequisite of the virtual display (SPEC-002 ADR-15, `yulius doctor`), so it is on the host,
  and contained sandboxes see the host's `/usr`.

## Requirements

- UPL-001: The default upload limit is 1 GiB (`YULIUS_UPLOAD_MAX_MB` default 1024; its range
  stays 1 to 1024). The web UI's fallback when `/api/me` names no limit is the same.
- UPL-002: An upload whose declared length would bring the data disk to its critical level
  (SPEC-276: 95 % used) is refused before any byte is read and before any folder or file is
  created, saying how much can still be added before that level, as
  `Invalid` (422, "the server's disk has no room for this file", like a file over the limit, so
  the web UI shows it and does not retry it as it retries `Capacity`); an unknown disk size
  refuses nothing. A full disk during the write stays `Capacity` as before.
- UPL-003: A non-image attachment opened from the original prompt or a queued request card
  downloads through the browser's own download (a link to the attachment endpoint, which already
  sends `Content-Disposition: attachment`), not as an in-memory `Blob` with a total timeout. A
  HEAD first, so a file removed since the list was read says "not on the server"; a non-ASCII
  name is sent as `filename*` (RFC 6266) with an ASCII fallback, so the saved file keeps it.
- UPL-005: Sizes from 1 GiB read in GB ("1 GB", "1.5 GB") in the server's messages and the web
  UI, so the default limit does not read "1024 MB".
- UPL-004: The operations guide's proxy examples (Caddy, nginx) allow 1 GiB bodies with margin
  (1100 MB), and an upgrade note says to raise an existing proxy's limit; product, API and spec
  documents name the new default.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Uploads up to 1 GiB are accepted by default
- [x] An upload that would fill the data disk is refused before its bytes are read
- [x] Large non-image attachments download straight to disk from the prompt views
- [x] Proxy examples, upgrade note and ADR-0219 allow 1 GiB uploads; video analysis checked in a sandbox
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-1 (UPL-001): `yulius serve` without `YULIUS_UPLOAD_MAX_MB` sets a 1024 MiB limit (a test of
  the parsed arguments); the default `AppConfig` and `filedrop.js` fall back to 1 GiB; `/api/me`
  reports it.
- AC-2 (UPL-002): with the data disk at 100 GB total and 90 GB used, a declared 2 GB upload is
  accepted and a declared 6 GB one is refused with 422 before any file is created; an
  unknown disk accepts both (integration test, `SettableSystem`).
- AC-3 (UPL-003): the original-prompt dialog's "Download notes.txt" starts a link download of
  `/api/tasks/<id>/prompt/attachments/<n>` after one HEAD, without an API read of the file; a
  file removed meanwhile toasts "not on the server" and downloads nothing (e2e); `błąd-ekranu.mp4`
  is served with `filename*=UTF-8''b%C5%82%C4%85d-ekranu.mp4` (integration test).
- AC-4 (UPL-004): the docs name 1 GiB and 1100 MB; `rg "25 MiB|32 ?MB|32m"` finds no stale
  upload limit outside history (ADRs, older specs, the changes log).

## Test plan

- Unit/integration: `crates/yulius/tests/uploads.rs` (disk room), a `serve.rs` argument test.
- e2e: `e2e/tests/original-prompt.e2e.ts` (link download), then the full suite.
- Manual in this sandbox: make a short test video, extract frames with `ffmpeg` and read one, as
  an agent would with an attachment.
- Gates: `scripts/dev ci`, `yulius security-scan`.
- Not tested here: a real 1 GB upload through the public proxy (its limit is host configuration
  that only a person changes), and how Claude Code renders an `@`-mention of a video (it is a
  paid agent run).

## Decisions

- **1 GiB by default, not only by configuration.** The limit was already configurable to 1024;
  the request is that a default instance takes such a video, and the streaming upload path holds
  no file in memory, so the size costs disk, not memory. ADR-0219 records the choice and the
  disk guard that comes with it.
- **Guard the disk by its level, not a per-task byte quota.** Refusing an upload that would take
  the data disk to SPEC-276's critical level protects the database and every other task with the
  existing threshold, and needs no new setting. A per-task quota would refuse a second video on
  a disk with plenty of room. Concurrent uploads are each checked alone (at most 8 GiB in flight
  beyond the check), which the 5 % margin absorbs on the disks Yulius runs on.
- **Link download, not a longer timeout.** A longer timeout would still hold the whole file in
  browser memory; the endpoint already streams with `Content-Disposition`, so the browser's
  download manager can take it with its own progress.
