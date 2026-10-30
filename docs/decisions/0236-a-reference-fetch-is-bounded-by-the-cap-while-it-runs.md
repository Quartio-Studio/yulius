# 0236. A reference fetch is bounded by the cap while it runs

## Status

Accepted. Specified in [SPEC-349](../../.ai/specs/SPEC-349-reference-repo-bounds-its-fetch-by-the-1.md);
task #333, fixes #920. Amends the consequences of
[ADR-0223](0223-a-reference-copy-carries-every-branch-and-tag-its-owner-can-read.md) (the cap was
checked after the fetch) and keeps the 1 GiB cap of
[ADR-0197](0197-tasks-read-other-repositories-of-their-owner-through-a-bundle.md).

## Context

`reference_repo` fetched every branch and tag of a repository into `<data>/tmp` and only then
compared the bundle with the 1 GiB a task gets. A repository of tens of GiB filled the server's
data disk, which also holds the database, for up to Git's 30-minute network timeout before it was
refused. Uploads already refuse a file that would bring the disk to its critical level (SPEC-276,
SPEC-325), and GitHub reports a repository's size, but neither was used here.

## Decision

- **Refuse what is known to be too large before anything runs.** The repository's `size` from
  GitHub (KiB) travels on `GhRepo`; above the cap, the tool refuses before taking a fetch slot,
  saying the size and the cap.
- **Refuse a fetch the disk has no room for**, with the uploads' check (`App::disk_room_if_short`,
  shared by `check_declared`): the fetch's peak is the fetched copy, its bundle and the attached
  bundle, three times GitHub's size, or three times the cap when GitHub gives none (or 0, as for a
  new repository).
- **Bound the copy while it is written.** The Git runner takes an optional size watch (paths and
  a byte limit) and measures them together every 250 ms while the command runs and once when it
  ends; past the limit it kills the process group, reaps it and fails with `GitError::TooLarge`;
  a file counts by the disk blocks it takes when more than its length (loose refs take a block
  each). `bundle_repository` takes the limit and watches the fetch's folder and then the bundle's
  file with Git's `<path>.lock`, which Git writes and renames once the bundle is complete. The
  tool's existing cleanup removes the folder.

## Alternatives

- **GitHub's size only.** Rejected as the only bound: GitHub updates it lazily and it measures
  GitHub's storage, not what a fetch writes; a just-pushed repository can report far less.
- **A shallow or partial fetch (`--depth`, `--filter=blob:limit`).** Rejected: the bundle must
  clone to a complete repository with its history (SPEC-330), and a filtered bundle does not.
- **A disk quota (a loop device or a filesystem quota on `<data>/tmp`).** Rejected for now: it
  needs privileges and setup the server does not have on every host (macOS is a development host).
- **Watching at the runner's 1-20 ms poll.** Rejected: walking the folder costs system calls per
  file; 250 ms bounds the overshoot to about 25 MB at 100 MB/s.
- **Keeping the disk under its critical level during the fetch too.** Not done: as for uploads,
  the check is made before the work with its declared size; the cap bounds the rest.

## Consequences

- A repository GitHub reports above 1 GiB is refused at once; one that grows past it while
  fetched costs at most about a quarter second of writes beyond the cap before Git is killed.
- A repository GitHub reports as small but that is larger is still refused, after at most the cap
  of transient disk use instead of the whole repository.
- GitHub's size counts all it stores (pull request refs too), so a repository it reports just
  above 1 GiB is refused even if its branches and tags would bundle below it.
- The disk check reserves nothing: two fetches and uploads checked at once may together pass the
  critical level, as uploads alone already could; the two fetch slots and the cap bound it.
- Other Git commands are unchanged: the watch is opt-in per invocation.
