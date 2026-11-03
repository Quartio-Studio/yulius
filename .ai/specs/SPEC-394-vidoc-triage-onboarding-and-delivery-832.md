---
yulius-task: 3164f503-7f40-49ea-ad14-8647041e5299
status: done
---

# SPEC-394: Vidoc triage of the onboarding and delivery findings (#832 #783 #744 #834 #781)

Status: done
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
  inside it keep working as before. A checkout in a sandbox (its clone, a task worktree) is read
  with `validate_sandbox_checkout`, only when its resolved root is inside the sandbox directory:
  a worktree folder swapped for a link to a host folder reads as empty.
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
- [x] validate_checkout reads the checkout only through no-follow handles (#744)
- [x] The production-branch warning shows only the error's public message (#834)
- [x] Narrow [[ignore]] entries for the dismissed wordings (#832 #783 #781)
- [x] Docs and changes entry
- [x] Full gate green on the reviewed head
- [x] Independent review answered
- [x] Dismissed issues closed with their evidence (#832 #783 #781)

## Acceptance criteria

- An integration test swaps the checkout's `yulius.toml` for a link to a host file between the
  root's resolution and the read (race point `CheckoutRead`); it fails on `dev` (the host file's
  text is returned) and passes.
- The same with a FIFO swapped in: the read on `dev` blocks (the test times out after 20 s); with
  the fix the file is missing at once.
- A unit test shows a task worktree that is a link to a host folder reads as empty through
  `validate_sandbox_checkout` (on an unanchored check it returned the host's `yulius.toml`).
- A unit test pins that an in-root folder link still reads and that a FIFO or a linked decision
  is missing.
- A test shows the production-branch warning for an `Internal` failure does not contain its
  detail and still says the check failed.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each issue ends closed: #744 and #834 by the merged pull request, the others as not planned
  with a comment giving the evidence.

## Test plan

- Unit tests in `crates/yulius-app/src/onboarding.rs` (tempdir checkouts and sandboxes).
- Integration tests in `crates/yulius/tests/projects.rs` with the `CheckoutRead` race point.
- `import_server::import_resumes_applies_and_reruns_idempotently` with the repository fake's new
  `fail_branch_guard` failing with `Internal`.
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
- The anchor is the sandbox directory, as ADR-0174 says: checking handles against the
  checkout's own resolved root alone still trusts that root, and the worktree folder is the
  agent's to swap. The import's first clone, read before any agent runs, keeps
  `validate_checkout`. Three `projects` tests placed a task worktree in a scratch folder outside
  the sandbox; they now use the sandbox's `worktrees` folder, where Yulius puts them
  (`supervisor.rs`).
- No new ADR: the fix applies ADR-0174 to one more reader.

## Evidence

- Triage on `dev` c1043d15 (the evidence in each issue comment is linked at that commit):
  - #832: `redact` (`yulius-domain/src/redact.rs:354-365`) only runs detectors, and `apply`
    (`:467-471`) returns `None` exactly when there are no spans. `add_handover`
    (`onboarding.rs:1445-1447`) thus falls back only to text with nothing to redact.
  - #783: the prompt tells the agent to read the repository's AGENTS.md, CLAUDE.md, skills,
    CONTRIBUTING and README, and to follow them as authoritative (`onboarding.rs:397-399`).
    Problem lines are `<path>: <fixed message>` (`spec_format/project.rs:145-187`), with at most
    `MAX_PROMPT_PROBLEMS` of them. The reviewer noted that a decision file's name can carry a
    few words of repository text; that comes from the same author as AGENTS.md, and the issue
    comment says so.
  - #781: `update_sandbox_profile` requires `Action::Administer` (`delivery.rs:2125-2132`).
    Every read goes through `delivery_profile` → `profile_from_contract` → `validate()`
    (`delivery.rs:355-360`, `:282-283`). That allows at most `MAX_COMMAND_BYTES` and no `\n`,
    `\r` or `\0` (`yulius-domain/src/delivery.rs:29`, `:388-395`).
- Regression tests that failed before the fix:
  - `projects::a_checkout_file_swapped_for_a_link_after_its_check_is_not_read`: on the old code
    it returned `Some("host = \"secret\"\n")`.
  - `projects::a_checkout_file_swapped_for_a_fifo_after_its_check_does_not_block`: on the old
    code it hit the 20 s `recv_timeout` (`Timeout`).
  - `onboarding::tests::a_sandbox_checkout_resolving_outside_the_sandbox_is_not_read`: an
    unanchored check returned the host's `yulius.toml`.
  - `import_server::import_resumes_applies_and_reruns_idempotently`: on the old code the warning
    said `(internal error: database /var/lib/yulius/yulius.db is locked)`.
- Review round 1 asked for two more tests. Both are proven by mutation:
  - `projects::onboarding_becomes_ready_when_the_preparation_task_lands`: with the call site
    back on `validate_checkout`, the status was `ready` instead of `preparing`.
  - `projects::a_checkout_folder_swapped_for_a_link_after_its_resolution_is_not_read`: an open
    by path with `O_NOFOLLOW` alone read the host file.
- `onboarding::tests::checkout_validation_follows_inner_folder_links_and_skips_fifos` pins that
  the old behaviour is kept: an in-root folder link is followed, and a FIFO or a linked decision
  file is missing.
- `scripts/dev ci` at 1c1432b8: exit 0, 2968 tests passed, 16 skipped.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius". Nothing was
  scanned, and this is not a pass. The three `[[ignore]]` entries were therefore not checked
  against a live scan.
- Independent review in a fresh session: round 1 approved 8c64fa1c with 2 minors and 2 nits, all
  fixed in 1c1432b8; round 2 approved 1c1432b8. The reviewer agreed with all three dismissals.
- #832, #783 and #781 have comments with their evidence and are closed as not planned.
- Related and not fixed here: #745 (open). `read_repo_file` in `delivery.rs`, used by
  `spec_check` and the Stop gate, still checks a spec path and then reads it by path.
