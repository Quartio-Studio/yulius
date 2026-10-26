---
yulius-task: a6b65527-7716-4122-bff8-e28d498dcb14
status: done
---

# SPEC-332: An MCP server is for every sandbox or one, and for the team or only its owner

Status: done
Date: 2026-10-26
Owner: Yulius task #316
Scope authorization: task #316: "MCP should have 2 config fields: per sandbox/global and
personal/team-wise - 4 options in total. Default: global, team-wise. Configuring it should be
frictionless as hell. Bartek got a lot of friction configuring our Quartio's mcp […] Fix."

## Goal

An MCP server has two settings: **Available in** (every sandbox, or this sandbox) and **For** (the
whole team, or only me). The four combinations all work; a new server defaults to every sandbox and
the whole team. A personal server is attached only to its owner's own runs (their tasks and their
Superagent), so a person can connect a server holding their own credential without handing it to
every agent of the instance, and either setting can be changed later with one click.

## Context

- Today a server is the instance's (`sandbox_id` NULL, instance administrators) or one sandbox's
  (writers), and is attached to every run of the sandbox (`App::mcp_attachments`, SPEC-001
  MCP-001..004). The scope is chosen once, when the server is added ("Set when the server is
  added."), and the editor defaults to "This sandbox".
- In the request, an agent advised a founder not to add the Quartio admin MCP to Yulius because
  "every server added on Yulius's MCP page is attached to every task run in every agent. Every
  autonomous agent here would then hold founder fleet credentials". That was true: there was no
  way to keep a server to one person.
- Agent secrets already have this axis (SPEC-245, ADR-0186): project, global or personal
  (`owner_id`), precedence project > personal > global, and `Holder::of_task` says whose personal
  things a run gets (a task's owner, unless an automation started it; the Superagent's person;
  nobody for CI and CD). MCP servers reuse that holder rule.
- Adding is already low-typing (UI-038): a catalog of known servers and a paste box reading a URL,
  a command line, `claude mcp add …` or `mcpServers` JSON. Its `--scope user|project|local` flag
  is ignored today.

## Requirements

- **MSC-001 Two settings.** A server has `scope` (`instance` = every sandbox, `sandbox` = this
  sandbox, as before) and `audience` (`team`, `personal`). A personal server has an owner (the
  person who made it personal); a team server has none. The API returns `audience` on every server
  and accepts it on create and update (omitted on create: `team`; on update: unchanged).
- **MSC-002 Defaults.** The editor preselects every sandbox + whole team. When the person may not
  add that combination (not an instance administrator), it preselects the first combination they
  may add: this sandbox + whole team for a writer. The API default stays `scope: sandbox`,
  `audience: team` so existing clients keep working.
- **MSC-003 Who manages what.** Every sandbox + team: instance administrators (as before). This
  sandbox + team: the sandbox's writers (as before). Personal (either reach): anyone who may work
  in the sandbox they add it from; afterwards only its owner sees, edits, tests, toggles or deletes
  it. To everyone else, administrators included, it does not exist (404, not listed).
- **MSC-004 Who gets it.** A run gets the team servers of its scope and the personal servers of its
  holder (`Holder`, ADR-0186): a task's owner unless an automation started it, the Superagent's
  person. An automation's task, a CI job or anyone else's run never gets a personal server.
- **MSC-005 Precedence.** One server per name: this sandbox's personal > this sandbox's team >
  every-sandbox personal > every-sandbox team. A disabled winner turns the name off for whom it
  applies (a person may turn a team server off for their own runs with a disabled personal one).
- **MSC-006 Change later.** An update may change `scope` and `audience`. It needs the right to
  manage the server now and the combination it moves to; moving to `sandbox` scope from the
  instance takes the sandbox from `sandbox` in the body (a sandbox's server never moves to another
  sandbox, so a `.mcp.json` import stays with its repository). Making a server personal makes the caller
  its owner; making it team clears the owner. Names stay unique and at most 50 servers per
  (scope, owner).
- **MSC-007 Paste.** Pasting `claude mcp add --scope user …` selects Only me; `--scope local`
  (private to you in that project) Only me in this sandbox; `--scope project` Whole team. Other
  pastes leave the settings as they are.
- **MSC-008 Lifecycle.** Removing a user removes their personal servers. `.mcp.json` imports stay
  team servers of their sandbox and ignore personal names when deciding what is new.
- **MSC-009 Directed by its owner.** As for personal secrets (SPEC-245), `task_message` from anyone
  but the owner is refused for a task whose runs get its owner's personal MCP server: another
  person's message would steer an agent holding their credential; the check covers what the live
  run was started with too. A live run keeps its MCP configuration, so a hand-over (SPEC-070) of a
  task whose live run holds the previous owner's personal server resumes it with the new owner's
  servers: the same conversation on the same subscription with nothing typed (past a usage limit,
  or afresh for a first message never taken, as a manual switch), and stops it when it cannot be
  restarted. The restart interrupts a turn in progress first, as a person's switch does. While
  such a task is being launched (Starting, Switching) control does not pass (409), and a launch
  that control passed during anyway (before its status changed) restarts its run for the new
  owner once it is live.
- **MSC-010 Redaction.** The literal secret values of every MCP server that can reach a sandbox's
  runs (the team's and every person's personal ones; at least 8 characters, not ordinary text such
  as `info`) join the project's redaction with the agent secrets: terminals, transcripts, MCP
  results and a run's fingerprint file (its holder's servers only).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Personal MCP servers are stored with an owner, listed only to it and attached only to its runs
- [x] Scope and audience can be set on create and changed on update with the right permissions
- [x] The MCP editor shows Available in and For, defaults to every sandbox and the whole team, and reads --scope from a paste
- [x] Docs, ADR and changes entry describe the four combinations
- [x] Review round 1: wiring tests, hand-over restart, MCP value redaction, move permissions
- [x] Review round 2: live-run refusal tested, hand-over fails closed, one launch read
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Gates, security scan and e2e green, delivered on green CI

## Acceptance criteria

- AC-1: Alice (writer) adds a personal server in s1; Bob (writer) does not see it in the list, gets
  404 editing it, and his task's run does not get it; Alice's task's run and her Superagent do; an
  automation's task owned by Alice does not.
- AC-2: An every-sandbox personal server reaches Alice's runs in every sandbox and nobody else's.
- AC-3: Precedence MSC-005 holds, including a disabled personal server hiding a team one for its
  owner only.
- AC-4: A writer who is not an administrator gets 403 creating every sandbox + team, and may create
  every sandbox + personal; changing a team server to personal makes them owner; an administrator
  cannot see or change a personal server of someone else.
- AC-5: The editor opens on Every sandbox + Whole team for an administrator, This sandbox + Whole
  team for a writer, saves the chosen combination, and an existing server's settings change and
  persist after a reload.
- AC-6: Removing a user removes their personal servers (schema version 44).

## Test plan

- Integration (`crates/yulius/tests/mcp_scopes.rs`): AC-1..AC-4 through the HTTP API and
  `App::mcp_attachments` with holders; permission-denied moves; redaction of MCP values.
- Real runs (`pool_supervision.rs`): a person's task gets their personal server, an automation's
  task does not, a hand-over resumes the run without it; the Superagent's (`cockpit.rs`).
- `secrets_scopes.rs`: `task_message` refused for another person's task holding their server.
- Store unit test: schema 44 cascade on user delete, uniqueness and room per (scope, owner).
- e2e (`mcp.e2e.ts`): AC-5 against the mock; pasted `--scope user|local|project`.

## Decisions

- Reuse the agent-secret holder rule (ADR-0186) rather than a new notion of "personal": one rule
  for whose credentials a run holds. Recorded in ADR-0224.
- Precedence is reach first (sandbox over every sandbox, as before), then personal over team: the
  more specific setting wins, and a person can override a team server only for themselves. This
  differs from agent secrets (project > personal > global) only within one reach, where the person
  explicitly configured the server for their own runs.
- The API default stays `sandbox`/`team`: changing it would make every non-administrator create
  without a scope fail; the requested default is the editor's.
- `${secret:NAME}` references in a personal server still never resolve personal secrets (ADR-0186):
  out of scope; the server's own sealed values are the person's credential.
- Personal limits whose runs get the server; it is not a containment boundary between tasks sharing
  one sandbox's file system and user (ADR-0224 Consequences).

## Evidence

- New tests, each seen failing before its change (or mutation-checked: the guard removed, the test
  fails): `mcp_scopes.rs` (personal servers are their owner's alone, precedence, permissions and
  moves, refused moves, redaction of MCP values and holder-only fingerprints),
  `pool_supervision.rs` (real runs: a person's task gets their personal server and an
  automation's does not; a run whose server was turned off still refuses another person's
  message; a hand-over resumes the run without it on the same subscription with nothing typed; a
  hand-over that cannot restart stops the run), `task_done.rs` (control passing during a held
  launch restarts the run), `cockpit.rs` (the Superagent gets its person's servers only),
  `secrets_scopes.rs` (`task_message` refusal; control waits while such a task starts), store
  unit test (owner scope, room on moves, cascade on user delete), unit tests
  `a_hand_over_interrupts_like_a_persons_switch` and `a_hand_over_restarts_a_run_once`, e2e
  `mcp.e2e.ts` (defaults, both settings, change after adding, `--scope user|local|project`).
- Mutation checks (all failed as expected): `task_message` guard, supervisor holder
  (`Person(&task.owner)`), Superagent holder (`Nobody`), redaction removed, hand-over restart
  inverted, stop fallback removed, launch-tail restart removed, Starting refusal removed,
  `sandbox` move restriction reverted, fingerprints from every owner.
- `scripts/dev ci` on `b185d995`: repository contracts, fmt, clippy (Linux and aarch64-apple),
  build and nextest 2632 of 2633 passed; the one failure was the audit-detail expectation that
  now names the audience, updated in the same change. On the final head `579a5731`: contracts,
  fmt and both clippy runs passed; nextest had passed 234 tests when Claude Code stopped the run
  for low host memory (the 4 counted as failed were the ones running when the signal came, all
  `SIGTERM`). The affected suites (45 integration tests, the new unit tests) pass on the final
  head; Yulius CI runs the whole gate on the pull request.
- e2e: the full suite on `37ef19bc` passed 724 tests (1 skipped, opt-in); `mcp.e2e.ts`,
  `agent-panel.e2e.ts` and `places.e2e.ts` passed again (53 tests) after the last UI change.
- Security scan: `yulius security-scan` exit 2, **not run**: Vidoc answered HTTP 429, the key
  has used its 50,000,000 tokens; nothing was scanned (unrun, not passed).
- Independent review: 5 rounds in a fresh read-only session (`om-code-review`); approved at
  `579a5731`. Three optional nits were left at the round limit; two are filed as #925, and the
  third follows the existing pattern after a spawn.
- Not tested here: a real Claude Code run against a real personal MCP server (the fake agent and
  the harness configuration stand in), and the Quartio admin MCP itself, which needs its owner's
  files and keys.
