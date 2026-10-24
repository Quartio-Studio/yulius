---
yulius-task: b8e293d1-1e32-498b-9e2f-a185f43bd27c
status: done
---

# SPEC-186: The Superagent uses the agent vault like a task agent, and sees new tools without a restart

Status: done
Date: 2026-10-16
Owner: Yulius task #171
Scope authorization: task #171, requested by MrFishPL: task #165 (SPEC-178, ADR-0144) was
delivered too narrowly; "a Superagent has the same access to the agent vault as a task agent",
and a Superagent that was already running gets new tools without the person restarting it. The
person's decision overrides ADR-0144's "not now / rejected" alternatives.

## Goal

A person's Superagent uses the project's agent secrets as a task's agent does: requests to bound
hosts carry them through the egress proxy, `secret_copy_to_env` puts allowed ones into an ignored
`.env`, `secret_request_hosts` asks a person to bind hosts, all without a value reaching the model
or the screen. Multi-line keys (PEM, OpenSSH) stored with literal `\n` come out of a `.env` copy
byte-identical to the original file. A Superagent that was running before an update that changes
what it is served gets the new tools and environment by itself.

## Context

- 2026-10-16: a person moved 14 deploy secrets (API tokens, two Ed25519 PEM keys, two OpenSSH
  private keys) into a project's agent vault and asked its Superagent to use and check them. It
  saw no secret tools and no variables: it had been started before #165's update, and Claude Code
  reads the MCP tool list once when it connects (a `--resume` keeps the connection's view).
- ADR-0144 (SPEC-178) serves the Superagent `secrets_list` (its own guidance, no placeholders) and
  `secret_verify` only; `secret_copy_to_env`, `secret_request_hosts` and `browser_fill_secret` are
  refused as unknown, and the Superagent is not behind the egress proxy (ADR-0121).
- Task agents: the proxy (`HTTPS_PROXY` with the run's task token, the egress CA bundle,
  placeholders `NAME=yulius-secret-NAME`) adds a secret to requests for its bound hosts
  (`vault/runtime.rs`, `EgressPolicy`); `secret_copy_to_env` writes an allowed secret into a
  git-ignored dotenv file of the task's worktree (SPEC-129, `vault/env_files.rs`), asking a person
  first (`env_files`); `secret_request_hosts` records hosts for a person to allow (SPEC-121); a
  production secret (SPEC-134) is refused by all of them.
- An agent secret never spans lines (`check_agent_value`), so a multi-line key is stored with
  literal `\n`. `dotenv::line` refuses any value holding `\`, so such a key cannot be copied into a
  `.env` at all today, by a task or anyone.
- The Superagent (`cockpit.rs`) runs as its person in the sandbox repository (no worktree), with
  its own `ck_` MCP bearer, the task runs' redaction (ADR-0121) and a server-side redacted screen.
  It has no browser tools. Its process survives a Yulius restart and is adopted again from its
  shell record (`shells.rs`).
- Claude Code 2.1.292 (inspected in its binary, 2026-10-16) handles `notifications/tools/list_changed`
  only for a server whose `initialize` result advertised `tools.listChanged`, and gets server
  messages from a `GET /mcp` stream it reconnects at most twice. Yulius answers `GET /mcp` with 405
  and advertises `listChanged: false`, so no session running today would act on a notification.

## Requirements

- SAV-001 (proxy): A Superagent started by Yulius with the egress proxy wired runs behind it as a
  task run does: `HTTP(S)_PROXY` with its own `ck_` bearer (secret environment), `NO_PROXY`, the
  CA bundle (a file in its shell's run directory, bound read-only into a contained session) and a
  placeholder variable per agent secret of its sandbox that its environment does not already set.
  The proxy accepts a live Superagent's bearer as a client of its sandbox (owner enabled and able
  to work there, checked as its MCP calls are; cached like task tokens), actor `cockpit:<login>`,
  audited once per Superagent process, secret and host; it never uses a production secret (SPEC-134).
  An ended Superagent's bearer is refused.
- SAV-002 (`.env`): `secret_copy_to_env` is served to the Superagent. Its target folder is the
  sandbox repository the Superagent runs in, with every check a task's copy has (dotenv name, no
  links, inside the folder, Git ignores and does not track it, production refused, all or
  nothing). A secret no person allowed is asked for (`env_requested_by`: `<login>'s Superagent`,
  audited with the Superagent as actor) and nothing is written; the answer gives the Secrets page
  link. Copies are audited `vault.agent.env` with the Superagent as actor.
- SAV-003 (hosts): `secret_request_hosts` is served to the Superagent: the request is recorded as
  a task's is, asked by `<login>'s Superagent`, audited with the Superagent as actor; no task is
  flagged. A production secret is refused.
- SAV-004 (browser): The Superagent has no project browser tools, so it is not served
  `browser_fill_secret`; sign-ins in the project browser stay a task's (the closest equivalent
  is a task).
- SAV-005 (multi-line keys): A value that is a key block (it starts with `-----BEGIN `, its
  literal `\n` read as line breaks give lines whose last is `-----END …-----`, and no other `\`
  remains) is written to a `.env` as a single-quoted multi-line value whose text is the key with
  its line breaks and one final line break, so dotenv readers and `sh` read the original file
  byte for byte, whether or not the stored value ends with a literal `\n`. Copying it again (after a
  rotation) replaces the whole multi-line value. Any other value holding `\` is still refused.
  Through the proxy a value is sent as stored (an HTTP header cannot carry line breaks).
  Its base64 lines are redaction needles, not its headers or blank lines (SPEC-302, ADR-0215).
  Tested with a 3-line PEM key and a 7-line OpenSSH key (synthetic).
- SAV-006 (new tools without a restart): Each Superagent records what it was started with, a
  digest of its MCP view (instructions, tools), whether the egress proxy was wired, and a
  revision constant bumped when its launch otherwise changes (environment, settings, the tools
  it runs without asking). An adopted Superagent whose
  record differs from what this Yulius would start, or has none (started before this change), is
  restarted once its conversation is idle (no turn open, no background work), resuming its newest
  conversation on a subscription with capacity; its viewers are told why and the panel follows it
  (`switched`). A Superagent mid-turn waits for the turn to end. The person restarts nothing.
- SAV-007 (briefing): The Superagent's MCP instructions and `secrets_list` guidance say what it can
  do with the vault: call bound hosts normally, use `$NAME` or the placeholder, copy allowed
  secrets into the repository's ignored `.env`, ask for hosts, verify a value with `secret_verify`;
  values never reach it.
- SAV-008 (records): ADR-0150 supersedes ADR-0144's "the rest stays a task's" and its rejected or
  deferred alternatives (proxy, `.env`, host requests) and records the refresh; SPEC-178, SPEC-037,
  the product summary, the architecture document, `docs/api.md` and the changes log say what the
  Superagent gets.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Multi-line keys reach a .env byte-identical to the original file
- [x] The Superagent copies allowed secrets into its repository's ignored .env and asks for hosts
- [x] The Superagent runs behind the egress proxy with its own bearer and placeholders
- [x] A Superagent started before an update restarts and resumes once idle with the new tools
- [x] The Superagent's briefing, ADR-0150 and the docs say what it can do with the vault
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- `secrets_env.rs`: a task copies a 3-line PEM key and a 7-line OpenSSH key, stored with literal
  `\n` (one with a final `\n`, one without), into `.env`; `sh` sourcing the file prints each
  byte-identical to the original file; copying again after a rotation replaces the whole value.
- `cockpit.rs`, fresh Superagent: its `tools/list` holds `secrets_list`, `secret_verify`,
  `secret_copy_to_env` and `secret_request_hosts` (not `browser_fill_secret`); it verifies the
  multi-line key; `secret_copy_to_env` asks for an unallowed secret (asked by "alice's
  Superagent", nothing written) and, once allowed, writes the key into the sandbox repository's
  ignored `.env` byte-identical through `sh`; it refuses an unignored file and a production
  secret; `secret_request_hosts` records the hosts; its launch environment holds the proxy with its
  own bearer, the CA bundle and the placeholders; the proxy accepts its bearer, adds a bound
  secret to a request for the bound host (a multi-line one as stored), uses no production secret,
  and refuses the bearer once the Superagent ended.
- `cockpit.rs`, already running: a Superagent whose shell record predates this change is adopted
  after a restart, restarted with `--resume` of its conversation, and the new one passes the same
  checks (tools, proxy environment); one mid-turn is not restarted until its turn ends; one whose
  record matches is left running.
- No real secret value in any test, fixture, the spec, the PR or an issue.

## Test plan

- Unit tests: `yulius-domain` `vault::dotenv` (key blocks: detection, quoting, final line break,
  non-keys still refused; `upsert` replacing a multi-line value), `yulius-app` `mcp` (the
  Superagent's tool list), `vault/requests.rs` (requester labels), the Cockpit's launch digest.
- Integration tests in `crates/yulius/tests/cockpit.rs` (real ptyhost, fake `claude`, the
  Superagent's own `ck_` token, the egress policy) and `crates/yulius/tests/secrets_env.rs`.
- Existing `cockpit::`, `secrets::`, `secrets_env::`, `manage_tools::` and harness tests keep
  passing; `scripts/dev ci`.

## Decisions

- ADR-0150 records the lasting choices; summary:
- The Superagent gets the proxy with its own bearer rather than the person's workspace token: the
  bearer lives and dies with the Superagent process, as a task token with its run, so the person's
  terminals stay outside the proxy and an ended Superagent's bearer injects nothing.
- `.env` copies go to the sandbox repository, where the Superagent works (it has no worktree), with
  the same allowance: an `env_files` allowance already lets every agent of the project put that
  value into an ignored file of the sandbox, and the Superagent is one more of them; the person's
  own terminal reads such files in task worktrees already, and values stay redacted on the
  Superagent's screen and in its tool results.
- The two tools that change state (`secret_copy_to_env`, `secret_request_hosts`) are not added to
  the tools the Superagent runs without asking: Claude Code asks the person in front of it first.
- New tools without a restart: restart-and-resume of an idle Superagent after an update rather
  than `notifications/tools/list_changed`, because sessions started before this change never
  advertised for it (Claude Code registers the handler only when `initialize` said
  `listChanged: true`), its notification stream reconnects only twice so an update longer than
  that would lose it, and the proxy environment can only change with a new process anyway.
- Key blocks are recognised by their PEM armour, so a value with literal `\n` that is not a key
  (a JSON service-account file kept on one line) keeps being refused rather than having its
  escapes rewritten. The final line break is added because PEM and OpenSSH files end with one
  (OpenSSH refuses a key without it).
- Independent review round 1 (REQUEST CHANGES; no value or production-secret leak found; the
  reviewer read a key-block `.env` back byte for byte with `sh`, Node `dotenv` 16, `python-dotenv`
  1.2.4 and Docker Compose; all own code, all fixed): the proxy's refusal of a Superagent whose
  person was disabled or lost access, and of another sandbox's secrets, was untested (medium: now
  in `the_superagent_runs_behind_the_egress_proxy_with_its_own_bearer`; the lost-access case
  waits out the access decisions' staleness bound, as the MCP calls do); `serve` adopted shells
  before wiring the proxy, so a Superagent restarted at once could start without it and never be
  restarted again (now wired first, and the launch digest holds whether the proxy is wired:
  `a_superagent_started_without_the_proxy_restarts_once_it_is_wired`, red before); a full context
  did not end a turn for the refresh (now as `finish.rs` reads it, unit test red before); a prompt
  sent between the idle check and the end could be cut (now the transcript's length is checked
  again right before the end, unit test red before); nits: the spec's digest wording, "worktree"
  in the Superagent's refusals and the `path` description, key armour labels must match (unit
  case red before), a one-element test loop.
- Independent review round 2 (APPROVE at `974423ee`, every round-1 fix confirmed, the targeted
  tests stress-run 4 times): own-code nits fixed in the next commit (the transcript's length is
  read before its poll, so all of it was observed; the ADR index names the proxy in the digest;
  the restart-once-wired test checks the new process's proxy). Left as it is: the lost-access and
  disabled cases do not first prove a proxy client existed, since that would fill the proxy's
  15-second cache the refusal must bypass; their Superagents starting shows they could work.

## Evidence

- Red before green, each with the change absent or stubbed: `dotenv` unit tests (key blocks,
  whole replacement, matching labels), `secrets_env::multi_line_keys_reach_the_env_byte_identical_to_their_files`
  (the key lines shown unredacted), `cockpit::the_superagent_copies_allowed_secrets_into_its_repositorys_ignored_env`
  ("unknown tool"), `cockpit::the_superagent_runs_behind_the_egress_proxy_with_its_own_bearer`
  (no proxy in its environment), `cockpit::a_superagent_started_before_an_update_restarts_resuming_its_conversation`
  and `cockpit::a_superagent_mid_turn_restarts_only_once_its_turn_ends` (adoption never marking a
  refresh due; the mid-turn one also with every turn treated as idle),
  `cockpit::a_superagent_started_without_the_proxy_restarts_once_it_is_wired`, the `cockpit.rs`
  unit tests (`ContextFull`, a transcript that grew), `vault::runtime` read-only bind (mutation).
- `scripts/dev ci` (contracts, check, lint, nextest, doctests, deps): exit 0 at `172c90e1` (1918
  passed, 18 skipped) and at `974423ee` (1920 passed, 18 skipped); the last commit (a read moved
  before a poll, a test assertion, an index line) is covered by the targeted `cockpit::` runs (37
  passed) and the final gate: exit 0 at `ecd514c0` (1920 passed, 18 skipped). Review round 3
  (APPROVE at `ecd514c0`) asked only for this line.
- e2e (`npx e2e run`, the Superagent panel's text changed): 568 passed, 1 skipped.
- Reviewer's own check: a key-block `.env` read back byte for byte by `sh`, Node `dotenv` 16,
  `python-dotenv` 1.2.4 and Docker Compose (`env_file` and `${KEY}`).
- `scripts/dev security`: not run (exit 2): Vidoc answers HTTP 401 for the stored key (#470,
  ADR-0104); unrun, not passed.
- Not tested here: a real Claude Code Superagent (the integration tests drive the real ptyhost
  with a fake `claude`, the MCP endpoint with the Superagent's own `ck_` token and the egress
  policy directly, not a TLS round trip through the proxy to a real host), and an update of a real
  instance with a Superagent open.
