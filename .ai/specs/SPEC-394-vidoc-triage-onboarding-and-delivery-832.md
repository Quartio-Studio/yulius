---
yulius-task: 3164f503-7f40-49ea-ad14-8647041e5299
status: in-progress
---

# SPEC-394: Vidoc triage of the onboarding and delivery findings (#832 #783 #744 #834 #781)

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #376
Scope authorization: task #376 (a person asked that each of the five issues end closed). Fixes
#744 and #834; #832, #783 and #781 are dismissed as false positives.

## Goal

Each of the five Vidoc findings about onboarding and delivery is checked against `dev`
(f416b400) and either fixed test-first or dismissed with file:line evidence and a narrow
`[[ignore]]` entry for its wording, so a repeat scan does not reopen it.

## Context

- `validate_checkout` (`crates/yulius-app/src/onboarding.rs`) reads a checkout's documents on the
  host, outside containment: the sandbox clone (`sandbox_repo`) or the onboarding task's worktree,
  both in the sandbox directory, where a contained shell (root in its sandbox) can swap any
  folder or file for a link at any moment. It checked each path with `symlink_metadata` and
  `canonicalize`, then opened the path again with `File::open`, which follows links and blocks on
  a FIFO.
- ADR-0174 / SPEC-220: such reads open a handle and decide on it: `O_NOFOLLOW | O_NONBLOCK`, a
  regular-file check on the handle, and (where links inside the root are followed by design) the
  handle's real path inside the root and out of `.git` (`files::open_inside`, `files::real_path`).
- `production_branch_warning` puts `{err}` (the `Display` of an `AppError`) into a warning that
  `onboarding_status` shows to everyone with View access. `AppError::Internal` documents "details
  are logged, not returned", and `public_message()` exists for that, but `Display` of `Internal`
  includes the detail.
- `yulius_domain::redact::redact` cannot fail: `None` means "nothing found" (`redact.rs` doc and
  `apply`).
- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring; one narrow
  entry per wording (SPEC-262, ADR-0202).

## Requirements

- VOD-001 (#744, real). `validate_checkout` reads no file and lists no folder by path after a
  check: each file is opened with `O_NOFOLLOW | O_NONBLOCK`, used only when the handle is a
  regular file whose real path is inside the canonical root and out of `.git`, and read through
  that handle; a decisions folder is opened the same way with `O_DIRECTORY` and its entries opened
  from the folder's handle with `O_NOFOLLOW`. A file swapped for a link to a host file after the
  root was resolved yields nothing; a FIFO does not block. Links inside the root that resolve
  inside it keep working as before.
- VOD-002 (#834, real). The production-branch warning carries the error's public message
  (`AppError::public_message`), so an internal error's detail stays in the server log.
- VOD-003 (#832, false positive). `redact` returns `None` only when it found nothing, so
  `unwrap_or(notes)` keeps text with no secret; dismissed with a narrow entry.
- VOD-004 (#783, false positive). The problem lines in the onboarding prompt are fixed messages
  plus repository paths, and the same prompt tells the agent to read and follow the repository's
  own AGENTS.md and CLAUDE.md as authoritative; repository text is the agent's input by design,
  inside containment. Dismissed with a narrow entry.
- VOD-005 (#781, false positive). The environment command changes only with Administer (or the
  importer's seed); every read revalidates it to at most `MAX_COMMAND_BYTES` without `\n`, `\r`
  or `\0`, and the command's own output reaches the run log anyway. Dismissed with a narrow
  entry.

## Plan

- [x] Spec SPEC-394 with the triage of all five findings
- [ ] validate_checkout reads the checkout only through no-follow handles (#744)
- [ ] The production-branch warning shows only the error's public message (#834)
- [ ] Narrow [[ignore]] entries for the dismissed wordings (#832 #783 #781)
- [ ] Gates, docs and changes entry
- [ ] Independent review answered
- [ ] Dismissed issues closed with their evidence (#832 #783 #781)

## Acceptance criteria

- An integration test swaps the checkout's `yulius.toml` for a link to a host file between the
  root's resolution and the read (race point `CheckoutRead`); it fails on `dev` (the host file's
  text is returned) and passes.
- A unit test shows a FIFO at a layout path is read as missing without blocking, and an in-root
  link to a folder still reads.
- A test shows the production-branch warning for an `Internal` failure does not contain its
  detail and still says the check failed.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each issue ends closed: #744 and #834 by the merged pull request, the others as not planned
  with a comment giving the evidence.

## Test plan

- Unit tests in `crates/yulius-app/src/onboarding.rs` (tempdir checkouts).
- Integration test in `crates/yulius/tests/projects.rs` with the `CheckoutRead` race point.
- Test of `production_branch_warning` with a fake GitHub failing with `Internal`.
- `scripts/dev ci` (the full gate) and `yulius security-scan`.

## Decisions

- #744 is fixed rather than dismissed: the checkout belongs to contained agents and the server
  reads it outside containment, which is exactly the ADR-0174 boundary. `files::open_inside`
  keeps the current semantics (in-root links followed, the final link refused) while deciding on
  the handle, so no repository layout that validates today stops validating. Walking every
  folder with `O_NOFOLLOW` instead would refuse in-root folder links, which repositories use.
- #834 is fixed although it is low: the leak is small (GitHub errors are scrubbed, most failures
  are actionable messages), but `public_message` is the documented contract and the fix is one
  call.
- No new ADR: the fix applies ADR-0174 to one more reader.
