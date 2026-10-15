---
yulius-task: b4939f6f-6afd-4cb2-97b6-aed728d10432
status: done
---

# SPEC-138: open GitHub issues triaged; stateless installation tokens redacted; flaky waits fixed

Status: implemented
Date: 2026-10-15
Owner: Yulius task #118
Scope authorization: user request: "Fix all open actionable issues from GitHub without creating
new issues, and close them."

## Goal

Every open GitHub issue of the repository ends this task in one of three states: closed as
already fixed on `dev`, fixed by this task or by a chained Yulius task that closes it, or left open
with a comment naming what only a person can provide. No new GitHub issue is created.

This task's own change fixes:
- the high-priority security gap #424: no redactor removes a whole stateless GitHub App
  installation token;
- the `.env` refusal and the test gaps of `secret_copy_to_env` (#475);
- two families of flaky tests whose waits do not match what they assert (#430, #446, #455).

## Context

The triage covered 63 open issues on 2026-10-15, against `dev` at 43731493 (PR #490 merged).

**Closed as already fixed on `dev`, each with a comment naming the change:**
- #423: PR #426 (SPEC-107). The update helper accepts tokens up to 4096 bytes.
- #425: PR #428 (SPEC-106, ADR-0086). Covers the worktree's main repository trusted, a trust
  answer that is not a draft, and a lost onboarding task that returns to Start onboarding.
- #465: PR #490 (SPEC-137) made the test wait for the page before it leaves the sandbox. The
  product behaviour behind it, a sandbox left while it loads, is #489, handed on below.

**Not actionable here, left open with a comment naming what is needed:**
- #5, #6, #7, #11, #14: checkpoints whose remaining items need live Codex or Claude accounts, a
  real GitHub App flow, or real Chromium and openvscode-server on the VM. Their last comments
  already say so.
- #15: the progress tracker.
- #321: the cgroup split reached `main` (#476), but checking it needs a person on the VM's host.
  Task sandboxes see no host cgroup tree (`/sys/fs/cgroup` is absent in the sandbox).
- #470: `api.vidoc.dev` is bound now, but Vidoc answers 401 to the stored `VIDOC_API_KEY`. A
  person must replace the key on the Secrets page.
- #412: SPEC-001 ACC-005 says every user signs in with GitHub and has no password. Inviting
  teammates without GitHub changes that user requirement, so the product owner must decide it.

**Handed to chained tasks.** Each chained task gets one concern, about 400 changed lines. There
are four lanes, each running one task at a time, so at most four Rust builds run beside the other
tasks (the sandbox's disk quota, #240):

| Lane | Tasks (issues), one after another |
| --- | --- |
| A: delivery, supervisor, onboarding | #123 (#366 #448) → #124 (#385 #389) → #125 (#346 #453) → #126 (#406 #431 #399) |
| B: tests under load, CI, playgrounds | #127 (#404 #464 #353 #443) → #128 (#460 #485) → #129 (#488) → #130 (#411) → #131 (#409 #420) → #132 (#410) |
| C: web UI and e2e | #133 (#438 #444 #461 #469 #415) → #134 (#354 #349 #356 #394) → #135 (#489 #439 #463) → #136 (#477 #478 #436) → #137 (#350 #380 #363) → #138 (#348 #331 #407 #449) |
| D: security and features | #139 (#486) → #140 (#466) → #141 (#395) → #142 (#416) |

Lane C starts after this task, because this task edits the e2e files that lane C's first tasks
also edit. Each chained task is told: create no issue, check first whether its issues are
already fixed, fix them test-first, and close each issue once its fix is on `dev`.

**This task's own defects:**
- **#424.** GitHub now issues installation tokens as `ghs_<APPID>_<JWT>`, about 520 characters
  long. Three redactors assume the old 36-character `[A-Za-z0-9]` body:
  - `yulius_domain::redact` (transcripts, sessions, the hook output) reads only the app id and
    does not reach its minimum of 36, so the token passes in full. Its JWT rule skips the JWT
    too, because a word byte (`_`) comes right before `eyJ`.
  - `yulius_git::redact` (Git errors) stops at the first `.`.
  - `yulius_github::error::scrub` (GitHub API errors) also stops at the first `.`. Its JWT pass
    then sees only two segments and leaves the payload and the signature in clear text.
- **#475.** `dotenv::upsert` treats everything after a quote that never closes as one value: the
  names it appends land inside that value. Through `secret_copy_to_env`, two refusals are covered
  only by unit tests: a multi-line value for a copied name, and an `.env.example` keeping its
  mode.
- **#430.** `context_full::a_full_fresh_session_needs_a_person` waits for the activity
  `NeedsAttention`, then asserts the last audit row. `give_up_on_full_context` raises the flag and
  sends the notice before it writes that row. Two sibling assertions (the compacted and the
  fresh-session paths) read the audit right after the notice in the same way.
- **#446, #455.** The web UI loads `mock.js` with a dynamic import, so `window.__yuliusMock` can be
  missing right after `app.open`. `support.ts` offers `mockReady(browser)`, but some tests touch
  the mock before any UI wait. `notes.e2e.ts`'s `editAs` and `stored` failed this way under load.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RED-101 | `yulius_domain::redact` replaces a whole stateless installation token, the `ghs_` prefix through the JWT signature, as one `github-token` finding labelled `ghs_` (#424). | `redact.rs` unit test |
| RED-102 | `yulius_git::redact` and `yulius_github::error::scrub` leave no part of a stateless installation token in their output (#424). | unit tests in both crates |
| RED-103 | Old-format tokens (`ghs_` plus 36 alphanumerics) and the existing near misses redact as before. Text that only looks like the new shape is not a token: it needs `ghs_`, digits, `_` and a JWT header starting with `eyJ`. | existing tests unchanged, plus a near-miss case |
| ENV-101 | `dotenv::upsert` refuses a `.env` whose quoted value is still open at the end of the file, naming the line, and nothing is written (#475). | domain unit test, `secrets_env.rs` |
| ENV-102 | Through `secret_copy_to_env`, a copied name that holds a multi-line value is refused before a person is asked: `env_files` stays `no` and nothing is written. An existing `.env.example` keeps its mode (#475). | `secrets_env.rs` |
| TST-101 | `context_full` tests wait for the audit rows they assert, with the bound they already use, never a sleep (#430). | `context_full.rs` |
| TST-102 | No e2e test reads or writes `window.__yuliusMock` before the mock exists. Either `app.open` cannot return earlier, or the test waits for the UI or for `mockReady` first (#446, #455). | e2e suite |

## Plan

- [x] Open issues triaged: fixed ones closed, the rest handed to chained tasks
- [x] Every redactor removes a whole stateless installation token (#424)
- [x] A .env with an unclosed quote is refused, and the copy's refusals are tested end to end (#475)
- [x] Flaky waits wait on what they assert: context_full audit rows and the e2e mock (#430, #446, #455)
- [x] Independent review approves the head, findings fixed test-first
- [x] Gates green, spec evidence and changes log written, issues closed

## Acceptance criteria

- RED-101, RED-102 and ENV-101 each have a test that fails on `dev` (43731493) and passes with the
  fix.
- ENV-102 has integration tests through the MCP tool.
- TST-101 and TST-102 change only the tests' waits. Each changed file passes repeatedly while the
  CPU is loaded.
- `scripts/dev ci` passes, the full e2e suite passes, Yulius CI is green on the head, and an
  independent review approved the head.
- Every issue this task fixes is closed with a comment naming the pull request. Every handed-off
  issue is named in a chained task's request. Every issue left open has a comment saying what
  blocks it.

## Test plan

- Unit tests:
  `cargo nextest run -p yulius-domain -p yulius-git -p yulius-github -E 'test(/redact|scrub|dotenv/)'`.
- Integration: `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^(secrets_env|context_full)::/)'`,
  `context_full` repeated under CPU load.
- e2e: `npx e2e run tests/notes.e2e.ts` repeated under load, then the full suite.
- `scripts/dev ci` and `scripts/dev security`.

## Decisions

- **Split into chained tasks, as task #47 did.** About 50 actionable issues across the
  supervisor, delivery, CI, playgrounds, security and the web UI are far beyond one reviewable
  pull request. Each chained task takes related issues that touch the same code, so one review
  can judge them together. Four lanes run in parallel. Lane C waits for this task, because both
  edit the e2e files.
- **#424 is fixed here, not handed on.** It is the only high-priority security issue whose fix is
  small and self-contained. While it is open, every installation token that reaches a transcript
  or a log is stored in full.
- **One matcher for the stateless shape, in `yulius-domain`.** `yulius-git` and `yulius-github`
  already depend on `yulius-domain`, so a single public function decides where such a token
  ends. The three redactors cannot then disagree about its end. Widening each redactor's
  character set to include `.` and `-` was rejected: it would change what the old formats redact,
  for example the full stop after a token at the end of a sentence.
- **The stateless shape is strict:** `ghs_`, one or more digits, `_`, then a header that starts
  with `eyJ`, followed by up to two more dot-separated base64url segments. A looser rule would
  catch `ghs_` identifiers in prose. Each segment is read up to the existing run limit (1024
  characters, about twice a whole real token), so a long input costs linear time.
- **`ghs_` comes first in the Git and GitHub redactors' prefix lists** (review round 1). They find
  a prefix anywhere in a word, so `ghu_` inside a token's base64url payload (about one token in
  10,000) used to cut it there before the `ghs_` pass, leaving the rest in clear text. The
  transcript redactor matches only at word starts, so it never had this problem.
- **#430 is fixed in the test, not by reordering the product.** The product's order (flag, notice,
  audit) is deliberate: a person sees the flag first. The test asserts the audit, so it waits for
  the audit.
- **#475's unclosed quote is refused, not repaired.** Yulius cannot know where a value that never
  closes was meant to end. Writing after it would hide the new names inside that value, so the
  person is told which line to fix.
- **#412 is left to the product owner.** ACC-005 is a user requirement of SPEC-001, not a
  SPEC-002 engineering assumption, so an agent cannot overturn it in an ADR.

## Evidence

Base `dev` 43731493. Head approved by the independent review: 774d793a.

| Requirement | Test | Before the fix | After |
| --- | --- | --- | --- |
| RED-101 | `yulius-domain` `redact::tests::a_stateless_installation_token_is_redacted_whole`, `…the_stateless_shape_needs_an_app_id_and_a_jwt_header` | failed: "not redacted" (the whole token kept) | pass |
| RED-102 | `yulius-git` `redact::tests::removes_a_whole_stateless_installation_token`; `yulius-github` `error::tests::scrubs_a_whole_stateless_installation_token` | failed: `[redacted].eyJ…payload….signature` left in clear | pass |
| RED-102 (review round 1) | `…a_stateless_token_holding_another_prefix_is_removed_whole` (Git), `…scrubs_a_stateless_token_holding_another_prefix_whole` (GitHub) | failed: `[redacted][redacted]-PAYLOAD….SIGNATURE…` | pass |
| RED-103 | the existing near-miss and provider-token tests, plus `ghs_1234567_notatoken` and `ghs_app_eyJ…` near misses; Git's `a_bare_prefix_does_not_hide_a_later_token` (review round 2) | the bare-prefix test failed: both tokens kept | pass |
| ENV-101 | `yulius-domain` `vault::dotenv::tests::a_quote_left_open_at_the_end_is_refused` | failed: the names were written after the open quote | pass |
| ENV-101, ENV-102 | `secrets_env::a_dotenv_that_cannot_be_updated_is_refused_and_the_example_keeps_its_mode` | failed with the old `dotenv.rs`: the unclosed quote was not refused and a person was asked | pass (7 of 7 `secrets_env::`) |
| TST-101 | `context_full::` (the 3 changed tests) | not reproduced: 15 rounds of the original under 12 extra CPU-bound workers, load average about 60, all passed (the window is one database write) | 50 of 50 rounds passed under the same load (load average 53–62); 8 of 8 `context_full::` |
| TST-102 | `e2e/tests/mock-ready.e2e.ts` (mock.js held back 1.5 s by a route) | failed: `EVALUATE_FAILED: Cannot read properties of undefined (reading 'sandboxes')`, the error of #455 | pass; `notes.e2e.ts` and `mock-ready.e2e.ts` passed 5 of 5 rounds under 12 extra CPU-bound workers (load average about 30) |

Gates:
- `scripts/dev ci` at 6ff8db4b: exit 0, 1752 tests passed, 16 skipped. The commits after it change only
  the Git and GitHub redactors' prefix order and loop, their tests, comments, `e2e/tests/support.ts`
  and the specs, all re-run: `cargo test -p yulius-domain -p yulius-git -p yulius-github --lib`
  (317, 23 and 15 passed) and `cargo clippy … --all-targets -D warnings` (clean).
- Full e2e suite (`npx e2e run`) at 67b7452c: exit 0, 53 files passed, 1 skipped; 443 tests passed, 1 skipped (`agent.e2e.ts`, which needs a model).
- The 15 touched e2e files, run together at 3e30c1c3: 150 of 151 passed. The one failure was
  `editor.e2e.ts` "two people in one file…", from Monaco's "depends on UNKNOWN service
  ICodeLensCache" under load: #394, handed to task #134, and not related to this change.
- `scripts/dev security`: **not run**, exit 2, "Vidoc rejected the key (HTTP 401: Invalid or
  expired token)". The stored key needs replacing by a person (#470). Per ADR-0104 this is
  reported as unrun, not passed.

Independent review (a fresh read-only session, skill `om-code-review`):
- **Round 1, 6ff8db4b: request changes.** The major finding was a partial leak when another prefix
  (`ghu_`) sat inside a token's payload. It was fixed test-first in 67b7452c, together with two
  minors (the mock wait's failure message, and the app's own `mock=1` test) and three nits.
- **Round 2, 67b7452c: approve.** One minor that predates this task (a bare prefix hid a later
  token in Git's redactor) was fixed test-first in 774d793a.
- **Confirmation, 774d793a: approve,** with no findings.

Not tested here: a live GitHub App token through the real update and transcript paths; the
synthetic tokens follow the documented `ghs_<APPID>_<JWT>` shape. macOS (`F_GETPATH` on a non-UTF-8
worktree path, a remark in #475) is not covered; this host is Linux.

