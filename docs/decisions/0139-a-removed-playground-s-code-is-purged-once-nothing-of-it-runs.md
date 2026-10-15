# 0139. A removed playground's code is purged on request, once nothing of its sandbox runs

## Status

Accepted. Specified in [SPEC-171](../../.ai/specs/SPEC-171-fix-409-420-yulius-onboard-uploads-a-che.md)
(#420). Amends [0081](0081-a-playground-keeps-its-git-on-the-server.md) (a removed playground no
longer counts toward the limit once purged).

## Context

A playground's code exists only on the server: its bare `origin` (`<data>/playgrounds/<id>.git`)
and its sandbox directory (clone, worktrees, home). Removing a sandbox keeps every file (PRJ-009),
so a removed playground kept its disk forever and still counted toward its maker's limit of 10.
Deleting it is destructive and cannot be undone: nothing else holds the code. Removal stops the
sandbox's tasks asynchronously; their runs end and are reaped after the removal returns, and a
shell, the sandbox browser or a CI run may still be running then.

## Decision

1. **Only a playground's code is purged.** A GitHub sandbox's code lives on GitHub; purging is
   refused for it.
2. **A purge is a request, recorded in `playground_purges`** (migration 0032) and audited
   (`sandbox.purge_requested`). An admin of a live playground (its maker or the superadmin) asks
   for it when removing it (`POST /api/sandboxes/{sid}/purge` removes and requests); the
   superadmin may request it for an already removed one.
3. **It deletes only once every process of the sandbox ended**: no live agent run and no task still
   stopping, no live shell, no running browser, and the CI checkouts claimed (no CI run). While one
   runs, nothing is deleted and the request waits. It is tried at once and again with every disk
   upkeep round (5 minutes). Then `origin` and the sandbox directory are deleted, the request is
   marked `purged_at` and audited (`sandbox.purged`).
4. **The quota counts what is on disk**: a purged playground's `origin` is gone, so its maker's
   limit no longer counts it. Task records, the removal export and the audit trail stay.
5. **The UI asks before it happens**: the removal dialog of a playground offers to delete its code
   for good and says it cannot be undone.

## Alternatives

- **Purge synchronously and refuse (409) while a process runs.** Simple, but right after removal a
  run is almost always still ending, so "delete on removal" would fail in practice and leave the
  person to retry later from somewhere.
- **Kill whatever still runs, then delete.** A kill is not proof a process is gone (a timeout is
  not proof a subprocess died); deleting under a process that still writes can recreate parts of
  the tree or fail half-way.
- **A `purge_requested_at` column on `sandboxes`.** Equivalent, but `SandboxRecord` is built in
  many places; a table of its own touches none of them and keeps the purge's facts together.
- **Expire removed playgrounds automatically after a delay.** Deleting someone's only copy of their
  code without their word is not ours to decide.

## Consequences

- Removing a playground can free its disk and its place in the limit of 10.
- A purge may take up to one upkeep round after the last process ended. It deletes the sandbox
  directory before `origin`, so one that fails half-way keeps the code and its place in the quota,
  and is retried on the next round.
- The sandbox host's own state of the sandbox (`<data>/sysroots/<id>`, `runtime/sandboxes/<id>`)
  stays; it is the host's to manage.
- A removed playground is invisible in the UI, so purging one removed before this change is an API
  call by the superadmin until a list of removed playgrounds exists.
