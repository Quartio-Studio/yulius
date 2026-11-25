---
yulius-task: b51a615a-4fa9-4c2a-a0e1-b442fad60d48
status: in-progress
---

# SPEC-654: An agent asks a secret's allowance with the tool that puts an Allow button on the Secrets page, not with needs_you

Status: in-progress
Date: 2026-11-25
Owner: Yulius task #638
Scope authorization: the person's request of 2026-11-25 (a screenshot of a Needs you card, "in
such case model should run yulius tools to create "allow" button but it didnt").

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

Other paths the cause reaches: host bindings (`secret_request_hosts`) and production copies
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
- AAB-003: the infra-bootstrap skill routes the allowance the same way.
- AAB-004: every description stays within `CLIENT_TEXT_MAX`.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- The regression test `needs_you_routes_a_secrets_allowance_to_the_tool_with_the_allow_button`
  fails on dev and passes with the fix.
- `every_text_served_to_a_client_fits_what_it_reads` passes.
- `.agents/skills` and the other generated copies match `catalog/skills` (the gate's drift check).

## Test plan

- Unit test in `crates/yulius-app/src/mcp.rs`. It checks that `needs_you`'s description names
  `secret_request_hosts`, `secret_copy_to_env` and `secret_request_copy` and the Allow button,
  and that it no longer contains "allow a secret". It checks that `secret_copy_to_env`'s
  description names dev-stage deploys and `[cd.`.
- Run the `yulius-app` lib tests of `mcp`. Yulius CI runs the whole gate.
- Not testable here: whether a model now picks the tool. That needs a live model run on a real
  project, and no test here drives one.

## Decisions

- Change the texts, not the tools. `secret_copy_to_env` already records the dev-deploy
  allowance and shows the Allow button. A new "request only" tool or parameter would be a second
  way to ask the same thing. Writing a `.env` copy in the task's worktree once the person allows
  it does no harm (Git ignores it, ADR-0105). Rejected: a `needs_you` heuristic that spots secret
  names in the action. It guesses from free text, and the cause is the routing text itself.
- `needs_you` keeps "mark a production secret". No tool requests it, by design (ADR-0109).
