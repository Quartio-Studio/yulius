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
  it restarts its engine front whenever the smoke's manager displaced it.
- **Two managers, one holder: any step can fail.** From the adoption on, the server's manager
  and the smoke's each hold the front they started and take the other's for a dead one. Which
  step fails depends on who started a front last. In the issue's runs the server's engine front
  was the last before the old run's `docker info` (the refusal step failed). In this task's
  reproduction the smoke's tick put its refusal back in time, and the server's next upkeep, 30
  seconds later, replaced the smoke's engine front during the old run's `docker compose up`,
  which stopped that engine: `FAIL docker compose up (two services) in the old run`. The sandbox
  had seven fronts where a smoke starts three.
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
  setup` of a fresh Ubuntu 24.04.5 guest with the `dev` binary ends with `FAIL Docker smoke test`
  (exit 1), with the server's journal naming the adoption (`replacing the sandbox's Docker front
  sandbox="docker-smoke-5884" to="engine"`, `adopted=1`) one second after its start.

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
  guest), and a server restarted while the smoke's Docker-off holder runs does not change its
  result.
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
  `Docker smoke test` ok, exit 0, and the server's journal says it left the smoke's sandbox to
  it). On both guests then: `yulius doctor --docker-smoke` beside the running server, and once
  more with `systemctl restart yulius.service` as soon as the smoke's holder is recorded.
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

## Evidence

Run on 2026-11-12 in the task's sandbox. The hosts are QEMU guests (no KVM, 16 CPUs, 16 GiB) from
Ubuntu's 24.04 cloud image (`noble-server-cloudimg-amd64.img`, checked against Ubuntu's
`SHA256SUMS`): Ubuntu 24.04.5, kernel 6.8.0-147, Docker 29.9.3 installed by `host setup`. The
binaries were built in an `ubuntu:24.04` container (`cargo build --release -p yulius --locked`)
from `dev` `0dd7a58a5` ("before") and from this branch at `8b5a87815`, the fix's commit ("after").

**Before the fix**

- `cargo nextest run -p yulius-sandbox -E
  'test(a_starting_server_leaves_a_running_smoke_tests_holders_alone)'` at `9009836b1` (the test
  alone on `dev`): 1 failed.

  ```
  assertion `left == right` failed
    left: "mode:engine:"
   right: "mode:refuse:Docker is turned off on this Yulius server (YULIUS_SANDBOX_DOCKER=off); an operator removes that setting and restarts Yulius"
  ```

- First `yulius host setup --public-url https://yulius.example.org` on a fresh guest: `host setup
  exit 1`, `changed 12, left alone 3, manual 1, failed 1`.

  ```
  18:17:17.434 changed  yulius running         started yulius.service
  18:17:18.009 running in a delegated transient unit as uid 999
  18:17:20.031 ok    a sandbox without Docker answers why         Client: Docker Engine - Community
  18:17:58.168 FAIL  docker compose up (two services) in the old run probe-1 exited with code 0
  18:19:24.859 FAIL     Docker smoke test      failed: exit status 1 (its lines are above)
  ```

  The server's journal of that minute (`journalctl -u yulius.service`):

  ```
  18:17:18.416 INFO yulius_sandbox::manager: replacing the sandbox's Docker front sandbox="docker-smoke-5884" to="engine"
  18:17:18.493 INFO yulius_sandbox::manager: sandbox Docker front serving sandbox="docker-smoke-5884" mode=engine pid=5926
  18:17:18.493 INFO yulius_sandbox::manager: adopted sandbox holders of the previous server process adopted=1
  18:17:18.580 INFO yulius_sandbox::manager: gave a running run the sandbox's Docker directory sandbox="docker-smoke-5884" line=bound 5922
  18:17:48.710 WARN yulius_sandbox::manager: the sandbox's Docker front ended; starting it again sandbox="docker-smoke-5884"
  18:18:19.247 WARN yulius_sandbox::manager: the sandbox's Docker front ended; starting it again sandbox="docker-smoke-5884"
  ```

  A watcher in the guest recorded each change of the sandbox's front record: seven fronts
  (refuse 5912 by the smoke, engine 5926 by the server, refuse 5937 by the smoke, engine 6043 by
  the smoke's phase 2, engine 7716 by the server's upkeep, engine 8288 by the smoke's phase 3,
  engine 9272 by the server's upkeep). In this run the smoke's refusal was back before the old
  run asked, so the refusal step passed, and the server's upkeep at 18:17:48 replaced the engine
  front under `docker compose up`.
- Same guest, later: `yulius doctor --docker-smoke` beside the running server: exit 0. Then the
  smoke with `systemctl restart yulius.service` as soon as the smoke's holder was recorded: exit
  1, with the issue's own line, and the restarted server's journal names the adoption:

  ```
  18:20:58.837 restarted yulius.service: active
  18:21:02.211 FAIL  a sandbox without Docker answers why         Client: Docker Engine - Community
  18:20:59.102 INFO yulius_sandbox::manager: replacing the sandbox's Docker front sandbox="docker-smoke-14808" to="engine"
  18:20:59.172 INFO yulius_sandbox::manager: adopted sandbox holders of the previous server process adopted=1
  ```

**After the fix**

- The same test at `8b5a87815`: passed; `cargo nextest run -p yulius-sandbox`: 163 passed.
- First `yulius host setup --public-url https://yulius.example.org` on a second fresh guest
  (`yulius 0.1.0 (8b5a87815…)`): `host setup exit 0`, `changed 12, left alone 4, manual 1, failed
  0`, all 14 smoke checks ok.

  ```
  18:22:59.018 changed  yulius running         started yulius.service
  18:22:59.587 running in a delegated transient unit as uid 999
  18:23:01.617 ok    a sandbox without Docker answers why         Client: Docker Engine - Community
  18:23:42.584 ok    docker compose up (two services) in the old run probe-1 exited with code 0
  18:24:10.417 ok       Docker smoke test      sandboxes run hello-world, a buildx build and a two-service compose
  ```

  The server met the smoke's sandbox in the same window and left it (journal):

  ```
  18:22:59.991 INFO yulius_sandbox::manager: left to the Docker smoke test that runs sandbox="docker-smoke-5666"
  ```

  The watcher saw the three fronts a smoke starts (refuse 5694, engine 5811, engine 8025 after
  the smoke killed the second) and the production holder's (8508). `refused.txt`, copied while
  the smoke ran, ends with `Error response from daemon: the sandbox's Docker engine is not
  available: Docker is turned off on this Yulius server (YULIUS_SANDBOX_DOCKER=off) …`. (The
  step's detail is the first line of `docker info`, `Client: …`, on a passing run too.)
- Same guest, later: `yulius doctor --docker-smoke` beside the running server: exit 0,
  `sandboxes get Docker on this host`. The smoke with `systemctl restart yulius.service` as soon
  as its holder was recorded, three times: exit 0 each time. In two of them the restarted server
  scanned during the smoke's Docker-off phase (the journal's line 0.36 s and 0.27 s before the
  refusal step), in the first during phase 2:

  ```
  18:26:45.291 restarted: active
  18:26:45.477 INFO yulius_sandbox::manager: left to the Docker smoke test that runs sandbox="docker-smoke-14029"
  18:26:45.835 ok    a sandbox without Docker answers why         Client: Docker Engine - Community
  18:27:46.985 sandboxes get Docker on this host
  ```

  After each smoke `runtime/sandboxes/` and `sandboxes/` were empty.

**Gates**

- `scripts/dev check`: passed. `scripts/dev security`: "Security scan disabled: Vidoc is turned
  off in Yulius, for this project or the whole instance; nothing was scanned." (exit 0; disabled,
  not passed).
- `scripts/dev ci` at `b1b7996d9`: stopped at the test stage, 4031 tests run, 4030 passed, 1
  failed, 16 skipped. The failure is
  `yulius mcp_bridge::tests::a_call_held_through_an_outage_is_answered_once_yulius_is_back`
  (`crates/yulius/src/mcp_bridge.rs:540`), outside this change (which touches
  `crates/yulius-sandbox/src/manager.rs` and a comment in `docker_smoke.rs`): the test frees an
  ephemeral port and expects nothing to listen on it for 400 ms, and in the parallel run another
  test's listener got that port (its output: "Yulius cannot be reached (connect: connection
  refused)" then "Yulius is back" before the test's own listener existed). Alone it passes:
  `cargo nextest run -p yulius -E 'test(/^mcp_bridge::tests::/)' --stress-count 30`: 30 of 30
  iterations. No issue tracks it; reported on issue #1160 (this task opens no issue).

**Not tested here.** Hosts other than Ubuntu 24.04 (the issue also saw 26.04 and Debian 13; the
changed code has nothing of the host in it). `yulius instance migrate`'s setup on a new host and
the smoke check of Settings → Instance during a restart were not run as such; the restart during
a hand-run smoke exercises the same code (`Manager::adopt` at the server's start beside a live
`docker-smoke-<pid>`).
