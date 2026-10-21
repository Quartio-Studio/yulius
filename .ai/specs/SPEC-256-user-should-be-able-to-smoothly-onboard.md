---
yulius-task: ccd5afca-7e6e-4cce-897c-52dcef060424
status: done
---

# SPEC-256: Onboard a laptop repository with one request to the local agent

Status: done
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
  NAME, described "From the file <file name>" (a leading `~/` is the home); a multi-line key is stored with literal `\n` as the dotenv import stores it; at most
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
  answer. The account check comes before anything is created. The onboarding task rests
  `closed` until it starts, so the notes join its request through a conditional update of a
  task that never ran (`append_resting_prompt`).
- **LOB-006** The onboarding prompt tells the task to read `secrets_list` and its request's
  hand-over before asking anyone for a credential or machine, and to use `secret_copy_to_env` for
  keys used outside the proxy.
- **LOB-007** The recipe: `REMOTE_INSTRUCTIONS` names the laptop onboarding steps, and
  `docs/remote-control.md` has "Onboard this folder" with the commands, what the agent looks for
  (workflow `secrets.*`, deploy scripts, `.env*`, `~/.ssh/config` hosts the deploy uses, cloud CLI
  credentials) and the rule that values go only from files to `yulius secrets import`, never into
  the conversation.

## Plan

- [x] Spec and ADR: the person's agent token delegates the confirmations
- [x] One token: onboard and secrets import use the remote operate token
- [x] Unattended secrets import: --yes with an operate token and key files with --from-file
- [x] Imported secrets can be allowed into .env files at creation
- [x] Hand-over notes reach the onboarding task
- [x] Laptop onboarding recipe in the remote MCP instructions and the guide
- [x] Independent review, gates and delivery

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

All at the branch head after `origin/dev` at `f19324cc` (no newer dev commits when delivered).

- `scripts/dev ci`: passed (exit 0) at `2e18cb47`, 2393 tests passed / 17 skipped. It covers the
  repository checks, script tests, `cargo check`, `fmt --check`, Clippy `-D warnings` for Linux and
  `aarch64-apple-darwin`, nextest, doctests, `cargo deny`, `cargo shear` and `cargo doc`. At
  `d739fe46` one test failed: `cli::secrets_import_refuses_a_malformed_file_without_showing_values`.
  The round 1 reorder had moved the token lookup before the file was read; it now comes first
  only with `--yes`, and the gate was run again at the final head (see the pull request's Yulius
  CI).
- `scripts/dev security` (Vidoc vidoc-1, change scan at `877dba10`, 22 files): no critical or
  high finding, and no medium finding in the change. There were no low findings in the change.
  Five medium or low findings are in lines this branch did not touch: `run.rs:391`,
  `sandboxes.rs:118`, `token_store.rs:115`, `cli.rs:20` and `main.rs:72`. Yulius CI files each one
  as an issue.
- New tests, each red-checked (a missing method or flag, a 422 for the unknown field, the
  old message, or the fix disabled):
  - `remote_mcp::a_laptop_project_is_onboarded_by_its_agent_alone`: the acceptance flow through
    the real `yulius` binary against a served instance, with only an operate token stored. It runs
    `yulius onboard --yes` (no device login), then
    `yulius secrets import --yes --env-files .env --from-file DEPLOY_SSH_KEY=~/.ssh/…` (no
    terminal, no value printed, both secrets `env_files: allowed` and `auto_allow`, audited), then
    a re-run that overwrites nothing, then `yulius project onboard --notes` (a hand-over over
    24 KiB is refused; the task's request holds the hand-over, redacted of a `ghp_` token; a
    second hand-over once onboarding started is refused).
  - `remote_mcp::an_unattended_import_needs_an_operate_token`: a `cli` token or a read token is
    refused. With an operate token the duplicate names, a malformed `NAME=PATH`, a missing file,
    the flag conflicts and `--yes` without a file are all refused, and nothing is sent.
  - `remote_mcp::a_hand_over_is_added_once_and_only_where_it_can_be_read`: with no Claude account
    nothing is created; a retry adds the hand-over once; a task that tried to run is refused; blank
    notes count as none.
  - `remote_mcp::a_hand_over_overtaken_by_another_start_is_refused`: uses the test-only pause point
    `race::Point::OnboardingApproved`.
  - `secrets_env::a_secret_created_allowed_is_copied_without_a_person`: `secret_copy_to_env`
    writes `.env` with no person involved. Creation is refused for a personal secret and with
    `auto_allow` off.
  - Store `a_resting_task_that_never_ran_takes_an_addition`; token store
    `the_remote_token_stands_in_for_a_missing_login_token`; import
    `an_unattended_run_asks_nothing` and `a_key_file_is_imported_by_path`; domain
    `a_hand_over_follows_the_request_under_whose_agent_wrote_it`; the remote instructions'
    `the_instructions_teach_onboarding_a_local_folder`; and the onboarding prompt's assertion on
    `secrets_list`.
- Independent review in a fresh session, three rounds, all findings in this change's own code:
  - Round 1: 6 minor findings and 5 nits. Fixed test-first: the hand-over is now added after the
    branch step and only once, refusals say what happened, the operate token is checked before
    reading, the missing tests were added, refusal messages are checked, and a failed or
    oversized read is reported as such. Accepted with the reviewer: a personal `env_files` create
    answers 422 (the answer endpoint says 409), and a stored remote read token makes
    `yulius onboard` fail with 403 instead of starting a device login.
  - Round 2: a start or skip landing between the two reads dropped the hand-over unsaid, and a
    concurrent duplicate append was possible; both are fixed.
  - Round 3: APPROVE at `d739fe46`. The one remaining nit is that the `skipped` arm of the race has
    no test of its own; it shares its code path with the tested `preparing` arm.
- Not tested:
  - The real Quartio-Studio/product repository: it is private and this sandbox's GitHub token
    cannot read it (404). The person runs it from their laptop.
  - A real Claude Code client following the remote instructions end to end: the steps are
    covered command by command by the binary tests above.
  - macOS shells' handling of `NAME=~/…`: the CLI expands a leading `~/` itself.
  - A real onboarding agent using the imported keys over ssh. `secret_copy_to_env` writing them is
    tested; the agent's use of them is the onboarding task's own work.
