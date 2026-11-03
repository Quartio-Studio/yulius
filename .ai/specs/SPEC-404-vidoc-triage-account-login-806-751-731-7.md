---
yulius-task: 9fafa37e-306c-42a1-ad23-c5541257a22e
status: done
---

# SPEC-404: Vidoc triage of the account login findings (#806 #729 #751 #731 #730)

Status: done
Date: 2026-11-03
Owner: Yulius task #383
Scope authorization: task #383 (a person asked that each of the five issues end closed). Fixes
#731. #806, #751 and #730 are dismissed as false positives, and #729 is a duplicate of #806.

## Goal

Each of the five Vidoc findings about the subscription sign-in in
`crates/yulius-app/src/accounts.rs` is checked against `dev` (ffbc2e91). Each one is either
fixed test-first or dismissed with file:line evidence and a narrow `[[ignore]]` entry for its
wording, so that a repeat scan does not reopen it.

## Context

- A sign-in runs `claude setup-token` in a server-owned PTY through the process host
  (`start_login`, `login_plan`). Only instance administrators start, watch, type into or cancel
  one (`require_instance_admin`). Its HOME and `CLAUDE_CONFIG_DIR` are new private folders under
  `<data_dir>/logins/<id>/`, which are wiped when it ends.
- Bubblewrap containment (`Containment::Bubblewrap`) is built per sandbox: its root, home and
  namespace holder are a sandbox's (`supervisor.rs`). A sign-in belongs to no sandbox.
- Viewers get output through the harness masker (`TokenMasker`), which holds back a possible
  token across chunks. A bounded raw copy (`RawOutput`, `LOGIN_RAW_MAX`) is kept for token
  extraction. `holds_credential` scrubs its temporary copies. `finish_login` zeroes the taken
  buffer before freeing it.
- The PTY host sends a `Snapshot` only while a client attaches (`host.rs` `attach`). The client
  reads it in its handshake (`client.rs` `handshake`), and `start_login` gets it as
  `attached.welcome.snapshot`.
- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring. There is
  one narrow entry per wording (SPEC-262, ADR-0202).

## Requirements

- ALG-001 (#731, real). The sign-in's raw output leaves no plaintext copy behind in freed memory
  that the server could have overwritten:
  - The buffer is allocated once at its bound and never reallocates. The old `Vec` growth freed
    earlier copies of the token without zeroing them.
  - When the sign-in ends, the whole allocation is zeroed up to its capacity, past the end of the
    output too. So is a buffer that is dropped without that, for example by a pump task that
    dies, and a buffer whose lock was poisoned.
  - The lossy UTF-8 copy that token extraction may make is scrubbed, as `holds_credential`
    already does.
  - Each raw output chunk is scrubbed once it has been copied and masked.
  - The extracted token is zeroized once it has been sealed into the account, or once sealing
    failed.
- ALG-002 (#806, false positive). The sign-in keeps the server's own `PATH` and runs uncontained.
  `PATH` comes from the server's environment, which only the operator sets. The argv is fixed,
  HOME and the config folder are new and empty, and only instance administrators type into it.
  It is dismissed with a narrow entry.
- ALG-003 (#729, duplicate of #806). It is labelled `duplicate`, closed, and given its own
  narrow entry, because it is Vidoc's other wording of the same finding.
- ALG-004 (#751, false positive). The prune and the lookup are separate locks, and that is
  harmless. A prune between them drops only a finished sign-in, so the answer is `NotFound`, the
  same as one a moment later. A replaced sign-in keeps its own id, `Notify` and phase. A second
  cancel is the second administrator's own audited request. It is dismissed with a narrow entry.
- ALG-005 (#730, false positive). The masker restarts only on a `Snapshot`. The PTY host sends a
  snapshot only in the handshake of an attach, which `start_login` makes right after the spawn,
  before any viewer can paste the code that makes the CLI print a token. It is dismissed with a
  narrow entry.

## Plan

- [x] Spec SPEC-404 with the triage of all five findings
- [x] The raw sign-in buffer is one allocation, wiped whole when the sign-in ends (#731)
- [x] Narrow [[ignore]] entries for the dismissed wordings (#806 #729 #751 #730)
- [x] ADR, changes entry and gates
- [x] Independent review answered
- [x] Dismissed issues commented and closed (#806 #729 #751 #730)

## Acceptance criteria

- `accounts::tests::raw_output_never_reallocates` fails on dev, where the `Vec` grows and moves,
  and passes with the fix. A chunk larger than the bound keeps its newest bytes.
- `accounts::tests::wiping_raw_output_zeroes_its_whole_allocation` shows that a buffer holding a
  token, partly pushed out by newer output, is all zeros over its whole capacity after the wipe.
- The existing sign-in tests (`accounts::` in the `yulius` integration binary: token split
  across chunks, cancel, grace capture) pass unchanged.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each issue ends closed: #731 by the merged pull request, and the others as not planned, each
  with a comment giving the evidence. #729 is also labelled `duplicate`.

## Test plan

- Unit tests in `crates/yulius-app/src/accounts.rs` (`RawOutput`).
- The `accounts::` integration tests in `crates/yulius/tests/accounts.rs`, unchanged.
- `scripts/dev ci` (the full gate) and `yulius security-scan`.

## Decisions

- #731 is fixed, not dismissed. The finding's claim that the buffer is "never zeroized" is wrong:
  `finish_login` zeroes it. But growth by doubling freed every earlier allocation with the token
  in it, unzeroed, and `drain` left old bytes past the end, which `fill` does not reach. Both are
  closed with safe code (`unsafe` is forbidden): the buffer is allocated once at
  `LOGIN_RAW_MAX`, and `resize` to its capacity then `zeroize` (volatile writes; the crate is
  already a dependency of `yulius-app`, used by the vault) zeroes all of it. The extracted
  `Secret` is zeroized once it has been sealed. The PTY client's read buffer and the masker's held
  bytes are outside this module and short-lived; they are out of scope. ADR-0256.
- #806 and #729: a sign-in runs on the host by design. Containing it would need a sandbox it does
  not have, the `claude` install and its runtime bound in read-only, and a network path to
  Anthropic, to guard against an instance administrator typing into a code prompt. A fixed
  `PATH` would break hosts where the operator installed `claude` elsewhere (docs/operations.md),
  and an attacker who can change the server's `PATH` already controls the server. ADR-0256.
- #751: tightening the two locks into one would change nothing observable, so the code stays as
  it is.
- #730: the restart is kept, because a later snapshot would show the whole screen again and the
  old masker's held bytes are on it.

## Evidence

- Triage on `dev` ffbc2e91 (the file:line evidence is in each issue's closing comment):
  - #806: `PATH` from the server's environment (`accounts.rs:1170`). The argv is fixed
    (`login.rs:21-23`), HOME and the config folder are new private folders (`accounts.rs:1098-1107`)
    and only administrators reach it (`:932`, `:977`). Containment is per sandbox
    (`supervisor.rs:1427-1449`). `tests/accounts.rs:395` pins `Containment::None`.
  - #729: the same code (`login_plan`, `accounts.rs:1169-1195`) and the same finding.
  - #751: `prune_logins` (`accounts.rs:1053-1071`) keeps running sign-ins. The lookups are at
    `:934-941` and `:979-986`. A cancel enables its wait before `is_running` (`:987-990`), and
    `notify_waiters` (`:1434`) comes after the final phase.
  - #730: the PTY host queues a snapshot only in `attach` (`host.rs:922-950`). The client reads
    it in the handshake (`client.rs:125-132`), so the reset arm (`accounts.rs:1299-1310`) is not
    reached by a sign-in.
  - #731: real. Growth freed earlier copies unzeroed, and the extracted `Secret` was dropped
    unzeroed.
- Regression tests:
  - `accounts::tests::raw_output_never_reallocates` failed on dev's `RawOutput` ("after a chunk of
    1 bytes": the pointer moved) and passes with the fix.
  - `accounts::tests::wiping_raw_output_zeroes_its_whole_allocation` fails when `wipe` does not
    zero, both with `fill(0)` removed and with `zeroize` removed (mutation checks), and passes
    with the fix.
  - The 12 `accounts::` integration tests pass unchanged.
- `[[ignore]]` entries in `.config/vidoc.toml`, each with the exact path, Vidoc's exact title and
  the function it is in:
  - `login_plan` "Login process launched with inherited PATH and no containment" (#806);
  - `login_plan` "Login process spawned without containment on the host" (#729);
  - `login_terminal` "TOCTOU between prune and login lookup" (#751);
  - `login_event` "Masking state reset on snapshots can split a masked token across chunks"
    (#730).

  The independent reviewer agreed to all four in round 1.
- `scripts/dev ci` at 35ec40e6: exit 0, nextest 3002 passed, 16 skipped. The run before it, at
  17e2e0ea, failed `cargo fmt --check`, and that was fixed by formatting.
- `scripts/dev ci` at e61175d9 (review round 1 answered): exit 0, nextest 3002 passed, 16 skipped.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius" (exit 0). This
  is disabled, not passed; nothing was scanned.
- Independent review, round 1 (fresh session, 35ec40e6): approved, with no blockers and no majors.
  It agreed to all four dismissals and entries. Its own-code minors were answered in e61175d9:
  - The `zeroize` rationale was wrong: the crate was already a dependency. The wipe now uses it.
  - The extracted `Secret` is now zeroized after sealing, and the docs no longer overclaim.
  - The wipe test now pushes the token's start out.
  - An oversized chunk now has to keep its newest bytes.
  - Each entry names its function, as `docs/engineering/development.md` asks.
  - The nits: a poisoned lock is still wiped, the "failed start" wording is fixed, the test
    paths in the comments are fixed, and so are the line numbers in the #751 comment.
- #806, #729 (labelled `duplicate`), #751 and #730 were closed as not planned, each with its
  file:line evidence on ffbc2e91. #731 closes with the pull request.
- Not tested: zeroing on `Drop` and the zeroing of the `Secret`. Safe code cannot observe freed
  memory (`unsafe` is forbidden). A sign-in's memory was not inspected on a real host either.
