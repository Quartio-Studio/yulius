---
yulius-task: 330f7aba-3e9d-4301-8d68-02be3cb652a3
status: in-progress
---

# SPEC-178: The Superagent reads the agent vault: names, and whether a value matches

Status: in-progress
Date: 2026-10-15
Owner: Yulius task #165
Scope authorization: task #165: "superagent should have access to agent's vault as well", with a
screenshot of the Superagent answering "I can't access vault secrets or compare them to what you
pasted here" after the person asked it to check that the vault's secrets are there, "names only,
no values", and to "especially confirm multiline" (a three-line `…_FOUNDER_PRIVATE_KEY`).

## Goal

A person asking the Superagent about the project's agent secrets gets the answers a task agent
gets: which agent secrets the project has (names, descriptions, hosts, never values), and whether
a value they hold in a file is the one in the vault, multi-line keys included, without the value
ever reaching the model or the screen.

## Context

- The Superagent (the Cockpit, SPEC-021) is served only `COCKPIT_TOOLS`, the 15 task and
  automation tools (SPEC-037 MGT-006, SPEC-066 MGT-012); `secrets_list` and the other vault tools
  are refused to it as unknown (`crates/yulius-app/src/mcp.rs`).
- Task agents get `secrets_list` (names, descriptions, bindings, placeholders; VLT-005),
  `secret_request_hosts` (SPEC-121), `secret_copy_to_env` (SPEC-129) and `browser_fill_secret`.
  Their values reach requests through the egress proxy and never the agent.
- Since SPEC-147 (ADR-0121) the Superagent has the sandbox's MCP servers and the task runs'
  redaction: a value a command prints shows as `[secret:NAME]`. That is an equality check of a
  kind, but per line (needles never hold a line break) and per encoding, so it cannot say whether
  a whole multi-line key matches, nor tell "differs by a trailing newline" from "differs".
- The Superagent is not behind the egress proxy (ADR-0121), has no worktree of its own and no
  browser tools; it runs as the person, whose own terminal reads their home unredacted. Agent
  secret values are shown to nobody without a two-factor unlock (ADR-0042).

## Requirements

- VAU-001: The Superagent is served `secrets_list` for its sandbox: the same records a task agent
  of that sandbox gets (name, description, scope, hosts, requested hosts, fields, `.env` state),
  never a value. Its `how_to_use` is the Superagent's own: it is not behind the proxy, so it has no
  placeholders in its environment; values are verified with `secret_verify`; a person adds, binds
  and reveals secrets on the Secrets page (link given); using a secret in requests, the project
  browser or `.env` belongs in a task.
- VAU-002: `secret_verify {secret, sha256}` is served to task agents and to the Superagent. Given
  the SHA-256 (64 hex digits) of a candidate value and the name of an agent secret that applies to
  the caller's sandbox, it answers whether the candidate is the secret's value: `exact`, or a match
  once a difference common in copied keys is set aside (`trailing_newline`: the candidate ends
  with a line break; `escaped_newlines`: the candidate has line breaks, LF or CRLF, where the
  vault's value has literal `\n`, as a multi-line key must be stored since an agent secret never
  spans lines), or `different`. It also says how many lines the value stands for (its literal
  `\n` read as line breaks). It never returns a value, a
  value's hash or anything else derived from it. An unknown secret, a malformed hash and a
  production secret (SPEC-134) are refused. Each distinct check is audited once
  (`vault.agent.verify`, with the task or `cockpit:<login>` as actor) while Yulius runs.
- VAU-003: Both tools run in the Superagent without asking the person (they change nothing but
  the audit): they join `COCKPIT_READ_ONLY_TOOLS`. The Superagent's MCP instructions name them.
  The other vault tools stay a task's: `secret_request_hosts`, `secret_copy_to_env` and
  `browser_fill_secret` are still refused to the Superagent as unknown.
- VAU-004: ADR-0144 amends SPEC-037 MGT-006 and records why the Superagent gets reading and
  verifying but not the proxy, `.env` copies or host requests; SPEC-037, the product summary, the
  architecture document, `docs/api.md` and the changes log say what the Superagent gets.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The Superagent lists its sandbox's agent secrets with secrets_list
- [ ] secret_verify says whether a candidate's SHA-256 matches an agent secret's value
- [ ] The Superagent runs both vault tools without asking; ADR-0144 and the docs say what it gets
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- `crates/yulius/tests/cockpit.rs`: with agent secrets in the Superagent's sandbox and another
  one, the Superagent's `tools/list` holds `secrets_list` and `secret_verify` but not
  `secret_copy_to_env`, `secret_request_hosts` or `browser_fill_secret`; `secrets_list` names its
  sandbox's secrets only, with no value, and its `how_to_use` names `secret_verify` and the Secrets
  page; `secret_copy_to_env` is refused as unknown.
- With a three-line key stored with literal `\n`, `secret_verify` answers `exact` for the SHA-256
  of the stored value, `trailing_newline` for it with a final line break, `escaped_newlines` for
  the key with real line breaks (LF or CRLF, with or without a final one), and `different`
  otherwise, with `lines: 3`; it
  refuses an unknown name, a hash that is not 64 hex digits, and a production secret; the check is
  audited with the caller as actor; no answer holds the value or its SHA-256.
- A task agent's `secret_verify` answers the same for its own sandbox, and nothing for another
  sandbox's secret.
- `COCKPIT_READ_ONLY_TOOLS` holds both tools, and the harness test plans them as allowed.

## Test plan

- Unit tests in `crates/yulius-app/src/vault/` for the comparison (each kind, a value without
  escapes, a candidate that is only close) and in `mcp.rs` for the Superagent's tool list.
- Integration tests in `crates/yulius/tests/cockpit.rs` (the Superagent over its `ck_` token) and
  `crates/yulius/tests/secrets.rs` (a task agent), using synthetic values only.
- Existing `secrets.rs`, `secrets_env.rs`, `manage_tools.rs` and harness launch tests keep passing.

## Decisions

- ADR-0144: the Superagent reads the vault (names) and verifies values by hash, rather than
  getting the proxy, `.env` copies or host requests. The proxy would route all of the person's
  traffic through Yulius (ADR-0121 left that as a decision of its own); `.env` copies would put
  values where the person's plain terminal reads them without a two-factor unlock; host requests
  and browser fills need a task's flags and browser. Those stay a task's.
- Verification by the candidate's SHA-256, not the candidate itself: the Superagent computes it in
  a shell (`sha256sum`), so the value reaches neither the model nor the screen. The answer is an
  equality oracle, which the redaction (and the salted fingerprints any process of the person
  already reads) already gives; it adds no way to learn an unknown value, only exactness.
- Lenient kinds are computed on the vault side (variants of the stored value), because Yulius
  never receives the candidate. An agent secret holds no control characters
  (`check_agent_value`), so a multi-line key is stored with literal `\n`: the variants are the
  value with a final LF or CRLF, and the value with its `\n` as LF or CRLF line breaks, with or
  without a final one. CRLF against LF inside the vault cannot occur and is not a kind.
- The line count the value stands for is returned: it is what the person asked to confirm and says
  next to nothing about the value.
- `secret_verify` is served to task agents too ("as well" goes both ways): the same check helps an
  agent confirm a key it was given before asking a person to store it.
- A production secret is refused, as `secret_request_hosts` and `secret_copy_to_env` refuse one:
  production values are kept out of every agent use.
