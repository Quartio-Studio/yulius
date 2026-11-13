# 0315. What the host reads in a sandbox it reads through handles, bounded and never waiting

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 3); task #465. Applies
[0174](0174-host-access-to-agent-owned-folders-goes-through-handles.md) to five readers it did
not cover; follows [0314](0314-the-host-starts-bubblewrap-by-its-own-path-and-writes-git-files-through-handles.md).

## Context

ADR-0174 says how the server writes and reads where an agent can write: through handles that
follow no link. The audit of task #465 found five readers that still went by path:

- **Session transcripts.** A session's transcript is `<sandbox>/home/.claude/projects/<slug>/
  <session>.jsonl`. The pages people read, the agents' `session_read`, the search index and the
  run watcher's tail opened it with `File::open`. A FIFO there held the opening thread for good:
  one of the server's blocking threads per read, or a runtime worker per watched run, until the
  server's file and Git work stopped for every sandbox. A link there, or at a folder on the way,
  was read as the session's transcript and indexed for search.
- **The sandbox system's `passwd` and `group`.** People's identity files in a sandbox are built
  on `upper/etc/passwd` of its system layer, whose `/etc` root in the sandbox owns. The server
  took the file when `is_file()` said so (a link is followed) and read it whole before looking
  at its size: a link to a file of the service user (the workspace secret, a key file) was
  copied into the `/etc/passwd` of the next shell a person opened there, and a huge file was
  read into memory.
- **Numbered directories.** `record_number` lists a directory of every live task's worktree to
  find the next free number. The directory is named by the agent; a link in the worktree made
  the answer tell the highest numbered name of a folder of the host.
- **Disk usage.** The quota's measurement walked the sandbox by path: a folder swapped for a
  link between its listing and its visit sent the walk into the host's folders, whose sizes
  and names then showed in the sandbox's usage.
- **Untracked files in a review.** The task review read an untracked file after a
  `symlink_metadata` check, by the same path again: a folder on the way swapped for a link
  after Git listed the file showed a file of the host in the review.

## Decision

- **A transcript is opened from the sandbox directory** (`transcripts::open`): every folder
  below it from its parent with `O_NOFOLLOW`, the file with `O_NOFOLLOW | O_NONBLOCK`, and only
  a regular file is read; its size is asked of the same handle. A transcript outside the
  sandbox directory is a copy in the server's own folders and is opened the same way at its
  name. The readers take the open file. The harness's tail opens the file the same way at its
  name on every poll.
- **The sandbox adapter reads `passwd` and `group` itself** (`manager::system_file_text`) and
  hands the text on: `upper`, `etc` and the file from one handle to the next, a regular file of
  at most 1 MiB. Anything else at the name or over `etc` (a link, a whiteout, a FIFO, a larger
  file) gives nothing: in the sandbox it hides the base's file too.
- **A numbered directory is listed** from the sandbox directory down with
  `files::open_dirs_below`; a link on the way gives no names.
- **Disk usage is measured from handles**: a folder is opened from its parent's handle without
  following a link and its entries are looked at with `fstatat` from it. A root that is a link
  is not measured.
- **An untracked file is read** from the worktree's folder through `yulius_git::handles`, at
  most one byte more than is shown; a link or a FIFO is listed without lines.

## Alternatives

- **Check with `symlink_metadata`, then open.** Two lookups of one path; the swap goes in
  between.
- **Resolve the real path and check it is inside the sandbox** (`files::open_inside`). It
  needs `/proc` and a second look; the walk from the sandbox directory needs neither.
- **A timeout around the open.** The blocked thread stays blocked; a blocking open cannot be
  cancelled.
- **Follow a link in `upper/etc` inside the system layer**, as the sandbox's own processes
  would see it. It would need the overlay's resolution rules on the host; a system whose
  `passwd` is a link gets the generated lines only.
- **Walk the tail's folders too.** The watcher feeds usage and activity, not text shown to
  anyone; a transcript that is no regular file ends its watch's reads, which is enough there.

## Consequences

- A session whose transcript is not a regular file has no page, no search index and no
  watcher lines; the task's status then comes from its hooks alone.
- A sandbox system whose `/etc/passwd` or `/etc/group` is a link, or larger than 1 MiB, gives
  people there the generated lines only (their own user, `root`, `nobody`).
- The join record's `system_passwd` and `system_group` carry text instead of a path.
- The measurement opens one handle per folder on the way to the one it lists; it visits the
  same entries as before, and a linked root counts nothing where it counted one entry.
