---
yulius-task: 3b36d422-c79c-44ce-8968-8bf49003df06
status: in-progress
---

# SPEC-400: Triage of the Vidoc findings on the vault, imports and recovery

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #379
Scope authorization: task #379 (close #841, #840, #740, #845 and #749, each as a false positive
or as solved; no new issues).

## Goal

Each of the five Vidoc findings is checked against `dev` (88a485ca). A real one is fixed
test-first. A false positive is dismissed with the guard that stops it, a test that pins that
guard, and a narrow `[[ignore]]` entry (one per wording) in `.config/vidoc.toml`, so Yulius does
not reopen the issue on the next scan.

## Context

- **#841 (fingerprint "guessing oracle").** `scrub_fingerprints_for`
  (`yulius-app/src/vault/runtime.rs:346`) gives the in-sandbox redactors (the Bash wrapper and the
  `PostToolUse` hook, `yulius-harness/src/scrub_client.rs:1-11`) each needle's length, a rolling
  hash and `SHA-256(salt || needle)`, with a fresh salt per set
  (`yulius-contracts/src/scrub.rs:1-6`). They redact inside the sandbox, so they must be able to
  test a window of output against a needle without the server. Any matcher that can do that is
  also an equality test for a guess, offline at hash speed while the run holds the set (the
  rolling hash with its known base alone already is). Redaction itself gives the same test online,
  one try per command: a printed guess comes back as `[secret:NAME]` over exactly the needle's
  bytes, which also shows the length. SPEC-002 ADR-20 and ADR-0014 (`docs/decisions/0014-secrets-vault.md:24`) accept
  "redaction is an oracle for weak values" with the 8-character minimum
  (`MIN_AGENT_VALUE_CHARS`, `yulius-domain/src/vault/mod.rs:45`, enforced at `:67`). ADR-0144
  (`:84-87`) names redaction and "the salted fingerprints a person's processes already read" as
  that equality oracle; ADR-0186 (`:86-87`) keeps it to weak values. A server-held
  HMAC key, Vidoc's suggested fix, would mean sending every output window to Yulius to be tested.
  That is the same oracle online, and it sends the run's output out of the sandbox.
- **#840 (host in audit keys).** `note_agent_use` (`runtime.rs:319-339`) builds the audit key
  `{action}|{run}|{record.id}|{context}`. The proxy passes `request.host` as `context`
  (`runtime.rs:1182-1188`), but only for secrets bound to that host: `bound_secrets`
  (`:298-314`) keeps only the records whose person-approved host patterns match (`bound_to`,
  `:135`). The host comes from hyper's parsed `Uri`. A CONNECT goes through `connect_target`
  (`yulius-egress/src/proxy.rs:514-522`, called at `:748`) and an absolute `http://` request
  through `uri().host()` (`:777-781`). `http` 1.5's `URI_CHARS` table gives 0 for `|` (124),
  space, `{`, `}` and every control byte, newline included, so the request is refused before
  Yulius sees it. The caller controls only the last field of the key, after the fixed prefix
  `action|run|id`, so the key cannot collide with another run's or secret's key. It can only
  repeat its own.
- **#740 (Superagent home root).** For a Superagent, `secret_source_root`
  (`yulius-app/src/vault/create.rs:387-390`) anchors `~/` at `users/<id>/home`. That folder is the
  Superagent's own `$HOME`: `user_space` sets `HOME` to it uncontained
  (`yulius-app/src/identity.rs:412`) and binds it as `/home/<login>` on a contained run
  (`:447`, `:473-474`). The Superagent therefore already reads every file there, and could print
  it or copy it into the project. Storing it as a secret is the safer path ADR-0167 chose for
  exactly this case, a private key in the person's own files
  (`docs/decisions/0167-agents-create-agent-secrets-from-a-file-yulius-reads.md:12`, `:30-34`).
  The new secret has no hosts and no `.env` allowance (`create.rs:331-338`), its creator (the
  person) needs write access, and a person allows each use. A task's agent gets the sandbox home
  only (`create.rs:384-386`).
- **#749 (recovery endpoint rate limit).** `recover_password`
  (`yulius-app/src/recovery.rs:93`) is stricter than a rate limit. It runs under one process-wide
  lock (`:100`) and counts each failure in the database before it answers (`:124-128`,
  `yulius-store/src/instance.rs:153-180`). After `MAX_FAILURES` = 3 wrong codes (`:19`) it erases
  the code and locks recovery for good (`:108-112`), until a signed-in superadmin issues a new
  code (ADR-0011). Three guesses at a 6-digit code succeed with probability 3·10⁻⁶. The test
  `recovery::the_recovery_code_resets_the_password_and_three_wrong_codes_lock_it`
  (`crates/yulius/tests/recovery.rs:107`) pins it, ten parallel guesses included (`:176-195`).
- **#845 (chunk upload race), real.** `put_import_chunk` (`yulius-app/src/imports.rs:659`) reads
  the job's `received` length, then `write_at` (`:1420`) truncates the file to that length and
  writes the chunk, and only after that does `advance_import_file` compare-and-set the length
  (`:749-757`). Two chunks of one file that both read the same `received` both write. The later
  write truncates and overwrites the earlier one even when its own compare-and-set then fails. An
  honest client can hit this: a retry racing its first attempt, or two clients. The file can then
  be shorter than its recorded length, and every later chunk fails as an internal error for good
  ("stored import file is shorter than its recorded progress"). It can also hold bytes the
  whole-file check never saw: if the competing write lands after the last chunk's verification,
  the file is marked complete with bytes that were never verified. Only the job's owner can
  upload (`owned_job`), so this is a broken integrity guarantee, not a cross-user attack.

## Requirements

- VTV-001 (#845, fixed). Chunks of one import file are applied one at a time. The length a chunk
  writes at is read again once nothing else writes that file, and it is the length that is
  verified and compared-and-set. A chunk that a competing chunk already covered is acknowledged as
  a repeat, never written. The part that holds the file runs to its end even when the request
  goes away (a spawned task owns the hold), so no write or stored length is still under way once
  the file is free. Regression tests reproduce both interleavings deterministically (test-only
  pause points, no sleeps): a short chunk that read a stale length is paused while a longer one
  completes; and a short chunk whose request is dropped mid-write makes a longer one wait for it.
  The upload must finish with the right bytes.
- VTV-002. The per-file lock is bounded: an entry lives while a chunk of that file holds it or
  waits for it; one left by a cancelled wait is removed when the next chunk is done.
- VTV-003 (#840, dismissed). A test in `yulius-egress` pins that a proxy target whose host holds
  `|`, a space or a line break is no `Uri` (refused before the policy). It also pins that
  `connect_target` returns a valid target's host and port.
- VTV-004 (#841, #740, #749, dismissed). These are covered by their existing guards and tests
  (Context). One narrow `[[ignore]]` per wording, with a reason that names the guard and its test
  or decision. `function` and `line` are given only where the function is short and does one
  thing (`scrub_fingerprints_for`, `note_agent_use`, `secret_source_root`), so a place match
  there cannot hide an unrelated finding. The route entry for #749 matches by title only.

## Plan

- [x] Spec SPEC-400 with the triage of all five findings
- [x] Chunks of one import file applied one at a time, pinned by a race test
- [x] Proxy host characters pinned by a test
- [x] Narrow [[ignore]] entries for the four dismissed wordings
- [ ] Issues commented and closed or linked to the pull request
- [ ] Gates, docs, changes entry and independent review answered

## Acceptance criteria

- `import_server::racing_chunks_of_one_file_never_overwrite_each_other` fails on `dev`'s code and
  passes with the fix: the stale short chunk is acknowledged without being written, the rest of
  the file uploads, and the stored file equals the offered bytes.
- `import_server::a_chunk_whose_request_goes_away_keeps_its_file_until_it_is_stored` fails
  without the hold (or with the hold freed by a dropped request) and passes with the fix: the
  longer chunk waits for the dropped one's write, then continues from its stored length.
- The per-file lock map is empty once no chunk is in flight (unit test).
- `yulius-egress` test `proxy_targets_with_delimiters_are_no_uri` passes.
- `.config/vidoc.toml` has four new entries. The scanner's own `apply_ignores` marks each of the
  four reported wordings ignored, and leaves #845's race wording and an unrelated title in the
  same files reported.
- #841, #840, #740 and #749 are closed as not planned with their evidence. #845 is linked to the
  pull request, whose `Fixes #845` closes it on merge.

## Test plan

- Two integration tests in `crates/yulius/tests/import_server.rs` (multi-thread runtime,
  `race::Point` hooks: `ImportChunkRead` runs the competing chunk while the first is paused;
  `ImportChunkWrite` holds a write while its request is dropped, and `ImportChunkWait`, reached
  only when `try_lock` fails, shows the other chunk waits).
- Unit tests in `imports.rs` (lock map bound) and `yulius-egress/src/proxy.rs` (host characters).
- `scripts/dev ci` (the full gate). The security scan is disabled for this project, and is
  reported as disabled, not as passed.

## Decisions

- **#845: an in-process lock per (job, file), held from a fresh read of the job through the
  compare-and-set.** The alternatives were:
  - write to a temporary file and rename it after the compare-and-set. That copies the whole
    prefix for every chunk, up to the file's declared size each time.
  - reserve the range in the store first, then write. That needs a new store state ("writing")
    and recovery for a crash between the two steps.
  - a database lock. SQLite cannot hold one across the file I/O.

  One server process owns the data directory (one binary, ADR-0001), so an in-process lock is
  enough. Its scope is one file, so other files and other jobs still upload in parallel.
- **The locked section runs in a spawned task** (review round 1). Its blocking write and its
  store calls keep running if axum drops the handler, so a hold dropped with the handler would
  free the file while they run. The task owns the hold and the body (`Vec<u8>`, taken from the
  route's `Bytes` without a copy) and the handler awaits it. Alternatives: moving the guard into
  the blocking closure frees it before the stored length; a cancel-safe handler is not something
  axum offers.
- The pause point sits after the first read of the job (authorization and validation) and before
  the lock. On `dev`'s code, which has no lock, the paused chunk then writes at the stale length.
  With the fix, it waits for nothing and reads the length again under the lock. The test therefore
  runs the same way on both and never deadlocks. The second test contends the lock for real:
  `ImportChunkWait` fires only after a failed `try_lock`, and the test waits for it or for the
  longer chunk's answer, whichever comes first (a 30 s bound only fails a hung run).
- The four dismissals are not "hardened" with a second mechanism. Fingerprints keyed by the server
  would move the oracle online and send output out of the sandbox. Escaping a host that cannot
  hold a delimiter would be dead code. A per-project Superagent home would hide nothing from an
  agent whose `$HOME` it is. A rate limit in front of a permanent lock after three attempts limits
  nothing.
- No issue is a duplicate of another. Each has its own wording, place and fingerprint, and each
  dismissed one gets its own `[[ignore]]` entry.

## Evidence

- Triage on `dev` at 88a485ca (file:line in Context and in each issue's comment): #845 is real
  and fixed; #841, #840, #740 and #749 are dismissed as false positives; none is a duplicate.
- **#845, red then green.**
  - `import_server::racing_chunks_of_one_file_never_overwrite_each_other` failed on `dev`'s code
    ("nothing written over the longer chunk: 111 bytes stored, 223 received", then 409 for the
    stale chunk). It passes with the lock at 593b8f58.
  - `import_server::a_chunk_whose_request_goes_away_keeps_its_file_until_it_is_stored` failed with
    the hold dropped along with the handler ("the longer chunk was applied beside a write still
    under way"). It passes with the spawned locked section at 1635da59.
  - Mutation at 1635da59: with the `hold` line deleted, the second test fails with that same
    message (the first still passes, as review round 1 predicted). Restored.
  - `imports::tests::chunk_locks_live_only_while_a_chunk_needs_them` passes. Mutation: with
    `retain(|_, _| true)` it fails ("nothing left once every chunk is done", 2 entries). Restored.
- **#840.** `yulius-egress` `proxy::tests::proxy_targets_with_delimiters_are_no_uri` passes.
- **`[[ignore]]` entries.** Four new entries in `.config/vidoc.toml`:
  - `scrub_fingerprints_for`, line 365 (#841)
  - `note_agent_use`, line 332 (#840)
  - `secret_source_root`, line 388 (#740)
  - the title of the route `Public recovery endpoint without visible rate limiting` (#749)

  `python3 -I` with `scripts/vidoc_scan.py` (`parse_config`, `apply_ignores`): the config parses
  (109 entries after merging `dev`). The four reported wordings at their reported lines are
  ignored, and so is a reworded repeat inside `scrub_fingerprints_for` or `note_agent_use`. These
  stay reported: #845's "Race condition in concurrent chunk uploads" in `put_import_chunk`, a CSRF
  title in `routes/mod.rs`, an unrelated title in `egress_apply`, and a reworded repeat on a line
  the branch changed. Review round 1 agreed to each dismissal and judged each entry narrow enough.
- **Gate.** `scripts/dev ci` at 8d7f1178: exit 0, 2991 tests passed, 16 skipped. At 56f39ac3
  (round-1 fixes, `origin/dev` 4a16b370 merged): exit 0, 2992 tests passed, 16 skipped.
- **Security scan.** `yulius security-scan`: "Security scan disabled" (Vidoc is off for this
  project in Yulius). Not run, not passed.
- **Not tested.** Two Yulius server processes sharing one data directory. The lock is in-process
  by design: one binary owns the data directory (ADR-0001).
- **Independent review, round 1 (APPROVE at 8d7f1178).** All four dismissals were agreed, with
  every citation checked. Findings, all own code, each answered:
  - Minor: the lock was never contended in a test. Answered by the second test and
    `race::Point::ImportChunkWait`, which is reached only after a failed `try_lock`.
  - Minor: the locked section was not atomic when the request is dropped. Answered by running it in
    a spawned task that owns the hold (1635da59).
  - Minor: no Evidence section, and Plan boxes unticked. Answered here.
  - Nit: VTV-002 wording. Aligned.
  - Nit: #841 is an offline test while a run holds the set, not only redaction's online test. The
    Context, the ignore comment and the issue comment now say so; the verdict is unchanged.
