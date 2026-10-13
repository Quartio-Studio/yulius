# 0038. Onboarding starts in the browser, never from the CLI

## Status

Accepted. Specified in [SPEC-051](../../.ai/specs/SPEC-051-onboarding-existing-project-using-cli-sh.md).
Replaces the "no approval step" of the `yulius onboard` path (SPEC-001 IMP-010, F 2026-10-10);
adding a repository in the web UI is unchanged.

## Context

`yulius onboard` adds a repository and uploads a person's branches, snapshot and Claude Code
sessions. Once the upload landed, Yulius started the onboarding task by itself: an agent that
adapts the repository to the Yulius standards (docs, `yulius.toml`, CI) and opens a pull request.
The founder asked for the upload to stay in the CLI, and for that adaptation to need an explicit
action in the GUI, "Start onboarding", never the CLI (task #43). The sandbox page already had a
button for a `pending` sandbox (`POST /api/sandboxes/{sid}/onboarding/approve`), but any API
client could call it, including the CLI's bearer token from `yulius login`.

## Decision

- Applying an upload no longer starts the onboarding task: the sandbox stays `pending` with its
  resting task, and new tasks wait after it as before (IMP-010).
- The approve endpoint is the **Start onboarding** button and accepts only a browser session (a
  cookie session, which carries a CSRF token). A bearer token gets 403 before the sandbox is read.
- Every other way to start it follows the same rule: continuing a `pending` sandbox's onboarding
  task needs a browser session, and adding a repository through `POST /api/sandboxes` starts
  onboarding at once only from the browser (a token import stays `pending`).
- The CLI prints that onboarding waits for Start onboarding, with the sandbox page's URL.

## Alternatives

- Only drop the auto-start and leave the endpoint open. Today's CLI would comply, but any script
  or later CLI command holding a token could still start it, so "never via the CLI" would rest on
  convention.
- A per-token scope ("browser-only actions"). It is the more general tool, but tokens have no
  scopes yet, and a cookie session is already exactly "a person in the browser".
- Also require Start onboarding after adding a repository in the web UI. That click is already an
  explicit GUI action, so it would add a second click without a reason; it stays a reversible
  choice.

## Consequences

- A repository onboarded through the CLI does nothing until someone opens its sandbox page and
  clicks Start onboarding; until then new tasks queue behind the onboarding task (an admin can
  still Skip for now).
- Scripts using the public API cannot start onboarding; they can still read its status.
