# 0174. Host access to agent-owned folders goes through handles opened without links

## Status

Accepted. Specified in [SPEC-220](../../.ai/specs/SPEC-220-fix-657-host-writes-into-agent-owned-fol.md);
task #206, fixes #657. It generalizes the rule `secret_copy_to_env` already follows
(SPEC-129, `vault/env_files.rs`) and the ones of #634 (`claude_config.rs`) and #649
(`leftovers.rs`). Refined by [0179](0179-a-tree-is-made-writable-through-o-path-handles.md): a
removal makes directories writable through `O_PATH` handles and `/proc/self/fd`, not `fchmod`.

## Context

- Yulius runs outside the agent's containment, as the service user, with more rights than the
  agent. A sandbox's whole directory (`repo`, `worktrees`, `home`, `attachments`) is mounted
  read-write into its runs and shells, so a contained process can rename, replace or link any
  folder of it at any moment; only the sandbox directory itself, the mount point, is out of reach.
- The server still read and wrote such folders by path after checking them by path: imported
  transcripts, uploads, skill copies, editor reads and saves (#657). A link planted at a fixed
  name, or swapped in between the check and the use, made the server truncate, replace or show a
  host file (the SQLite database, another sandbox's `.git/hooks`).
- Path checks cannot close this: whatever a path resolved to when it was checked, it may resolve
  elsewhere a microsecond later.

## Decision

- **Anchor, then handles.** A folder the server uses below a sandbox is opened once, from an
  anchor the agent cannot replace (the sandbox directory, or a canonical editor root), one
  component at a time with `O_DIRECTORY | O_NOFOLLOW`; missing folders are made with `mkdirat`.
  Where links inside an editor root are followed by design, the folder or file is opened from its
  resolved path and used only when the handle's real path (`/proc/self/fd`, `F_GETPATH`) is still
  inside the root and out of `.git`.
- **Everything else through the handle.** Files are created with `O_CREAT | O_EXCL | O_NOFOLLOW`
  under a random name, then `linkat`/`renameat` within the same handle; modes are set with
  `fchmod`; names are tested with `fstatat(AT_SYMLINK_NOFOLLOW)`; reads open with
  `O_NOFOLLOW | O_NONBLOCK` and use only regular files; trees are removed handle by handle with
  `unlinkat`. A link is never replaced, followed or removed through.
- **Races are tested deterministically.** `yulius-app` has test-only pause points
  (`yulius_app::pause`, compiled only with its `test-support` feature, which only the test build
  of `yulius` enables; the harness uses `cfg(test)`): a test swaps a folder for a link exactly
  between the check and the use. No sleeps and no rename loops.

## Alternatives

- **Re-check the path right before each use.** What uploads did: it shrinks the window but cannot
  close it, and every new use needs its own check.
- **Do the writes inside the sandbox** (a helper run in the agent's containment). Correct, but a
  process per transcript, upload or save, and the server needs the result anyway.
- **`openat2(RESOLVE_BENEATH)`** would do the walk in one call, but it is Linux 5.6+ only and macOS
  is a supported development host (ADR-0169); the component walk works on both.

## Consequences

- An agent that links a folder Yulius writes into makes that one operation fail with a message
  naming the link, instead of steering it; links are still fine wherever the editor follows them
  inside its root.
- New code that writes into a sandbox must take a handle first; reviews and the Vidoc scan look
  for path-based writes there.
- `files_list`, folder creation and deletion in the editor, and attachment and draft removal,
  went through handles too in [SPEC-265](../../.ai/specs/SPEC-265-editor-delete-folder-create-and-listing.md)
  (#660): walked from the sandbox directory, so they need no real-path check.
