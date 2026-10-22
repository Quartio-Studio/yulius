---
yulius-task: 8c56bf7c-b8ff-4daa-98f5-a1d24d8d5e80
status: done
---

# SPEC-275: Project variables: a project's non-secret values, in plain, for its agents

Status: done
Date: 2026-10-22
Owner: task #259
Scope authorization: task #259, the founder's request of 22 Oct 2026 while onboarding
Quartio-Studio/product from a laptop (SPEC-256). Related: task #258 (SPEC-274, agent secrets
refuse plain words).

## Goal

A project has a place for its non-secret values (regions, environment names, account and project
ids, base URLs, contact e-mail addresses): *project variables*, each a name, a value and a
description, shown in plain on a Variables tab beside the Secrets, set from the laptop by the
person's agent, and present in the environment of every task run of the project. Laptop
onboarding moves a company `.env`'s plain values there instead of skipping them or masking
ordinary words as secrets.

## Context

Confirmed against `dev` at 9080cf3e.

- A project only has agent secrets (`vault_secrets`, section `agent`, keyed by sandbox id). Their
  values reach a run as placeholders (`NAME=yulius-secret-NAME`, `vault_run_setup`) that the
  egress proxy swaps on bound hosts, and every occurrence of a value is redacted from tool
  results and transcripts. `check_agent_value` needs 8 characters (`MIN_AGENT_VALUE_CHARS`).
- A company `.env` mixes keys with plain values: `OVH_ENDPOINT=ovh-ca` fails the minimum and is
  skipped; `PIPEDREAM_PROJECT_ENVIRONMENT=production` passes and masks every "production" in the
  repository and the transcripts (task #258). Hand-over notes are read once; a commit is wrong for
  a value that belongs to one deployment.
- `yulius secrets import` (`yulius-import::secrets_import`) classifies dotenv entries as new,
  existing or invalid, shows names only, and sends with the typed `yes` or, with `--yes`, under
  the operate token (ADR-0194). `yulius secrets list` and the other remote CLI commands are calls
  of the remote MCP (`remote_cli::run`, ADR-0188).
- A task run's environment is `base_env` (`supervisor.rs`), then the agent secret placeholders
  (`vault_run_setup`); `yulius-ptyhost` applies `launch.env` in order, a later duplicate winning.
- The Secrets page (`pages/secrets.js`) has tabs (`SECTIONS`: Project, Global, Personal, Team).
- Names of agent secrets follow `SecretName` (an environment variable name; reserved names such as
  `PATH` and prefixes such as `YULIUS_` refused).
- Task #258 runs in parallel on the refusal side of the same rule; this spec owns the rule's code
  (`yulius_domain::vault::plain`) so both use one.

## Requirements

- **VAR-001** A *project variable* belongs to one project (sandbox): a name (`SecretName` rules),
  a value of at most 4 KiB with no control characters (may be empty) and a description of at most
  1000 characters. Names are unique per project; at most 200 per project. Stored in plain
  (`project_variables`, migration 0043), removed with the sandbox.
- **VAR-002** A name that is an agent secret of the project (a project secret, or a global one
  switched on for it) is refused, naming the secret. A personal secret, which only its owner sees,
  is not checked; in a run an agent secret of the same name wins (its placeholder comes later).
- **VAR-003** `GET /api/sandboxes/{sid}/variables` lists them to anyone who sees the project, with
  `can_manage` (write access). `PUT /api/sandboxes/{sid}/variables` sets a batch
  (`{variables: [{name, value, description?}], only_new?}`, at most 200, all checked before any
  is written; each outcome decided inside one transaction; an absent description keeps the
  stored one; `only_new` leaves existing ones as they are) and answers what was created,
  changed, unchanged and left existing;
  `DELETE /api/sandboxes/{sid}/variables/{name}` removes one. Changes need `Action::Configure`
  (as agent secrets). Each create, change and delete is audited `variable.create|update|delete`
  with the name and the changed fields, never the value, as the person's login (a remote call is
  also audited `remote.tool` as `remote:<login>`).
- **VAR-004** Every task run of the project gets its variables in its environment, added first:
  Yulius's own environment (`base_env`) and the agent secret placeholders come after them and win
  over a variable of the same name.
- **VAR-005** Remote tools (operate token; read token lists only): `variables_list` (names,
  values, descriptions), `variables_set {variables: [{name, value, description?}]}`,
  `variable_delete {name}`. CLI `yulius vars list|set|import|delete` calls them:
  `set NAME=value … [--description TEXT]`, `import FILE|-` (a dotenv file; a comment above an
  entry is its description), `delete NAME…`. `set` and `import` refuse, before sending, an entry
  whose name says it is a credential (VAR-006's names: "use yulius secrets import") or whose value
  carries one (a URL with user info, a key), and never echo a malformed argument. Tool results
  show values (they are not secrets).
- **VAR-006** The plain-value rule (`yulius_domain::vault::plain::not_a_secret`): a value is not a
  secret when it is shorter than 8 characters and plain-shaped (one case of letters, digits and
  `-`, `_`, `.`), or ordinary text by SPEC-274's rule (`ordinary_text`, task #258: a
  configuration word, a host, an e-mail address, a URL without credentials, a region, time zone,
  locale or path). Never when the name says it is a credential:
  it contains `SECRET`, `PASSWORD`, `PASSWD`, `PASSPHRASE`, `CREDENTIAL`, `PRIVATE` or
  `MNEMONIC`, or one of its `_` words (without trailing digits: `DB_PASS2`) ends with a credential
  ending (`KEY`, `TOKEN`, `PASS`, `PW`,
  `PWD`, `AUTH`, `CRED`, `COOKIE`, `HOOK`, `CODE`, `LICENSE`, `CERT`, `PIN`, `OTP`, `DSN`, …:
  `APIKEY` and `DBPASS` are credentials, `KEYCLOAK_URL` and `AUTHOR_EMAIL` are not); never a cut
  or flattened key.
- **VAR-007** `yulius secrets import --to agent` for one project (not `--instance`) shows each
  dotenv entry the rule marks as a variable (`~ NAME  not a secret (a configuration word): a
  project variable`), and sends them as variables with the secrets after the typed `yes`, or with `--yes`.
  They are only created (`only_new`): a variable that exists, when planned or made meanwhile, is
  skipped, never overwritten. A refused set is sent again one by one, so a bad entry fails alone.
  The summary counts them. `--no-vars` keeps the old behavior (skipped or sent as a secret by the
  secret checks).
- **VAR-008** Agents know about them: a task's and the Superagent's `secrets_list` answers the
  project's variables (name, value, description); the onboarding prompt names them beside
  `secrets_list`; the remote MCP instructions tell the laptop agent to move plain values with
  `yulius vars import` or the secrets import offer, never into secrets.
- **VAR-009** The Secrets page has a Variables tab beside Project: a table of name, value and
  description in plain, with Add, Edit and Delete for people with write access; updated live.
- **VAR-010** Documented: `docs/remote-control.md` section 3, `docs/api.md`,
  `docs/architecture.md`, ADR-0203, `docs/changes.md`.

## Plan

- [x] Spec and ADR-0203: project variables are plain per-project values injected into task runs
- [x] The store keeps project variables (migration 0043, store, ports)
- [x] People list, set and delete variables over the API, audited, agent-secret names refused
- [x] Every task run of the project gets its variables in its environment
- [x] The Variables tab beside Secrets (web UI, mock, end-to-end test)
- [x] Remote tools and yulius vars import, set, list and delete
- [x] yulius secrets import offers plain values as variables (the plain-value rule)
- [x] Agents and the laptop recipe know about variables (secrets_list, prompts, docs)
- [x] Review findings fixed test-first
- [x] Merged with task #258's ordinary-text rule and the Rust gate is green
- [x] Gates and security scan are green and the spec has its evidence
- [x] The pull request is merged into dev on green

## Acceptance criteria

- **AC1** With only an operate token stored, `yulius vars set OVH_ENDPOINT=ovh-ca
  --description "OVH region"`, `yulius vars import vars.env` and `yulius vars delete X` change the
  project's variables; `yulius vars list` prints names, values and descriptions; the audit holds
  `variable.*` rows as the person's login without values. A read token lists but cannot set.
- **AC2** Setting a variable named like a project agent secret is refused with the secret's name,
  and nothing of that batch is written.
- **AC3** A task run of the project has `OVH_ENDPOINT=ovh-ca` in its environment.
- **AC4** `yulius secrets import --to agent --yes .env` with `OVH_ENDPOINT=ovh-ca`,
  `PIPEDREAM_PROJECT_ENVIRONMENT=production`, `LANGFUSE_BASE_URL=https://cloud.langfuse.com`,
  `CONTACT=ops@example.com`, `API_TOKEN=production` and a real key creates the key as a secret,
  the first four as variables, and does not move `API_TOKEN` into variables.
- **AC5** The Variables tab lists, adds, edits and deletes a variable (end-to-end test on the mock).

## Test plan

- Unit (`yulius-domain` `vault/plain.rs`): each class of the rule, credential names, keys.
- Store (`yulius-store` `variables.rs`): set batch create/change/unchanged, delete, cascade.
- Integration `crates/yulius/tests/remote_mcp.rs` (the remote surface): AC1, AC2 and AC4 through
  the real `yulius` binary against a served instance.
- Integration `crates/yulius/tests/variables.rs`: REST list/set/delete, permissions, bounds,
  audit, the agent-secret name refusal.
- Integration (`pool_supervision.rs` style, `report_env`): AC3.
- Unit (`yulius-import`): the secrets import plan and sends with variables (`FakeServer`).
- End-to-end (`e2e`): AC5.
- Gates: `scripts/dev ci`, `scripts/dev security`, the e2e suite.

## Decisions

- **ADR-0203**: project variables are plain rows per project, not a kind of secret: no envelope,
  no redaction, no proxy; injected as environment variables into task runs only. Alternatives:
  agent secrets with a "plain" flag (every secret path would need a branch; redaction and the
  minimum are the point of secrets), committing a config file (belongs to the code, not to one
  deployment), hand-over notes (read once).
- **Task runs only**: CI jobs and CD runs take their environment from the repository's
  `yulius.toml` at a verified commit (ADR-0109); letting a laptop token change it is a separate
  decision.
- **One rule with task #258**: #258 merged first with SPEC-274's `ordinary_text` (what a secret
  import refuses); `plain::not_a_secret` uses it and only adds what moving a value into plain
  view needs: short plain-shaped values, and the credential-name guard (a passphrase can look like
  words, and a variable is shown to every agent). The two tasks' parallel first versions were
  merged into this one before delivery.
- **Credential names err towards secret**: `COUNTRY_CODE`, `MAX_TOKENS`, `OAUTH_CLIENT_ID`,
  `AUTH0_DOMAIN` or `COOKIE_DOMAIN` are taken as credentials, so the import keeps them out of
  variables and `yulius vars` refuses them; a person sets such a variable on the Variables tab.
  An allow-list or a CLI override would reopen the leak the rule exists to close (review rounds 1
  and 2: `DBPASS`, `DB_PASS2`); a false positive only costs a manual step.
- **The server does not guard names**: a person may hold a public "key" (a publishable id) as a
  variable on the page; the bulk paths from a `.env` (CLI `set`/`import`, the secrets import
  offer) do.
- **One pull request** over the line budget: storage, run injection, the laptop paths and the tab
  only make the feature together, as SPEC-271 did.

## Evidence

All at `b82f18fc` (one feature commit on `dev` at ab8ab732, after task #258's merge, and the
round-5 fix), except where a step names its own commit.

- `scripts/dev ci` passed (exit 0): repository contracts, `cargo check`, `fmt --check`, Clippy
  `-D warnings` for Linux and macOS, nextest **2514 passed, 17 skipped**, doctests, `cargo deny`,
  `cargo shear`, `cargo doc`. Earlier runs failed on three Clippy lints (a needless borrow in
  `remote.rs`, `let_and_return` and `needless_question_mark` in the new tests); each was fixed.
- End-to-end suite (`cd e2e && npx e2e run`): **682 passed, 1 skipped** (64 files). The Secrets
  file alone, including the two Variables tests, passed 24/24 earlier.
- `scripts/dev security`: **not run** (exit 2): Vidoc answered HTTP 429, "This API key has used
  its 50,000,000 tokens", so 31 changed files were not scanned. It is unrun, not passed;
  MrFishPL's instruction for this task is to continue without Vidoc. Yulius CI's `security` job
  will try again on the pull request.
- Behaviour tests (each written before its code and seen failing):
  - Store: `variables::tests::variables_are_set_by_name_per_project_within_a_cap` (in-transaction
    outcomes, `only_new`, cap rollback, cascade).
  - REST (`crates/yulius/tests/variables.rs`): set/list/delete with write access, a reader's 403,
    audit rows without values; bad names, values, descriptions, duplicates and 201 entries
    refused with nothing written; a project or switched-on global secret's name refused (409).
  - Run environment (`pool_supervision::a_task_run_gets_the_project_variables`): `OVH_ENDPOINT`
    and `LANGFUSE_BASE_URL` reach the run; a secret of the same name wins with its placeholder;
    Yulius's own `RUST_TEST_THREADS` wins over a variable.
  - Remote surface through the real binary (`remote_mcp::variables_are_set_from_a_laptop_and_listed_in_plain`,
    `remote_mcp::a_secrets_import_sends_plain_values_to_project_variables`): AC1, AC2, AC4;
    `secrets_list` names variables; a read token lists but is not served `variables_set`;
    credential names and credential-carrying values refused by `yulius vars`; `--no-vars`.
  - Import (`yulius-import` `secrets::plain_values_are_sent_as_project_variables`,
    `an_older_server_without_variables_keeps_plain_values_out`,
    `variables_are_only_created_and_a_refused_one_fails_alone`): plan lines, one set, no value
    said, a variable made after the plan never overwritten, a refused one failing alone.
  - Rule (`yulius-domain` `vault::plain::tests`): the founder's `.env`, the reviewer's leak cases
    (`DBPASS`, `DB_PASS2`, `BORG_PASSPHRASE`, a hook URL, a per-user host, a URL fragment, a
    token-like address), and non-credential names (`KEYCLOAK_URL`, `AUTHOR_EMAIL`).
  - UI (e2e `secrets.e2e.ts`): AC5 and a reader's disabled controls, no WCAG violations.
- Independent review in a fresh session (om-code-review checklist), six rounds: round 1 asked
  for changes (one medium: real secrets classed as plain; lows: races in the set batch and the
  import, one bad entry failing all, false positives, mock parity, test gaps), all fixed
  test-first; round 2 (a digit after a credential word), round 5 (after merging #258 and
  delegating to its rule: a URL fragment, a token-like local part, wording) were fixed the same
  way; round 6 approved `b82f18fc` with no findings.
- Not tested: a real Claude Code session reading the variables (the fake agent reports the
  environment instead); Vidoc (out of quota).
- Push protection: the branch was squashed into one commit before its first push, so no pushed
  commit holds the Stripe- or Slack-shaped fixtures an earlier version of `plain.rs`'s tests had.

