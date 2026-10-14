# 0094. Onboarding installs the gate in its own pull request

## Status

Accepted. Specified in [SPEC-116](../../.ai/specs/SPEC-116-linter-dead-code-should-be-installed-on.md);
task #101.

## Context

The onboarding of `Quartio-Studio/studio` (task #1, PR #59) ended with "Studio has no linter or
dead-code check yet (#57)" and "Client and studio projects have no Yulius CI jobs yet (#58)" as
follow-ups. The onboarding prompt asked for exactly that. Missing tests, lint, a dead-code check
or environments were to be laid "as follow-up pull requests", and the onboarding itself was to
change "documentation, configuration and CI only". The `infra-bootstrap` skill asked for "a short
series of pull requests". In practice nobody started those follow-ups. The project was "ready"
while the gate that every later task's discipline relies on (step 6: run every project gate) was
missing.

The same onboarding also showed two other problems:
- **The pull request.** The goal's evaluator kept its turn going, so Yulius never reached the
  clean end of turn that opens the pull request, and the agent pushed by hand.
- **A Windows-only workflow.** It was kept "as provisional until a founder accepts it", which
  asked a person for a decision that onboarding should make.

## Decision

- **The gate is part of onboarding.** Both onboarding prompts (the full preparation and the
  environment-only one) lay it in the onboarding pull request itself:
  - a test command;
  - a lint and format check;
  - a dead-code and unused-dependency check (skill `dead-code-cleanup`, pinned, allowlist
    committed);
  - all of them wired into `[commands]` and `[[ci.jobs]]`;
  - in a monorepo, a CI job for every project.
- **Ratchet, don't rewrite.** Existing findings do not change application code during
  onboarding. The agent commits the tool's baseline or allowlist, or scopes the check to what
  passes today, so the gate passes now and fails on new findings. It files one issue to burn the
  baseline down. The `infra-bootstrap` skill says the same for every bootstrap.
- **Workflows Yulius CI cannot run are kept.** A workflow whose jobs need a runner Yulius CI
  lacks (Windows, macOS) is kept and recorded in a decision record. No person is asked to accept
  it.
- **Onboarding delivers like every task.** The agent gets the review, calls `delivery_merge`,
  starts `yulius wait` and ends its turn. `delivery_merge` itself pushes the head and opens or
  updates the pull request when the task has none or it is behind, with the same readiness rules
  as the end of a turn. A goal task therefore never depends on a turn's end to get its pull
  request.
- **Deploy environments stay a person's matter.** They need machines and credentials, so the
  agent files an issue when they are missing.

## Alternatives

- **Keep follow-up pull requests, but chain tasks for them automatically.** This would keep
  onboarding small. However, it starts paid agents nobody asked for, and the project would still
  be worked on without its gate until those tasks finish.
- **Fix every existing finding during onboarding.** This yields a clean code base at once, but it
  produces a huge diff of application code in a pull request whose purpose is configuration. Such
  a diff is hard to review and can break behavior.
- **Open the pull request whenever the goal verdict is "not met".** This also breaks the
  deadlock, but it opens pull requests while the agent is still working. The merge call is where
  the agent says "deliver this head", already requires the review, and answers synchronously.

## Consequences

- **Larger onboarding pull requests.** They carry check configuration, baselines and CI jobs, but
  stay free of application-code rewrites.
- **Every later task starts with a gate.** The briefing's "Project foundations" gap list no
  longer stays open after onboarding.
- **Refused merge calls.** `delivery_merge` refuses a head it cannot deliver (open step, invalid
  spec, uncommitted work) instead of arming a merge that would never be decided.
- **Older open pull requests and `main` go red.** New CI jobs gate every ref, including branches
  made before onboarding, until those merge the integration branch. That is the intended
  contract.
