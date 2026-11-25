---
yulius-task: b51a615a-4fa9-4c2a-a0e1-b442fad60d48
status: done
---

# SPEC-654: An agent asks a secret's allowance with the tool that puts an Allow button on the Secrets page, not with needs_you

Status: done
Date: 2026-11-25
Owner: Yulius task #638
Scope authorization: the person's request of 2026-11-25 (a screenshot of a Needs you card, "in
such case model should run yulius tools to create "allow" button but it didnt"). Issue
[#1759](https://github.com/Quartio-Studio/yulius/issues/1759).

## Goal

When an agent needs a person to allow an agent secret for something a Yulius tool can request,
it calls that tool, which records the request and shows the person an Allow button on the
Secrets page. A free-text `needs_you` card stays for what no tool requests.

## Context

Symptom (the screenshot, task #405 of another project): the task added `GEMINI_API_KEY` to its
`[cd.dev] secrets`. The secret exists but is not allowed outside the proxy, so dev deploys would
stop on it. The task raised `needs_you` with "Secrets page: allow GEMINI_API_KEY for [cd.dev]
deploys; mark GEMINI_API_KEY_PROD as a prod secret". The person got a card with no button and had
to find the secret and its switch by hand.

Root cause (the texts the model reads route the allowance to `needs_you`):

- The `needs_you` tool description (`crates/yulius-app/src/mcp.rs`) lists "allow a secret" among
  "something only a person can" do. It is the only routing the model has.
- `secret_copy_to_env`'s description talks only about `.env` copies. But the allowance it asks for
  (`env_files`, "outside the proxy", ADR-0105) is the same one dev-stage CD runs check
  (`allowed_for` in `crates/yulius-app/src/vault/env_files.rs`). `ask_env_files` puts the Allow
  button on the Secrets page and marks the task as waiting on a secret request. No text tells a
  model that naming a secret in `[cd.dev]` needs this allowance or that this tool asks for it.
- The infra-bootstrap skill (`catalog/skills/yulius/infra-bootstrap/SKILL.md`) says the same:
  "What only a person can do (add a machine's credentials, allow a secret, deploy production) you
  ask with `needs_you`".
- `cd_deploy` asks for the allowance too, but only once the change is merged (it runs the
  integration branch's `yulius.toml`). `cd_status` shows the protected branch's tables, so a
  task branch's new secret is not in it.

Why tests missed it: the tool descriptions had no test for how they route a secret's allowance.

Other paths the cause reaches: the nightly dev-deploy automation template
(`catalog/automations/nightly-dev-deploy.md`) told its run to ask with `needs_you` when a dev
secret waits for allowance, so it raised the same buttonless card (found by review round 1). Host
bindings (`secret_request_hosts`) and production copies
(`secret_request_copy`) already have request tools, but the same `needs_you` sentence steers
agents away from them too. Marking a secret as a production secret (`cd_prod`) has no request tool
on purpose (ADR-0109: production is a person's), so `needs_you` stays right for that part.

## Requirements

- AAB-001: `needs_you`'s description no longer lists allowing a secret as a `needs_you` job. It
  says that an allowance a tool requests is asked with that tool, which shows the person an Allow
  button: hosts with `secret_request_hosts`, use outside the proxy (`.env` files and dev-stage
  deploys) with `secret_copy_to_env`, a copy of a production secret with `secret_request_copy`.
- AAB-002: `secret_copy_to_env`'s description says the same allowance gives the secret to
  dev-stage deploys, and that a secret a dev-stage `[cd.*]` table names is asked for with it.
- AAB-003: the infra-bootstrap skill routes the allowance the same way. `secret_copy_to_env`
  is asked only for a secret whose `env_files` is not allowed, and a deploy-only secret needs no
  second call (no `.env` copy) once allowed.
- AAB-005: the nightly dev-deploy automation asks a secret that waits for allowance with
  `secret_copy_to_env`; `needs_you` only for a missing secret.
- AAB-006: the Superagent's `secrets_list` advice names no tool it is not served (`needs_you`).
- AAB-004: every description stays within `CLIENT_TEXT_MAX`.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- The regression test `needs_you_routes_a_secrets_allowance_to_the_tool_with_the_allow_button`
  fails on dev and passes with the fix.
- `every_text_served_to_a_client_fits_what_it_reads` passes.
- `automations::templates::tests::the_shipped_catalog_is_valid` pins the nightly template's route.

## Test plan

- Integration tests `secrets_env::secrets_list_teaches_the_env_default` and
  `cockpit::the_superagent_lists_its_sandboxs_agent_secrets_without_values` check that
  `secrets_list`'s advice (task and Superagent) names the Allow button and dev-stage deploys.
- Unit test in `crates/yulius-app/src/mcp.rs`. It checks that `needs_you`'s description names
  `secret_request_hosts`, `secret_copy_to_env` and `secret_request_copy` and the Allow button,
  and that it no longer contains "allow a secret". It checks that `secret_copy_to_env`'s
  description names dev-stage deploys and `[cd.`.
- `automations::templates::tests::the_shipped_catalog_is_valid` checks that the nightly dev-deploy
  template names `secret_copy_to_env` and the Allow button and no longer says "allow on the
  Secrets page" for `needs_you`.
- Run the `yulius-app` lib tests of `mcp`. Yulius CI runs the whole gate.
- Not testable here: whether a model now picks the tool. That needs a live model run on a real
  project, and no test here drives one.

## Decisions

- Change the texts, not the tools. `secret_copy_to_env` already records the dev-deploy
  allowance and shows the Allow button. A new "request only" tool or parameter would be a second
  way to ask the same thing. Consequences: while the secret waits, the call writes nothing. A
  second call (only when code outside the proxy needs the value) writes the value into the
  worktree's git-ignored `.env` and `NAME=` into `.env.example`, so the texts say a deploy-only
  secret needs no second call. A project whose `.env` Git does not ignore is refused before
  anyone is asked; it adds the ignore rule first, as ADR-0105 already asks. Rejected: a `needs_you` heuristic that spots secret
  names in the action. It guesses from free text, and the cause is the routing text itself.
- `needs_you` keeps "mark a production secret". No tool requests it, by design (ADR-0109).

## Evidence

- Root cause: `needs_you`'s description (`crates/yulius-app/src/mcp.rs`) listed "allow a secret";
  `allowed_for` (`crates/yulius-app/src/vault/env_files.rs`) gives dev-stage runs only
  `env_files == Allowed` secrets, the allowance `secret_copy_to_env` and an agent's `cd_deploy`
  ask for through `ask_env_files`. Issue
  [#1759](https://github.com/Quartio-Studio/yulius/issues/1759).
- Red first: `cargo test -p yulius-app --lib mcp::tests::needs_you_routes` failed at commit
  `19cf6197a` (the description still said "allow a secret, deploy production").
  `cargo test -p yulius --test main -- secrets_env::secrets_list_teaches_the_env_default
  cockpit::the_superagent_lists_its_sandboxs_agent_secrets_without_values` failed with dev's
  `runtime.rs` (2 failed). The round-1 tests (`needs_you_routes...`,
  `automations::templates::tests::the_shipped_catalog_is_valid`,
  `cockpit::the_superagent_lists...`) failed before their fix commit.
- Green: `cargo test -p yulius-app --lib`: 731 passed. Two runs out of ten each had one failure
  in untouched code (`sweep::tests::*`, `identity::tests::the_agent_yulius_shim_offers_the_session_commands`),
  and those runs passed alone and in six later full runs.
  `cargo test -p yulius --test main -- secrets_env:: cockpit:: secrets automation workflow`:
  230 passed. `cargo fmt --all -- --check`: clean.
- Review round 1 (Fable 5.2, head `0ba2a0c54`): request changes. Fixed: the nightly template
  (AAB-005), the Superagent's advice naming `needs_you` (AAB-006), the copy's condition (AAB-003),
  wording. Not changed: `catalog/workflows/infra-bootstrap.md` (the workflow block is at its
  1,444-byte budget; the skill it points to carries the route). The skill's pre-existing "outside
  the agent's reach" wording is outside this change.
- Not testable here: whether a live model now picks `secret_copy_to_env`; no test drives a model.
