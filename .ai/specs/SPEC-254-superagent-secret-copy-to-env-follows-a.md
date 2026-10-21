---
yulius-task: c2e208f5-5f87-4e62-a54d-db846db56189
status: in-progress
---

# SPEC-254: Superagent secret_copy_to_env follows a sandbox repo swapped for a link

Status: in-progress
Date: 2026-10-21
Owner: Yulius task #240
Scope authorization: task #240, started by task #235 (work through open issues): fix
[#635](https://github.com/Quartio-Studio/yulius/issues/635), found by the independent review of
task #198 ([SPEC-211](SPEC-211-yulius-should-be-able-to-create-secrets.md)).

## Goal

The Superagent's `secret_copy_to_env` writes an allowed secret's real value only into its own
sandbox repository: a `repo` folder it swapped for a link to another folder is refused, and nothing
is written anywhere.

## Context

- Symptom: for the Superagent, `secret_user_folder` (`vault/users.rs`) resolved the target root as
  `std::fs::canonicalize(sandbox_repo)`, which follows links, and `open_folder`
  (`vault/env_files.rs`) only checked `inside(root, real)` against that already redirected root.
- Root cause: the root itself was resolved by path, through links, from a folder the agent can
  write. The Superagent's containment binds its sandbox folder writable
  (`yulius-ptyhost/src/containment.rs`, `sandbox_root`), so it can rename
  `data/sandboxes/<id>/repo` and put a link to another folder (another project's repository, the
  person's home) in its place. The Git check (`is_disposable`) and the temporary-name check
  (`writing`) ran against the same redirected root, so an ignoring Git repository elsewhere passed
  them too.
- Why the tests missed it: the Superagent's copy test (`cockpit.rs`,
  `the_superagent_copies_allowed_secrets_into_its_repositorys_ignored_env`) never replaces the
  repository; link tests (`env_files.rs` unit tests) start below an already trusted root.
- The anchored version exists: `secret_source_root` (`vault/create.rs`, SPEC-211 review finding 1)
  walks from the canonical sandbox folder (`sandbox_dir`) with `repo` (Superagent) or the
  worktree's folders (task) below it, used only by `secret_create_from_file`.
- Decision 0174 (host access to agent-owned folders goes through handles) is the rule this
  applies.

## Requirements

- ACR-001. `secret_copy_to_env` opens its target folder from the anchor `secret_source_root`
  gives for the work folder: the canonical sandbox folder, then `repo` (Superagent) or the task
  worktree's folders below it, then the folders the caller named, one at a time without following
  links. The folder reached must be inside the work folder (not only inside the anchor).
- ACR-002. A Superagent whose `repo` is a link (or not a folder) is refused with "the folder you
  work in is a link or not a folder: Yulius follows no links"; nothing is read, asked for or
  written, in the sandbox or in the link's target.
- ACR-003. The Git checks (`is_disposable` for the target, the temporary name and the
  `.env.example`) run in the anchored work folder (`<canonical sandbox>/repo`, or the task's
  canonical worktree), never in a path resolved through a link.
- ACR-004. Task copies behave as today: the same files, paths in answers and messages relative to
  the worktree.

## Plan

- [x] Spec for the anchored copy root
- [x] Regression test: a swapped repo is refused and nothing is written
- [x] Superagent copies open from the canonical sandbox folder
- [x] Architecture and changes describe the anchored root
- [ ] Gates green and independent review approved

## Acceptance criteria

- An integration test replaces `sandboxes/s1/repo` with a link to another ignoring Git repository;
  the Superagent's `secret_copy_to_env` of an allowed secret is refused with the "follows no links"
  message, and no `.env` or `.env.example` appears in the link's target or the moved repository.
- The existing Superagent and task copy tests (`cockpit.rs`, `secrets_env.rs`, `secrets_scopes.rs`,
  `secrets_create.rs`) pass unchanged.
- `docs/architecture.md` says `secret_copy_to_env` walks from the canonical sandbox folder.

## Test plan

- Regression: `cockpit::the_superagent_never_copies_into_a_repo_swapped_for_a_link` (fails before
  the fix: the value lands in the link's target).
- Unit: `env_files::tests::hidden_folders_root_the_walk_below_the_anchor`: with hidden anchor
  folders, paths are relative to the work folder, a folder outside it (but inside the anchor) is
  refused, and so is a hidden folder swapped for a link.
- Unchanged behavior: the `secrets_env`, `secrets_scopes`, `secrets_create` and `cockpit` suites.
- Gates: `scripts/dev ci`, `scripts/dev security`.

## Decisions

- Reuse `secret_source_root` (`vault/create.rs`, now `pub(super)`; `secret_user_folder` is
  removed) rather than a second anchoring for copies: one rule for both tools that
  touch the work folder. Anchoring task copies too changes nothing for them (their worktree is
  canonical and checked inside the sandbox) and removes a path-based root.
- `open_folder` gives `Folder::relative` below the hidden folders (the work folder), so Git paths
  and messages stay what they were; `secret_create_from_file` never read it with hidden folders.
- No new ADR: this applies decision 0174; nothing new is decided.
- Not fixed here: Git runs by path in the anchored work folder, so a swap between the walk and the
  Git check could make Git answer for another folder. The write still goes through the opened
  handle into the real repository, so at worst the Superagent's own repository gets a `.env` its
  `.gitignore` may not cover, which it could cause by editing `.gitignore` itself.
