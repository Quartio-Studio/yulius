# 0314. The host starts bubblewrap by its own path and writes a clone's Git files through handles

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 2); task #465. Applies
[0174](0174-host-access-to-agent-owned-folders-goes-through-handles.md) and
[0257](0257-the-config-scrub-and-new-names-never-go-through-a-checked-path.md) to three more
places; amends SPEC-002 ADR-03 (how a contained run is started).

## Context

The server and its process hosts run as the service user outside every containment, and a
sandbox's whole directory, its home and its clone's `.git` included, is the agents' to write, at
the same paths on the host. The audit of task #465 found three places where the host trusted
what is there:

- **The program that contains a run.** A contained run is started as `bwrap …`, by name, with
  the run's environment. A task run's `PATH` begins with `<sandbox>/home/.cargo/bin` and
  `.local/bin`. A program named `bwrap` put there by an agent was started instead of
  bubblewrap at the next task launch in that sandbox: by the process host directly, or by
  `yulius sandbox-exec` after it joined the holder's namespaces, in both cases as the service
  user with the host's files in reach and before any containment. A test did it with a
  stand-in that left a marker.
- **`.git/info/attributes`.** Every task start keeps Yulius's block of merge attributes in the
  clone's `info/attributes` (SPEC-083). The file was read with `read_to_string` and replaced by
  writing `attributes.yulius-tmp` and renaming it, all by path, in a folder found by asking Git
  in the agent's worktree. A link at `attributes` made the server copy a host file to where the
  agent reads it; a link at the temporary name made it overwrite a host file with the agent's
  lines; a FIFO or an endless device held or filled the server. A test overwrote a file outside
  the repository.
- **The pre-push guard.** A new worktree's Git directory exists before its checkout ends. The
  guard's fifteen files were written there by path, each through `<name>.yulius-tmp`: a link at
  one of those names, planted by a running agent of the sandbox, made the server truncate the
  file it points at and write a hook script into it. A test did it.

## Decision

- **Bubblewrap is started by its path on the host.** The process host resolves `bwrap` against
  its own `PATH` (the server's, set by the operator): the first absolute directory holding an
  executable file of that name; the launch fails when there is none. `sandbox-exec` refuses a
  program named without an absolute path. The run's `PATH` no longer decides what contains it.
- **The clone's Git files are reached through handles** (`yulius_git::handles`, which the
  config scrub now shares): `.git` is opened from the clone's folder with `O_NOFOLLOW`, each
  folder below it from its parent's handle (made with `mkdirat` when it must exist), a file is
  read through an `O_NOFOLLOW | O_NONBLOCK` handle only when it is a regular file within a
  bound, and replaced by a new file of a random name created `O_EXCL` and renamed over the name
  in the same handle.
- **The clone is named by the caller, never asked of the worktree.** `set_merge_attributes`
  and `install_push_guard` take the sandbox's clone. For the guard Git still says which of the
  clone's Git directories is the worktree's (`rev-parse --path-format=absolute --git-dir
  --git-common-dir`); only the name below the common directory is used (nothing, or
  `worktrees/<name>`), opened below the clone's own `.git`.
- **`info/attributes` is bounded** at 256 KiB and must be text; a link, a FIFO or a larger
  file is refused, the task starts anyway (as before on a failure) and the log says why.

## Alternatives

- **Check, then write by path.** The check and the write are two lookups; the link goes in
  between (ADR-0257's finding).
- **Carry the manager's resolved `bwrap` path in every launch record.** One more field in a
  record that crosses three processes, for a path the process host can find the same way.
- **Clear `PATH` for the lookup only in `sandbox-exec`.** The uncontained fallback (no join)
  starts `bwrap` from the process host itself and would stay open.
- **Write the guard into a folder outside the sandbox** and point `core.hooksPath` there. The
  agent's own Git inside the containment must read the hooks, so the folder would have to be
  bound into every run.
- **Keep the temporary names fixed and create them `O_EXCL`.** A planted name then fails every
  install until someone removes it.

## Consequences

- A host without `bwrap` on the server's `PATH` fails a contained launch with "bwrap was not
  found on the host's PATH" instead of an exec error; `yulius doctor` already checks for it.
- `Git::common_dir` is gone; the attributes writer runs no Git.
- The server's own `git config` calls in a worktree (`core.hooksPath`, the commit identity,
  `extensions.worktreeConfig`, `core.fileMode`) and everything else Git itself writes there
  still go by path, as Git does. That, and the commands a repository's configuration can make
  Git run, is the remaining hole of
  [ADR-0114](0114-agents-hold-a-token-github-refuses-on-production.md)
  (#54 part B), taken up in a later slice of SPEC-483.
