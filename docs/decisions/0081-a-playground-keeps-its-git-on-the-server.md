# 0081. A playground keeps its Git on the server: a bare `origin` of its own and local squash merges

## Status

Accepted. Specified in [SPEC-099](../../.ai/specs/SPEC-099-on-onboarding-we-want-a-mode-to-play-aro.md).

## Context

Teams want to try Yulius on their own code before they connect their GitHub organisation. Every
project path needed GitHub: import and template creation need the GitHub App, the clone came from
github.com, delivery merged pull requests on GitHub, CI listed them there, and access mirrored
repository permissions. Yet every Git operation of Yulius goes through the sandbox clone's
`origin` remote, a task starts from a local clone, and the superadmin already works without a
GitHub identity (ACC-012).

## Decision

1. **A playground is a sandbox whose `origin` is a bare repository in its own sandbox directory**
   (`<sandbox>/origin.git`). It is seeded from a template or from a folder uploaded from the
   browser. The clone, worktrees, merges and trains use it as they use GitHub, so the code stays
   on the server by construction.
2. **The record marks it with an empty repository owner** (`SandboxRecord::is_playground`). A
   GitHub owner is never empty, and the repository name is the slug. No migration is needed.
3. **Yulius never calls GitHub for a playground.** CI sync, hygiene, the automatic pull request and
   delivery reconciliation skip it, and the delivery views carry a playground note instead of a
   GitHub error.
4. **`delivery_merge` squashes the task branch onto the playground's integration branch.** The
   same task gates apply: spec, steps, a clean worktree, commits ahead, and the branch policy's
   title. The commit is pushed to the local `origin`, and the stage is recorded as merged. There is
   no CI in a playground, and the definition of done says so.
5. **From an uploaded `.git` only the history is taken** (`HEAD`, `packed-refs`, `shallow`,
   `objects/`, `refs/`), fetched with the hardened Git. Its `config` and hooks never reach the
   server.
6. **Access is the maker's and the superadmin's.**

## Alternatives

- **A local forge adapter emulating GitHub** (pull requests, statuses, merges behind
  `GitHubPort`). Delivery and CI would run unchanged, but it means far more code, it pretends
  pull requests exist, and a bug in it would be a bug in every GitHub path.
- **No remote.** Every `origin/<branch>` path (worktree base, ahead/behind, merges, trains) would
  need a second implementation.
- **A `kind` column.** It is explicit, but it needs a migration that parallel tasks must number,
  and it adds no information over the empty owner.
- **Taking the uploaded `.git` as is.** Its `config` can name filters, `fsmonitor` or `sshCommand`
  programs, and the server's Git runs outside containment.

## Consequences

- An instance is useful the minute it is set up: no GitHub App, organisation or repository is
  needed to try tasks, agents, the browser and the editor on one's own code.
- A playground has no Yulius CI and no pull requests. Agents run the gates themselves, and a
  person reviews the result in the Git page and the editor.
- A playground's `origin.git` sits inside the sandbox root, so its agents can write to it. The
  pre-push guard keeps `dev` and `main` for Yulius, as on GitHub, but no GitHub ruleset backs it
  up. That is acceptable for a trial.
- Follow-ups: CLI onboarding into a playground, moving a playground to GitHub, CI on local heads,
  and teammates without GitHub.
