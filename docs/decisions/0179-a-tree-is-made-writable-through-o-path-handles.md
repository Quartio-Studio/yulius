# 0179. A tree is made writable through O_PATH handles, its modes set through /proc/self/fd

## Status

Accepted. Specified in [SPEC-232](../../.ai/specs/SPEC-232-leftovers-make-writable-at-cannot-open-a.md);
task #218, fixes #680. Refines [0174](0174-host-access-to-agent-owned-folders-goes-through-handles.md)
("modes are set with `fchmod`") for directories the owner cannot read, in the leftovers removal of
[0176](0176-leftovers-below-a-sandbox-go-through-its-mapped-root.md).

## Context

- `remove_below` makes a tree that a tool left read-only removable with `make_writable_at`, which
  gives the owner `0o700` on every directory through handles opened with `O_NOFOLLOW`, so a link
  never carries the change outside the tree (ADR-0174).
- It opened each directory read-only before `fchmod`. That open needs the owner's read permission
  on the directory, so a directory of mode `0o000` or `0o300` was never changed and the removal
  failed with "Permission denied" on every round (#680). As root of a mapped namespace
  (`CAP_DAC_OVERRIDE`) the open succeeds, which hid the bug in the gate.
- A contained process can swap a directory below `sandboxes/` for a link at any moment, so the fix
  must not change a mode by name.

## Decision

- Each directory is opened from its parent's handle with `O_PATH | O_DIRECTORY | O_NOFOLLOW`
  (Linux), which needs no permission on the directory itself and never follows a link, and is
  used only when `fstat` on the handle says it is a directory.
- Its mode is set with `chmod` on the handle's `/proc/self/fd/N` entry, which resolves to the very
  inode the handle pinned, never to a name (`fchmod` refuses an `O_PATH` handle; glibc emulates
  `fchmodat(AT_SYMLINK_NOFOLLOW)` the same way). It is then listed through that entry and its
  children are opened relative to the handle.
- The walk is Linux only, as it was: it lists and removes through `/proc/self/fd`. Other platforms
  (macOS, a development host) only compile it, with a read-only open in place of `O_PATH`.
- A test that needs the kernel to check an owner's permission bits drops `CAP_DAC_OVERRIDE`,
  `CAP_DAC_READ_SEARCH` and `CAP_FOWNER` from its own thread's effective set (`rustix`'s `thread`
  feature, a Linux dev-dependency) instead of skipping as root: the gate runs as root.

## Alternatives

- **`fchmodat(parent, name)`** after checking the name through an `O_PATH` handle: it looks the
  name up again, so a directory or link swapped in after the check is changed (without
  `AT_SYMLINK_NOFOLLOW` a link is followed out of the tree) and differs from the one walked.
- **Reopen the child by name with `O_DIRECTORY | O_NOFOLLOW` after the chmod** to descend: a
  directory swapped in between would be walked instead; descending through the pinned handle has
  no such window.
- **Skip the test as root**, as the issue suggested: it would never run in the gate.

## Consequences

- An owner's directory without read or search permission no longer blocks a leftover's removal on
  the path without an isolation; with one, root of the mapped namespace already coped.
- The walk depends on `/proc` being mounted, as its listing already did.
