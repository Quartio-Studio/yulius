---
yulius-task: 03c4d16d-68a1-457f-878e-5d455635e89d
status: in-progress
---

# SPEC-405: Vidoc triage of the egress cache and vault request findings (#835 #711 #710 #758 #815)

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #382
Scope authorization: task #382 (a person asked that each of the five issues end closed). Fixes
#710. #711 is a duplicate of #757, which #1022 fixed. #835, #758 and #815 are dismissed as false
positives.

## Goal

Each of the five Vidoc findings about the vault runtime, vault requests and the supervisor is
checked against `dev` (ffbc2e91). Each one is fixed test-first, closed as a duplicate of a fixed
issue, or dismissed with file:line evidence. Each dismissal gets a narrow `[[ignore]]` entry for
its wording, so that a repeat scan does not reopen it.

## Context

- Production secrets (`cd_prod`, SPEC-134) are exclusive: the store refuses to allow production
  for a secret with bound hosts (`yulius-store/src/vault.rs:502`), and refuses hosts on a
  production secret (`:225`, `:431`, `:458`). A production secret may still carry hosts kept from
  before that exclusivity. SPEC-308 R2 shows those hosts in the Edit dialog with "Remove these
  hosts", and the egress proxy still uses the secret on them for agent runs
  (`EgressClient::may_use`).
- `agent_secret_value` never resolves a production secret (`vault/runtime.rs:264`).
  `tool_browser_fill_secret` (`vault/runtime.rs:775`) looks the secret up by name only and checks
  its hosts (`:817`, `:824`), never `cd_prod`.
- The run's fingerprint file is the Bash wrapper's fallback while Yulius cannot answer
  (`yulius-harness/src/scrub_client.rs:105-137`). A contained session gets it as a read-only bind
  (`vault/runtime.rs:106-111`), which pins the file's inode. That is why
  `rewrite_run_fingerprints` (`vault/runtime.rs:501-531`) rewrites it in place.
- #757 (closed) reported the same egress client cache as #711. #1022 (merged as c1043d15)
  checks the client's run on every cached hit (`cached_egress_client`, `vault/runtime.rs:918-937`;
  `egress_client_live`, `:942-962`) and on every request (`egress_apply`, `:1071`; ADR-0252).
- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring; there is
  one narrow entry per wording (SPEC-262, ADR-0202).

## Requirements

- VEV-001 (#710, real). `browser_fill_secret` refuses a production secret (`cd_prod`) with
  `Forbidden` and types nothing, whatever hosts it carries. A filled page is in the agent's hands
  (it reads it back and screenshots it), unlike a request the proxy completes outside the run.
  The proxy keeps using a production secret's old bound hosts (SPEC-308 R2).
- VEV-002 (#835, false positive). A reader of a partly rewritten fingerprint file fails closed: an
  empty file, or any strict prefix of a serialized fingerprint set, is `FileState::Invalid`, so
  `--scrub-exec` does not run the command. This is pinned by a test and gets a narrow entry.
- VEV-003 (#711, duplicate). #711 is closed as a duplicate of #757, whose fix (#1022) is on `dev`.
- VEV-004 (#758, false positive). `among_askers` splits names that can never contain `", "`, and
  it authorizes nothing. This gets a narrow entry.
- VEV-005 (#815, false positive). `open_credential` is a crate-private helper whose every caller
  gets the account from the server's own pool choice, after the caller's own authorization. This
  gets a narrow entry.

## Plan

- [x] Spec SPEC-405 with the triage of all five findings
- [ ] browser_fill_secret refuses a production secret (#710)
- [ ] Fingerprint-file guard pinned: a partly written file fails closed (#835)
- [ ] Narrow [[ignore]] entries for the dismissed wordings (#835 #758 #815)
- [ ] Architecture, API docs, changes entry and gates
- [ ] Independent review answered
- [ ] Dismissed issues commented and closed (#711 #835 #758 #815)

## Acceptance criteria

- `browser::a_production_secret_is_never_typed_into_a_page` fails on dev (the value is typed into
  a page of the production secret's old bound host) and passes with the fix: the call is an
  error that names the production secret, and `Input.insertText` is not called.
- `scrub_client::tests::a_partly_written_fingerprint_file_fails_closed` shows that an empty file
  and every strict prefix of a serialized set are `Invalid`, and that `run_scrub_exec` does not
  run the command for one (exit 126).
- `.config/vidoc.toml` has one narrow entry per dismissed wording (#835, #758, #815), each with
  its reason and function.
- Each issue ends closed: #710 by the merged pull request, #711 as a duplicate, and the others as
  not planned, each with a comment giving the evidence.

## Test plan

- Integration test in `crates/yulius/tests/browser.rs`. It uses the fake browser, and a production
  secret with an old bound host written to the store as older versions left it.
- Unit test in `crates/yulius-harness/src/scrub_client.rs`.
- `scripts/dev ci` (the full gate) and `yulius security-scan` (disabled for this project).

## Decisions

- #710 is fixed rather than dismissed. A production secret with a bound host exists only from
  before exclusivity, but it exists (SPEC-308 R2). The proxy exception is kept because the proxy
  adds the value outside the agent's reach. A filled field sits in a page the agent drives: it
  reads it back (redacted) and screenshots it (not redacted). Refusing it matches
  `agent_secret_value` and ADR-0171 ("production secrets never resolve to agents"). This narrows
  an existing rule, so it needs no ADR.
- #835: an atomic rename would break the fallback. A contained session binds the file read-only by
  inode (bubblewrap `--ro-bind`), so a renamed replacement would never reach it. It would keep
  reading the previous holder's fingerprints, the leak SPEC-248 FUP-002 closed. A partly written
  file never parses: the set is one JSON object whose closing brace is its last byte, and
  `serde_json` refuses an unclosed object and trailing data. The file is read only while Yulius
  cannot answer, and the rewrite runs inside Yulius.
- #711: the same cache and wording as #757. Its fix checks the run on every cached hit, so nothing
  is left to change.
- #758: asker names are `#N`, `<login>'s Superagent` (GitHub logins are letters, digits and
  hyphens), or either with ` in <slug>` (slugs are `[a-z0-9-]`, `projects.rs:100-118`). Their only
  readers are `requester` (the shown list of askers) and `secret_waits_for` (whether a task's
  waiting flag stays). Who may allow a request is the person who manages the secret, never an
  asker. A structured list would change a persisted column for no boundary.
- #815: the four callers are the task start (`supervisor.rs:1246-1258`, after `choose_account`),
  its switch preflight (`:3475-3476`), the Superagent (`cockpit.rs:331-362`, after
  `require(.., Action::Work)` at `:302`) and split proposals (`intake.rs:359-362`). Each gets the
  account from the server's pool choice (`choose_account`, `pool_account`), never from a caller's
  input that is not checked. The credential is the instance's pooled subscription, used to start
  the agent process, and is never returned to a caller.

## Evidence

To be completed before delivery.
