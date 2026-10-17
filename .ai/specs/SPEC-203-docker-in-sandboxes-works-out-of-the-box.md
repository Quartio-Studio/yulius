---
yulius-task: 67f022d5-fa83-485c-8405-ff108aa3ad0a
status: in-progress
---

# SPEC-203: Docker in sandboxes works out of the box on a fresh install and after every update

Status: in-progress
Date: 2026-10-17
Owner: task #188
Scope authorization: task #188 (requested by MrFishPL): "Make it work by default: a fresh install
on Ubuntu 24.04 gives every sandbox a working `docker` (run, buildx build, compose) with no extra
manual steps, and an update (Settings → Instance → Update now) or a reboot never silently breaks
it." Runs after #187 (SPEC-202: #617, #618), whose fixes it builds on. Amends ADR-0151, ADR-0158
and ADR-0160 through ADR-0162, and SPEC-196 DKL-007.

## Goal

Two commands are all an operator needs: install the `yulius` binary, run `sudo yulius host setup`.
The setup brings a fresh Ubuntu 24.04 VM, or an existing install, to every prerequisite of Docker
in sandboxes, says what it changed and what it left alone, and proves the result with the
contained doctor and the Docker smoke test. Every update re-checks the same prerequisites and
reports a regression in the update's outcome and in Settings → Instance. Settings → Instance shows
whether sandboxes get Docker, why not and the fix, in the doctor's words, runs the smoke test on
demand and says when a base rebuild that brings `dockerd` will be ready.

## Context

What it took on the reference VM (2026-10-12 to 2026-10-16; tasks #172, #182, #185, #186, #187):

1. **Docker is no documented prerequisite.** The §1 table and "Install once" do not install it, a
   sandbox's engine runs the base's `dockerd` (a copy of the host's), and an operator session had
   purged Docker from the host on 2026-10-12. Nothing noticed.
2. **Installing Docker has side effects.** `apt-get install docker-ce …` enables and starts
   `docker.service`, `docker.socket` and `containerd.service`. `needrestart` then restarted
   `yulius.service` mid-install (task #186 transcript: "Restarting services... systemctl restart
   yulius.service"). Disabling `docker.service` alone leaves `docker.socket` active.
3. **The base must be rebuilt to pick up `dockerd`.** By hand it broke running sandboxes (#618,
   fixed by SPEC-202); automatically it takes 10–40 minutes (`FINGERPRINT_EVERY` 30 min plus
   `REBUILD_DEBOUNCE` 10 min) and nothing says so.
4. **Kernel modules** are loaded and persisted by hand (`/etc/modules-load.d/yulius-docker.conf`).
5. **The binary's owner.** An executable the service user owns or may write gives no engine
   (`DOCKER_WRITABLE_EXE`), but `yulius doctor` does not check it.
6. **Checks behind flags.** `yulius doctor` shows "sandbox Docker" only with `--contained`.
7. **A sandbox's own `docker.io`** (29.1.3 in the product sandbox) shadows the base's Docker
   (29.9.1): `usr/sbin/dockerd` comes before `usr/bin/dockerd` in `DOCKERD_PATHS`.
8. **Nothing in the product shows whether sandboxes have Docker.** The server logs it once at start.

Code today:
- `doctor::docker_check` (setting, delegation, modules, host `dockerd`) and
  `Manager::docker_unavailable` (setting, delegation, executable) give overlapping, different
  answers.
- `update_helper` (root, `yulius-update.service`, `ProtectSystem=strict`) stops, backs up,
  installs, starts and health-checks; it knows nothing about the host's prerequisites. The helper
  that installs version N+1 is version N's binary.
- `docker_smoke` runs as the service user in a delegated cgroup; the server's own process
  (`…/yulius.service/server`) is one.

## Requirements

### Host setup (`yulius host setup`, root)

- **HST-001** `yulius host setup` (root only; non-root is refused with why) checks, in this order,
  and fixes where it is safe: the needrestart rule; the host packages (`git`, `uidmap`,
  `slirp4netns`, `bubblewrap`); Docker Engine with buildx and compose; the host's own Docker units;
  the kernel modules; the service user and data directory; its subordinate id range; the
  `yulius.service` unit; the update helper unit's write access; the binary's owner and mode; the
  sandbox base; then it starts a unit it created, and runs the contained doctor and the Docker
  smoke test (`--no-smoke` skips the smoke). Each item prints one line: `changed`, `ok` (left
  alone), `manual` (with the reason it is not automatic) or `FAIL`; a summary ends it. Exit 0 when
  nothing failed (manual items do not fail), 1 otherwise. `--dry-run` changes nothing and prints
  what it would change.
- **HST-002** Idempotent: a second run on the host the first one set up changes nothing ("nothing
  to change").
- **HST-003** Docker Engine: with `docker-ce`, `docker-ce-cli`, `containerd.io`,
  `docker-buildx-plugin` and `docker-compose-plugin` installed, nothing. With Ubuntu's `docker.io`,
  it is kept and only a missing `docker-buildx` or `docker-compose-v2` is installed. Otherwise
  Docker's apt repository is added (unless a source naming `download.docker.com` exists; key in
  `/etc/apt/keyrings/docker.asc`, `/etc/apt/sources.list.d/docker.sources`) for Ubuntu or Debian
  (`/etc/os-release`), and the five packages are installed. Another distribution makes the item
  manual. `apt-get` runs non-interactively with needrestart suspended; a failing `apt-get update`
  (another broken source) does not stop the install.
- **HST-004** The host's `docker.socket`, `docker.service` and `containerd.service` are disabled
  and stopped when enabled or active. When the host's Docker (`docker ps`) or containerd (`ctr`
  tasks outside Docker's `moby` namespace) runs containers, or they cannot be counted, they are
  left alone and the item is manual (the operator uses them for something else).
- **HST-005** `/etc/modules-load.d/yulius-docker.conf` lists exactly the engine's modules (one
  per line, after a comment); a different file is rewritten. Modules neither loaded nor built in
  are loaded with `modprobe -a`.
- **HST-006** When needrestart is installed (`/etc/needrestart`), `/etc/needrestart/conf.d/
  yulius.conf` keeps it from restarting `yulius.service` (`$nrconf{override_rc}`); it runs before
  any package install.
- **HST-007** The service user is the data directory's owner, else `--user` (default `yulius`).
  A missing user is created as a system user with the data directory as its home (mode 0700). A
  user without a subordinate uid or gid range of 65,536 ids gets the range right after every range in
  use, at or above 100,000 (`usermod --add-subuids --add-subgids`).
- **HST-008** `yulius.service`: missing, it is written from the documented unit when
  `--public-url` is given (else manual, naming the flag), enabled and started at the end. Present
  without `Delegate=yes`, `DelegateSubgroup=server` or `KillMode=process`, a drop-in
  (`yulius.service.d/10-yulius-sandboxes.conf`) adds them and systemd reloads; the restart that
  applies them is manual (it is the operator's to time).
- **HST-009** With `yulius-update.service` installed, a drop-in lets it write
  `/etc/modules-load.d` and `/etc/needrestart/conf.d` (HST-012 needs it under
  `ProtectSystem=strict`). Without the helper, the item is manual (§8: root's Rust toolchain).
- **HST-010** The running binary and the unit's `ExecStart` binary (links resolved to their
  targets) are made root-owned and not writable by group or others, through one descriptor
  opened without following a link. A binary in a directory the service user or others may write
  fails the item instead. A linked data directory is judged at its target too.
- **HST-011** The base: when there is none, it has no `dockerd` while the host has one, or it no
  longer matches the host, it is built now as the service user (`yulius sandbox-base build`, which
  keeps live holders' bases, SPEC-202 RSB-002), so it is safe while sandboxes run. Base builds
  take turns under a lock (`<data>/system/.build.lock`), so the setup's build and the server's
  never overlap.

### Updates

- **HST-012** `yulius host check` runs the same items with only safe, idempotent fixes (needrestart
  rule, host Docker units, modules file and loading, the binary's owner) and checks the rest; with
  `--json` it prints one JSON report. After an install is healthy, the update helper runs the
  installed binary's `host check --json` (bounded to 2 minutes) and stores the report in its
  status (`HelperStatus.host_check`) and in `<state>/host-check.json`. A failed item or a "sandbox
  Docker" that is not ok is a regression: the update's outcome and Settings → Instance show it
  with its reason. A binary without `host check` is reported as not checked. The helper passes
  its unit's name (`--service`, `YULIUS_UPDATE_SERVICE`) and bounds every field of the report.
  A setup or check run as root also keeps its report in `host-check.json`, so the card shows the
  latest root check.

### Visible status

- **HST-013** One verdict, "sandbox Docker", is computed by one function for `yulius doctor`, the
  host setup and check, and the server: the setting, the cgroup delegation, the binary's owner, the
  kernel modules, the host's `dockerd`, the current base's `dockerd` (and whether it is being
  rebuilt). `yulius doctor` without `--contained` on Linux prints a "sandbox Docker" line naming
  the flag that adds the check (`--contained`; `yulius.service` sets `YULIUS_CONTAINED=true`).
- **HST-014** `GET /api/admin/sandbox-docker` (superadmin) returns the verdict, the base (id,
  `dockerd` or not), the rebuild (building since, or pending since, and when it will be ready),
  the sandboxes running their own engine, the last update re-check and the smoke run. A server
  without isolation says why sandboxes get no Docker.
- **HST-015** `POST /api/admin/sandbox-docker/smoke` (superadmin) starts `yulius doctor
  --docker-smoke --json` as the server's child (one at a time, bounded to 40 minutes); each check
  arrives as it ends. Settings → Instance shows a "Sandbox Docker" card with the verdict, the fix,
  the rebuild and its estimate, the own engines, the update re-check, and "Run smoke check" with
  each check's result.
- **HST-016** When the host has `dockerd` and the current base has none, the server checks the
  host's fingerprint at every tick instead of every 30 minutes, so the rebuild starts after the
  10-minute debounce.

### Sandboxes on their own `docker.io`

- **HST-017** A sandbox whose upper layer provides the `dockerd` its engine runs while its base has
  one too is listed ("runs Docker installed in the sandbox (`usr/sbin/dockerd`) instead of the
  base's"), in the doctor and the Instance card, with the way back: in that sandbox, `sudo apt-get
  install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin`
  (apt replaces `docker.io`; never `apt-get remove docker.io`, whose whiteouts hide the base's
  files at the same paths). Nothing is removed by Yulius.

### Reboot

- **HST-018** What a reboot needs persists: the modules file, the disabled units, the needrestart
  rule, the unit drop-ins, the subordinate ids and the base. A test reboots a set-up fake host
  (modules unloaded, inactive units) and shows a second setup changes nothing but loading.

## Plan

- [x] Spec SPEC-203 written and validated
- [x] One "sandbox Docker" verdict for doctor and server, shown without --contained
- [x] Host setup items: Docker Engine, host units, modules, needrestart, with fake-host tests
- [x] Host setup items: user, ids, units, binary, base, doctor and smoke; the host CLI
- [x] Update helper re-checks the host and reports regressions
- [x] Server sandbox Docker status: verdict, rebuild estimate, own engines, re-check
- [x] Smoke check started from the server with per-check results
- [x] Settings → Instance Sandbox Docker card with end-to-end test
- [x] Fresh-install run in an Ubuntu 24.04 container
- [x] ADR-0162, operations guide, architecture and changes updated
- [ ] Gates green and independent review answered

## Acceptance criteria

- Fake-host tests, failing before the change: a fresh host gets every change in order (needrestart
  rule before the first install); a set-up host changes nothing; a partly set-up host changes only
  what is missing; a running host Docker is disabled and stopped, but one with containers is left
  (manual); missing modules are written and loaded; a writable or service-user-owned binary is
  fixed; `host check` never installs packages, adds ids, writes units or builds a base; a rebooted
  host changes nothing but module loading.
- Update helper test: after a healthy install it runs the installed binary's `host check --json`
  and the status carries the report; a failing re-check is a regression in the status but not a
  rollback; a binary without the command is "not checked".
- Verdict tests: a writable binary, a base without `dockerd` (being rebuilt or not) are not ok,
  with the fix; doctor without `--contained` names the flag.
- Server test: the sandbox Docker view for a superadmin only; the smoke run's checks arrive as
  they end. E2E: the Instance card shows the verdict, runs the smoke and lists each check.
- A real run of `yulius host setup` in an Ubuntu 24.04 container, as close to a fresh host as
  this sandbox allows: Docker from Docker's repository, the modules file, the needrestart rule
  (read by needrestart itself), the user, the ids, the binary's owner; a second run changes
  nothing; after a container restart nothing changes. Amended (task #188): systemd cannot be PID
  1 in a sandbox engine's container (a threaded cgroup; `--privileged` cannot mount sysfs in the
  sandbox's user namespace), and nested user namespaces are refused. The systemd items, the base
  build and the smoke are therefore proven against the fake host, and on the VM by the operator's
  commands in docs/operations.md.

## Test plan

- Unit tests in `yulius` (`host_setup::tests`, `doctor::tests`, `update_helper::tests`) and
  `yulius-sandbox` (verdict, own engines, rebuild schedule), red before each change.
- Server integration test in `crates/yulius/tests` for the API; e2e test for the card.
- Real run: a systemd-capable `ubuntu:24.04` container in this sandbox's Docker engine.
- Gates: `scripts/dev ci`, `scripts/dev security`, the e2e suite.

## Decisions

- **One PR, though above 400 lines.** The request is one concern (Docker in sandboxes without
  manual host steps); the setup, the update re-check and the status share one verdict and one item
  list, and splitting them would ship a status that names a command that does not exist yet.
- **The setup is a subcommand of the binary**, not a shell script: it reuses the doctor's and the
  sandbox crate's checks (one verdict, one module list), is tested against a fake host, and is
  what the update helper can run from the newly installed binary.
- **The re-check runs the installed binary** (`host check`), not the helper's own code: the helper
  that installs version N+1 is version N, and only N+1 knows what N+1 needs.
- **The re-check reports; it does not roll back.** A Docker regression does not make the server
  unhealthy, and rolling back the database for it would lose more than it saves.
- **The update does not run the smoke test.** It pulls from Docker Hub and takes minutes, so it
  would turn a rate limit into a reported regression; the re-check is fast and deterministic, and
  the smoke runs from Settings → Instance on demand.
- **Host Docker with containers is left running** (HST-004): Yulius never needs it, but stopping
  someone's containers is not a safe automatic fix.
- **needrestart never restarts `yulius.service`** (HST-006): a restart outside the update flow
  does not pause tasks, and #186's install restarted it mid-install. The operator restarts it, or
  the next update does.
- **A unit drop-in, not an edit** (HST-008, HST-009): the operator's unit keeps its local lines.
- **The setup does not install the update helper** (HST-009): it needs root's Rust toolchain and
  local values; it stays a documented manual step.

## Evidence

- **Gate** `scripts/dev ci` (check, lint, test, deps, doc): exit 0 at 606c4b09 (2085 passed, 18
  skipped). Two earlier runs failed and were fixed: the repository check (a padding off the 4 px
  scale; modulepreload list, synced) and `cargo doc` (links to private constants).
- **E2E** (`cd e2e && npx e2e run`): 604 passed, 1 skipped, exit 0; the new
  `sandbox-docker.e2e.ts` 6/6.
- **Security** `scripts/dev security`: **unrun**, exit 2: Vidoc refused the agent's key (HTTP 401,
  as since PR #600). The CI `security` job scans the pull request head.
- **Tests first.** Red before the change, green after:
  - `self_update::a_docker_regression_after_an_update_is_in_its_outcome` failed to compile (no
    `UpdateRunView.docker`), passes after HST-012's view.
  - `host_setup::tests` (fake host): after the real run found that Docker's packages had no
    install candidate, the fake was taught that apt lists are read by `apt-get update`; 8 tests
    then failed exactly as the real host did, and pass after the lists are read again once a
    source is added. `an_unanswering_systemd_never_gets_its_units_rewritten` failed (the unit was
    rewritten), passes after the fix.
  - The other new tests exercise functions the change adds (verdict, host items, `--json`,
    `run_smoke`, the API): they could not compile before it.
- **Real fresh install** (step 9), `ubuntu:24.04` container in this sandbox's Docker engine,
  `yulius` debug build of this branch installed `-m 0755` to `/usr/local/bin`:
  - run 1: `changed` needrestart rule, git/uidmap/slirp4netns/bubblewrap, Docker Engine
    (docker-ce 5:29.9.1, containerd.io 2.4.2, buildx 0.38.0, compose 5.6.0 from Docker's
    repository through `docker.sources`), modules file, system user `yulius` with
    `/var/lib/yulius` 0700, ids 165536–231071 (after `ubuntu`'s 100000–165535); the contained
    doctor passed as `yulius` ("sandbox isolation: namespace holder available").
  - needrestart's own config loading maps `yulius.service` to 0 (never restarted); apt printed
    "packages have been installed but needrestart is suspended".
  - run 2: `changed 0`; after `docker restart` (the reboot), run 3: `changed 0`.
  - `chown 1000; chmod 775` on the binary, then `yulius host check --json`: the binary is back
    to root 755 (`changed`), the report is JSON with every item.
  - In an earlier build the unit written from `--public-url` had `User=yulius`,
    `YULIUS_CONTAINED=true`, `Delegate=yes`, `DelegateSubgroup=server`, `KillMode=process`,
    `ExecStart=/usr/local/bin/yulius serve`.
- **Sandbox side**, in this task's sandbox (base from the VM's docker-ce): `docker run --rm
  hello-world` ("Hello from Docker!"), `docker buildx build` (`built-by-buildx`) and a
  two-service `docker compose up` (`probe-1 | compose-web`, exit 0), engine 29.9.1.
- **Not testable here**, and why:
  - systemd as PID 1: a sandbox engine's container gets a *threaded* cgroup (systemd: "Structure
    needs cleaning"), and `--privileged` cannot mount sysfs in the sandbox's user namespace. So
    unit operations (disabling the host's Docker units, the drop-ins, `daemon-reload`, enable and
    start, `systemd-run` for the smoke) ran only against the fake host; in the container
    systemctl answers "Failed to connect to bus" and the setup reports it.
  - a base build, a holder, an engine and the smoke test in the fresh container: nested user
    namespaces are refused by the sandbox's seccomp profile ("bwrap did not start its namespace").
  - `modprobe` and a kernel reboot: the container shares the host's kernel, whose modules are
    loaded.
  - The operator runs these on the VM after promotion (docs/operations.md, "Notes for specific
    upgrades").
- **Independent review**, round 1 at 606c4b09 (fresh session, `om-code-review`): request changes.
  Two majors and seven minors, all in this change's own code, were answered and fixed test-first:
  - **Overlapping base builds** (major): `base::build` takes `<data>/system/.build.lock`.
    `a_build_waits_for_the_running_one` failed ("the second build waits"); it passes now.
  - **Evidence and acceptance criterion** (major): this section, and the amended criterion
    (no systemd manager in a sandbox engine's container).
  - **`own_by_root` followed links** (minor): it now uses one descriptor opened with
    `O_NOFOLLOW`, and a writable directory fails the item. `owning_a_link_never_touches_its_target`
    and `a_binary_in_a_writable_directory_fails` are new.
  - **Linked binary and data directory** (minor): both are judged at their targets
    (`links_are_judged_by_their_targets`).
  - **Hard-coded unit name** (minor): `--service`; the helper passes its own
    (`the_service_name_is_an_option`, and the helper's argument test).
  - **Containers that cannot be counted, and containerd's tasks** (minor): manual
    (`uncounted_or_busy_containers_keep_the_host_runtime`).
  - **Unbounded report fields** (minor): `an_oversized_report_is_bounded` failed; it passes now.
  - **Stale re-check on the card** (minor): root runs keep their report
    (`a_root_check_replaces_the_kept_report`), and the card says "The last host check".
  - Nits: apt keeps changed configuration files (`--force-confdef`, `--force-confold`).
    Commands are bounded to 30 s each (2 minutes to stop the host's Docker); the helper's
    re-check gets 4 minutes. The card polls every 30 s during a rebuild, and retries after a
    failed read.
  - Kept by choice: Docker's key is not pinned by fingerprint. It is fetched over HTTPS from
    `download.docker.com`, as Docker documents, and apt checks every package against it.
