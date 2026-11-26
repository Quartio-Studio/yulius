---
yulius-task: 728e003b-5abd-430b-82f2-d69fd9946b8c
status: in-progress
---

# SPEC-677: The superadmin deploys production and releases through the GitHub App

## Goal

The owner reported an urgent production blocker on 2026-11-26. On a customer instance the
superadmin, who signs in with a password and has no GitHub identity (ACC-012), cannot deploy
production. The CD page answers "Couldn't deploy production: Deploying main acts as a person on
GitHub, and the superadmin account has no GitHub identity." Yet the superadmin administers every
project. With this change the superadmin promotes through the Yulius GitHub App instead of being
refused. Every other rule of a production release stays as it is.

## Context

- `App::person_github_token` (`access.rs`) refuses an account without a GitHub login. Four actions
  call it: the production deploy's promotion (`ci/cd.rs`, `cd_promote`), the release
  (`delivery/promote.rs`, `promote_sandbox`), a person's merge (`delivery/merge.rs`,
  `merge_task_pr`) and the onboarding task's production protection (`production_protect.rs`).
- Opening a pull request already acts through the App for the superadmin
  (`App::dlv_author_token`, `delivery/open.rs`). A task whose owner has no usable token acts
  through the App too (`App::task_github_token`), so the superadmin's agent already merges its pull
  requests as the App.
- The production deploy and the release are refused before any token is asked unless the person
  is an administrator who typed the project's slug (`cd::authorize`, `cd::may_deploy`). They then
  promote only the revision staging verified (SPEC-575 MFS-009), after the rollback drill and under
  the branch policy (`dlv_promote_exactly`, `dlv_check_policy`). GitHub's own ruleset on the
  production branch still applies to whichever token writes it.

## Requirements

- SAP-001: A production deploy with promotion and a release (`POST …/delivery/promote`) by the
  superadmin without a GitHub identity use the GitHub App's installation token for the project's
  owner (`App::person_or_app_token`). A person with a GitHub login, the superadmin included, keeps
  using their own token. Anyone else without a GitHub login (a guest) is still refused as before.
- SAP-002: Every other rule is unchanged and runs first. These are administrator access, the
  typed slug, only the staging-verified revision, the rollback drill and the branch policy.
- SAP-003: The run's note says `promoted 'dev' to 'main' at <sha> by the superadmin through the
  GitHub App`. The release's audit entries (`delivery.promote`, `delivery.promotion_pr.open`,
  `delivery.promote.reconciled`) name the actor `superadmin (through the GitHub App)`.
- SAP-004: When GitHub refuses a write by the App (HTTP 403, 405 or 422, for example a ruleset
  whose bypass list leaves the App out), the refusal says that GitHub refused the Yulius GitHub App
  acting for the superadmin, gives GitHub's reason, and says what to do: let the App through the
  production branch's ruleset, or do it on GitHub. The error keeps its kind.
- SAP-005: A person's merge of a task pull request (`POST …/delivery/merge`) by the superadmin
  without a GitHub identity goes through the App too. Its `delivery.merge` audit names the App.
  The same gates apply, and it is the same token the superadmin's agent already merges with.

## Plan

- [ ] Spec: the superadmin acts through the App for production
- [ ] Failing tests: superadmin deploy, release and merge through the App
- [ ] The App's token and the App's name in the note and the audit
- [ ] A plain refusal when GitHub refuses the App
- [ ] Docs, changes entry and evidence
- [ ] Independent review approves the head

## Acceptance criteria

- The superadmin deploys production on a `promote = true` target. `main` moves through the App
  (`inst` in the fake GitHub), and the run's note says "by the superadmin through the GitHub App".
- The superadmin's release promotes through the App, and its `delivery.promote` audit names the
  App.
- A writer (not an administrator) is still refused a production deploy and a release (403), and
  nothing moves.
- A GitHub-signed-in administrator still promotes with their own token (`bob` in the fake), and
  the note does not mention the App.
- A ruleset refusal of the App's write reads "GitHub refused the Yulius GitHub App".

## Test plan

- `crates/yulius/tests/cd.rs`: the superadmin deploys production through the App. The existing
  administrator test is extended with an assertion on the token used. A ruleset refusal of the
  App's fast-forward gives the plain refusal.
- `crates/yulius/tests/delivery.rs`: the ACC-012 test becomes "the superadmin delivers, merges
  and releases through the App", and a writer's release is refused.
- `crates/yulius-app/src/access.rs` unit tests: which GitHub errors count as the App's refusal.
- Run the whole `yulius` integration binary's `cd::` and `delivery::` files and `yulius-app`'s
  unit tests.

## Decisions

- **Superadmin only, through the App.** The App's installation token is already what Yulius uses
  for the superadmin's pull requests and its agent's merges, so this adds no new authority holder.
  Guests stay refused: they are not instance owners, and an App fallback for them would let a
  project admin without a GitHub identity act beyond their own GitHub rights. GitHub's ruleset on
  the production branch still binds the App, which is not in a default bypass list.
- **Who acted is read from the account, not passed along.** A superadmin without a GitHub login
  can reach GitHub only through the App, so the audit label follows from the account
  (`App::dlv_github_actor`). Nothing new has to be threaded through `dlv_promote_exactly`.
- **The App's refusal is recognised from GitHub's HTTP 403, 405 and 422 answers.** Yulius's own
  refusals ("nothing was promoted: …") carry no HTTP code and pass through unchanged.
- **Merging is done too (SAP-005).** It is a one-line token change behind the same gates, and the
  superadmin's agent already merges with that token.
- **Not done: protecting the production branch** (`production_protect.rs`). It first checks that
  the task's owner is a repository admin *on GitHub*, then creates a ruleset that keeps the App
  and agents out. The superadmin has no GitHub account to check, the App would need Administration
  write, and the App would write the very rule that binds it. That is not equally small or safe,
  so the onboarding task keeps asking a person ("Needs you") as ADR-0129 says.
