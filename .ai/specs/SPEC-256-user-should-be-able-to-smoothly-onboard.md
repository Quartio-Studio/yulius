---
yulius-task: ccd5afca-7e6e-4cce-897c-52dcef060424
status: draft
---

# SPEC-256: Onboard a laptop repository with one request to the local agent

Status: draft
Date: 2026-10-21
Owner: task #242
Scope authorization: task #242. The operator: "User should be able to smoothly onboard their repo
using cli or mcp from laptop … When we tell claude to onboard the repo, it should send the files,
… adjust the format, reverse engineer docs, upload keys and approve their usage (not human action
needed) … after a few hours a user can start working with a product with nothing else needed from
him. If the product was developed through the laptop and worked, it can be done from yulius as
well." First user: Quartio-Studio/product.

## Goal

A person whose project works from their laptop (a GitHub checkout with its own deploy infra,
GitHub Actions and keys kept somewhere on the laptop) says to their local Claude Code "onboard
this into Yulius". With nothing else from them, the local agent sends the checkout and sessions,
moves the keys the project needs into the project's agent secrets, already usable by the
project's agents, hands over what it learned about the infra, and starts onboarding. The
onboarding task adapts the format, reverse-engineers the docs, moves CI and CD into Yulius and
finds the keys there instead of asking the person.

## Context

Confirmed against `dev` at f19324cc. SPEC-247 (task #233) made the remote MCP and the
`yulius remote|project|task|ci` CLI; SPEC-205/216 made `yulius secrets import`; SPEC-001 IMP-*
made `yulius onboard`. What still needs the person, step by step:

1. **Two sign-ins.** `yulius onboard` and `yulius secrets import` read only the token of
   `yulius login` (`~/.config/yulius/credentials.json`, a browser device approval, scope `cli`).
   The local agent's token from Settings (`operate`, `yulius remote login`,
   `remote-credentials.json`) is not used, so `yulius onboard` starts a device login and waits for
   the person in the browser.
2. **A typed yes.** `yulius secrets import` sends nothing until `yes` is typed at `/dev/tty`
   (ADR-0163, "no flag skips it"). Claude Code's Bash tool has no terminal: the import cancels.
3. **Key files.** A key kept as a file (`~/.ssh/deploy_ed25519`, a PEM, a service account JSON)
   goes in only inside a dotenv file, so the agent would have to read the key to write one.
4. **`.env` copies wait for a person.** Code outside the egress proxy (ssh, deploy scripts) gets a
   key through `secret_copy_to_env`, which a person must allow once per secret on the Secrets page
   (`env_files: asked → allowed`, SPEC-129); `POST /api/agent-secrets/{id}/env` cannot allow a
   secret nobody asked for. Hosts are already set up by agents (`auto_allow`, ADR-0171).
5. **No hand-over.** The onboarding prompt (`onboarding_prompt`) knows the repository only; what
   the laptop agent found (which secret is which, deploy hosts, how a deploy runs from the laptop)
   is lost, and `CD_SETUP` tells the task to ask a person with `needs_you` for credentials.
6. **No recipe.** `REMOTE_INSTRUCTIONS` and `docs/remote-control.md` describe import and
   onboarding of a GitHub repository, not "onboard this folder" with its keys.

Existing pieces kept: `yulius onboard --yes` (upload without questions, creates the project when
the person may), `onboarding_start` (operate tokens may start onboarding, ADR-0188), queued
messages (`append_unstarted_prompt`, SPEC-052), `yulius_domain::redact::redact`.

## Requirements

- **LOB-001** One token: `yulius onboard` and `yulius secrets import` use the `yulius login`
  token for the server when one is stored, else the token `yulius remote login` stored for it. A
  server is chosen by `--server`, else the only server either store knows. `yulius onboard`
  starts a device login only when neither store has a token.
- **LOB-002** `yulius secrets import --yes` sends without the typed `yes`, and only with an
  `operate` token stored by `yulius remote login` (checked with the instance before anything is
  read or sent): a `cli` or `read` token, or none, is refused with what to do, nothing sent. The
  names, their classes and every rule of ADR-0163 (no values out, nothing overwritten, invalid
  entries skipped) stay; with `--yes` no terminal is needed.
- **LOB-003** `--from-file NAME=PATH` (repeatable) imports the whole file at PATH as the secret
  NAME, with the description "From the file <file name> on <person>'s computer" for agent
  secrets; a multi-line key is stored with literal `\n` as the dotenv import stores it; at most
  64 KiB read; a NAME given twice (also in the dotenv file) refuses the run before anything is
  sent. Combinable with the dotenv file.
- **LOB-004** `--env-files` (agent secrets) creates each secret already allowed into `.env` files:
  `AgentSecretInput.env_files: true` stores `env_files = allowed`, audited
  `vault.agent.env_allow` ("at creation") beside `vault.agent.create`. Refused (400) for a
  personal secret and for one that is not `auto_allow` (`--confirm-hosts`: a person allows each
  use). Absent, nothing changes; older servers never receive the field.
- **LOB-005** Hand-over notes: `onboarding_start {notes}` (remote MCP), the body
  `{"notes": …}` of `POST /api/sandboxes/{sid}/onboarding/approve`, and `yulius project onboard
  --notes FILE|-` add the notes to the onboarding task's request before it starts, under
  "Hand-over from <login>'s agent on their computer", redacted of secrets
  (`yulius_domain::redact`). At most 24 KiB; empty notes are none. When the onboarding already
  started, notes are refused (409, "tell its task with task_message"); no notes keeps today's
  answer. The account check comes before anything is created.
- **LOB-006** The onboarding prompt tells the task to read `secrets_list` and its request's
  hand-over before asking anyone for a credential or machine, and to use `secret_copy_to_env` for
  keys used outside the proxy.
- **LOB-007** The recipe: `REMOTE_INSTRUCTIONS` names the laptop onboarding steps, and
  `docs/remote-control.md` has "Onboard this folder" with the commands, what the agent looks for
  (workflow `secrets.*`, deploy scripts, `.env*`, `~/.ssh/config` hosts the deploy uses, cloud CLI
  credentials) and the rule that values go only from files to `yulius secrets import`, never into
  the conversation.

## Plan

- [ ] Spec and ADR: the person's agent token delegates the confirmations
- [ ] One token: onboard and secrets import use the remote operate token
- [ ] Unattended secrets import: --yes with an operate token and key files with --from-file
- [ ] Imported secrets can be allowed into .env files at creation
- [ ] Hand-over notes reach the onboarding task
- [ ] Laptop onboarding recipe in the remote MCP instructions and the guide
- [ ] Independent review, gates and delivery

## Acceptance criteria

- With only `remote-credentials.json` holding an `operate` token, `yulius onboard --yes` uploads
  without a device login and `yulius secrets import --to agent --yes .env --from-file
  KEY=path --env-files` creates the secrets with no terminal; each secret is `env_files:
  allowed` and `auto_allow`.
- `--yes` with a `cli` or `read` token, or no remote token, exits 1 and sends nothing.
- `onboarding_start {notes}` on a pending project starts the onboarding task whose request holds
  the hand-over (redacted); on a preparing project with notes it is refused; notes over 24 KiB are
  refused.
- `env_files: true` with `auto_allow: false` or a personal scope is refused.

## Test plan

- `yulius-import` unit tests: `import_secrets` with `assume_yes` asks nothing; `--from-file`
  entries; duplicate names.
- `crates/yulius/src/cli.rs` unit tests: token and server resolution across both stores.
- `crates/yulius/tests/remote_mcp.rs`: the CLI binary against a served instance with only a remote
  operate token (`onboard`-style secret import `--yes --from-file --env-files`, refusal with a
  `cli` token); `onboarding_start` with notes, refused notes.
- `crates/yulius/tests` vault test: `env_files` at creation and its refusals.
- `onboarding.rs` unit test: the prompt names `secrets_list` and the hand-over.
- Gates: `scripts/dev ci`, `scripts/dev security`.

## Decisions

- **The agent token is the person's delegation for confirmations too** (ADR-0194): ADR-0188 let
  an `operate` token start onboarding because the person made it on purpose for their agent; the
  same holds for the import's `yes` and the `.env` allowance. `cli` tokens keep ADR-0163's typed
  yes. Alternatives: a `--yes` for every token (rejected by ADR-0163: copied into scripts); values
  through the remote MCP (`secret_create {value}`: the value would pass through the agent's
  context and transcript); a server-side "trust this import" toggle on Settings (one more thing
  the person must do first).
- **Values travel file → CLI → vault only.** `--from-file` takes a path, so the agent locates a key
  and never reads it into the conversation; the CLI prints names only.
- **Notes are a queued addition of the onboarding task's request** (SPEC-052's mechanism), so every
  reader of the request (goal, request file, `task_get`) has them, and nothing new is stored.
- **One pull request.** The pieces are only useful together for the one request; each is small
  and reuses an existing path.

## Evidence

Pending.
