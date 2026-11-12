---
yulius-task: bddd6293-420b-4d90-9a39-83f7a8fbd8b4
status: in-progress
---

# SPEC-477: A starting server leaves a running Docker smoke test's sandbox alone

Status: in-progress
Date: 2026-11-12
Owner: Yulius task #459, issue #1160
Scope authorization: the request of task #459 (from task #458, which works through open issues):
fix issue #1160 at its cause in one pull request that says `Fixes #1160`, reproduce it first,
comment progress on the issue, open no new issue.

## Goal

The first `sudo yulius host setup` on a fresh host ends with `Docker smoke test` ok. The smoke's
step "a sandbox without Docker answers why" reads the refusal whatever a Yulius server on the same
data directory does meanwhile, and `yulius doctor --docker-smoke` keeps passing beside a running
server.

## Context

- **Symptom (issue #1160).** On a fresh host the first `host setup` ends with `FAIL Docker smoke
  test` (exit 1): the step `a sandbox without Docker answers why` shows `Client: Docker Engine -
  Community`, and `probe/refused.txt` holds the answer of a live engine named `docker-smoke`
  instead of the refusal. Every other check passes, and so does a later `sudo yulius doctor
  --docker-smoke` on the same host. Task #438 saw it on 4 of 4 first runs (Ubuntu 24.04 twice,
  Ubuntu 26.04, Debian 13; QEMU guests).
- **What the smoke does there** (`crates/yulius/src/docker_smoke.rs`, phase 1): it starts the
  holder of its throwaway sandbox `docker-smoke-<pid>` with a manager whose Docker is off, so the
  holder's Docker front refuses with the reason; a run started without the Docker directory gets
  the directory at the manager's tick and writes what `docker info` says to `refused.txt`. Phase 2
  adopts the same holder with a Docker-on manager, as an updated server would, and only then is an
  engine expected.
- **Root cause.** `host setup` starts `yulius.service` ("yulius running") when it wrote the unit,
  that is on the first run, and runs the smoke seconds later (`running_part`,
  `crates/yulius/src/host_setup.rs`). The unit is `Type=simple`, so `systemctl start` returns
  before the server did anything. The server's sandbox wiring ends with `manager.adopt()`
  (`crates/yulius/src/wiring/sandbox.rs`): `Manager::adopt` (`crates/yulius-sandbox/src/manager.rs`)
  takes every entry of `runtime/sandboxes`, the directory the smoke shares, as "a holder a
  previous server process started". When it meets the smoke's Docker-off holder, `adopt_one` →
  `keep_front` compares the running refuse front with the mode this server wants (an engine) and
  replaces it (`FrontAction::Replace`): the refuse front is stopped and an engine front is started
  at the same socket, named after the host name in `holder.json` (`docker-smoke`). The smoke's
  old run then asks a live engine. The server also keeps the holder in its 30-second upkeep, where
  it would restart its engine front whenever the smoke's manager displaced it.
- **Why only the first run.** A server adopts only when it starts. On later runs the unit is not
  written, so nothing starts the server next to the smoke.
- **The departure.** The smoke's module documentation says its sandbox is "next to the real ones
  but adopted by nobody else". Every other reader of those directories already treats
  `docker-smoke-<pid>` as the smoke's while process `<pid>` runs: the leftovers scan
  (`crates/yulius-app/src/leftovers.rs`, `smoke_pid` of `yulius-domain`), the smoke's own
  `stale_smokes`, and `own_engines` of `docker_status.rs`. `Manager::adopt` was the one that did
  not.
- **Why the tests missed it.** The manager's tests adopt one holder per data directory, and the
  smoke's own tests (`isolation_docker_live`) run it with no second manager on its data
  directory. Nothing ran a smoke next to a server that starts.
- **What else the cause breaks.** Any server start while a smoke runs: `yulius instance migrate`
  runs `host setup` (start, then smoke) on the new host; and the smoke started from Settings →
  Instance is the server's child, which a restart of the server does not end (`KillMode=process`),
  so the new server process met its sandbox the same way.
- **Reproduced** before any fix, see Evidence: the manager test of this spec fails on `dev`
  (`0dd7a58a5`) with the engine's answer where the refusal is expected, and the first `host
  setup` of a fresh Ubuntu 24.04.5 guest with the `dev` binary fails as in the issue.

## Requirements

- **SMK-001** `Manager::adopt` leaves the holders of a running Docker smoke test alone:
  `docker-smoke-<pid>` and its production holder `docker-smoke-<pid>.prod` while process `<pid>`
  runs. Left alone means: not counted as adopted, no entry in this manager's upkeep, no relay
  started, its Docker front neither stopped, replaced nor started, its records not written.
- **SMK-002** A smoke sandbox whose process is gone is adopted as before, so its holder is kept
  in the upkeep and stops when idle; its files stay with the paths that remove them today (the
  next smoke's `stale_smokes`, the leftovers scan).
- **SMK-003** `Manager::adopt_one` is unchanged: the smoke adopts its own holder through it
  (phase 2) and removes its sandbox through it at the end.
- **SMK-004** The rule is one function next to `adopt`, built on `yulius_domain::leftovers::smoke_pid`
  (the name rule the leftovers scan uses) and the manager's own process check.
- **SMK-005** The smoke's module documentation, the architecture document, the operations
  guide and the change log say whose the smoke's sandbox is; the decision is recorded.

Out of scope: where the smoke keeps its sandbox, the order of `host setup`'s steps, the unit's
type, and the smoke's checks themselves.

## Plan

- [ ] Reproduce the defect on a fresh Ubuntu 24.04 guest and name its cause
- [ ] Spec and the manager test that fails without the fix
- [ ] A starting server leaves a running smoke test's holders alone
- [ ] Documents and the decision record say whose the smoke's sandbox is
- [ ] The first host setup of a fresh Ubuntu 24.04 guest passes the smoke with the fix
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- [ ] The cause is named with a test that fails without the fix:
  `manager::tests::a_starting_server_leaves_a_running_smoke_tests_holders_alone` (SMK-001,
  SMK-002).
- [ ] `sudo yulius host setup` on a fresh Ubuntu 24.04 host ends with `Docker smoke test` ok on
  its first run (a QEMU guest from Ubuntu's cloud image, binary built from this branch).
- [ ] `yulius doctor --docker-smoke` still passes beside the running server on later runs (same
  guest), and a server restarted in the middle of a smoke does not change its result.
- [ ] `scripts/dev ci` and `scripts/dev security` pass (or say why a part did not run).

## Test plan

- **Unit, `crates/yulius-sandbox/src/manager.rs`:**
  `a_starting_server_leaves_a_running_smoke_tests_holders_alone`. A Docker-off manager adopts the
  holder `docker-smoke-<this test's pid>` (a refuse front serves its socket) and a record of its
  production holder exists; a Docker-on manager with a sandbox of its own then runs `adopt()`.
  Expected: the socket still answers the refusal, the refuse front still runs, the server knows
  only its own sandbox and counts 1, the production holder got no front. Then a smoke sandbox of
  a process that ended: adopted, with an engine front. Without the fix the first assertion fails
  with `mode:engine:`.
- **Host, QEMU guest (Ubuntu 24.04 cloud image, no KVM):** the first `yulius host setup
  --public-url …` on a fresh guest, once with the binary of `dev` (expected: the issue's failure,
  with the server's journal naming the replaced front) and once with this branch's (expected:
  `Docker smoke test` ok, exit 0). On the fixed guest then: `yulius doctor --docker-smoke` beside
  the running server, and once more with `systemctl restart yulius.service` while the smoke is
  in its first phase.
- **Gates:** `scripts/dev ci`, `scripts/dev security`.

## Decisions

- **The server skips, the smoke does not defend.** The fix is in `Manager::adopt`, the one reader
  that broke the rule the others keep. A smoke that noticed a foreign front and put its refusal
  back would hide the fight over the socket, not end it: the server would keep the holder in its
  upkeep and start its engine front again.
- **The name and the live process decide, as elsewhere.** No new marker file: `docker-smoke-<pid>`
  already carries its owner, and the leftovers scan and the smoke's own cleanup decide by the
  same two facts. A sandbox id can never have that shape.
- **Not by making `host setup` wait for the server.** Waiting until the server answers would
  narrow the first run's window and leave every other server start beside a smoke (a migration's
  new host, a restart during the page's smoke check) as it is.
- **`adopt_one` stays unconditional.** The smoke adopts its own sandbox with it; the rule belongs
  to the scan that takes over "a previous server process's" holders.
