---
yulius-task: ed4be085-6aa1-410e-81e8-9f1f42fea584
status: in-progress
---

# SPEC-228: Leftovers below a sandbox are removed as the sandbox's mapped root, link-safely

Status: in-progress
Date: 2026-10-18
Owner: task #214
Scope authorization: task #214: "Fix GitHub issue #675". Fixes #675, found by task #213. Extends
SPEC-215 (leftovers) and keeps SPEC-220 / ADR-0174 (no link followed into a sandbox); decision in
[ADR-0176](../../docs/decisions/0176-leftovers-below-a-sandbox-go-through-its-mapped-root.md).

## Goal

The automatic leftovers round deletes a removed sandbox's `ci`, worktrees and browser profile and
a live sandbox's stale CI checkout even when CI jobs or agents wrote files there as sub-uids, so
the "Failed: …/ci: Permission denied" banner clears after the next round. A removal that still
fails names the file that stayed.

## Context

Confirmed against `dev` at c3ba79d0.

- **Symptom.** Settings → Instance → Leftovers on the server (77925fd): "Failed:
  /var/lib/yulius/sandboxes/6b76ccc7-…/ci: Permission denied (os error 13)" after every round.
- **Root cause.** `remove_leftover_path` (`crates/yulius-app/src/leftovers.rs`) hands a path to
  the isolation's mapped removal only for the system trees and for `Kind::mapped()` kinds
  (`RemovedSystem`, `RemovedRuntime`, `StaleSmoke`, `Unowned`, `UnusedBase`). `RemovedCi`,
  `StaleCiCheckout`, `RemovedWorktrees`, `RemovedBrowser` (and the other kinds below
  `sandboxes/<sid>/`) go through `remove_below` as the service user. CI jobs and agents run in the
  sandbox's user namespace (inside uid 0 is the service user, other ids are sub-uids), so a file a
  job wrote as a non-root user lies in a directory owned by a sub-uid; the service user cannot
  unlink in it and `make_writable_at`'s `fchmod` fails with `EPERM` (ignored). The item stays
  "safe", is retried every hour and fails again.
- **Why the mapped removal could not simply be used.** `Manager::remove_owned_tree` →
  `base::remove_tree` resolves the path by path, as the service user (`make_writable`,
  `remove_dir_all`) and then as root of a mapped namespace (`sandbox-host remove-tree`, by path).
  For the system trees that is fine (no contained process can replace `sysroots/<sid>`), but a
  sandbox's runs can replace any folder below `sandboxes/<sid>/` with a link (ADR-0174): a stale
  checkout of a live sandbox removed by path could be steered into another sandbox, with
  `CAP_DAC_OVERRIDE` over every mapped id.
- **Why the tests missed it.** `crates/yulius/tests/leftovers.rs` writes every file as the test
  user, and its fake `remove_owned_tree` is `remove_dir_all`; nothing asserted which paths reach
  the mapped removal besides `sysroots/<sid>`.

## Requirements

- **LSM-001** With an isolation, every leftover path below `sandboxes/` (a removed sandbox's `ci`,
  `worktrees`, `browser`, everything of it a person deletes; a live sandbox's stale checkout,
  ended cache, orphan worktree, Git record, old draft) is removed through the isolation's
  `remove_owned_tree`. Without an isolation the service user's `remove_below` stays.
- **LSM-002** The mapped removal of a path below `sandboxes/` keeps SPEC-215/SPEC-220's link-safe
  walk: as root of the mapped namespace it opens every directory from the data directory down with
  `O_NOFOLLOW` and removes through the opened directory (`sandbox-host remove-tree --below <data>`);
  a link on the way fails the removal and nothing outside is touched. The system trees keep
  `base::remove_tree` (service user by path first). `remove-tree` always takes `--below`.
- **LSM-003** A removal that fails names the entry that could not be deleted (`<path of the
  entry>: <error>`), not only the item.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Regression test: a sandbox's leftovers reach the mapped removal
- [ ] Leftovers below a sandbox go through the link-safe mapped removal
- [ ] A failed removal names the file that stayed
- [ ] Spec, ADR, docs, review and gates green

## Acceptance criteria

- `tests/leftovers.rs`: after the round, the fake isolation's mapped removal received
  `sandboxes/old/ci`, `sandboxes/old/worktrees` and `sandboxes/old/browser`, and a stale checkout
  `sandboxes/live/ci/jobs/old` (fails on `dev`: none of them reached it).
- `remove_below` refuses a link on the way (existing test) and the `remove-tree --below` helper
  does too: a tree reached through a link is not removed, the target is kept.
- A removal that fails reports the entry (`/proc/self/ns/<entry>: …`), not only the item.

## Test plan

- Regression (fails before the fix): `leftovers::a_removed_sandbox_loses_its_disposable_parts…`
  and `leftovers::unused_ci_checkouts_go_while_no_run_runs` assert the mapped removal's paths.
- `yulius-app` unit test: a removal that fails (an entry of `/proc`, which nobody can unlink,
  root included) names that entry.
- `yulius` unit test of `sandbox-host remove-tree --below`: removes a plain tree, refuses one with
  a link on the way and keeps the link's target.
- Not testable here: the real mapped namespace (needs `bwrap`, `newuidmap` and sub-ids; the
  opt-in isolation suite). On the server after the update, the next round clears the banner.

## Decisions

- Route by path (below `sandboxes/`), not by kind: every folder of a sandbox is written by its
  runs, so every kind there can hold sub-uid files; `Kind::mapped()` is gone (the path decides).
- The link-safe walk runs inside the helper, reusing `yulius_app::leftovers::remove_below`
  (one implementation), rather than a second copy in `yulius-sandbox` or a descriptor handed
  across `bwrap`. See ADR-0176 for the alternatives.
- No separate service-user attempt for these paths: inside the mapped namespace uid 0 is the
  service user, so the helper removes whatever the service user could.
