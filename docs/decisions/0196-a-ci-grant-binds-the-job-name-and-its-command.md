# 0196. A CI grant binds the job name and its command

## Status

Accepted. Specified in [SPEC-264](../../.ai/specs/SPEC-264-ci-grants-agent-secrets-only-to-the-trus.md);
task #250, issue #659. Amends [0116](0116-ci-jobs-use-named-agent-secrets-through-the-proxy.md)
(the grant is the integration branch's, by job name).

## Context

ADR-0116 grants a CI job the agent secrets that the integration branch's `[[ci.jobs]]` entry of
the same name lists. A pull request that changes the CI is tested with its own jobs, so it can
name a job like a granted one and give it another command: that command then ran with the
secret's placeholder and a proxy credential, and the proxy replaced the placeholder toward the
secret's bound hosts. SPEC-217 closed this for the scanning jobs only, by replacing a pull
request's scanning job with the trusted one (`keep_security_jobs`).

## Decision

- A job is granted the secrets of the integration branch's entry with the same name **and** the
  same command text, byte for byte. Matrix values share their definition's command and grant.
- A same-named job with another command runs without any secret, proxy or CA bundle, and its
  output says that its command is not the one the integration branch grants them to.
- The scanning jobs keep being replaced by the trusted ones (SPEC-217), so they keep their grant.

## Alternatives

- **Replace a pull request's job that reuses a granted name with the trusted job**, as
  `keep_security_jobs` does: closes the hole too, but a pull request changing a granted job's
  command would never test its new command, which is why a CI-changing pull request runs its own
  jobs.
- **Normalise the command before comparing** (whitespace, comments): more pull requests keep the
  grant, but two texts judged equal could still run differently; exact text is the simplest
  sound rule.
- **Grant by a hash of the command in `yulius.toml`**: the same check with more configuration to
  keep in sync.

## Consequences

- A pull request that changes a granted job's command is tested without the secret; the job's
  output says why, and the grant follows the new command once it is merged.
- The trusted command still runs the pull request's checkout (unchanged from ADR-0116): a granted
  command calling a script runs the pull request's script, bounded by the secret's hosts.
