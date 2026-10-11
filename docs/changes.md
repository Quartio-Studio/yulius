# Changes

Notable changes, newest first. Per-change detail lives in the merged pull requests and in the
evidence and changelog sections of [SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md)
(sections 14 and 15).

## 2026-10-11

- A full sandbox disk no longer ends running sessions or needs a person: a switch checks that the new session can start before it ends the running one (the reason shows in the terminal and the switch log); a refusal over the quota measures again when its reading is over a minute old; Yulius removes build caches (Cargo `target` and similar, or `[disk] caches`) of tasks that are not working and the worktrees of closed and cancelled tasks, prunes idle tasks' caches from 90 % of the quota, and tells the agent what was freed ([SPEC-005](../.ai/specs/SPEC-005-sandbox-disk.md)).
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
- Repository prepared for Yulius: `yulius.toml`, product summary, architecture, decision
  records, changes log; GitHub Actions jobs translated into Yulius CI
  (the workflows stay until the GitHub App may post commit statuses and change workflow files).
- Tasks wait for preparation instead of being refused (#175); onboarding survives multi-byte
  text and titles are the first real prompt (#173); Secrets adds from its header and popups
  close on an outside click (#171).
