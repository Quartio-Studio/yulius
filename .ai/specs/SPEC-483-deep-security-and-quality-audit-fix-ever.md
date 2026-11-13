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
- **Slices.** The fixes are delivered as pull requests of one concern each, in this order:
  1. the redaction fingerprints (this document's first Plan items);
  2. host access to agent-owned paths (Git directory files written by path, transcripts opened
     by path, `bwrap` found on a launch PATH the agent writes, `upper/etc/passwd`);
  3. access rules (a `read` token on the editor's routes, GitHub sign-in bound to its browser,
     token revocation and open sockets, the brand lab in shipped builds, the merge gate's base,
     production-stage environment runs, vault edits and redaction failures);
  4. contradictions between documents, shipped texts and code;
  5. duplicated and dead code, and lifecycles decided in more than one place.
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
