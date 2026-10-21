# 0192. "Work through open issues" decides its scope by repository permission and this project's App

## Status

Accepted. Specified in [SPEC-251](../../.ai/specs/SPEC-251-work-through-open-issues-misses-core-tea.md);
task #237, GitHub issue #594. Amends the scope rule of the ready-made automation "Work through
open issues" ([ADR-0034](0034-ready-made-automations-catalog.md), SPEC-042 AUT-011, SPEC-194
TOP-002); its ranking, ownership and safety rules are kept.

## Context

The automation took a person's issue only when GitHub's `author_association` read `OWNER`,
`MEMBER` or `COLLABORATOR`, and an agent's issue only when a bot opened it with the
`Opened by task #<n> in Yulius.` footer. GitHub reports `CONTRIBUTOR` for a repository admin who
is neither an organization member nor an invited collaborator, and an agent's issue filed with its
person's GitHub token (the App's user access token) is authored by that person, not by the bot.
On this repository 2 of 109 open issues were in scope on 2026-10-21, while 50 were by an admin.

## Decision

- The collect command lists each issue's author login and `performed_via_github_app.slug`.
- A person whose repository permission is `write`, `maintain` or `admin`
  (`gh api repos/{owner}/{repo}/collaborators/<login>/permission`, asked once per author) is the
  core team whatever the association says; `OWNER`, `MEMBER` and `COLLABORATOR` still count. A
  failed lookup (an unknown login, a refused call) or any other answer counts as no write access.
- An issue this project's Yulius App's bot (`<slug>[bot]`) opened whose body ends with the footer
  is an agent's issue. An issue an agent filed through the App on a person's behalf
  (`performed_via_github_app` `<slug>`) counts as that person's own issue, by the rules above:
  every person on any project of the instance holds a user token of the same App, so it proves no
  write access here, and the footer is plain text. The prompt finds the slug from the App `gh` works through:
  the GraphQL `viewer` is its bot with the installation token; with a person's App user token,
  `user/installations` names the App. Without a slug no issue is in scope by this rule.
- Everyone else (contributors and outsiders without write access) stays out, untouched.

## Alternatives

- Widen the association list to `CONTRIBUTOR`: anyone with a merged commit would steer the
  automation; refused.
- Keep "any bot with the footer" and add "any App with the footer": the footer is plain text and
  any installed App could carry it; pinning this project's App costs one command.
- Take an issue filed through this project's App on a person's behalf with the footer, whoever the
  person (as #594 first asked): a person with write access on another project of the instance, or
  read access only, could steer the automation here; writers' issues are in scope by permission
  anyway.
- Expose the App slug through `project_context`: exact, but a server and MCP change larger than the
  defect, while `gh` already names the App.

## Consequences

- A run asks GitHub one permission call per distinct human author (one on this repository).
- `MEMBER` and `COLLABORATOR` still count without a permission check, as before, although GitHub
  reports them for read and triage access too; tightening that is outside #594.
- Bot-authored issues without the footer, such as Yulius's security findings
  ([ADR-0172](0172-medium-findings-in-the-change-block-and-the-rest-become-issues.md)), stay out
  as before; taking them in is a separate decision.
- The prompt's App lookup with a person's token (`user/installations`) could not be run in a
  sandbox holding the installation token; the installation-token path was checked live.
