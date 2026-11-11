---
yulius-task: eb6e7bbc-ecf2-47d3-a18b-f6095b090583
status: in-progress
---

# SPEC-455: A secret name of one's own that starts with ANTHROPIC_ is accepted

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #437
Scope authorization: task #437 (`/goal`, with a screenshot of the Add agent secret dialog refusing
`ANTHROPIC_API_KEY_QUARTIO`: "ANTHROPIC_API_KEY_QUARTIO is reserved for the agent's environment;
choose another name"), "i once typed something that's reserved and it kept telling me that it's
reserved even if i changed the name … anything in format RESERVED_SOMETHING". Fixes #1153.

## Goal

A person can name an agent secret (or a project variable) for their own Anthropic key, such as
`ANTHROPIC_API_KEY_QUARTIO`, while the names the agent CLI and the Anthropic SDKs read stay
reserved.

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
- **Related.** #638 asks for more reserved names (`GH_TOKEN`, `NODE_OPTIONS`, …); this change
  neither adds nor removes any outside `ANTHROPIC_`.

## Requirements

- **RSV-001** Names starting with `ANTHROPIC_` are reserved only when the agent CLI or the
  Anthropic SDKs read them: `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL`,
  `ANTHROPIC_MODEL`, `ANTHROPIC_BETAS`, `ANTHROPIC_LOG`, `ANTHROPIC_UNIX_SOCKET`, and the
  families `ANTHROPIC_CUSTOM_*`, `ANTHROPIC_DEFAULT_*`, `ANTHROPIC_SMALL_FAST_MODEL*`,
  `ANTHROPIC_BEDROCK_*`, `ANTHROPIC_VERTEX_*`, `ANTHROPIC_FOUNDRY_*`.
- **RSV-002** Any other `ANTHROPIC_…` name of the right shape is accepted, such as
  `ANTHROPIC_API_KEY_QUARTIO` or `ANTHROPIC_KEY_PROD`.
- **RSV-003** Every other reserved name and prefix is unchanged.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Regression test: own ANTHROPIC_ names accepted, the agent CLI's still refused
- [ ] Anthropic names reserved by what the agent CLI and SDKs read (fix, ADR-0292, docs)
- [ ] Independent review answered and gates green

## Acceptance criteria

- AC-1 `SecretName::parse("ANTHROPIC_API_KEY_QUARTIO")` and `("ANTHROPIC_KEY_PROD")` are `Ok`.
- AC-2 Each RSV-001 name (and a member of each family) is refused with the reserved message.
- AC-3 The existing refusals (`PATH`, `YULIUS_*`, `CLAUDE_*`, `GIT_*`, `LD_*`, `NODE_*`, …) hold.

## Test plan

- Unit, `yulius-domain` `vault::names::tests`:
  `own_anthropic_names_are_accepted_and_the_cli_s_are_reserved` (AC-1, AC-2), failing before the
  fix on AC-1; `names_are_environment_style_and_not_reserved` keeps AC-3.

## Decisions

- [ADR-0292](../../docs/decisions/0292-anthropic-names-are-reserved-by-what-the-agent-cli-reads.md):
  `ANTHROPIC_` is reserved by the names and families the agent CLI and SDKs read, not as a
  whole prefix. Rejected: allowing only `ANTHROPIC_API_KEY_*` (still refuses
  `ANTHROPIC_KEY_PROD`), dropping the prefix with no list (`ANTHROPIC_BASE_URL` as a variable
  would redirect the agent CLI and its credential), keeping it and only rewording the message.
