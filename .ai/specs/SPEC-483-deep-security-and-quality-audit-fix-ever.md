---
yulius-task: 1fb06e11-08e1-4c37-90d0-3219e19122ad
status: in-progress
---

# SPEC-483: Deep security and quality audit: fix what it finds

Status: in-progress
Date: 2026-11-13
Owner: Yulius task #465
Scope authorization: the request of task #465: "Execute a deep security and quality audit and fix
everything. After this iteration, the system must be production-quality, secure and efficient.
Especially: (1) fix all ways we could hack the system (2) look for hidden state machines (3) look
for duplicated code, useless abstractions, overengineering and simplify (4) look for dead code and
dead solutions and remove it (5) look for contradictory logic in both code and product."

## Goal

Every confirmed finding of an audit of the whole repository is fixed and merged into `dev`: no
known way for an agent, a signed-in person with little access or an outside client to get what
Yulius's rules deny them; lifecycles decided by one rule; no logic kept twice; nothing kept that
no code path reaches; documents, texts shown to agents and people, and code that say the same.

## Context

- **How the audit ran.** Eleven read-only auditors, one per area (public API and access; internal
  listener and egress proxy; containment and host access to agent-owned files; vault; GitHub,
  Git, delivery, CI and CD; browser, previews, imports, updates and moves; web UI; domain, store
  and contracts; the two halves of the application layer with the composition crate; product
  documents), each with the same brief: threat model, what counts as a finding, read end to end,
  report with file and line. Each finding is verified here before it is fixed: the code path is
  read again, and a test that fails without the fix is written first. Their notes are kept
  outside the repository (`$HOME/.cache/yulius-audit-465/`).
- **Slices.** The fixes are delivered as pull requests of one concern each, most harmful first:
  1. the redaction fingerprints (V1, V2);
  2. what the host runs and writes where an agent can write: `bwrap` found on a run's PATH, the
     Git attributes and hook files written by path (C1, D1, D2);
  3. what the host reads where an agent can write: transcripts, `upper/etc/passwd`, numbered
     directories, disk usage (C2, C3, C5, I4);
  4. access rules of the public API and the merge gate (A1-A4, W1, D3, P8);
  5. the sandbox browser (E1-E3);
  6. the vault's remaining rules (V3-V6);
  7. delivery and CI decided once (D4-D6, QB1);
  8. lifecycles decided once (QA1-QA7, E6, E7, QC8, W2, W3);
  9. documents and shipped texts against the code (P1-P14, QB9);
  10. dead code (W7, D7, E5, QA9, QB4, QB6, QC2, QC5, QC7);
  11. duplicated code (W4-W6, QB2, QB3, QB7, QB8, QC3, QC4);
  12. efficiency (QA8, QB5, QC1, QC6).
  The Plan grows by a slice's steps when the slice before it is merged. The ledger below lists
  every finding with its slice and state.

### Slice 1: the redaction fingerprints

- **What they are.** The Bash wrapper and the `PostToolUse` hook run inside the sandbox and
  redact agent secrets from command output and tool results. They get fingerprints of every
  value and its encodings from `POST /internal/scrub/fingerprints` (the run's token) and from
  the run's `scrub.json` (`App::scrub_fingerprints_for`, `crates/yulius-app/src/vault/runtime.rs`).
- **The defect (critical).** A fingerprint held `rolling`, a polynomial hash of the needle
  modulo 2^61 − 1 at the set's `base` (`redact::rolling_hash`), and the server drew a new random
  `base` on every request. The hash is the needle's polynomial evaluated at `base`; as many
  requests as the value has bytes give as many points, and the bytes follow by solving the
  linear system. Any process with the run's token could read every agent secret that reaches
  the run this way, production secrets and the instance Vidoc key included. The accepted risk
  (ADR-0014, `.config/vidoc.toml`) was a guessing oracle for weak values, not this.
- **Guests.** A playground a guest joined gives no run an agent secret (SPEC-410 GST-004), but
  `scrub_fingerprints_for` still answered names, lengths and digests of the project's, the
  global and the instance's values.
- **Sessions across an update.** Task runs and Superagent shells outlive a server restart and
  keep the `scrub.json` their launch wrote.

## Requirements

- **FPR-001** The fingerprint set of a sandbox and holder is the same on every request and
  launch until an agent secret changes: the salt is derived from the server's workspace secret
  and the sandbox id; nothing else is random.
- **FPR-002** A fingerprint is `{secret, len, tag, digest}`. `tag` is the top `tag_bits(len)`
  bits (4 for 8 to 15 bytes, one more per doubling of the length, at most 16) of a 16-bit hash
  of the needle's first 8 bytes. The set has no `base`, a needle no `rolling`; the format
  version is 2.
- **FPR-003** The redactors find every form of a value as before (whole texts, streams, chunk
  boundaries) and leave other text alone, a text sharing a tag included.
- **FPR-004** A playground a guest joined gets a set without names and needles.
- **FPR-005** A starting server rewrites, in place, the fingerprint file of every live task run
  and of every adopted Superagent.
- **FPR-006** Redaction of 1 MB of output is not slower than before for a project of up to 40
  secrets.

## Plan

- [x] Fingerprints are fixed per sandbox and carry a tag instead of a rolling hash
- [x] A playground with guests gets no fingerprints
- [x] A starting server rewrites the fingerprint files of live sessions
- [ ] Independent review of slice 1 and its findings answered

## Acceptance criteria

- **AC-1** Three requests in a row to `/internal/scrub/fingerprints` answer the same body; it
  has the fields `version`, `salt`, `names`, `needles` only, each needle `secret`, `len`, `tag`,
  `digest` only, and every tag is below `2^tag_bits(len)`. Two sandboxes have different salts.
- **AC-2** The attack that recovered a 28-character value from 28 requests on `dev` has nothing
  to work on: no `base`, no `rolling`, equal answers.
- **AC-3** A fingerprint set made by the server redacts a value in the wrapper's matcher
  (`scrub_client::matcher`), in every encoding the tests cover.
- **AC-4** After a guest joined a playground, the maker's and the guest's tokens get empty
  names and needles; before, the maker's got the project secret's.
- **AC-5** A version 1 `scrub.json` of a live run is a version 2 file with the same inode after
  `refresh_fingerprint_files`, and redacts the run's holder's value.

## Test plan

- Domain (`vault/redact.rs`): tags (bits per length, only the first 8 bytes count, spread over
  all values), a text sharing a tag is not redacted, fingerprints too short or of an unknown
  secret are ignored; the existing stream and whole-text tests run on the new matcher.
- Harness (`scrub_client.rs`): the existing wrapper and hook tests on version 2 sets.
- Integration (`crates/yulius/tests`): `secrets::repeated_fingerprint_requests_answer_the_same_set`
  (AC-1, AC-2), `secrets::agents_list_names_and_read_redacted_results` (AC-3),
  `playground_guests::a_playground_with_guests_gives_no_run_an_agent_secret` (AC-4),
  `secrets_scopes::a_starting_server_rewrites_the_fingerprint_files_of_live_runs` (AC-5).
- The proof of the defect was a temporary test on `dev` (Evidence).
- Speed: a release-build probe on 1 MB of text with 10, 40 and 200 secrets, before and after.

## Decisions

- [ADR-0313](../../docs/decisions/0313-redaction-fingerprints-are-fixed-per-sandbox-and-say-a-few-bits.md):
  a fixed set per sandbox; a tag of 4 to 16 bits of a hash of the first 8 bytes instead of the
  rolling hash; an empty set for a playground with guests; live sessions' files rewritten at
  start. Rejected there: limiting requests, a fixed base, bits of a whole-needle rolling hash,
  more tag bits, redacting on the server, leaving production secrets out.
- The tag hash has no key: the redactor needs whatever the server uses, so a key would be in the
  set, and a tag that is the same in every sandbox says nothing a holder of two sets could not
  compute.
- The audit's slices are separate pull requests of this task instead of chained tasks: the
  findings share one ledger and one reviewer's context, and a slice's fix often changes how the
  next is best made.

## Findings ledger

State: `fixed` (merged), `slice N` (planned), `accepted` (a recorded decision, named).

| # | Area | Finding | Severity | State |
| --- | --- | --- | --- | --- |
| V1 | vault | Fingerprints with a per-request rolling-hash base give every value away | critical | slice 1 |
| V2 | vault | A playground with guests still gets fingerprints | medium | slice 1 |
| C1 | containment | A run's `bwrap` is looked up on the launch PATH, whose first entries are in the sandbox home | critical | slice 2 |
| D1 | git | `set_managed_attributes` reads and writes `.git/info/attributes` by path (host file read and write through links) | critical | slice 2 |
| D2 | git | The pre-push guard is written by path into the new worktree's Git directory; `git config --worktree` and `review.rs` open by path too | high | slice 2 |
| C2 | containment | `upper/etc/passwd` and `group` are read through links, unbounded, into people's `/etc/passwd` | high | slice 3 |
| C3 | host reads | Transcripts in the sandbox home are opened by path (FIFO blocks threads, links followed) | medium | slice 3 |
| C5 | host reads | `usage::measure` walks by path | low | slice 3 |
| I4 | host reads | `record_number` lists a directory through a link | low | slice 3 |
| A1 | access | A `read` API token reads the working tree (ignored `.env` included) over REST GET | medium | slice 4 |
| A2 | access | GitHub sign-in is not bound to the browser that started it | low | slice 4 |
| A3 | access | Revoking an API token leaves its open terminal sockets typing for up to 30 s | low | slice 4 |
| A4 | access | `GET /api/auth/github/start` stores states without a limit | low | slice 4 |
| W1 | web UI | The brand lab ships in the binary: `?lab=` injects CSS, which makes a device approval one blind click | high | slice 4 |
| D3 | delivery | The merge gate never pins a task pull request's base to the integration branch | high | slice 4 |
| P8 | delivery | A production-stage environment run starts for a writer with `confirm: true` (ADR-0110: an administrator typing the slug) | medium | slice 4 |
| E1 | browser | A viewer's mouse and keys drive a Chrome on the host: file chooser, context menu and DevTools reach host files | critical | slice 5 |
| E2 | browser | Browser tools reach other sandboxes' ports, host loopback services and link-local addresses | high | slice 5 |
| E3 | browser | `browser_fill_secret` checks a cached URL, then types after waiting for the input lock | high | slice 5 |
| V3 | vault | An edit re-seals the value it read: a concurrent rotation is undone | medium | slice 6 |
| V4 | vault | Redaction that cannot load the secrets passes text through in the terminal and the proxy, withholds elsewhere | medium | slice 6 |
| V5 | vault | Reading a value needs two-factor; sending it to a new host does not | medium | slice 6 |
| V6 | vault | "May this production secret reach this run" is decided four ways | low | slice 6 |
| D4 | CI | `ci_rerun` picks other jobs than the run it repeats | medium | slice 7 |
| D5 | delivery | "Checks green at this head" has five copies and three rules; a promotion never asks Yulius CI | medium | slice 7 |
| D6 | delivery | An armed merge outlives its task's run, and a restart decides it the other way | medium | slice 7 |
| QB1 | interface | The internal listener maps errors to statuses by a second table; `yulius wait` reads Yulius's own 502 as an outage | medium | slice 7 |
| QA1 | lifecycle | A start that has to queue forgets how it was to start | medium | slice 8 |
| QA2 | lifecycle | A failed re-login turns `relogin` into `error`, and the next usage reading into `ok` | medium | slice 8 |
| QA3 | lifecycle | After a restart a hand-over no longer restarts a run holding the previous owner's personal MCP servers | medium | slice 8 |
| QA4 | lifecycle | The `/goal` still to be typed into a resumed session is lost at a restart | medium | slice 8 |
| QA5 | lifecycle | A background probe without an answer means "busy" everywhere but at a subscription switch | medium | slice 8 |
| QA6 | lifecycle | "May Yulius type now" has three rules; `/compact` is typed behind a person's draft | medium | slice 8 |
| QA7 | lifecycle | A switch that times out records the old run lost without proof | medium | slice 8 |
| E6 | move | A move's report state is a string with three readings of "lost" | medium | slice 8 |
| E7 | move | An unreadable move mark does nothing and stays | low | slice 8 |
| QC8 | store | `tasks.closed_at` is never cleared | low | slice 8 |
| W2 | web UI | Sidebar groups (client) and counts (server) decide a paused task differently | medium | slice 8 |
| W3 | web UI | A 403 on a live edit leaves the document unsaved for good | medium | slice 8 |
| P1-P7, P9-P14 | documents | Contradictions between documents, shipped texts and code (promotion, hand merges, spec template, docs and chore workflows, UI copy, API table, guard text, briefing, stale engineering documents) | medium | slice 9 |
| QB9 | documents | Undocumented options and routes, stale architecture sentences | low | slice 9 |
| W7 | web UI | Dead exports and CSS rules | low | slice 10 |
| D7 | GitHub | `webhook_secret` is stored and never read | low | slice 10 |
| E5 | previews | The preview bridge has no listener since ADR-15 | low | slice 10 |
| QA9, QC2, QC5, QC7, QB4, QB6 | dead code | Store functions, task events and reasons, contract items, a one-shot workflow shim, Cargo features nothing uses | low | slice 10 |
| W4, W5, W6 | web UI | The unsaved-changes guard five times, the pull request chip three times, byte and commit formatters | low | slice 11 |
| QB2, QB3, QB7, QB8 | duplication | Three loopback HTTP clients, the device contract twice, the Superagent's tools dispatched twice, five bearer parsers | low | slice 11 |
| QC3, QC4 | duplication | RFC 3339 parsing four times with different inputs; the GitHub-token scrub twice | low | slice 11 |
| QA8, QC1, QC6, QB5 | efficiency | Store round trips per task, unindexed capacity scans, the watched transcript re-read every 1.5 s | medium | slice 12 |
| C4 | containment | The engine leaf's limits are writable from a nested cgroup namespace | low | slice 5 |
