---
yulius-task: 33d4d53d-0016-47a7-9b6a-9f96caf36caf
status: done
---

# SPEC-437: The Superagents use the project browser

Status: done
Date: 2026-11-07
Owner: Yulius task #419
Scope authorization: task #419 (`/goal`): "add browser use for superagents", with a screenshot of a
sandbox Superagent answering "A task agent with Yulius' project browser can sign in to
https://staff…/login with browser_fill_secret … This session doesn't have the browser tools, so I
can't test that from here." Supersedes the "No browser" bullet of
[ADR-0150](../../docs/decisions/0150-the-superagent-uses-agent-secrets-as-a-task-does.md) (SPEC-186
SAV-004); decided in [ADR-0281](../../docs/decisions/0281-the-superagents-use-the-project-browser.md).

## Goal

Both Superagents drive the shared sandbox browser as a task's agent does: the sandbox Superagent
in its own sandbox's browser, the Global Superagent in the browser of any project it names. They
open and read pages, click and type, take screenshots, hand a tab to their person, and sign in with
an agent secret through `browser_fill_secret` without seeing it.

## Context

- One Chromium per sandbox (`App::browser_session`, SPEC-001 BRW-001..008), its profile kept per
  sandbox; people view it on the sandbox's Browser page with write access (`Action::Work`). A
  task's agent reaches it through nine MCP tools (`browser_tabs`, `browser_open`,
  `browser_navigate`, `browser_click`, `browser_type`, `browser_snapshot`, `browser_screenshot`,
  `browser_request_user_action`, `browser_fill_secret`), each taking its `AgentContext` only for
  the sandbox, the cursor label and (for `browser_request_user_action`) the task it flags.
- The sandbox Superagent (`ck_` bearer, `COCKPIT_TOOLS`) was kept out of the browser in ADR-0144
  and ADR-0150 because it was not behind the egress proxy and runs as its person. ADR-0150 since put
  it behind the proxy with every agent-secret tool a task has (`secret_copy_to_env`,
  `secret_request_hosts`, the `SecretUser` caller); the browser was the one task tool left out.
- The Global Superagent (`gs_` bearer, ADR-0271) is served the remote MCP's project tools with a
  `project` argument, checked per call against its person's access; it has no shell, files or
  egress proxy, and acts only through Yulius tools. Browser tools run in Yulius, not in its root,
  so they need none of those.
- Running Superagents get new tools by the launch digest (ADR-0150): a change to their tool
  descriptors or instructions restarts each once idle.

## Requirements

- SAB-001: The sandbox Superagent is served the nine browser tools on its sandbox's browser, with
  the same arguments, results and refusals as a task's agent. `browser_tabs`, `browser_snapshot`
  and `browser_screenshot` read only and run without asking; the others are asked before on an
  uncontained server (ADR-0165 bypass on a contained one stays).
- SAB-002: The Global Superagent is served the same nine tools with a required `project`
  (owner/name), acting in that project's browser; each call needs its person's write access to the
  project (`Action::Work`, as a viewer of the browser), refused as not found when they cannot see
  it. A person's own remote agent (an API token) is not served them.
- SAB-003: `browser_fill_secret` from either Superagent resolves the project's agent secrets for
  its person (as `secrets_list` shows them) and keeps every safeguard: bound hosts only, never a
  personal or production secret, no value in the result, page text redacted. Each use is audited
  once per Superagent process, secret and host, as its actor (`cockpit:<login>` or
  `superagent:<login>`).
- SAB-004: Viewers see the Superagent's cursor labelled `<login>'s Superagent` or `<login>'s Global
  Superagent`.
- SAB-005: `browser_request_user_action` from a Superagent highlights the tab and notifies the
  sandbox's viewers (`browser.action_requested` without a task) and returns the tab's link; no task
  is flagged, since its person is in front of it.
- SAB-006: Both Superagents' MCP instructions say they can use the project browser; running ones
  get the tools by their launch digest.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The sandbox Superagent drives its sandbox's browser and types agent secrets into it
- [x] The Global Superagent drives a named project's browser
- [x] ADR, architecture, API and product docs describe the Superagents' browser
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-SAB-1: alice's sandbox Superagent lists the nine browser tools; `browser_tabs`,
  `browser_snapshot` and `browser_screenshot` are in its launch's allowed tools and the six others
  are not; it opens `https://docs.example/login` in `s1`'s browser, clicks, types, snapshots and
  screenshots it, and fills `DOCS_LOGIN` into it (the value reaches the page, not the result); on
  another host the fill is refused; the audit holds a `vault.agent.fill` entry by `cockpit:alice`.
- AC-SAB-2: its `browser_request_user_action` returns `/s/fixture/browser?tab=<id>` and publishes
  `browser.action_requested` with no task.
- AC-SAB-3: alice's Global Superagent lists the nine browser tools, each requiring `project`;
  `browser_open` with `project: acme/fixture` opens a tab the `s1` browser lists; bob (read
  access) and a project alice cannot see are refused; a remote `operate` token is not served the
  tools and its call is an unknown tool.

## Test plan

- Integration, `crates/yulius/tests/superagent_browser.rs` (new): the Cockpit fixture (real ptyhost,
  fake `claude`) with the real Chromium manager over the fake DevTools server, as
  `tests/browser.rs` uses it: AC-SAB-1..3.
- Unit, `mcp.rs` and `global_superagent.rs`: the served tool lists.

## Decisions

- **The same tools, not a Superagent variant.** The Superagent works in the same sandbox and
  holds the same agent secrets as its tasks (ADR-0150); separate tools would only diverge.
  Alternative kept out: a read-only subset (tabs, snapshot, screenshot), which would not sign in
  and so not answer the request.
- **The Global Superagent gets them too, `browser_fill_secret` included.** They run in Yulius, not
  in its root, and a typed secret only ever reaches a page of the secret's bound hosts in its own
  project, as from a task. Text it reads in one project could ask it to act in another; the worst
  such a call does is sign into a bound host of that project, which it could already reach with
  `task_create`.
- **Page text is data for the Global Superagent.** Web pages are a new kind of untrusted text for
  a session whose tools all run without asking; its instructions name web pages and browser
  snapshots as data, never instructions (review round 1). A secret bound to no host points it to
  the Secrets page, since it has no `secret_request_hosts`.
- **Not for API tokens.** The person's own agent has their own browser; serving the shared one to
  it is a separate decision.
- **Reuse the `SecretUser` caller** for the browser tools: it already says, for a task or a
  Superagent, which sandbox, which person's secrets, which audit actor and process.

## Evidence

On 2026-11-07, branch at `c15e5822` plus the pinned-launch test fix (current with `origin/dev` at
`7a9f669c`):

- Reproduced the request before the change: the sandbox Superagent's `tools/list` had no
  `browser_*` tool and its `browser_fill_secret` call answered "unknown tool" (the test
  `cockpit::the_superagent_lists_its_sandboxs_agent_secrets_without_values` asserted exactly that).
- New tests, each failing without its change (watched fail first):
  - `superagent_browser::the_sandbox_superagent_drives_its_sandboxs_browser` (AC-SAB-1, AC-SAB-2):
    failed with "browser_tabs runs without asking" (not in the launch's allowed tools) before the
    change; passes after.
  - `superagent_browser::the_global_superagent_drives_a_named_projects_browser` (AC-SAB-3):
    failed with "browser_tabs not served"; after review round 1 it failed on the instructions'
    "web pages" before their fix. It also covers the Secrets-page refusal for an unbound secret,
    bob's read access ("needs write access"), carol's unseen project ("not found") and the
    `remote.tool` audit of changing calls only.
  - `global_superagent::tests::only_it_is_served_a_projects_browser` and the browser check added to
    `remote_mcp::the_public_mcp_takes_only_personal_agent_tokens` (an operate token is not served
    the tools and its call is an unknown tool).
- `scripts/dev ci` at `c15e5822`: doctor, repository contracts, check, lint (format, clippy) and
  macOS targets passed; `cargo nextest run --workspace --profile ci` ran 3203 tests, 3202 passed,
  1 failed, 16 skipped. The failure was
  `yulius-harness::launch a_cockpit_reaches_the_yulius_tools_with_its_token_only_in_its_environment`,
  which pins the Cockpit's `--allowedTools` string and now includes the three reading browser
  tools; after updating it, `cargo nextest run -p yulius-harness` 143 of 143 passed. Then
  `scripts/dev deps` (cargo-shear: no issues) exit 0 and `cargo doc --workspace --no-deps` built
  without warnings. Yulius CI runs the full gate again on the pull request head.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius" — disabled, not
  passed.
- e2e not run: no web UI file changed (the Browser page already shows any agent's cursor label).
- Independent review (fresh session, `om-code-review`): round 1 requested changes (one medium:
  page text as untrusted input for the Global Superagent; lows: two test gaps, the Global
  Superagent's refusal naming `secret_request_hosts`, stale decision records; nits), all fixed
  above; round 2 approved `c15e5822`. Its pre-existing finding (a secret typed into a plain-text
  field shows in a screenshot, true for tasks before this change) is issue #1113.
- Not tested: a real Chromium and a real Claude Code session (the tests use the fake DevTools
  server and the fake `claude`), and the restart of already-running Superagents to pick up the
  tools, which the existing launch-digest tests cover generically.
