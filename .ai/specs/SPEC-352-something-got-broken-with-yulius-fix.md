---
yulius-task: 1874e329-dbd8-4a49-9ec7-6e053c8b7cde
status: done
---

# SPEC-352: The Superagent's Bash keeps working while Yulius restarts

Status: implemented
Date: 2026-10-30
Owner: Yulius task #334
Scope authorization: task #334, "something got broken with yulius, fix", with a screenshot of the
Superagent at 21:34 on 2026-10-30.

## Goal

While the Yulius server restarts, the Superagent's Bash commands run with agent secrets redacted,
as a task run's do, instead of being refused until the server is back.

## Context

- Symptom (the screenshot): during a server restart the Superagent answered that "every shell
  command is refused with 'Yulius is restarting… try again'" and concluded that Yulius was down.
  The person could not use it to look at anything until the server came back. Task sessions kept
  working.
- Every Bash command, hook and stdio MCP server of a task run and of the Superagent runs through
  `yulius --scrub-exec` (SPEC-147 SAM-002, ADR-0121). The wrapper loads the redaction fingerprints
  from `POST /internal/scrub/fingerprints`. If that fails, it falls back to the file named by
  `YULIUS_SCRUB_FILE`. With neither, the command does not run (`scrub_client::load_fingerprints`,
  exit 126).
- A task run gets `<run>/scrub.json` and `YULIUS_SCRUB_FILE` (`App::vault_run_setup`). The
  Superagent gets the wrapper (`cockpit.rs`) and the proxy (`App::vault_superagent_setup`) but no
  file. SPEC-147 recorded that on purpose: "while Yulius is unreachable its Bash commands do not
  run …, for at most a restart". A restart (an update builds and installs, SPEC-160) is long
  enough to block a person who is using the Superagent to find out what is wrong.
- Why the tests missed it: they asserted that recorded behavior (`cockpit.rs` test
  `the_superagent_gets_the_sandbox_mcp_servers_with_their_secrets_redacted`, "without its
  fingerprints a command does not run").
- Also seen while reproducing, outside this change: from 21:36 to about 21:39 the egress proxy
  answered every `CONNECT` with `503 Yulius proxy: too many connections; try again` (all 256 of the
  instance's slots were held). It recovered by itself. Reported as an issue (see Decisions).

## Requirements

| ID | Requirement |
| --- | --- |
| SRF-001 | A Superagent's shell gets a fingerprint file in its run directory (`<data>/shells/<id>/scrub.json`, mode 0600, read-only in a contained session) with the fingerprints of the agent secrets that apply to its person in its sandbox, and `YULIUS_SCRUB_FILE` naming it. While Yulius is unreachable, its Bash commands run through the wrapper with those fingerprints, as a task run's do. |
| SRF-002 | Without both the server and a readable file, a Superagent's command still does not run (fail closed, unchanged). |
| SRF-003 | The wrapper reads the file only when Yulius cannot be reached or is not ready: no connection, a timeout or a 5xx answer. A Yulius that answers and refuses the token with a 4xx (an ended run, a person who may no longer work in the sandbox) is not a restart. The command does not run, for the Superagent and task runs alike, so SAM-003's access check keeps stopping the Superagent of a person who lost access. |
| SRF-004 | Superagents already running when this ships start again to get the file (`COCKPIT_LAUNCH_REVISION` 3, SPEC-186 SAV-006). |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec, ADR and a failing regression test for the Superagent's restart fallback
- [x] The Superagent gets its fingerprint file, regression test green
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Gates green, pull request armed to merge into dev on green CI

## Acceptance criteria

- With Yulius unreachable (`YULIUS_URL` pointing nowhere), a Bash command in the Superagent runs
  and prints an agent secret's value as `[secret:NAME]`.
- With Yulius unreachable and no readable file, the command does not run (status 126).
- With Yulius reachable and refusing the token, the command does not run although the file is
  there (status 126).
- The file is written in the shell's run directory, never holds a value, and is bound read-only
  in a contained session.

## Test plan

- `crates/yulius/tests/cockpit.rs`
  `the_superagent_gets_the_sandbox_mcp_servers_with_their_secrets_redacted`: adds SRF-001 (a
  command with `YULIUS_URL=http://127.0.0.1:1` runs and prints the wrapper's label). It keeps
  SRF-002 with `YULIUS_SCRUB_FILE=/nonexistent` (status 126), and SRF-003 with a refused workspace
  token `ws_0000` against the reachable test server (status 126). It fails on `dev` without the
  fix: the command does not run. The SRF-003 part failed before the wrapper change: the command
  ran (`ran-62`, status 0).
- `crates/yulius-app/src/vault/runtime.rs`
  `a_superagent_gets_its_fingerprint_file_read_only`: the file's path, mode and content, the
  variable replacing an inherited one, and the read-only bind in a contained session.
- The existing SAV-001 test still holds (proxy, CA bundle, placeholder).

## Decisions

- ADR-0239: the Superagent gets a fingerprint file as a task run does. This supersedes the
  "no fingerprint file" decision of SPEC-147 and ADR-0121. The file holds only salted hashes of
  values the person's own task runs get in the same file. It is read only while Yulius cannot be
  reached or is not ready (SRF-003). Whenever Yulius answers, SAM-003's access check decides. Alternatives: keep failing closed (the defect); run the
  command unredacted while Yulius is unreachable (leaks values); a longer fetch timeout (does not
  cover a restart).
- The file is written once, at the shell's start, like a task run's. A secret added later is not
  in it until the Superagent restarts, which matters only while Yulius is unreachable. This is
  the same staleness a task run has.
- The proxy's slot exhaustion is a separate defect with an unknown holder. It is filed as an issue
  rather than widened into this fix.

## Review

Independent review in a fresh session (`om-code-review`, diff `5936553c..HEAD`).

- Round 1: REQUEST CHANGES.
  - Major (own code): the file stood in for a 4xx refusal too, so a person who lost access kept a
    working Superagent. Fixed test-first: SRF-003, the refused-token check in the cockpit test was
    red (`ran-62`, status 0) and is green.
  - Major (own code): `COCKPIT_LAUNCH_REVISION` was not bumped, so running Superagents would never
    get the file. Fixed: SRF-004, revision 3.
  - Minor (own code): the read-only bind was untested. Fixed: the `superagent_scrub_file` unit test
    (red: the helper did not exist).
  - Minor (pre-existing): the file is written once. Recorded in ADR-0239.
  - Nits: the docs wording was fixed.
- Round 2: APPROVE.
  - Minor (pre-existing): an expired GitHub token past the staleness bound answers 401, so the
    Superagent fails closed then, as on `dev`. Recorded in ADR-0239.
  - Nit: the ADR-0121 note named the wrong consequence. Fixed.

## Evidence

- Root cause, in a task sandbox on 2026-10-30:
  - `cockpit.rs` gives the Superagent `--scrub-exec` but no `YULIUS_SCRUB_FILE`.
  - The red run of the regression test printed the screenshot's message: "the agent-secret
    fingerprints of this run cannot be loaded, so the command was not run (Yulius is restarting,
    …); try again".
- Red, then green:
  - `cargo test -p yulius --test main the_superagent_gets_the_sandbox_mcp_servers`: FAILED (timed
    out waiting for `file=<[secret:APIFY_KEY]> ran-42`) at `dd7521f1`, ok at `e74a0d81`.
  - The SRF-003 check: FAILED (`ran-62`, `refused=0`) before the wrapper change, ok at `ed8bb621`.
  - `cargo test -p yulius-app --lib a_superagent_gets_its_fingerprint_file`: did not compile without
    the helper, ok after it.
- `scripts/dev ci` at `ed8bb621`: exit 0.
  - 2794 tests run: 2794 passed, 16 skipped.
  - Repository contracts passed.
- `yulius security-scan`: not run (exit 2). Vidoc refused the instance key (HTTP 401), so 4 files
  were not scanned. This is unrun, not passed.
- Not tested:
  - A real server restart with a live contained Superagent. The integration test stands in for it
    with an unreachable `YULIUS_URL`. The read-only bind is covered by the unit test, not by a
    bwrap run.
  - The root cause of the restart itself: the running commit is unchanged since 2026-10-28, and the
    server log cannot be reached from a sandbox.
- Seen while reproducing, outside this change: the egress proxy answered every `CONNECT` with
  `503 too many connections` from 21:36 to about 21:39, then recovered. Filed as #964.
