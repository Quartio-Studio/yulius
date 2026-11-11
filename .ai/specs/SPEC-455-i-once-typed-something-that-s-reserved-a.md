---
yulius-task: eb6e7bbc-ecf2-47d3-a18b-f6095b090583
status: in-progress
---

# SPEC-455: Own Anthropic keys are accepted, and a reserved name says why

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #437
Scope authorization: task #437 (`/goal`, with a screenshot of the Add agent secret dialog refusing
`ANTHROPIC_API_KEY_QUARTIO`: "ANTHROPIC_API_KEY_QUARTIO is reserved for the agent's environment;
choose another name"), "i once typed something that's reserved and it kept telling me that it's
reserved even if i changed the name … anything in format RESERVED_SOMETHING". Fixes #1153.

## Goal

A person can name an agent secret (or a project variable) for their own Anthropic key, such as
`ANTHROPIC_API_KEY_QUARTIO`, while every name the agent CLI reads stays reserved; and a refused
name says what is reserved and a name that works, so nobody guesses one rename after another.

## Context

- **Symptom.** `ANTHROPIC_API_KEY` is reserved; every rename the person tried that kept the
  `ANTHROPIC_` start (`ANTHROPIC_API_KEY_QUARTIO`, …) got the same refusal, so the dialog looked
  stuck on the first name.
- **Root cause.** `SecretName::parse` (`crates/yulius-domain/src/vault/names.rs`) refuses every
  name starting with one of `RESERVED_PREFIXES`, which holds `ANTHROPIC_`. Unlike `CLAUDE_`,
  `GIT_` or `LD_`, `ANTHROPIC_` is not a namespace only tools use: it is also the natural start of
  a person's own Anthropic key for an app the project builds.
- **Why tests missed it.** `names_are_environment_style_and_not_reserved` checks only refusals
  inside the prefixes, never an own name that merely starts with one.
- **The dialog is not stuck.** The modal hides its error on the next submit and shows the
  server's answer for the name sent (`dom.js` `openModal`); the answer just never changed.
- **Who uses the validator.** Agent secrets (page, `yulius secrets import`, agents'
  `secret_create_from_file`), project variables (SPEC-275, real values in every task run) and
  CD secret names. The repository's `[resources.env]` has its own, stricter list
  (`resources.rs`), which an agent can edit; it is unchanged.
- **What the agent CLI reads.** Claude Code 2.1.292 (the installed agent CLI) holds 76 distinct
  `ANTHROPIC_` strings (`grep -aoE 'ANTHROPIC_[A-Z0-9_]+'` on its binary): credentials, request
  routing (`ANTHROPIC_AWS_*`, `ANTHROPIC_GOOGLE_CLOUD_*`), config location
  (`ANTHROPIC_CONFIG_DIR`), workload identity federation, models. None extends
  `ANTHROPIC_API_KEY_` or `ANTHROPIC_AUTH_TOKEN_`.
- **Related.** #638 asks for more reserved names (`GH_TOKEN`, `NODE_OPTIONS`, …); this change
  neither adds nor removes any outside `ANTHROPIC_`.

## Requirements

- **RSV-001** `ANTHROPIC_` stays a reserved prefix, except a person's own key
  `ANTHROPIC_API_KEY_<NAME>` or `ANTHROPIC_AUTH_TOKEN_<NAME>` (at least one character after the
  `_`) that does not end in `_FILE`.
- **RSV-002** A refusal for a reserved prefix names the prefix; for `ANTHROPIC_` it names the
  own-key forms. Every refusal suggests `APP_<NAME>` when that fits in 64 characters.
- **RSV-003** Every other reserved name and prefix is unchanged; the web mock's variable check
  follows RSV-001.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Regression test: own ANTHROPIC_ names accepted, the agent CLI's still refused
- [x] Anthropic names reserved by what the agent CLI and SDKs read (fix, ADR-0292, docs)
- [x] Review round 1: own-key forms under the reserved prefix, refusals that say why
- [ ] Independent review answered and gates green

## Acceptance criteria

- AC-1 `ANTHROPIC_API_KEY_QUARTIO`, `ANTHROPIC_AUTH_TOKEN_PROD` and `ANTHROPIC_API_KEY_2` parse.
- AC-2 All 76 `ANTHROPIC_` names of the agent CLI, `ANTHROPIC_API_KEY_`, `ANTHROPIC_API_KEY_FILE`,
  `ANTHROPIC_AUTH_TOKEN_FILE` and `ANTHROPIC_KEY_PROD` are refused as reserved.
- AC-3 The refusal of `ANTHROPIC_KEY_PROD` names `ANTHROPIC_`, the own-key form and
  `APP_ANTHROPIC_KEY_PROD`; that of `GIT_TOKEN` names `GIT_` and no Anthropic form; a suggestion
  parses; none is offered past 64 characters.
- AC-4 The existing refusals (`PATH`, `YULIUS_*`, `CLAUDE_*`, `GIT_*`, `LD_*`, `NODE_*`, …) hold.

## Test plan

- Unit, `yulius-domain` `vault::names::tests`:
  `own_anthropic_keys_are_accepted_and_every_other_anthropic_name_is_reserved` (AC-1, AC-2, the
  CLI's names pinned in `CLI_READS`), `a_refusal_names_the_reserved_prefix_and_a_name_that_works`
  (AC-3), `names_are_environment_style_and_not_reserved` (AC-4). Each failed before its fix
  (commits 0472eb89, 9aa4cb50).
- The server integration tests that refuse reserved names (`variables`, `secrets_create`,
  `secrets_copy`) match on "reserved", which the new messages keep.

## Review

- Round 1 (REQUEST CHANGES on e152ab17): (1, high, own) the hand-made list of the CLI's
  `ANTHROPIC_` names missed `ANTHROPIC_CONFIG_DIR`, `ANTHROPIC_PROFILE`, the federation family and
  more; (2, 3, medium, own) `ANTHROPIC_AWS_*`, `ANTHROPIC_GOOGLE_CLOUD_*`,
  `ANTHROPIC_WEBHOOK_SIGNING_KEY`, the environment-worker names. Answered by changing the
  decision: the prefix stays reserved with only the own-key forms carved out, and the test pins
  all 76 names. (4, low, own) the web mock reserved the whole prefix: aligned. (5, low, own) the
  ADR understated variables' real values: rewritten. (6, low, own) Plan boxes: ticked. (7, low,
  pre-existing) the refusal never named the prefix: fixed here, as it is the reported "kept
  telling me" symptom.

## Decisions

- [ADR-0292](../../docs/decisions/0292-own-anthropic-keys-are-allowed-under-the-reserved-anthropic-prefix.md):
  `ANTHROPIC_` stays reserved; own keys `ANTHROPIC_API_KEY_<NAME>` / `ANTHROPIC_AUTH_TOKEN_<NAME>`
  (not `_FILE`) are accepted; refusals name the prefix and `APP_<NAME>`. Rejected: reserving only
  the names the agent CLI reads (incomplete once already, grows each release), dropping the
  prefix (`ANTHROPIC_BASE_URL` as a variable redirects the agent CLI and its credential),
  rewording only.
