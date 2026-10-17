---
yulius-task: 32fd28f3-76f8-46ba-9b51-97cf6d4f16fe
status: in-progress
---

# SPEC-205: `yulius secrets import` brings team and agent secrets in from the CLI, safely

Status: in-progress
Date: 2026-10-17
Owner: Yulius task #192
Scope authorization: task #192, requested by MrFishPL: "To cli add and option to import secrets
for agents and team. It should be safe, leave no logs and require confirmation".

## Goal

A person moves the keys they hold on their computer (a project's `.env`, a handful of tokens) into
the Yulius vault from the terminal, as team secrets (for people) or as agent secrets of a sandbox
(for agents), without retyping each in the Secrets page. The command is safe: a value never
appears on the command line, on the screen, in an error, in a file the command writes or in a
server log; nothing that exists is overwritten; and nothing is sent before the person has read
the list of names and typed `yes` at the terminal.

## Context

- The vault (SPEC-040, SPEC-121, SPEC-129, SPEC-186, SPEC-193) already has the create calls the
  web page uses: `POST /api/team-secrets` (`TeamSecretInput`: title, value, notes) and
  `POST /api/sandboxes/{sid}/agent-secrets` (`AgentSecretInput`: scope, name, description, hosts,
  value), and the lists `GET /api/team-secrets` and `GET /api/sandboxes/{sid}/agent-secrets`
  (names only, never values). Creating reveals nothing, so it needs no two-factor code; each
  create writes a vault audit entry with the name (`vault.team.create`, `vault.agent.create`),
  never the value (`crates/yulius-app/src/vault/team.rs`, `agents.rs`).
- Every `/api` route accepts the bearer token that `yulius login` stores
  (`crates/yulius-server/src/auth.rs`, `crates/yulius-import/src/token_store.rs`); `yulius
  onboard` already talks to the server with it through `yulius-import`'s blocking `ApiClient`
  (https only, bounded bodies, bounded retries).
- Agent secrets: a name `[A-Z][A-Z0-9_]*` that is not reserved (`SecretName`), a value of at least
  8 characters on one line; a PEM or OpenSSH key is stored with literal `\n` and is written back
  as its file (`check_agent_value`, `dotenv::key_block`). Team values may span lines.
- The server has no request tracing layer: request bodies are not logged
  (`crates/yulius-server/src/lib.rs`); vault code logs errors, never payloads.
- `console` (already a dependency) reads a hidden line from `/dev/tty` when stdin is not a
  terminal.

## Requirements

- SIM-001 (command): `yulius secrets import --to team|agent [FILE] [--sandbox <owner/name|slug|id>]
  [--instance] [--server <url>] [--no-color]`. No option takes a secret value.
- SIM-002 (sources): FILE is a dotenv file; `-` reads it from stdin; without FILE the command asks
  for a name (shown) and its value (hidden, not echoed) until an empty name. At most 1 MiB and 500
  secrets per run.
- SIM-003 (dotenv): Blank lines and `#` comment lines are skipped; an optional `export ` prefix;
  `NAME=value` with spaces around `=` and the value trimmed; an unquoted value ends at ` #`; a
  single-quoted value is literal and may span lines; a double-quoted value may span lines and
  reads `\n`, `\r`, `\t`, `\"`, `\\`; `${…}` is kept literally (never expanded); a byte order mark is
  ignored. The comment lines directly above a key become its description (agent) or notes (team),
  except a commented-out assignment (`# NAME=…`, often an old key), which is never kept. A malformed line, an
  unterminated quote or a name defined twice refuses the whole file before anything is asked,
  naming the line numbers and never quoting a value.
- SIM-004 (target): `--to team` creates team secrets titled by the name. `--to agent` creates
  agent secrets of a sandbox: `--sandbox` matched against the caller's sandboxes by id, slug or
  `owner/name` (case-insensitive), else the sandbox of the current directory's GitHub remote;
  `--instance` makes them apply to every sandbox (the server allows instance administrators only).
  Agent secrets start with no hosts: agents propose them and a person allows them (SPEC-121).
- SIM-005 (checks): Before asking, each secret is classified: `new`; `exists` (same team title, or
  same agent name in the same scope), skipped and never overwritten; or `invalid` with the reason
  (agent name rules, value too short, too long, empty, a multi-line value that is not a key
  block, or a key's first line alone, as a key pasted at the hidden prompt arrives), skipped; the
  hidden prompt refuses such a value at once. An agent value that spans lines and is a PEM/OpenSSH key is sent with literal
  `\n`, as the Secrets page stores it.
- SIM-006 (confirmation): The target (sandbox, existing names) is checked before anything is
  typed. The command shows the server, the target and every name with its class (also at the
  terminal when stdout is redirected), then asks to type `yes` at the terminal (`/dev/tty`, so stdin can carry the file). Anything else,
  end of input or no terminal cancels: nothing is sent. No flag skips the confirmation. With no
  `new` secret nothing is asked or sent.
- SIM-007 (import): New secrets are created one by one; a refused one is reported with the
  server's message and the rest go on; an expired sign-in or an unreachable server stops the run,
  saying which were imported; so does a 403 (no right to manage them), which would refuse every
  other. Creates are never retried. The summary counts imported, skipped and failed; the exit code is 0
  when nothing was invalid, failed or cancelled, 1 otherwise.
- SIM-008 (no traces): Values are never printed, never part of an error, never written to disk by
  the command and are wiped from memory when dropped. Output shows names only (no lengths). After
  importing from a file the command reminds the person that the file still holds the values.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A dotenv reader turns a file into named secrets without echoing values
- [x] The import client lists, checks and creates team and agent secrets over the existing API
- [x] `yulius secrets import` asks for typed confirmation and imports from a file, stdin or hidden prompts
- [x] Docs, ADR and changes log describe the import
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-1: The dotenv reader returns names, values and comments for bare, quoted, multi-line,
  `export` and commented entries, and refuses a line without `=`, an unterminated quote and a
  duplicate name with line numbers, the error text holding no value.
- AC-2: Against a fake server, a team import of a file with a new and an existing title, after
  `yes`, creates only the new one (title, value, notes as sent) and reports the existing one as
  skipped.
- AC-3: An agent import resolves `--sandbox owner/name`, skips an invalid name and an existing
  name, sends a PEM key with literal `\n`, sends `scope: instance` with `--instance`, and creates
  only the valid new secrets.
- AC-4: Answering anything but `yes`, end of input or no terminal sends no create request; a file
  with nothing new asks nothing.
- AC-5: A refused create is reported and the next one still goes; a 401, a 403 or a 5xx stops
  the run after one request.
- AC-8 (review): a commented-out assignment never becomes a description; a byte order mark and
  `export<TAB>` are read; a key's first line alone is skipped and refused at the hidden prompt; an
  unknown sandbox is refused before anything is typed.
- AC-6: Nothing the command prints (plan, prompts, errors, summary), for success, refusal or
  failure, contains any imported value.
- AC-7: `yulius secrets import --to team <malformed file>` exits 1 before asking anything, naming
  the line, without the value.

## Test plan

- Unit (`crates/yulius-import/src/dotenv.rs`): AC-1.
- Integration (`crates/yulius-import/tests/import/secrets.rs`, fake server with vault routes,
  scripted terminal): AC-2..AC-6.
- Binary (`crates/yulius/tests/cli.rs`): AC-7.
- Manual: the built binary against a local Yulius server is not possible in this sandbox without a
  signed-in CLI; the fake server covers the HTTP contract, which is the server's existing one.

## Decisions

- ADR-0163: the CLI imports secrets client side over the existing vault API; values come only from
  a file, stdin or a hidden prompt; a typed `yes` at the terminal is always required; existing
  secrets are never overwritten; the audit trail keeps names.
- Existing secrets are skipped rather than replaced: replacing a value is a rotation, which the
  Secrets page already does per secret with its context; an import that silently replaces could
  destroy a working key. Reversible: a later `--replace` can be added behind the same confirmation.
- `${…}` is kept literally: expanding would read the person's environment and could pull a
  different secret into the vault.
- The source file is left in place: deleting a person's file is not the command's job; it reminds
  them instead.
