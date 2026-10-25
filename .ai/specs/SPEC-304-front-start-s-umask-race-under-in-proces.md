---
yulius-task: 1414d3b5-ea46-44c2-9932-fbc8092c73d5
status: done
---

# SPEC-304: The Docker front's socket is bound under a thread-private umask (#825)

Status: done
Date: 2026-10-25
Owner: Yulius task #290
Scope authorization: task #290: "Solve GitHub issue #825 […] in one pull request whose description
says `Fixes #825`." Expected: "the socket is 0660 whatever runs concurrently".

## Goal

`Front::start` (SPEC-196 DKL-002) creates `<state>/docker/docker.sock` 0660 however many fronts
start at once in one process, and leaves the process umask untouched.

## Context

- Symptom (#825): under plain `cargo test -p yulius-sandbox --lib` (every unit test a thread of
  one process), `the_socket_replaces_a_planted_link_and_is_group_only` once saw a socket mode other
  than 0660. nextest (the gate, one process per test) never hit it.
- Code path: `Front::start` (`crates/yulius-sandbox/src/docker_front.rs`) sets the umask to 0o117,
  binds and restores the mask it saved. The umask is process-wide (one `fs_struct` shared by every
  thread). Two failure modes: A sets, B sets, A restores, B binds under the process default (0077
  here: a 0700 socket); and B then restores the 0o117 it saved, so the process keeps 0o117 and any
  directory another test creates afterwards lacks its x bits.
- Production: the issue says one server can start fronts of two sandboxes concurrently. It
  cannot race: `Front::start`'s only production caller is `docker_front::run`, the body of
  `yulius sandbox-host docker`, a process of its own per front (`SandboxManager` spawns it,
  `crates/yulius-sandbox/src/manager.rs`), which starts one front before it serves anything. The
  race is test-only.
- Why the tests missed it: each test starts one front; nextest gives each test a process.
- The mode comes from the umask on purpose (ADR-0158): the directory is writable by the engine, so
  a `chmod` by name after `bind` could follow a link planted in between.

## Requirements

- R1. The socket `Front::start` binds is created 0660 (never chmod'ed by name) whatever other
  threads of the process do, including other `Front::start` calls.
- R2. `Front::start` never changes the process umask, even for a moment: other threads creating
  files meanwhile get the process umask.
- R3. Everything else of DKL-002 stays: a stale socket or planted link is replaced, never
  followed; `replaced()` sees the bound inode.

## Plan

- [x] Spec for the race-free socket bind
- [x] Regression test with concurrent fronts fails
- [x] Socket bound under a thread-private umask
- [x] ADR, docs and changes record the rule
- [x] Independent review answered and gates green
- [x] Pull request ready to merge on green CI

## Acceptance criteria

- A test starting 8 fronts at once (a barrier, a multi-thread runtime, 25 rounds, separate
  directories) sees every socket 0660 and the process umask (`/proc/self/status`) unchanged
  afterwards; it fails before the fix.
- The six existing `docker_front` tests pass, and `cargo test -p yulius-sandbox --lib` passes
  repeatedly.

## Test plan

- `docker_front::tests::fronts_started_at_once_bind_group_only_sockets_and_keep_the_umask`: the
  acceptance test above, committed first and run against the old code to show it failing.
- `cargo test -p yulius-sandbox --lib` several times in a row (in-process, the setting of #825),
  and the gate (`scripts/dev ci`, nextest).

## Decisions

- **Thread-private umask (ADR-0217).** The socket is bound on a short-lived thread that first
  calls `unshare(CLONE_FS)`, which gives it its own copy of the umask (and cwd and root), sets the
  umask 0o117 there and binds. Allowed in a multithreaded process and unprivileged. Rejected: a
  process-wide mutex around set/bind/restore (serializes fronts, but every other thread creating a
  file in the window still gets 0o117, so R2 fails); binding in a private subdirectory and renaming
  (needs a `chmod` by name, which a writer of the directory with the same uid, the engine, could
  redirect with a link; doing it without following links takes `fchmodat2` (Linux 6.6) or libc's
  `O_PATH` emulation, more moving parts than a private umask).
- **Linux only.** `unshare` is Linux's. Elsewhere the bind keeps the process umask, set and
  restored as before (review round 1: refusing broke the front's tests on a development Mac); the
  front itself runs only on Linux, inside a holder's namespaces. The regression test is Linux-only
  (it reads `/proc/self/status`).
- **Thread start failure is an error** (review round 1): `Builder::spawn_scoped`, not
  `Scope::spawn`, so a sandbox at its process limit gets `docker.sock: …` instead of a panic.

## Evidence

- **Reproduced** (test first, `4682edbd`, old code): `cargo test -p yulius-sandbox --lib
  docker_front::tests::fronts_started` failed 3 of 3 runs, with both modes: socket mode 0700 (448)
  or 0600 (384) instead of 0660 (432), and the process umask left `0117` (was `0077`).
- **Fixed** (`c9765ec9`, review fixes `684f40c6`): the 7 `docker_front` tests passed 5 of 5
  runs; `cargo test -p yulius-sandbox --lib` (125 tests, in one process) passed 10 of 10 runs in a
  row after the fix and 10 of 10 again after the review fixes.
- **Mutation check**: with the `own_umask()` call removed the new test fails again (socket 0700).
- **Lint**: `cargo clippy -p yulius-sandbox --all-targets -D warnings` clean for Linux and for
  `aarch64-apple-darwin` (the non-Linux `own_umask`); `cargo fmt --check` clean.
- **Gate**: `scripts/dev ci` at `8b0bde37`: exit 0, nextest 2612 passed, 16 skipped; rerun at the
  final head before delivery (see the pull request).
- **Security**: `scripts/dev security` at `322dfc8a`: not run, exit 2: Vidoc answered HTTP 429
  "This API key has used its 50,000,000 tokens" (unrun, not passed); the pull request's CI
  `security` job runs it again.
- **Review** (fresh session, `om-code-review`): round 1 at `8b0bde37` APPROVE with 2 minors (macOS
  test runs failed with the non-Linux refusal; `Scope::spawn` panics when no thread can be made)
  and 4 nits (`fchmodat` claim, spec status, machine-specific modes in changes, a long line), all
  own code, all fixed in `684f40c6` and `322dfc8a`.
- **Production claim of #825 corrected** (comment on the issue): each front is its own
  `yulius sandbox-host docker` process, so only in-process tests raced.
- **Not tested:** a real macOS run (compile and clippy only; the regression test is Linux-only),
  and thread creation failing (the error path is `Builder::spawn_scoped`'s `?`).
