# 0415. Merge-first is the bootstrap default and reaches onboarded projects through a task

## Status

Accepted. Specified in [SPEC-575](../../.ai/specs/SPEC-575-make-task-local-testing-and-one-staging.md)
(MFS-010 to MFS-012); task #561. Builds on
[0410](0410-the-full-suite-runs-on-the-merged-revision-beside-a-merge-first-staging-target.md)
and [0411](0411-staging-changes-through-a-queue-of-tested-merged-revisions.md).

## Context

Merge-first staging changes what infra bootstrap lays for a project with a shared staging
target:

- a merge-first `[cd.<name>]`;
- the long checks as `after_merge` jobs;
- a reusable deployment inside each task's own sandbox.

Yulius ships its skills and workflows with each release, so new onboardings get the new default.
Projects already onboarded do not. Their `yulius.toml`, scripts and guidance are theirs, so they
change only through a pull request in their repository.

Those projects differ:

- their own commands and targets;
- vault grants a person made;
- often no shared staging target at all.

## Decision

- **The default, taught.**
  - The built-in `infra-bootstrap` skill (step 3), the Infra bootstrap workflow, the
    `delivery-pipeline` skill, the onboarding prompt and the new-project template all say the
    same thing.
  - A shared staging target is merge-first (`merge_first = true`), with its long checks as
    `after_merge = true` jobs (every check kept, at least one quick), and a `[local]` deployment.
  - Without a target, the full suite stays on pull requests and `[cd.dev] about` says so.
- **`[local]`, the task-local deployment.**
  - The table holds `deploy`, an optional `verify` and `about` (`parse_yulius_toml_local`).
  - Its scripts deploy and test the project inside the sandbox only: its ports, test data of
    its own, the dev credentials agent sessions get, and a refusal of any other target that the
    gate checks.
  - Staging SSH, release signing and production credentials stay named only in `[cd.*]`, which
    runs out of the agent's reach.
  - The briefing names it with the dev-server ports.
  - For a merge-first project it replaces "merges deploy nothing" with how staging changes.
- **The upgrade, a task.**
  - `BOOTSTRAP_REVISION` (now 1) names the default.
  - At a server start, each onboarded sandbox (onboarding `ready` or `not_needed`, not a
    playground, not removed) still below it gets one task: "Adopt merge-first staging (Infra
    bootstrap revision 1)".
  - The task follows the Infra bootstrap workflow and is owned by the sandbox's creator. Its
    origin is that of an automation, so it holds nobody's personal secrets.
  - Its prompt asks it to keep every project-specific command, job, target, secret and vault
    grant, and to switch only a real shared target. It reconciles conflicting guidance in the
    same pull request and reports switched, no target, or blocked.
  - Each sandbox's record (`bootstrap_upgrades`, migration 0079) is in one of three states.
    - **`laid`:** the sandbox is at the revision with nothing to upgrade. A sandbox is recorded
      `laid` when it is created (`projects.rs`), and when a start finds it not onboarded yet. Its
      onboarding lays the current default.
    - **`claimed`:** its task is being made.
    - **`linked`:** its task was made. Deleting the task leaves the record `linked`, so it is
      never made again.
  - The revision is claimed before the task is made, and the claim stays until the task is
    linked by a compare-and-set.
  - Every start finishes a claim still `claimed`:
    - It adopts the upgrade task made since the claim: a stop between making and linking it, or
      a store error after its record. The task is recognised by its origin, title and prompt.
    - Otherwise it makes the task: a stop before making it, or a creation refused for want of a
      Claude account.
  - A restart therefore makes no second task and never skips a project.
  - A sandbox that is not onboarded at the release (waiting, preparing, or skipped) is laid,
    although an onboarding task created before the release has the older prompt. Such a project
    adopts the default through the skill its agent reads, or a person asks for it; that case is
    narrow enough not to warrant an upgrade task.
- **Deliveries in progress.** Nothing pins them: a staging request whose revision staging
  already serves is answered `included` (ADR-0411), so the rollout never deploys one twice.

## Alternatives

- **Rewrite each project's `yulius.toml` from the server.** That is faster, but it is a change
  to a repository without review. It also cannot tell a staging machine from a preview or know
  which jobs are long, and it would override what people configured.
- **A person-started automation template.** That leaves projects on the old default until
  someone remembers. The goal is to migrate the onboarded sandboxes; a task in each does it
  through the normal workflow and reports.
- **A task at every start until the project switched.** A project with no shared target never
  switches; the task says so once.

## Consequences

- Every onboarded sandbox runs one agent task after the release that raises the revision. Its
  pull request goes through review and CI like any other, so a person sees what changes.
- A project without a shared target ends with no CI change and a recorded reason.
- A later change to the default raises `BOOTSTRAP_REVISION` and writes its own prompt.
