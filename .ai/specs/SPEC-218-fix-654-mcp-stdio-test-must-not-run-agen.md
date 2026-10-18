---
yulius-task: 7ee59f6b-6d3a-4226-a2bc-b25e73e965b8
status: done
---

# SPEC-218: The MCP Test never runs a command an agent wrote

Status: done
Date: 2026-10-18
Owner: Yulius task #204
Scope authorization: task #204, "Fix GitHub issue #654 … the minimal fix (an `origin` column
person|mcp_json, refusing the stdio test of imported servers until a person changes their command,
showing the origin in the UI …)". Found by task #160's Vidoc audit (SPEC-217).

## Goal

A command an agent wrote never runs outside containment through the MCP Test. Servers Yulius
imported from the repository's `.mcp.json` (SPEC-095) are marked as such, the Test refuses to run
them on the host until a person has changed what they run, and the MCP pages say where a server
came from.

## Context

- **Symptom (#654).** An agent writes `.mcp.json` into the sandbox repository with a stdio server
  whose command is its own (`/bin/sh -c …`). Before the next run, `import_project_mcp`
  (`mcp_config/project.rs`) stores it as an enabled sandbox server, indistinguishable from one a
  person added. On a contained instance an instance administrator presses Test, or only opens
  `…/mcp/<id>?test=1` (the edit page tests on load), and `probe_stdio` (`mcp_config/probe.rs`)
  spawns the command on the host as the service user: `master.key`, the database, every sandbox.
- **Root cause.** `test_mcp_server` (`mcp_config.rs`) gates the stdio Test only on who presses it
  (an instance administrator when contained). That gate assumes a person chose the command, but
  `McpServerRecord` carries no provenance, so an imported server passes it. Existing tests covered
  the import (cockpit.rs `a_cockpit_takes_the_repository_mcp_servers_into_yulius_without_asking`)
  and the probe separately, never the Test of an imported server.
- **Lesser variant.** An imported HTTP server makes the Test POST fixed JSON-RPC to a loopback or
  private URL the agent chose (blind apart from tool names); the private-address Test is also
  gated only on the presser.
- **Env.** The edit page keeps stored environment values when they are not replaced (they are
  write-only and never shown). An imported server's env values come from the file too, so an agent
  could set `LD_PRELOAD` or `NODE_OPTIONS` there: a person changing only the command would still
  run agent-chosen code.
- What else the cause could break: any other host-side use of an imported server's fields. Agent
  runs attach servers inside the run's own containment (or on the host when uncontained, where the
  agent already runs); the Test is the only host-side spawn.

## Requirements

- **MOR-001** Every MCP server has an `origin`: `person` (created or taken over by a person) or
  `mcp_json` (imported from the repository's `.mcp.json`). The import stores `mcp_json`; the API
  create stores `person`. A migration adds the column, `person` by default, and marks every server
  whose id an `mcp.imported` audit entry names as `mcp_json` (conservative: a person re-confirms).
- **MOR-002** The Test of a stdio server whose origin is `mcp_json` is refused, whoever asks and
  whether or not the instance is contained, without starting a process; the result says why and
  how to lift it. The Test of an `mcp_json` HTTP server whose URL resolves to a private or loopback
  address is refused the same way; public addresses are tested as before.
- **MOR-003** An update makes an `mcp_json` server `person` only when it changes what the server
  runs: for stdio, `command` or `args` differ from the stored ones **and** the update replaces
  `env` (or none is stored); for HTTP, `url` differs. An update that re-sends the same fields (the
  enable toggle, or the edit page re-sending the arguments exactly, empty ones included) keeps
  `mcp_json`. A take-over that does not replace `env` drops the stored one (review round 1). A
  `person` server never goes back to `mcp_json`.
- **MOR-004** `McpServerView` carries `origin`. The MCP list shows a "from .mcp.json" badge and
  disables Test of a refused server, saying why; the edit page says the server came from
  `.mcp.json`, what lifts the refusal, and never runs the Test on load (`?test=1`) for a refused
  server.

## Plan

- [x] Spec and root cause of the host-run MCP Test written
- [x] Regression test: an imported stdio server's Test is refused in a contained harness (fails first)
- [x] Servers carry their origin (person or .mcp.json) and the Test refuses imported ones
- [x] A person's command change makes the server theirs; an enable toggle does not
- [x] The MCP pages show a server came from .mcp.json and never auto-test it
- [x] Docs, ADR, gates and independent review done

## Acceptance criteria

- [x] Regression test (contained harness, next to cockpit.rs
  `a_cockpit_takes_the_repository_mcp_servers_into_yulius_without_asking`): an imported
  `.mcp.json` stdio server whose command touches a marker file, tested by an instance
  administrator, is refused and the marker does not exist.
- [x] After a person changes its command (and replaces its env), the Test runs; a PUT that only
  toggles `enabled` keeps it refused; a command change that keeps the file's env keeps it refused.
- [x] `?test=1` never runs an imported server's command (server-side refusal; the page does not
  call the Test).
- [x] The MCP page shows that a server came from `.mcp.json`.
- [x] An imported HTTP server on a loopback URL is refused; the migration marks imported rows.

## Test plan

- Integration (`crates/yulius/tests/cockpit.rs`, contained `RecordingHarness`): `.mcp.json` with a
  stdio server `sh -c 'touch <marker>'`; opening the Cockpit imports it; the view says
  `origin: mcp_json`; an instance administrator's `POST /api/mcp/{id}/test` answers `ok: false`
  with the reason and the marker is absent; the enable toggle PUT keeps it refused; a PUT changing
  the args but keeping the env keeps it refused; a PUT changing the args and replacing the env
  makes it `person` and the Test runs it (the marker appears). A loopback HTTP server from the file
  is refused.
- Store: the migration marks rows named by `mcp.imported` (`crates/yulius-store/src/lib.rs`
  `version_thirty_six_imported_mcp_servers_come_from_mcp_json`); the origin's insert, update and
  read are proved through the API by the integration test, whose Test reads the server back.
- Unit (`mcp_config.rs` `only_a_change_of_what_runs_takes_a_server_over`): the take-over rule.
- e2e (`e2e/tests/mcp.e2e.ts`): the list shows the badge; the edit page with `?test=1` shows the
  notice and sends no Test request; a rename of a server with an empty argument keeps it the
  file's. The integration test also switches an imported stdio server to HTTP: its env is gone.

## Decisions

- Refuse instead of running the probe inside the sandbox containment: the minimal fix the issue
  names; a contained probe needs the bwrap launch of a terminal for a one-shot JSON-RPC exchange
  and is a larger change. Recorded in ADR-0173.
- Provenance is a column, not a lookup of the audit log at Test time: the audit log is append-only
  history, not state, and a person's change must lift the refusal.
- Refuse everywhere, not only on contained instances: one rule; uncontained instances lose nothing
  an agent run does not already do.
- The env must be replaced with the command: the file's env values are agent-chosen and hidden.

## Evidence

- Reproduced first: `cockpit::the_mcp_test_never_runs_a_command_from_the_repository_mcp_json`
  failed on dev's code at commit 98f415a0 with "the agent's command ran on the host" (the marker
  file existed after an instance administrator's Test on a contained harness). It passes with the
  fix, as do the review-round cases added at 5d5a49b9
  (they failed first: `env_keys` was `["LD_PRELOAD"]`; the rename took the server over).
- `cargo nextest run -p yulius -p yulius-app -E '(binary_id(yulius::main) & test(/^cockpit::/)) |
  test(/mcp_config/)'`: 62 passed. Migration test
  `version_thirty_six_imported_mcp_servers_come_from_mcp_json` and unit test
  `only_a_change_of_what_runs_takes_a_server_over` pass.
- `scripts/dev ci` at b51a2c02 (all code; later commits change only `.config/vidoc.toml` and this
  spec): exit 0, nextest 2217 passed, 18 skipped; lint, macOS, deps, docs green.
- e2e: `tests/mcp.e2e.ts` 3/3 (the new test failed first: no "From .mcp.json" badge; the rename
  case failed first too). Full suite: 616 passed, 1 failed, 1 skipped; the failure is the known
  load flake #581 (onboarding "leaving and coming back while Start onboarding runs"), outside this
  change (onboarding, not MCP), which passed 3/3 when its file ran alone; commented on #581.
- `scripts/dev security` (Vidoc vidoc-1, run as `python3 scripts/vidoc_scan.py`): ran (not
  skipped). It reported the pre-existing uncontained stdio-Test gate (Configure equals Work when
  uncontained) in changing words and severities, and "Only stdio imports from .mcp.json are gated"
  on `mcp.js`. Both are false positives with `[[ignore]]` entries in `.config/vidoc.toml` that the
  independent reviewer agreed to (one narrow entry per wording; a topic-wide title was refused in
  review because it would hide a #654 regression). The two #654 entries now name the fix. Outside
  the change (not failing; filed by CI): the public HTTP Test's reach (pre-existing), keys of
  `.mcp.json` env/headers unvalidated (pre-existing), imported stdio servers enabled without
  confirmation (the SPEC-095 design), and the migration list order comment (pre-existing).
  No low findings in the change.
- Independent review (fresh session, om-code-review): round 1 approved with two own-code minors
  (take-over through another transport kept the file's env; the edit page dropped empty
  arguments), both fixed test-first; round 3 requested narrower Vidoc ignores, done; approved.
- Not tested: a real contained (bwrap) instance and the production UI against a real server; the
  regression test uses the contained configuration with a recording harness, and the UI runs on the
  mock, whose MCP rules mirror the server's.

