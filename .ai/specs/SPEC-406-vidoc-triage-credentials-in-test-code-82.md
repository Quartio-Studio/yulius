---
yulius-task: 5caab8de-c311-4c56-8723-04c080df043c
status: in-progress
---

# SPEC-406: Vidoc triage of the test-code findings (#822 #821 #797 #773 #746 #747 #772)

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #388
Scope authorization: task #388 (a person asked that each of the seven issues end closed; a fixture
with a real provider's token prefix is to be treated as real). Fixes #822, #797 and #772. #821,
#773, #746 and #747 are dismissed as false positives.

## Goal

Each of the seven low Vidoc findings in `crates/yulius/tests` is checked against `dev`
(d51b98ae). Each one is either fixed test-first or dismissed with file:line evidence and a narrow
`[[ignore]]` entry for its wording, so that a repeat scan does not reopen it.

## Context

- The production master key is random: `serve.rs:438` calls `MasterKeyCrypto::load_or_create`,
  which fills 32 bytes from `rand::rng()` and writes them mode 0600 (`crypto.rs:58-67`).
  `MasterKeyCrypto::from_key` ("Builds from raw key bytes (tests)", `crypto.rs:70-74`) has no
  caller outside test code; `[7u8; 32]` and `[9u8; 32]` appear only in `crates/yulius/tests` and
  in `crypto.rs`'s `#[cfg(test)]` module.
- The product detects credentials by their provider prefix: `yulius-domain/src/redact.rs`
  (`sk-ant-`, `sk_live_`, `sk_test_`, `ghp_`, …) and the sign-in's
  `yulius-harness/src/login.rs` `CLAUDE_TOKEN_PREFIX` (`sk-ant-oat01-`). So the tests need
  values of those shapes at run time.
- 26 literals in 17 files under `crates/yulius/tests` carry a real provider prefix and a body
  (`sk-ant-oat01-test-credential`, `sk_test_51NzQ9vaultValue8f3a`, `sk_live_51EnvCopyValue9q2w`,
  …). All are synthetic, but the shape is what secret scanners and GitHub push protection match,
  and a real token pasted into a fixture looks like the rest. `secrets_env.rs` already builds PEM
  headers from parts (`format!("{} PRIVATE KEY", "OPENSSH")`), and `redact.rs`'s tests build
  tokens with `fake(prefix, …)`.
- `secrets_env::sourced` (`secrets_env.rs:842-852`) runs `sh -c` with the variable name in the
  script text. AGENTS.md: "No shell interpolation of prompts, IDs, paths or repository content."
  Its callers (`secrets_env.rs`, `secrets_create.rs:254-258`, `cockpit.rs:3424`) pass literals.
- `World::remote` (`support/repos.rs:1093-1110`) writes `files` under its own temporary
  `scratch` folder. Every caller passes literal relative paths or the product's own scaffold
  (`scaffold_files`, `templates.rs:741`).
- `[[ignore]]` (`scripts/vidoc_scan.py` `apply_ignores`) matches a finding whose path fits the
  entry's `fnmatch` pattern and whose title contains the entry's title (case-insensitive),
  anywhere in the file. On untouched code, `function` and `line` widen that to a reworded repeat
  in the same function or within a few lines; they never narrow it. One entry per wording
  (SPEC-273, ADR-0202).

## Requirements

- TCF-001 (#822, #797, real). No file under `crates/yulius/tests` holds a provider-prefixed
  credential literal: a word that starts with one of the redaction rules' prefixes and goes on
  with 8 or more token characters (for prefixes that are also common words, such as `npm_`,
  `hf_` or `secret_`, the rule's own minimum). Fixtures that need
  the shape put it together from the prefix and a synthetic body (`concat!`), so the value at
  run time and every test's behavior stay the same. A test reads every source file there and
  fails naming each such literal, so a real token pasted into a fixture fails the gate.
- TCF-002 (#797, rest). The other values on `secrets.rs:22-27` (a password, the RFC 4226 test
  seed in base32, vault task tokens) have no provider shape and reach only the test's own
  in-memory store and fake hosts. They stay, with a narrow entry for Vidoc's wording.
- TCF-003 (#772, real). `sourced` passes the variable name to `sh` as an argument (`"$1"`),
  never as script text, and returns the value byte for byte as before. A name with shell
  syntax in it runs nothing.
- TCF-004 (#821, #773, #746, false positives). The fixed master keys are test fixtures for
  temporary stores; production never builds a key from bytes. Dismissed with narrow entries.
- TCF-005 (#747, false positive). `World::remote` writes only the test's own fixed paths into
  its own temporary folder. Dismissed with a narrow entry.

## Plan

- [x] Spec SPEC-406 with the triage of all seven findings
- [x] Test fixtures carry no provider-shaped credential literal, guarded by a test (#822 #797)
- [x] sourced passes the variable name as an argument, not script text (#772)
- [x] Narrow [[ignore]] entries for the dismissed wordings (#821 #773 #746 #747 #797 rest)
- [x] ADR, docs, changes entry and gates
- [ ] Independent review answered
- [ ] Dismissed issues commented and closed (#821 #773 #746 #747)

## Acceptance criteria

- `fixture_hygiene::test_fixtures_hold_no_provider_shaped_credential` fails on dev, naming the
  26 literals, and passes with the fix.
- `secrets_env::sourced_runs_no_shell_syntax_from_the_name` fails on dev (the name's command
  runs and creates a file) and passes with the fix.
- The tests that use the converted fixtures and `sourced` (`secrets_env::`, `secrets_create::`,
  `secrets::`, `supervision::`, …) pass unchanged; the full gate passes.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each issue ends closed: #822, #797 and #772 by the merged pull request, the others as not
  planned, each with a comment giving the evidence.

## Test plan

- New `crates/yulius/tests/fixture_hygiene.rs` (declared in `tests/main.rs`).
- New test beside `sourced` in `crates/yulius/tests/secrets_env.rs`.
- `scripts/dev ci` (the full gate) and `yulius security-scan`.

## Decisions

- #822 and #797 are treated as real, as asked: `sk-ant-oat01-` and `sk_test_51` are real
  provider prefixes. The values are not live secrets, so the fix is about the shape: the whole
  `crates/yulius/tests` tree is cleaned, not only the two flagged lines, and a test keeps it
  clean. `concat!` of the prefix and the body was chosen over a run-time helper: the fixtures
  stay `const`, no call site changes, and the value is the same. A random value per run was
  rejected: tests compare against fixed values, and determinism helps debugging. ADR-0258.
- The guard covers `crates/yulius/tests`, this group's scope. Provider-shaped fixtures elsewhere
  (`yulius-harness/tests/login.rs`, `yulius-egress/tests/proxy.rs`, `#[cfg(test)]` modules in
  `accounts.rs`, `split.rs`, `scrub_client.rs`, `secret.rs`) are named in the pull request.
- #772 is fixed although every caller passes a literal: AGENTS.md bans the pattern itself, and
  the fix is small. `set -a` exports what `.env` sets and `printenv "$1"` prints it; one
  trailing newline (printenv's) is removed.
- #747 is dismissed rather than fixed: no repository rule covers it, the inputs are the test's
  own, and the folder is the helper's own temporary one.

## Evidence

(Filled in at the end.)
