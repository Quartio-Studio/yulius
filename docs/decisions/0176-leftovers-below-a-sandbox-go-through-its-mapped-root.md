# 0176. Leftovers below a sandbox go through its mapped root, walking down without links

## Status

Accepted. Specified in [SPEC-228](../../.ai/specs/SPEC-228-fix-675-leftover-cleanup-of-sandbox-owne.md);
task #214, fixes #675. Amends [0170](0170-leftovers-are-deleted-by-kind-rules-checked-again-at-deletion.md)
(which kinds use the mapped removal) and applies [0174](0174-host-access-to-agent-owned-folders-goes-through-handles.md).

## Context

- A sandbox's CI jobs and agents run in its user namespace: inside, uid 0 is the service user and
  every other id is a sub-uid. A job that writes as a non-root user leaves directories the service
  user cannot unlink in, and cannot `chmod`. The leftovers rounds removed everything below
  `sandboxes/<sid>/` as the service user, so a removed sandbox's `ci` failed with "Permission
  denied" every hour (#675).
- The removal that copes with mapped ids, `base::remove_tree`, resolves its path by path, as the
  service user and then as root of a mapped namespace with `CAP_DAC_OVERRIDE` over every mapped
  id. Below `sandboxes/<sid>/` a sandbox's runs can swap any folder for a link at any moment
  (ADR-0174): a path-based removal there (a live sandbox's stale CI checkout) could be steered into
  another sandbox's files.

## Decision

- **By path, not by kind.** With an isolation, every leftover path below `sandboxes/` goes through
  the isolation's `remove_owned_tree`, whatever its kind; `Kind::mapped()` is gone. Without one the
  service user's `remove_below` stays.
- **Link-safe as mapped root.** For a path below `sandboxes/`, `Manager::remove_owned_tree` runs
  `yulius sandbox-host remove-tree --below <data dir> <path>` as root of a mapped namespace; the
  helper's body is `yulius_app::leftovers::remove_below`, the same walk the service user's removal
  uses (each directory from the data directory down opened with `O_NOFOLLOW`, then removed handle
  by handle with `unlinkat`). The system trees (`sysroots/`, `runtime/sandboxes/`, `system/`) keep
  `base::remove_tree`.
- **Errors name the entry.** The handle-by-handle removal reports the entry that could not be
  removed (`<path>: <error>`), so the Leftovers banner says which file stayed.

## Alternatives

- **Add the four kinds to `Kind::mapped()`.** Smallest diff, but it routes a live sandbox's
  checkout through the path-based `base::remove_tree` (the hole above), and leaves the other kinds
  below a sandbox (ended caches, orphan worktrees, drafts) failing the same way.
- **Try as the service user first, mapped root only on failure.** Saves a namespace start per
  item, but a mapped root removes whatever the service user can (uid 0 inside is the service
  user), and the fallback is the path nothing can test without sub-ids.
- **A second link-safe removal in `yulius-sandbox`, or a directory handle passed across `bwrap`.**
  Two copies of the walk to keep equal, or descriptor plumbing through `Mapper::run`; the helper
  already lives in the `yulius` binary, which links `yulius-app`.

## Consequences

- Sub-uid files below a sandbox are removed by the automatic rounds; a link a sandbox plants on the
  way fails that one removal and nothing outside is touched.
- One short-lived mapped namespace per item below `sandboxes/` (hourly, a handful of items).
- The real mapped namespace is not exercised by the default suite (needs `bwrap`, `newuidmap` and
  sub-ids); the helper's walk and the routing are.
