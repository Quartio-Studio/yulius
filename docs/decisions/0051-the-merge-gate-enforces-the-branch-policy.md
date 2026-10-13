# 0051. Yulius's merge gate enforces the branch policy

## Status

Accepted. Specified in [SPEC-069](../../.ai/specs/SPEC-069-fix-223-yulius-s-merge-gate-enforces-the.md)
(#223). It replaces the GitHub Actions `Branch policy` check removed by SPEC-010.

## Context

The branch policy (typed short-lived branches, conventional one-line titles of at most 100
characters, changes into `dev`, promotions `dev` → `main` titled `release: promote dev to main`)
lived in `scripts/check_git_policy.py` and ran only as a GitHub Actions workflow reading the pull
request from GitHub's event. Yulius CI runs commands at a commit and has no pull request title or
base, and the rulesets now require only `yulius/ci`, so nothing enforced the policy. Every merge
Yulius performs, however, goes through its own merge gate.

## Decision

- The rules live in the domain (`yulius_domain::branch_policy`), checked by Yulius before every
  merge it performs: a task's pull request (`delivery_merge`, armed merges, the Git page) and an
  existing promotion pull request. `delivery_merge` checks before it arms a merge.
- A project configures the policy in `yulius.toml` `[branch_policy]`: `preset = "standard"` (this
  repository's rules, the default) or `"none"`, and optionally `types` replacing the standard
  conventional types. It is read from `yulius.toml` at the integration branch, so a pull request
  cannot relax its own gate; an unreadable or invalid file means the standard preset.
- A violation is a conflict naming the rule, the offending value and the fix. Agents fix it
  themselves (edit the title, move the work to a typed branch).
- Yulius's own names comply: promotion pull requests are titled `release: promote <integration>
  to <production>`, task titles take an allowed type and are clipped to 100 characters, and task
  branches always have a letter in their slug.
- `scripts/check_git_policy.py` and its test are removed.

## Alternatives

- **A Yulius CI job.** CI runs at a commit and has no title or base; passing them in would make
  the check depend on the pull request, not the commit, and re-run CI on every title edit.
- **Keep the Python script as a local check sharing the rules.** Two implementations of one
  grammar drift; the gate's refusal already tells an author what is wrong, and the domain's tests
  are the local check.
- **Policy off unless the project opts in.** The rules are Yulius's own discipline and Yulius
  generates compliant names, so the default protects every project; a project that does not want
  them opts out in one line.
- **Keys in `[delivery]`.** Its keys are strict: an older Yulius reading this repository's
  `yulius.toml` (the one running CI) would reject an unknown key, whereas it ignores a new table.

## Consequences

- Pull requests with long or non-conventional titles are refused until their title is fixed;
  merges made directly on GitHub are not checked (the rulesets still require `yulius/ci`).
- A promotion pull request opened by an older Yulius (`Promote dev to main`) must be retitled
  before Yulius merges it.
- Projects without `[branch_policy]` now get the standard rules; `preset = "none"` restores the
  earlier behaviour. Configured `types` must include `feat`, the type of Yulius's task branches.
- An imported task that keeps an untyped branch (`claude/…`) cannot be merged by Yulius under the
  standard rules; its project delivers it by hand or turns the policy off.
- The policy is read from the sandbox clone as last fetched; a change on GitHub applies after the
  next fetch. An unreadable or invalid policy means the standard rules, and the refusal says why.
