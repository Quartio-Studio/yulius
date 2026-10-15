---
yulius-task: 166d22db-c7e8-40d6-8464-e8eca6340316
status: done
---

# SPEC-095: a repository's .mcp.json servers join Yulius's MCP section, and the Superagent never asks about them

Status: implemented
Date: 2026-10-14
Owner: Yulius task #80
Scope authorization: task request: "while onboarding other project, I got something like this in
Superagent [Claude Code's '3 new MCP servers found in this project' dialog: supabase, apify,
pipedream-gmail]. It should require no user's interaction and mcps should be all connected in
yulius section 'MCP'". Fixes #391.

## Goal

A project whose repository has a `.mcp.json` works in Yulius without anyone answering a dialog.
Its servers appear in the sandbox's MCP section and reach every task run as any server configured
there does. The Superagent opens straight on its prompt.

## Context

- **Symptom (reproduced).** Claude Code 2.1.292 started in a folder whose `.mcp.json` lists servers
  it has neither approved nor rejected opens on "N new MCP servers found in this project. Select
  any you wish to enable", and waits. Reproduced in a pseudo-terminal with a two-server
  `.mcp.json` and a clean environment ("2 new MCP servers found in this project", `[✔] alpha`,
  `[✔] beta`, "Enable selected"); `claude mcp list` shows both as "Pending approval".
- **Root cause.** Two gaps, both in how Yulius launches Claude Code:
  - The Superagent (`plan_cockpit`, SPEC-021, SPEC-037 MGT-004) runs `claude` in the sandbox
    repository without `--strict-mcp-config`, so the person's own MCP servers keep working.
    Claude Code therefore reads the repository's `.mcp.json` and asks. Task runs pass
    `--strict-mcp-config` (SPEC-002 ADR-05), so they never ask, but they never see the servers
    either.
  - Nothing in Yulius reads `.mcp.json`, so the MCP section (instance and sandbox servers,
    SPEC-001 MCP-001..004, `crates/yulius-app/src/mcp_config.rs`) never lists the project's
    servers and no run gets them.
  - Found while fixing: a stdio server's value travelled in the run's environment under the
    server's own variable name (`APIFY_TOKEN`). An agent secret of that name already puts its
    placeholder there (SPEC-002 ADR-20), and the session host refuses the duplicate ("name also
    present in the non-secret env"), so every run of the sandbox failed to spawn. With the import,
    `${APIFY_TOKEN}` names the agent secret `APIFY_TOKEN`, so the clash becomes the normal case.
- **Why no test caught it.** The Superagent tests run it in an empty repository, and no test
  had a `.mcp.json` in a sandbox.
- **What settles the dialog (measured on Claude Code 2.1.292).** Claude Code keeps the answers as
  settings (`enabledMcpjsonServers`, `disabledMcpjsonServers`, `enableAllProjectMcpServers`): an
  answer given in the dialog lands in the project's `.claude/settings.local.json`. Passed with
  `--settings '<json>'`, in a pseudo-terminal with a clean environment:

  | `--settings` | Dialog |
  | --- | --- |
  | none | "2 new MCP servers found" |
  | `{"enableAllProjectMcpServers": false}` | "2 new MCP servers found" |
  | `{"disabledMcpjsonServers": ["alpha", "beta"]}` | none, and no file written to the repository |
  | `{"disabledMcpjsonServers": ["alpha"]}` | asks about `beta` |

- `.mcp.json` format (Claude Code's project scope): `{"mcpServers": {"<name>": {...}}}`. A server
  has `type` `stdio` (`command`, `args`, `env`), `http` (`url`, `headers`) or `sse`. A missing
  `type` means stdio when `command` is set, and http when `url` is. Values may name environment
  variables as `${VAR}` or `${VAR:-default}`. Yulius references agent secrets as
  `${secret:NAME}` (MCP-003), resolved when a run starts. A header may be `Bearer ${secret:NAME}`.
- Gaps found and not fixed here: the Superagent gets none of the servers configured in Yulius
  (MGT-004 attaches only `yulius`), and they would reach it without the redaction hook that task
  runs have. Filed as #395, fixed by SPEC-147.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| PMC-001 | Before a task's run starts and before a Superagent starts, Yulius reads the sandbox repository's `.mcp.json`. It reads only a regular file of the repository (no link out of it, no FIFO or directory, opened without blocking, as the other agent-written files the server reads outside containment, #70), at most 256 KiB and at most 50 servers. Each server whose name no instance or sandbox server of that sandbox has (case-insensitive, `-` and `_` alike) is added as an enabled sandbox MCP server, audited `mcp.imported` by `yulius`. A server already there is never changed, so a server turned off in Yulius stays off. | `pool_supervision.rs`, `cockpit.rs`, unit tests |
| PMC-002 | Mapping: stdio keeps `command`, `args` and `env`; http keeps `url` and `headers` (runs still get only `Authorization: Bearer`, MCP-003; others are kept and a warning says so). Agents can write the file, so it never chooses which command receives a secret's value (VLT-004/006, ADR-20). An env or header value that is exactly `${VAR}` or `${secret:VAR}`, also after `Bearer `, becomes the agent secret's placeholder `yulius-secret-VAR`, which the egress proxy replaces on the secret's bound hosts only. `${VAR:-default}` becomes the placeholder when VAR is an agent secret of the sandbox, else the default. A VAR that is no valid agent secret name is left out. A stored value is never a `${secret:…}` reference. Any other value is stored as written (sealed). `sse` servers, and servers whose name or fields Yulius does not accept, are skipped with a warning. | unit tests in `mcp_config/project.rs`, `pool_supervision.rs` |
| PMC-003 | A missing, unreadable, invalid or oversized `.mcp.json`, or a server the store refuses (limit, a concurrent import of the same name), never stops a run or a Superagent; a warning is logged and the rest goes on. | unit tests, `cockpit.rs` |
| PMC-004 | The Superagent starts with `--settings '{"disabledMcpjsonServers":[…]}'` naming every server in the repository's `.mcp.json` (also skipped ones, up to 64 KiB of names, since one argv element is bounded at 128 KiB), so Claude Code never asks about them; the names are JSON in one argv element, never through a shell. Without such servers it starts as before, with no `--settings`. This amends MGT-004 ("no settings") for this one key. | harness unit test, `cockpit.rs` |
| PMC-005 | Task runs are unchanged: `--strict-mcp-config` with the Yulius servers, which now include the imported ones with agent-secret placeholders in place of variables (PMC-002; MCP-003, MCP-004). | `pool_supervision.rs` |
| PMC-006 | An MCP server's secret values reach a run in variables of their own: `YULIUS_MCP_<SERVER>_<VAR>` for a stdio server's variable, and `YULIUS_MCP_<SERVER>_TOKEN` for an HTTP bearer as before. A name is cut to 120 characters (the harness takes at most 128), and a name that two values would share gets a numbered suffix. The run's `mcp.json` maps each variable back to the server's own name, so an agent secret's placeholder of that name stays in the agent's environment and the run starts. | wiring unit test, `pool_supervision.rs` |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] A repository's .mcp.json servers join the sandbox's MCP section before every run
- [x] The Superagent starts without asking about the project's MCP servers
- [x] ADR, architecture, product and changes updated; every gate green
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- A sandbox repository with a `.mcp.json` listing a stdio server (`env` `${APIFY_TOKEN}`) and an
  http server (`Bearer ${secret:APIFY_TOKEN}`): a task's first run has both in its `--mcp-config`
  file, both with the placeholder of the agent secret `APIFY_TOKEN` and never its value, and both
  are listed as sandbox servers and audited `mcp.imported`.
- A `.mcp.json` that is a link out of the repository, a FIFO or a directory imports nothing and
  never blocks.
- The Superagent in that sandbox starts with `--settings` naming both, and without `--settings`
  in a repository with no `.mcp.json`.
- A server turned off in Yulius stays off after the next run, and an invalid `.mcp.json` starts
  the run and the Superagent as before.

## Test plan

- Unit (`crates/yulius-app/src/mcp_config/project.rs`): stdio, http and typeless servers,
  `${VAR}`, `${VAR:-x}`, `${secret:VAR}` and `Bearer ${VAR}` mapped to placeholders or defaults,
  no stored value a secret reference, literal values kept, `sse` and invalid names skipped but
  named, invalid JSON, a non-object `mcpServers`, more than 50 servers, names bounded for argv, a
  link out of the repository, a FIFO and a directory.
- Wiring unit (`crates/yulius/src/wiring/harness.rs`): every MCP value gets a variable of its own,
  within 128 characters for a 64-character server and a 128-character variable.
- Harness unit (`crates/yulius-harness/tests/launch.rs`): `plan_cockpit` with project server names
  adds one `--settings` JSON element with `disabledMcpjsonServers`; without names, none.
- Integration (`crates/yulius/tests/pool_supervision.rs`): the run of a task in a repository with
  a `.mcp.json` gets the imported servers with the placeholder, never the value, even for a
  `${secret:…}` the file names; the imports are audited; a server turned off before the run stays
  off.
- Integration (`crates/yulius/tests/cockpit.rs`): the Superagent's argv carries the settings and
  the sandbox lists the imported servers; an invalid `.mcp.json` still starts it.
- Each test is written first and fails on `dev` (no import, no `--settings`). The review's
  findings (clear values for file-defined servers, the file read through links and FIFOs, long
  variable names) each got a regression test with its fix; the run test's `${secret:…}` header
  would carry the clear value under the first version's `${secret:VAR}` mapping.
- Gates: `scripts/dev ci`.

## Decisions

- [ADR-0078](../../docs/decisions/0078-repository-mcp-json-feeds-the-mcp-section.md): the
  repository's `.mcp.json` feeds Yulius's MCP section (added when missing, never overwritten), and
  the Superagent rejects the file's servers through `--settings`. Alternatives: approving them in
  the Superagent, `--strict-mcp-config` for the Superagent, importing only once at onboarding.

## Evidence

- Reproduced on Claude Code 2.1.292 in a pseudo-terminal with a clean environment: a two-server
  `.mcp.json` shows "2 new MCP servers found in this project"; `--settings` with
  `disabledMcpjsonServers` naming both starts without it, `enableAllProjectMcpServers: false` does
  not (table in Context).
- Regression tests first (commit 3167cb3a): `pool_supervision::a_repository_mcp_json_reaches_the_run_through_yulius`
  failed on dev (the run had only `yulius`), `cockpit::a_cockpit_takes_the_repository_mcp_servers_into_yulius_without_asking`
  failed (no `--settings`), and the harness test did not compile (no field). After the fix the run
  test first failed to spawn ("name also present in the non-secret env"), which exposed PMC-006.
- Live check (Claude Code 2.1.292, `claude mcp list` with the server approved): a stdio MCP server
  inherits `HTTPS_PROXY`, `NODE_USE_ENV_PROXY` and `NODE_EXTRA_CA_CERTS` and gets
  `APIFY_TOKEN=yulius-secret-APIFY_TOKEN` expanded from `${YULIUS_MCP_PROBE_APIFY_TOKEN}`.
- Independent review in a fresh session, three rounds: round 1 asked for changes (clear secret
  values for file-defined servers, the file read through links and FIFOs, variable names over 128
  characters, minors); fixed with regression tests; approved at 3f9d29ef.
- `scripts/dev ci` at 3f9d29ef: exit 0; nextest 1,532 passed, 16 skipped; doctests, lint, deny,
  shear and `cargo doc -D warnings` green.
- Not tested here: a real Superagent session on the instance and a real MCP server reaching its
  host through the egress proxy (no credentials or proxy in this environment).
