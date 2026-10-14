---
yulius-task: 6be6436f-0918-489b-a668-5695688862ad
status: in-progress
---

# SPEC-129: keys live in a git-ignored .env; agents copy agent secrets there without seeing them

Status: in-progress
Date: 2026-10-14
Owner: Yulius task #111
Scope authorization: the person's request (task #111): "Default place for keys should be .env that
is gitignored, with not-ignored .env.example. When agent starts using key from the agent's vault in
prod, it should copy them into .env. Remember the agent sees only the identifier so it should copy
it mechanically instead of write because it'll write masked value, not the real one. It's a
preparation for implementing Yulius CD".

## Goal

A project's keys have one default home: a `.env` file Git ignores, next to a tracked `.env.example`
that names every key with no value. When the project's own code starts to need an agent secret's
real value (code that runs outside Yulius' proxy, such as a deployed app or a deploy script of the
coming Yulius CD, task #113), the agent copies it there with one MCP tool, `secret_copy_to_env`.
Yulius writes the value; the agent never sees it, so it cannot write the masked `[secret:NAME]` by
mistake. Because a value in a file leaves the proxy's host binding, a person who manages the
secret allows each secret to be copied, once, on the Secrets page, the way they allow its hosts.

## Context

- Agents get agent secrets only as placeholders (`yulius-secret-NAME` in the variable `NAME`); the
  egress proxy replaces them for bound hosts only (SPEC-002 ADR-20). Anything that would show a
  value to the agent shows `[secret:NAME]`: the Bash wrapper and the `PostToolUse` hook redact
  file reads, `base64` and the rest from fingerprints (AC-VLT-4,
  `crates/yulius/tests/secrets_redaction.rs` reads a `.env` back). So if an agent edits a `.env`
  it read, it writes `[secret:NAME]`, not the value: that is the mistake the person names.
- The bound hosts are the boundary that keeps a value from a server an agent was tricked into
  calling (ADR-0098). ADR-0078 refused to let an agent-written file hand a value in clear to a
  command an agent chose: "Only a person editing the server on the MCP page can give it a secret's
  value directly." A value in `.env` is exactly that, so the copy needs a person's allowance.
- `secret_request_hosts` (`crates/yulius-app/src/vault/requests.rs`, SPEC-121) is the pattern:
  the agent asks, the task is flagged as needing a person, the Secrets page shows "Task #N asks…"
  with Allow and Dismiss, the answer is a compare-and-set update and is audited.
- `GitPort::is_disposable` (`git ls-files` + `git check-ignore`) already answers "ignored by Git
  and not tracked" for a worktree path; `yulius_domain::redact::is_env_file` already tells dotenv
  files (`.env`, `.env.local`, `prod.env`, `.envrc`) from templates (`.env.example`, …).
- Agent-secret values have no control characters (`check_agent_value`), so one value is one line.
- The Yulius repository's `.gitignore` already ignores `.env` and `.env.*` and keeps
  `!.env.example`; Yulius itself keeps its own keys in its sealed vault, so it has no `.env`.
- Task #113 (CD) is chained after this task and will read keys from `.env` in its deploy scripts.

## Requirements

- R1. `secret_copy_to_env {secrets: [NAME…], path?: ".env", reason?}` writes `NAME=<value>` for each
  named agent secret of the task's project into the dotenv file at `path`, relative to the task's
  worktree (default `.env`). It writes all or nothing.
- R2. It refuses, writing nothing: a path that is absolute, leaves the worktree (also through a
  symbolic link), names `.git`, is not a dotenv file name, or is a template; a target that is a
  symbolic link or not a regular file; a target Git does not ignore or tracks ("add `.env` to
  `.gitignore`"); an unknown secret; more than 16 secrets; a task without a worktree.
- R3. A secret a person has not allowed to be copied is not written. The call records a request
  for it (who asked, audited `vault.agent.env_request`), flags the task as needing a person and
  answers with the Secrets page link. Asking again while it waits adds the asker, nothing else.
- R4. On the Secrets page a person who manages the secret sees "Task #N asks to copy it into the
  project's .env" with Allow and Dismiss; an allowed secret shows "Copied into .env files" with
  Revoke. `POST /api/agent-secrets/{id}/env {allow}` answers or revokes, compare-and-set on the
  previous state, audited `vault.agent.env_allow`, `vault.agent.env_dismiss` or
  `vault.agent.env_revoke`; others get 403, invisible secrets 404, a stale answer 409.
- R5. The file keeps every other line. A line for the name (also `export NAME=…`) is replaced in
  place (later duplicates are dropped); a missing one is appended. Values are written unquoted
  when they hold only `A-Za-z0-9_-./:+=@,%~^`, else in single quotes, else (a value with `'`) in
  double quotes, refused if it also holds `"`, `\`, `$` or a backtick. The file is written
  atomically with mode 0600 and is at most 256 KiB.
- R6. The `.env.example` next to the file (created if missing, kept if it is a symbolic link)
  gains `NAME=` for each copied name it lacks (a commented `# NAME=` counts). The answer says when
  Git ignores it.
- R7. Every copy is audited once per run, secret and file (`vault.agent.env`) and marks the
  secret used.
- R8. `secrets_list` shows whether each secret may be copied (`env_files`: no, asked, allowed) and
  says how keys are kept: in the git-ignored `.env`, documented by `.env.example`, copied with
  `secret_copy_to_env`, never by editing the file. The briefing and the onboarding skill
  (`infra-bootstrap`) set `.env` / `.env.example` as the default place for keys.

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] Dotenv lines are written by a pure domain function that quotes and replaces safely
- [ ] A person allows, dismisses or revokes copying a secret into .env files on the Secrets page
- [ ] secret_copy_to_env writes real values into an ignored .env and names into .env.example
- [ ] secrets_list, the briefing and the onboarding skill teach .env as the default place for keys
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC1 (R1, R3, R4, R7): an agent asks for `STRIPE_KEY`; nothing is written, the task needs a
  person and the secret shows the request; after Allow, the same call writes the real value into
  the worktree's `.env` (mode 0600), the agent's tool result never holds it, and the activity
  shows `vault.agent.env`.
- AC2 (R2): `.env` not ignored, tracked, a symbolic link, `../x.env`, `.env.example`, `README.md`
  and an unknown secret are each refused and nothing is written.
- AC3 (R5): existing lines, comments and `export` survive; the name's line is replaced; quoting
  follows R5; a value Yulius cannot write portably is refused.
- AC4 (R6): `.env.example` gains `STRIPE_KEY=` once and never a value.
- AC5 (R4): dismiss, revoke, a stale answer (409), a reader (403) behave as R4 says.
- AC6 (R8): `secrets_list` carries `env_files` and the `.env` guidance; the briefing names the
  default place for keys.

## Test plan

- Domain unit tests (`yulius-domain` `vault::dotenv`): quoting classes, replacement, `export`,
  duplicates, append without a final newline, example names, refusal.
- Integration tests (`crates/yulius/tests/secrets_env.rs`, real Git over a temporary repository):
  AC1, AC2, AC4, AC5 through MCP and the HTTP API.
- `secrets.rs`/briefing tests for AC6.
- Gates: `scripts/dev ci`, and the e2e suite for the Secrets page change.

## Decisions

- **A person allows each secret once (ADR-0105).** The agent copies mechanically, as asked, but a
  value in a file leaves the host binding that ADR-0098 and ADR-0078 keep under a person's
  control. Alternatives: the agent copies freely (defeats the boundary for any prompt-injected
  run); allowance per copy (a click for every task and file, no added safety once the value is on
  disk in the project); a toggle in the add dialog (the person asked for fewer options, SPEC-121).
- **Only the task's own worktree, only ignored dotenv files.** The guard is Git itself
  (`check-ignore`, `ls-files`), so a value can never be committed by a later `git add .`. Task #113
  can reuse the same writer for its deploy checkout.
- **All or nothing.** A partial copy leaves a half-configured app; the agent calls again once
  every secret is allowed.
- **Portable quoting over completeness.** Single quotes are literal in Node `dotenv`,
  `python-dotenv`, Docker Compose and `sh`; a value that needs escapes no common parser agrees on is
  refused with a reason instead of written wrongly.
- **Rotation is not followed.** A copy is a snapshot; the guidance says to copy again after a
  value is rotated. Following rotations would mean Yulius rewriting files in worktrees behind the
  agent's back.
- **No `.env` for Yulius itself.** Its `.gitignore` already follows the convention and its keys
  live in its vault.
