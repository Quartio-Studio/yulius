---
yulius-task: 330f7aba-3e9d-4301-8d68-02be3cb652a3
status: done
---

# SPEC-178: The Superagent reads the agent vault: names, and whether a value matches

Status: done; VAU-003's refusal of `secret_copy_to_env` and `secret_request_hosts` and VAU-004's
"not the proxy, `.env` copies or host requests" are superseded by SPEC-186 (ADR-0150).
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
  never a value, and without the proxy's placeholder and injection, which it cannot use. Its `how_to_use` is the Superagent's own: it is not behind the proxy, so it has no
  placeholders in its environment; values are verified with `secret_verify`; a person adds, binds
  and reveals secrets on the Secrets page (link given); using a secret in requests, the project
  browser or `.env` belongs in a task.
- VAU-002: `secret_verify {secret, sha256}` is served to the Superagent only (not to task agents,
  whose transcripts every member who can view the project reads). Given
  the SHA-256 (64 hex digits) of a candidate value and the name of an agent secret that applies to
  the caller's sandbox, it answers whether the candidate is the secret's value: `exact`, or a match
  once a difference common in copied keys is set aside (`trailing_newline`: the candidate ends
  with a line break; `escaped_newlines`: the candidate has line breaks, LF or CRLF, where the
  vault's value has literal `\n`, as a multi-line key must be stored since an agent secret never
  spans lines), or `different`. On a match it also says how many lines the value stands for (its
  literal `\n` read as line breaks). Every variant is compared in constant time, without an early
  exit, and no copy of the value is made to hash it. `sha256sum`'s own line (`<hash>  <file>`) is
  read as its hash. It never returns a value, a value's hash or anything else derived from it;
  the candidate's hash the caller sends stays in the Superagent's conversation, which is its
  person's own. An unknown secret, a malformed hash and a
  production secret (SPEC-134) are refused. Each distinct check is audited once per vault
  generation and sandbox (`vault.agent.verify`, with `cockpit:<login>` as actor) while Yulius
  runs, so a check repeated after a rotation is audited again; the once-cache keeps a digest of
  the candidate, never the candidate's hash itself.
- VAU-003: Both tools run in the Superagent without asking the person (they change nothing but
  the audit): they join `COCKPIT_READ_ONLY_TOOLS`. The Superagent's MCP instructions name them.
  The other vault tools stay a task's: `secret_request_hosts`, `secret_copy_to_env` and
  `browser_fill_secret` are still refused to the Superagent as unknown.
- VAU-004: ADR-0144 amends SPEC-037 MGT-006 and records why the Superagent gets reading and
  verifying but not the proxy, `.env` copies or host requests; SPEC-037, the product summary, the
  architecture document, `docs/api.md` and the changes log say what the Superagent gets.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The Superagent lists its sandbox's agent secrets with secrets_list
- [x] secret_verify says whether a candidate's SHA-256 matches an agent secret's value
- [x] The Superagent runs both vault tools without asking; ADR-0144 and the docs say what it gets
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Review round 2 findings fixed test-first: secret_verify is the Superagent's only
- [x] Review round 3 findings fixed test-first: the rotation re-audit is proven
- [x] Review round 4 confirms the round-3 fixes on the head
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- `crates/yulius/tests/cockpit.rs`: with agent secrets in the Superagent's sandbox and another
  one, the Superagent's `tools/list` holds `secrets_list` and `secret_verify` but not
  `secret_copy_to_env`, `secret_request_hosts` or `browser_fill_secret`; `secrets_list` names its
  sandbox's secrets only, with no value, and its `how_to_use` names `secret_verify` and the Secrets
  page; `secret_copy_to_env` is refused as unknown.
- With a three-line key stored with literal `\n`, `secret_verify` answers `exact` for the SHA-256
  of the stored value, `trailing_newline` for it with a final line break, `escaped_newlines` for
  the key with real line breaks (LF or CRLF, with or without a final one), and `different`
  otherwise, with `lines: 3` on a match and no `lines` for `different`; it
  refuses an unknown name, a hash that is not 64 hex digits, and a production secret; the check is
  audited with the caller as actor; no answer holds the value or its SHA-256.
- A task agent is not served `secret_verify` and its call is refused as unknown; it keeps
  `secrets_list`. After a rotation the Superagent's same check answers `different` and is audited
  again.
- `COCKPIT_READ_ONLY_TOOLS` holds both tools, and the harness test plans them as allowed.

## Test plan

- Unit tests in `crates/yulius-app/src/vault/` for the comparison (each kind, a value without
  escapes, a candidate that is only close) and in `mcp.rs` for the Superagent's tool list.
- Integration tests in `crates/yulius/tests/cockpit.rs` (the Superagent over its `ck_` token) and
  `crates/yulius/tests/secrets.rs` (a task agent is refused), using synthetic values only.
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
- The line count the value stands for is returned on a match only: it is what the person asked to
  confirm, and a caller who does not hold the value learns nothing of its shape.
- `secret_verify` is the Superagent's only (review round 2 reversed the first choice of serving it
  to task agents too). On a match the candidate's hash is the value's unsalted SHA-256, and it
  stays in the caller's transcript (the Bash output and the tool input), which redaction does not
  catch. A task's transcript is read by every member who can view the project, who could attack a
  short value offline; the Superagent's conversation is its person's own (other people's shells
  are not found), and that person holds the candidate already and has the redaction oracle. A
  salt handed out with the check would sit next to the hash in the same transcript, so it would
  not help.
- A production secret is refused, as `secret_request_hosts` and `secret_copy_to_env` refuse one:
  production values are kept out of every agent use.
- Independent review round 1 (REQUEST CHANGES, all own code, all fixed): the digest comparison
  used `==` on caller-chosen bytes (now `CryptoPort::constant_time_eq` over every variant, no
  early exit); the audit key ignored rotations (now keyed by the vault generation; the rotation
  test was replaced in round 3, see below); variant strings left freed copies of
  the value (now hashed piece by piece in place, and `sha2`'s `zeroize` feature wipes the hasher);
  `lines` was answered for `different` too (now on a match only, tests updated, red before). The
  direct use of `sha2` instead of the crypto port stays, as in `imports.rs` and `uploads.rs`.
- Independent review round 2 (REQUEST CHANGES; round-1 fixes confirmed; all own code, all
  fixed): the candidate's hash in shared task transcripts (high: `secret_verify` is now the
  Superagent's only, test `agents_are_not_served_secret_verify`, red before); `lines` counted a
  final literal `\n` as a line (now ends the last line, unit test red before); the audit-once key
  lacked the sandbox and held the candidate's hash (now keyed by the sandbox and a digest of the
  candidate, unit test); `sha256sum`'s own line was refused (now read, red before); the
  Superagent's own production refusal and rotation are now tested in `cockpit.rs`.
- Independent review round 3 (REQUEST CHANGES; product code found correct and secure; all own
  code, all fixed): the rotation check in `cockpit.rs` could not fail (an identical "→ different"
  entry existed before the rotation; it now asserts a seventh entry, and fails with the
  generation dropped from the key, checked); the Superagent's `secrets_list` carried the proxy's
  `placeholder` and `adds` (now left out, test red before); no wrong-sandbox `secret_verify` test
  (added); the repeated check's answer was not asserted (now `exact`, `lines: 3`); product wording
  ("as a task does") corrected; the generation is read before the record. Left as they are (nits):
  `lines` counts any literal `\n`, also in a value that is not a key, and `sha256sum`'s escaped
  line for a file name with a backslash is refused with the message saying what is wanted.
- Review rounds: the rule allows three; a fourth, focused on the round-3 fixes (tests, two small
  code changes, wording), confirms the exact head before delivery, because the merge needs an
  approved head. Decided here rather than merging on a self-check.
- Independent review round 4 (APPROVE at `82072bf2`, all six round-3 fixes confirmed, the merge
  of `origin/dev` clean): two own-code nits fixed in the next commit (the rotation test's comment
  says the count is the proof; ADR-0144 says the Superagent's records leave out the placeholder
  and injection); the same reviewer confirmed the final head.

## Evidence

- Red before green, each with the change absent: `cockpit::the_superagent_lists_its_sandboxs_agent_secrets_without_values`
  ("unknown tool: secrets_list"; later with `placeholder` still listed),
  `cockpit::a_cockpit_manages_the_project_for_its_owner` (tool list without the vault tools),
  `cockpit::the_superagent_verifies_a_multi_line_value_by_its_hash` ("unknown tool: secret_verify";
  then `sha256sum`'s line refused; the rotation re-audit failing with 6 entries when the generation
  was dropped from the audit key), `secrets::agents_are_not_served_secret_verify` (agents were
  served it), the harness test `a_cockpit_reaches_the_yulius_tools_with_its_token_only_in_its_environment`
  (`--allowedTools` without the two tools), and the unit tests in `vault/verify.rs` (line count
  with a final escape, audit key, `sha256sum` line).
- `cargo nextest` on `cockpit::`, `secrets::`, `secrets_env::` and `manage_tools::`: 75 passed;
  `yulius-app` `verify` and `mcp::` unit tests: 11 passed; `yulius-harness`: 115 passed.
- `scripts/dev ci` (repository contracts, check, lint, nextest, doctests, deps) on the merged head
  `82072bf2`: exit 0, 1884 tests passed, 18 skipped. Earlier heads: `ec0c404f` 1879, `fb3381ef`
  1880, `c436eb12` 1882, all passed. The last commit changes a test comment and documents only.
- `scripts/dev security`: not run (exit 2): Vidoc answers HTTP 401 for the stored key (#470,
  ADR-0104); unrun, not passed.
- Not tested here: a real Claude Code Superagent calling the tools (the integration tests drive
  the MCP endpoint with the Superagent's own `ck_` token and the fake `claude`); the constant-time
  comparison is by inspection (`CryptoPort::constant_time_eq`, `subtle`).
