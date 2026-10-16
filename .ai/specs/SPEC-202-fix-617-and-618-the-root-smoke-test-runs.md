---
yulius-task: d0edd6c8-f580-4ab2-8790-146ab7bba4aa
status: in-progress
---

# SPEC-202: The root smoke test runs as the service user; a hand-run base build keeps live bases

Status: in-progress
Date: 2026-10-16
Owner: task #187
Scope authorization: task #187 (requested by MrFishPL), continuing task #186 (SPEC-201), which an
instance restart ended before its last part. Fixes #617 and #618, and the smoke's overwritten
compose line. Amends SPEC-196 DKL-007 and the base collection of ADR-22 (SPEC-002).

## Goal

The two operator commands documented in `docs/operations.md` are safe and do what they say:
`sudo yulius doctor --docker-smoke` from a root SSH shell runs the checks as the service user, and
a hand-run `yulius sandbox-base build` never removes a base that a running sandbox still uses.
The smoke prints one readable line per check.

## Context

- **#617.** `docker_smoke::main` re-ran itself through `systemd-run --uid=<data dir owner>` only
  when `wiring::sandbox::delegation()` failed. `cgroup::detect` accepts the parent of the
  process's cgroup when its control files are writable, which they always are for root. A root
  login shell's `user.slice/user-0.slice` therefore counted as delegated, `prepare` made an empty
  `sandboxes/` group there, and the smoke ran as uid 0, which has no subordinate id range:
  `/etc/subuid has no range of at least 65536 ids for root (0)` (VM, 2026-10-16).
- **#618.** `sandbox_cli::base` `BaseAction::Build` calls `base::collect(&bases, &keep, …)` with
  the CLI's `--keep`, empty when an operator runs it. Only the server's `Manager::spawn_build`
  fills `keep` from its live holders. The documented hand build deleted `base-3`, the lower layer
  of every running holder; their `/usr/lib` went empty until the service was killed and
  restarted (VM, 2026-10-16).
- **Overwritten compose line.** The smoke printed `last_line(compose.txt)` raw as the detail of
  "docker compose up (two services) in the old run". Compose's output carries carriage returns
  and escapes, and its last line is `web-1 exited with code 137`, the expected stop of the web
  container by `--abort-on-container-exit`, not the probe's result.

## Requirements

- **RSB-001** Run as root, `yulius doctor --docker-smoke` always re-runs itself through
  `systemd-run --uid=<the data directory's owner> -p Delegate=yes -p DelegateSubgroup=server`,
  without detecting or preparing the cgroup it was started in (so no `sandboxes/` group appears
  under a root login slice). When the data directory belongs to root, it refuses, naming why
  (sandboxes need a service user with a subordinate id range). Non-root callers keep today's
  path: a delegated cgroup runs the checks, else the refusal that asks for root.
- **RSB-002** `base::collect` never removes a base named by the record (`host.json`) of a live
  holder under `<data>/runtime/sandboxes/*/`, judged by `holder::alive` as the server judges it,
  whoever calls it and whatever `--keep` says. `--keep` still adds bases. Bases neither current,
  kept nor used by a live holder are still removed; a dead holder's record keeps nothing.
- **RSB-003** Every smoke check detail is printable on one terminal line: escape sequences and
  control characters are removed, and of a line rewritten with carriage returns only its last
  text is kept.
- **RSB-004** The compose check's detail is the probe service's result (its last line in
  compose's output), not the web container's expected exit.

## Plan

- [x] Spec SPEC-202 written and validated
- [ ] Root smoke always runs as the data directory's owner (#617)
- [ ] Base collection keeps every live holder's base (#618)
- [ ] Smoke check details are printable and compose shows the probe
- [ ] Docs, ADR and changes entry updated
- [ ] Gates green and independent review answered
- [ ] Merged into dev on green CI

## Acceptance criteria

- A unit test shows root takes the `systemd-run` path even when its own cgroup would pass
  `detect`, and that the delegation is not even looked at (fails before the fix).
- A unit test shows a root-owned data directory is refused rather than re-run as root.
- A unit test with a live holder record on `base-N` keeps `base-N` when `collect` gets no `--keep`
  (fails before the fix); unused bases and a dead holder's base are removed.
- A unit test shows a compose output with carriage returns and escapes gives the probe's plain
  line as the detail.
- `docs/operations.md` says what a manual build keeps; ADR-0161 records the collection rule.

## Test plan

- `cargo nextest` unit tests in `yulius` (`docker_smoke::tests`) and `yulius-sandbox`
  (`base::tests`), run red before each fix and green after.
- The whole gate: `scripts/dev ci` (check, lint, test, deps), and `scripts/dev security`.
- Not testable here (constraint of the task: nothing is built or restarted on the host): the
  operator runs, after promotion and update, `sudo yulius doctor --docker-smoke` from a root SSH
  shell, and `sudo -u yulius yulius sandbox-base build --data-dir /var/lib/yulius` while sandboxes
  run, then `python3 -c 'import encodings'` in a running sandbox.

## Decisions

- **Root never detects its own cgroup** (RSB-001) rather than `detect` refusing a root-owned
  unit: a server deliberately run as root under `Delegate=yes` has a root-owned unit too, and its
  own detection must not change. The smoke only ever needs root to switch user, so deciding by the
  effective uid is exact and leaves `detect` alone.
- **`collect` itself reads the holder records** (RSB-002, ADR-0161) rather than only the CLI: the
  rule holds for every caller, including the server's own build, where a holder started between
  the server's tick and the build's end was unprotected too. The server keeps passing `--keep`.
- **Detail cleaning at the check** (RSB-003): every check, not only compose, goes through it, so a
  later check cannot bring the bug back.
