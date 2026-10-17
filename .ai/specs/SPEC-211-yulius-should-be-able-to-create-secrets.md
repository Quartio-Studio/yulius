---
yulius-task: 15a3e4fd-ba52-4b0b-92a1-01b47a465db5
status: in-progress
---

# SPEC-211: Agents create agent secrets from a file, never seeing the value

Status: in-progress
Date: 2026-10-17
Owner: Yulius task #198
Scope authorization: task #198, requested by MrFishPL: "yulius should be able to create secrets
but without printing them, it's useful in situations like this one" (screenshot: a Superagent asks
the person to `cat ~/.ssh/quartio-v2-operator` in their own terminal and paste it into the Secrets
page, because it cannot add a key that sits in a file on the VM without the key passing through
its conversation; the person asks "can you not add the key by yourself?").

## Goal

An agent (a task's or the Superagent) that has a secret in a file it can reach (a key it just
generated with `ssh-keygen`, a token a CLI wrote, a key in `~/.ssh`) stores it as an agent secret
of its project in one tool call, without the value passing through the model, the transcript or
the screen: Yulius reads the file itself. Optionally Yulius deletes the file afterwards, so the
vault holds the only copy. Using the new secret still needs a person (hosts, `.env` copies), as
for any secret an agent did not create.

## Context

- Agent secrets (SPEC-121, SPEC-129, SPEC-186, SPEC-193): a name `[A-Z][A-Z0-9_]*`
  (`SecretName`), a value of at least 8 characters on one line (`check_agent_value`); a PEM or
  OpenSSH key is stored with literal `\n` (ADR-0156) and written back to a `.env` as its file
  (`dotenv::key_block`, ADR-0150). Today only people create them: the Secrets page and
  `yulius secrets import` (SPEC-205, ADR-0163), both through `App::create_agent_secret`, audited as
  `vault.agent.create`.
- Agents already use secrets without seeing them (ADR-0144, ADR-0150): `secrets_list`,
  `secret_request_hosts` (a person binds hosts), `secret_copy_to_env` (a person allows `.env`
  copies), `secret_verify` (Superagent only, by SHA-256). New tools reach running Superagents
  through the launch digest (ADR-0150); Claude Code asks the person before any Superagent tool not
  in `COCKPIT_READ_ONLY_TOOLS`.
- Yulius runs outside the agent's containment and with more rights than it.
  `secret_copy_to_env` already opens a folder of the worktree one component at a time without
  following links and checks its real path (`vault/env_files.rs::open_folder`), so a folder
  swapped for a link is never followed.
- Every tool result is redacted of the sandbox's agent secrets; creating a secret bumps the vault
  generation, so the new value is redacted from then on.
- The CLI import turns a multi-line key file into the stored form with its own code
  (`yulius-import::secrets_import::agent_value`).

## Requirements

- SCF-001 (tool): `secret_create_from_file {name, file, description?, delete_file?}` is served to
  task agents and to the Superagent. For the Superagent it is not among the tools it runs without
  asking: Claude Code asks the person first.
- SCF-002 (where the file may be): `file` relative to the folder the caller works in (a task's
  worktree, the Superagent's sandbox repository) or starting with `~/` for the caller's home (a
  task's: the sandbox home; the Superagent's: its person's home in the sandbox). Absolute paths,
  `..`, `.git` and NUL are refused. Every folder on the way is opened without following links and
  the folder reached must lie inside that root; the file itself is opened without following a
  link and without blocking, and must be a regular file of at most 16 KiB. Errors name the path,
  never content.
- SCF-003 (value): The file must be UTF-8. One final line break (LF or CRLF) is dropped. A text
  that still spans lines is stored only when it is a key block (PEM, OpenSSH, a key with its
  certificate), with blank lines around it dropped and each line break written as literal `\n`, as
  the Secrets page and the CLI import store it; any other multi-line text is refused. The result
  then passes `check_agent_value`. The same conversion serves the CLI import.
- SCF-004 (creation): The name passes `SecretName`; a name that already applies to the project (a
  sandbox or instance-wide agent secret) is refused, nothing is overwritten. The person on whose
  authority the caller acts (a task's person, the Superagent's person) must be able to manage the
  sandbox's agent secrets (write access), as on the Secrets page. The secret applies to this
  sandbox only, with no hosts, no `.env` allowance and not for production deploys; its description
  at most 1000 characters. It is created by that person, and audited as `vault.agent.create` with
  the caller as actor (`task:<n>`, `cockpit:<login>`) and a detail saying it came from a file of an
  agent.
- SCF-005 (result): Name, file, whether the file was deleted, and what to do next (bind hosts with
  `secret_request_hosts`, `.env` with `secret_copy_to_env`, delete the file if the vault should
  hold the only copy). Never the value, its length or a hash of it.
- SCF-006 (deleting the file): With `delete_file: true`, once the secret is stored, Yulius removes
  the file through the folder it opened. A removal that fails leaves the secret and says so.
- SCF-007 (guidance): The tool's description, the Superagent's instructions and `secrets_list`'s
  guidance name the tool, so an agent with a key in a file stores it instead of asking a person
  to paste it.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] A key file's text becomes a stored agent-secret value (domain, shared with the CLI import)
- [ ] Yulius reads a file from the caller's folder or home without following links
- [ ] secret_create_from_file creates the secret for a task and the Superagent, audited, optionally deleting the file
- [ ] Docs: ADR, architecture, product, instructions and briefing name the tool
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-1: `stored_from_file` turns a one-line token with a final LF or CRLF into the token, a PEM or
  OpenSSH key file (LF or CRLF, blank lines around it) into its `\n`-escaped form that
  `dotenv::key_block` reads back as the file, and refuses multi-line text that is not a key, a
  value under 8 characters and control characters, with messages that hold no content.
- AC-2: A task stores `~/.ssh/deploy_key` (an OpenSSH key) and `secrets/token.txt` of its worktree
  as agent secrets of its sandbox: the vault holds the expected values, with no hosts, `.env` not
  allowed, not production, created by the task's person, audited `vault.agent.create` as
  `task:<n>`; the tool result and the audit never hold the value.
- AC-3: A link (as the file or as a folder on the way), a FIFO, a file over 16 KiB, a non-UTF-8
  file, an absolute path, `..` and a missing file are refused, and nothing is created.
- AC-4: A name already in use (sandbox or instance-wide) is refused and the stored value is
  unchanged; an invalid name is refused.
- AC-5: A task whose person can only read the sandbox is refused, and nothing is created.
- AC-6: With `delete_file: true` the file is gone after the secret is stored; without it the
  file stays.
- AC-7: The Superagent is served the tool, Claude Code asks before it runs, and it stores a key
  from its person's home, audited as `cockpit:<login>`.
- AC-8: Right after the call, a tool result holding the value shows `[secret:NAME]`.

## Test plan

- Unit (`crates/yulius-domain/src/vault/dotenv.rs`): AC-1.
- Unit (`crates/yulius-app/src/vault/env_files.rs`): AC-3's link, FIFO and size cases on the file
  reader.
- Integration (`crates/yulius/tests/secrets_create.rs`, through the MCP endpoint with a real
  worktree): AC-2..AC-6, AC-8.
- Integration (`crates/yulius/tests/cockpit.rs`, `manage_tools.rs`): AC-7.
- Existing CLI import tests keep passing on the shared conversion.

## Decisions

- **Yulius reads the file; the value never goes through the model.** The alternative, a `value`
  argument, puts the value in the tool call, the transcript and the screen; that is what the
  person wanted to avoid. ADR-0167.
- **Tasks get it too, not only the Superagent.** Creating a secret reveals nothing and grants
  nothing: the secret starts with no hosts and no `.env` allowance, so a person still decides
  every use. A task that generates a deploy key is the same situation as the screenshot.
- **Only the working folder and home, without links.** Yulius has more rights than the agent; a
  link or an absolute path could make it read Yulius' own files (its vault key) into a secret the
  agent then asks to use. The two roots cover the cases (a generated key in the worktree, a key in
  `~/.ssh`); anything else the agent can copy there first without printing it.
- **Never overwrite.** Replacing a working key silently could break every agent using it; rotation
  stays on the Secrets page, as for the CLI import (ADR-0163).
- **Delete on request only.** It is the person's or agent's file; deleting by default would
  surprise a person who keeps a key in `~/.ssh` for SSH itself.
- **The person's write access gates it.** An agent acts for its person; one who could not add the
  secret on the Secrets page cannot add it through an agent.
