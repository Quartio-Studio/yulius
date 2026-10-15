---
yulius-task: 5f694c3f-8f49-46a1-b0bb-7901ef770a3d
status: in-progress
---

# SPEC-147: The Superagent gets the sandbox's MCP servers, with their secrets redacted

Status: in-progress
Date: 2026-10-15
Owner: Yulius task #141 (part of task #118, SPEC-138)
Scope authorization: task #141: "Give the Superagent the same effective MCP servers as a task run
of its sandbox: its `--mcp-config` lists the enabled servers next to `yulius`, with values only as
`${YULIUS_MCP_<SERVER>_<VAR>}` references and the values in its secret environment. A secret value
that a server or a Bash command prints in the Superagent must be redacted. […] Amend MGT-004 in an
ADR, and add an integration test in `crates/yulius/tests/cockpit.rs`." Issue #395.

## Goal

A person asking the Superagent (the Cockpit, SPEC-021) a quick question can use the project's MCP
servers (supabase, apify, …) exactly as a task run of the same sandbox can, and an agent secret
that reaches the session through one of those servers, or that a Bash command prints, is redacted
there as it is in a task run.

## Context

- `plan_cockpit` (`crates/yulius-harness/src/launch.rs`) attaches only `yulius` (SPEC-037
  MGT-004), inline in `--mcp-config`, and sets `--settings` only for `disabledMcpjsonServers`
  (SPEC-095 PMC-004). SPEC-021 CKP-002 says the Cockpit has no settings or hooks.
- Task runs get `App::mcp_attachments` (enabled instance and sandbox servers, a sandbox server
  overriding an instance one, `${secret:NAME}` resolved to the agent secret's value). The wiring's
  `mcp_specs` (`crates/yulius/src/wiring/harness.rs`) moves each value into a variable of its own,
  `YULIUS_MCP_<SERVER>_<VAR>` (`…_TOKEN` for a bearer), in the secret environment; the MCP config
  names only `${VAR}`.
- Task runs redact agent secrets in the session (SPEC-002 ADR-20): `CLAUDE_CODE_SHELL_PREFIX` runs
  every Bash command, hook and stdio MCP server through `yulius --scrub-exec`, and the
  `PostToolUse` hook (`yulius hook claude PostToolUse`, matcher `*`) replaces a tool result holding
  a value. Both redact from fingerprints fetched from `POST /internal/scrub/fingerprints` with
  `YULIUS_TASK_TOKEN` (or the run's `scrub.json`), and fail closed when they cannot load them.
- The Cockpit's environment is the person's terminal environment (`App::user_space`): it already
  holds `YULIUS_URL` and `YULIUS_TASK_TOKEN` = the person's workspace token (`ws_…`, for the Git
  credential helper), and `yulius` is bound read-only into a contained session. The scrub
  endpoints accept only task-attempt tokens, so the wrapper would fail closed there today.
- What is redacted in task runs is the agent secrets of the sandbox (the vault). A literal value
  typed into an MCP server's form is sealed and never shown in the UI, but it is not an agent
  secret and no run redacts it; `${secret:NAME}` is how a value is kept from everyone.

## Requirements

- SAM-001: The Superagent's `--mcp-config` (inline JSON, not strict) lists `yulius` and every
  server `App::mcp_attachments` gives a task run of its sandbox, in the same shape as a task run's
  `mcp.json`: stdio `env` and HTTP `Authorization: Bearer` values only as `${YULIUS_MCP_<SERVER>_<VAR>}`
  references, named by the same rule as a task run's. The values are in the Superagent's secret
  environment only, never in argv, a file or the shell record. A server is validated as for a task
  run (name, command, URL, variable names); `yulius` stays reserved. Without servers and without
  the Yulius server there is no `--mcp-config`.
- SAM-002: The Superagent gets the task runs' redaction of agent secrets: `--settings` holds the
  `PostToolUse` hook (`'<yulius>' hook claude PostToolUse`, matcher `*`, the tool-hook timeout) and
  its environment has `CLAUDE_CODE_SHELL_PREFIX=<yulius> --scrub-exec`, so Bash commands, hooks and
  stdio MCP servers run through the wrapper. No other hook, status line, briefing or setting is
  added; `disabledMcpjsonServers` stays (PMC-004).
- SAM-003: `POST /internal/scrub/fingerprints` accepts a person's workspace token: it answers the
  fingerprints of the agent secrets of that workspace's sandbox while the person is enabled and may
  work there, and refuses otherwise as an invalid token. `POST /internal/scrub/report` accepts it
  too and audits `vault.agent.redacted` with the person's login as actor, once per person,
  sandbox, secret and channel.
- SAM-004: A value of an agent secret that a Bash command prints in the Superagent, or that a tool
  result (a stdio or HTTP MCP server's included) holds, reaches neither the model nor the screen
  through those channels: it is replaced by `[secret:NAME]`. Without fingerprints (Yulius
  unreachable, the person no longer allowed) a command does not run and a tool result is withheld,
  as in task runs.
- SAM-005: ADR-0121 amends SPEC-037 MGT-004 and SPEC-021 CKP-002 and records why literal MCP
  values are not redacted (as in task runs); SPEC-021, SPEC-037, the architecture document and the
  product summary say what the Superagent gets.

## Plan

- [ ] Spec SPEC-147 and ADR-0121 amending MGT-004 and CKP-002, validated
- [ ] The harness plans a Superagent with the sandbox's MCP servers and the redaction hook
- [ ] The scrub endpoints accept a person's workspace token
- [ ] The Superagent launches with the sandbox's MCP servers, their values only in its secret environment, and the Bash wrapper
- [ ] Docs: SPEC-021, SPEC-037, architecture, product summary and changes say what the Superagent gets
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Gates green locally; merged into dev on green Yulius CI; #395 closed

## Acceptance criteria

- Harness unit test: a Cockpit request with a stdio server (two variables) and an HTTP server
  (bearer) plans `--mcp-config` holding `yulius` and both servers with `${…}` references only, the
  values absent from argv, env and `Debug`; `--settings` holds the `PostToolUse` hook and
  `disabledMcpjsonServers`; an invalid server name, a duplicate or `yulius` is refused.
- `crates/yulius/tests/cockpit.rs`: with an agent secret `APIFY_KEY`, a sandbox stdio server
  `apify` whose `APIFY_TOKEN` is `${secret:APIFY_KEY}` and an HTTP server with a bearer, the
  Superagent's `--mcp-config` lists `yulius`, `apify` and the HTTP server with references only; the
  values are in its environment and not in its argv or shell record; typed into the session, a
  command run through `$CLAUDE_CODE_SHELL_PREFIX` that prints the variable shows `[secret:APIFY_KEY]`
  and not the value, and the `PostToolUse` hook command from its settings, given a tool result with
  the value, answers it redacted.
- Integration test: the fingerprint endpoint answers a workspace token while its person may work
  in the sandbox and refuses it (401) once they lost access, and an unknown token.

## Test plan

- `crates/yulius-harness/tests/launch.rs`: plan shape, references, validation, redaction hook.
- `crates/yulius/tests/cockpit.rs`: the end-to-end launch on the real ptyhost with the fake
  `claude`, the real `yulius` wrapper and hook against the served internal listener, and the
  workspace-token endpoint cases.
- Existing cockpit, `secrets_redaction.rs` and `pool_supervision.rs` tests keep passing (task runs
  unchanged).

## Decisions

- ADR-0121: the Superagent gets the task run's servers and the task run's redaction (wrapper and
  `PostToolUse` hook) rather than no secret-bearing servers, a Yulius-side MCP relay or a
  Superagent-only scheme. The redaction hook is the only hook; CKP-002's "no hooks" was about the
  task lifecycle (briefing, Stop, status line), which the Superagent still does not get.
- The wrapper and hook authenticate with the workspace token the person's environment already
  holds, not a new variable: the person's process already has it, the fingerprints are salted
  hashes a task agent of the same person gets anyway, and the token is checked against the
  person's current access at every call.
- No fingerprint file for the Superagent: while Yulius is unreachable its Bash commands do not run
  and tool results are withheld (fail closed), for at most a restart.
- Literal MCP values stay unredacted, as in task runs (recorded in ADR-0121); a value meant to be
  hidden from people is an agent secret referenced as `${secret:NAME}`.
- A failure to build the MCP attachments fails the Superagent start, as it fails a task run start.
