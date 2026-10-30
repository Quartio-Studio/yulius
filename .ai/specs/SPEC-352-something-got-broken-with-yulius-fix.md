---
yulius-task: 1874e329-dbd8-4a49-9ec7-6e053c8b7cde
status: in-progress
---

# SPEC-352: The Superagent's Bash keeps working while Yulius restarts

Status: in progress
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

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Spec, ADR and a failing regression test for the Superagent's restart fallback
- [ ] The Superagent gets its fingerprint file, regression test green
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Gates green, pull request merged into dev

## Acceptance criteria

- With Yulius unreachable (`YULIUS_URL` pointing nowhere), a Bash command in the Superagent runs
  and prints an agent secret's value as `[secret:NAME]`.
- With Yulius unreachable and no readable file, the command does not run (status 126).
- The file is written in the shell's run directory, never holds a value, and is bound read-only
  in a contained session.

## Test plan

- `crates/yulius/tests/cockpit.rs`
  `the_superagent_gets_the_sandbox_mcp_servers_with_their_secrets_redacted`: adds SRF-001 (a
  command with `YULIUS_URL=http://127.0.0.1:1` runs and prints the wrapper's label). It keeps
  SRF-002 with `YULIUS_SCRUB_FILE=/nonexistent` (status 126). It fails on `dev` without the fix:
  the command does not run.
- The existing SAV-001 test still holds (proxy, CA bundle, placeholder).

## Decisions

- ADR-0239: the Superagent gets a fingerprint file as a task run does. This supersedes the
  "no fingerprint file" decision of SPEC-147 and ADR-0121. The file holds only salted hashes of
  values the person's own task runs get in the same file. It is only read while Yulius cannot be
  reached. Whenever Yulius runs, SAM-003's access check still decides, and the file grants nothing
  beyond what the person's shell already has: it already runs, with its environment. Alternatives: keep failing closed (the defect); run the
  command unredacted while Yulius is unreachable (leaks values); a longer fetch timeout (does not
  cover a restart).
- The file is written once, at the shell's start, like a task run's. A secret added later is not
  in it until the Superagent restarts, which matters only while Yulius is unreachable. This is
  the same staleness a task run has.
- The proxy's slot exhaustion is a separate defect with an unknown holder. It is filed as an issue
  rather than widened into this fix.
