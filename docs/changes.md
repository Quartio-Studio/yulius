# Changes

Notable changes, newest first. Per-change detail lives in the merged pull requests and in the
evidence and changelog sections of [SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md)
(sections 14 and 15).

## 2026-10-12

- A subscription whose only task is running but idle is read again: the usage round skipped every
  subscription with a running task, but an idle session reports nothing, so its reading went stale
  (5.6 h on production); it now skips only fresh readings (SPEC-007 USG-002, #4).
- Agents wake on new messages again: Yulius typed its notice and Enter in one write, which Claude
  Code read as a paste with a newline, so the notice never submitted and piled up in the input box;
  Enter now follows separately, and an unsent notice is only submitted.

## 2026-10-11

- Agent primitives follow the agentic-coding workshop methodology (SPEC-005, ADR-0016): repository
  skills are APM packages (`.apm/skills` source, `apm.yml`, `apm.lock.yaml`, generated
  `.agents/skills` and `.claude/skills`), the repository check rejects deployment drift and skills
  without a "Use when/for" sentence or over 200 lines, a primitive design standard
  (`docs/engineering/primitives.md`) and the `yulius-primitives` skill apply the pattern
  vocabulary, and `infra-bootstrap` now audits a project's conventions into primitives.
- A green pull request behind its base (the repository requires up-to-date branches) is not
  mistaken for a missing permission: the task is told to merge the base, and `delivery_merge`
  says why it refuses.
- A task whose pull request conflicts with its base is told to merge the base and resolve it;
  GitHub runs no checks on a conflicting pull request, so the agent would otherwise wait for them.
- Debug and test builds keep only line tables (dependencies none): a worktree's test build had
  reached 48 GB and parallel tasks filled the sandbox disk quota, so CI runs and session starts
  were refused.
- Agents carry delivery through on their own: Yulius tells a task when its pull request opened and
  when its checks passed or failed, the Stop gate asks for a missing spec at once (and follows a
  renamed one), and the briefing says never to ask a person — an issue covers what only a person
  can do.
- Yulius CI `quality` job failed at once on a clean machine (no rustup): `scripts/bootstrap.py`
  installs it when the run is unattended or given `--install-rustup` (no default toolchain, no
  profile edits).
- Everything a task does on GitHub shows the person who created it: commits with the no-reply
  address GitHub links to their account, and the agent's Git and `gh`, Yulius's pushes and pull
  requests, the agent's merges, promotions and issues with their own token; the App only when
  they have none, said once in the task's terminal (SPEC-001 GIT-003, #188).
- The GitHub CLI is logged in as the user in sandbox shells and editors: a `gh` shim fetches the
  user's own GitHub token per call (nothing stored on disk), like Git's credential helper
  ([SPEC-004](../.ai/specs/SPEC-004-gh-cli-login.md)).
- Subscriptions show usage all the time: for a sign-in the usage endpoint refuses, an idle
  subscription is read from the rate-limit headers of a one-token request every few minutes
  instead of waiting for a task (ADR-17, #4).
- Repository prepared for Yulius: `yulius.toml`, product summary, architecture, decision
  records, changes log; GitHub Actions jobs translated into Yulius CI
  (the workflows stay until the GitHub App may post commit statuses and change workflow files).
- Tasks wait for preparation instead of being refused (#175); onboarding survives multi-byte
  text and titles are the first real prompt (#173); Secrets adds from its header and popups
  close on an outside click (#171).
