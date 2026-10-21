---
yulius-task: fa2040dd-83cb-48a7-8264-9027d1f0a806
status: in-progress
---

# SPEC-253: Claude config dir links and the commondir canonicalize (#634)

Status: in-progress
Date: 2026-10-21
Owner: Yulius task #239
Scope authorization: task #239 (from task #235), the still-open part of #634. Applies
[ADR-0174](../../docs/decisions/0174-host-access-to-agent-owned-folders-goes-through-handles.md);
decided in [ADR-0193](../../docs/decisions/0193-the-claude-config-dir-is-walked-from-above-home-and-the-main-repository-is-read-from-the-git-layout.md).

## Goal

Before every task run and Superagent launch, Yulius prepares the agent's Claude Code config dir
(`<home>/.claude`) and records folder trust for the worktree and its main repository
(`ensure_claude_config`, SPEC-024, SPEC-106, SPEC-210). The host never writes through a link the
agent put in that dir's path, and never resolves agent-written git files on the host filesystem.

## Context

- PR #641 (task #197) made every read there open the last component with
  `O_NOFOLLOW | O_NONBLOCK` and use it only when `fstat` says regular file (`open_regular`).
- Still open (verified by task #235 on `dev` at 4e796d87):
  - **Linked config dir.** `ensure_claude_config` creates `config_dir` with a recursive
    `DirBuilder`, which accepts an existing link to a directory; `.claude.json`, `settings.json` and
    the diff-plugin stores (`plugins/store/*.json`) are then read and renamed over by path
    (`PlannedFile::write`). A task's home is `<sandbox>/home`, inside the read-write sandbox mount, so
    the agent can replace `home`, `home/.claude`, `plugins` or `store` with a link to any folder the
    service user can write, and the next launch reads and replaces files there.
  - **Host-side `canonicalize`.** `main_repository` resolves `<admin>/<commondir>` with
    `canonicalize`, where both the admin dir (`gitdir:` line of `<worktree>/.git`) and `commondir`
    are written by the agent. The host then stats any path the agent names and writes the result,
    link targets resolved, into the agent's `.claude.json`: an oracle for host paths and links.
    The trust keys are canonicalized a second time (`claude_config.rs:78-83`).
- `.config/vidoc.toml` has two ignores pointing at #634 ("as trusted", "trust arbitrary
  directories"), to be removed once this is fixed.
- Root cause: the code predates ADR-0174 and kept path-based writes; the existing tests created
  every directory themselves, so no test had a link on the way to the config dir.

## Requirements

- CFG-001. The config dir is opened from an anchor the agent cannot replace: the folder above its
  home (a task's sandbox directory, a person's user folder for the Superagent). The home and the
  config dir below it are opened one component at a time with `O_DIRECTORY | O_NOFOLLOW` and
  created `0700` with `mkdirat` when missing.
- CFG-002. A link (or a file) in place of the home or the config dir refuses the launch with an
  error naming the component ("… is a link or not a folder: Yulius follows no links"); nothing is
  created or written at the link's target.
- CFG-003. `.claude.json`, `settings.json` and the diff-plugin stores are read with `openat`
  (`O_NOFOLLOW | O_NONBLOCK`, regular files only) and replaced through the folder's handle: a new
  file `O_CREAT | O_EXCL | O_NOFOLLOW` (mode `0600`), synced, `renameat` within the same handle,
  the folder synced.
- CFG-004. `plugins` and `store` are opened through the config dir's handle without links and
  never created; a link (or anything else) there leaves the diff pane as it is and the launch goes
  on, its target untouched.
- CFG-005. The main repository of a linked worktree is computed lexically, with no filesystem
  access for the computed path: the admin dir is the `gitdir:` path (relative paths against the
  worktree's key) with `.` and `..` removed; it must be `<common>/worktrees/<name>` and its
  `commondir` file must lead (lexically) to that `<common>`, as `git worktree add` lays them out;
  otherwise the worktree alone is trusted. The trusted repository is `<common>`'s parent when
  `<common>` is named `.git`, else `<common>` (bare).
- CFG-006. The worktree's own trust key stays its real path (Claude Code keys trust by the real
  path of its working directory, which the host chose); no other path is canonicalized.
- CFG-007. The two `.config/vidoc.toml` ignores citing #634 are removed.

## Plan

- [x] Spec SPEC-253 for the config dir walk and lexical commondir
- [ ] Regression tests for linked config dirs and commondir resolution
- [ ] Config dir walked and written through handles
- [ ] Main repository derived lexically from the git layout
- [ ] Docs, ADR and Vidoc entries updated
- [ ] Gates green and independent review approved
- [ ] Merged into dev on green CI

## Acceptance criteria

- `home/.claude` linked to an outside folder: `ensure_claude_config` fails naming `.claude`, and
  the outside folder stays empty (no `.claude.json`, no `settings.json`). The same for `home`
  itself linked.
- `plugins` or `store` linked to an outside folder holding an open diff store: the launch
  succeeds and the outside store still reads `"open": true`.
- A swap of the config dir for a link after it was opened (test pause point) does not redirect
  the writes: they land in the folder that was opened.
- A `commondir` naming a path outside the git layout (`/`, a link, a path that does not exist)
  trusts the worktree alone; a real `git worktree add` (absolute and `--relative-paths`) and a
  bare repository's worktree still trust their main repository.
- Every existing `claude_config` test passes.

## Test plan

Unit tests in `crates/yulius-harness/src/claude_config.rs` (run by `scripts/dev test`):
`a_linked_config_dir_is_refused_and_its_target_untouched`,
`a_linked_plugin_folder_is_left_alone`, `writes_go_to_the_folder_that_was_opened` (pause point,
`cfg(test)`, ADR-0174), `the_main_repository_comes_from_the_git_layout_alone`; the existing
trust tests cover the git layouts. Then the full gate (`scripts/dev ci`) and `scripts/dev
security`.

## Decisions

- **Anchor = the folder above home**, derived inside `ensure_claude_config` from the config dir it
  is given (`<anchor>/<home>/<config>`), so both callers keep passing `<home>/.claude`. Passing an
  explicit anchor would make each caller repeat the same derivation. Recorded in ADR-0193.
- **Refuse rather than replace a linked config dir.** Replacing would remove an entry the agent
  made; a refusal names it and the agent (or a person) fixes it. Matches ADR-0174.
- **Lexical main repository instead of a handle walk.** Git writes the real absolute admin path
  (`git worktree add`) or a path relative to the worktree, and `commondir` is `../..`; lexical
  resolution gives the same answer for every layout Git makes, without touching paths the agent
  names. A walk with `O_NOFOLLOW` from `/` would refuse host links above the data directory (macOS
  `/var`), which the current code accepts.
- **Keep resolving the worktree path.** It is the host's own path; Claude Code resolves its working
  directory through the same links, so the key must match that.
