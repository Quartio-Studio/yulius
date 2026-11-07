# 0281. The Superagents use the project browser

## Status

Accepted. Specified in [SPEC-437](../../.ai/specs/SPEC-437-add-browser-use-for-superagents.md);
task #419 ("add browser use for superagents"). Supersedes the "No browser" bullet of
[0150](0150-the-superagent-uses-agent-secrets-as-a-task-does.md) (SPEC-186 SAV-004) and the
"browser fills need a task" consequence of
[0144](0144-the-superagent-reads-the-agent-vault-and-verifies-values-by-hash.md); extends the tools
of [0271](0271-a-global-superagent-acts-in-every-sandbox-of-its-person-through-yulius-tools-only.md).

## Context

A person asked their sandbox Superagent to sign in to a staff panel with a password from the agent
vault. It could only answer that a task's agent with the project browser could do it with
`browser_fill_secret`, and that it had no browser tools. The sandbox browser (one Chromium per
sandbox, [0008](0008-streamed-sandbox-browser.md)) is served to a task's agent through nine MCP
tools, which used the task only for its sandbox, its cursor label and the task that
`browser_request_user_action` flags.

ADR-0144 and ADR-0150 left the browser out because the Superagent was then outside the egress
proxy and runs as its person. ADR-0150 since gave it every other agent-secret tool a task has,
through the `SecretUser` caller, so the browser was the one task tool it lacked. The Global
Superagent works only through Yulius tools, each project tool with `project`; browser tools run in
Yulius, not in its root, so its missing shell, files and proxy do not matter to them.

## Decision

- **The sandbox Superagent gets the nine browser tools** on its own sandbox's browser, with a
  task's arguments, results and refusals. `browser_tabs`, `browser_snapshot` and
  `browser_screenshot` only read and are pre-allowed (`COCKPIT_READ_ONLY_TOOLS`); the six others
  are asked before on an uncontained server, bypassed on a contained one (ADR-0165).
- **The Global Superagent gets them with a required `project`** (`GLOBAL_ONLY_TOOLS`), each call
  checked as `Action::Work` on that project, as a browser viewer is. Calls that change something
  are audited `remote.tool`; the three reading ones are not. A person's own remote agent (an API
  token) is not served them.
- **One caller for both.** The browser tools take the `SecretUser` (a task's agent or a
  Superagent's `ProjectCaller`): its sandbox, the person whose agent secrets it resolves, its audit
  actor and process. `browser_fill_secret` keeps every safeguard (bound hosts only, never a
  personal or production secret, no value in the result, page text redacted) and audits
  `vault.agent.fill` as `cockpit:<login>` or `superagent:<login>`.
- **Viewers see who drives.** The cursor reads `<login>'s Superagent` or `<login>'s Global
  Superagent`. A Superagent's `browser_request_user_action` highlights the tab, publishes
  `browser.action_requested` without a task and returns the link; no task is flagged, since its
  person is in front of it.

## Alternatives

- **Only the reading tools** (tabs, snapshot, screenshot): it could look but not sign in, which is
  what was asked. Rejected.
- **Superagent variants of the tools** (own names or arguments): the same browser and secrets as
  its tasks, so two sets would only drift. Rejected.
- **Not for the Global Superagent**: it would still have to start a task to look at a page in any
  project. Text it reads in one project could ask it to act in another, but the worst a browser
  call does is sign into a bound host of that project, which a task it may already create could do.
  Rejected.
- **Serving them to API tokens too**: the person's own agent has the person's own browser; whether
  it should drive the shared one is a separate decision. Not now.

## Consequences

- A Superagent can open, read and sign into the project's sites without starting a task. People
  watching the Browser page see its cursor among the tasks'.
- Running Superagents get the tools by the launch digest (ADR-0150): their tool list changed, so
  each restarts once idle.
- A Superagent and its tasks share one browser and its logins: a sign-in by one is the others'
  too, as between tasks.
