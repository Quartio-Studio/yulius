# 0162. One host setup command brings Docker to sandboxes, and every update re-checks it

## Status

Accepted. Specified in [SPEC-203](../../.ai/specs/SPEC-203-docker-in-sandboxes-works-out-of-the-box.md);
task #188. Amends [0151](0151-a-sandbox-runs-its-own-docker-engine-under-an-engine-seccomp-profile.md)
(its host prerequisites are installed by a command, not by hand),
[0158](0158-docker-reaches-every-run-through-a-front-and-a-directory-moved-into-live-runs.md)
(Settings → Instance shows whether sandboxes get Docker and runs the smoke test) and
[0160](0160-a-sandbox-s-engine-comes-from-its-own-system-not-the-host-s.md) (the host hint names the
command; a sandbox on its own `docker.io` is listed with the way back).

## Context

Docker in sandboxes worked on the reference VM on 2026-10-16, after a day of manual host work
(tasks #172, #182, #185, #186, #187). The next fresh install, or the next update, would have to
rediscover it:

- Docker was no documented prerequisite. A sandbox's engine runs its base's `dockerd`, a copy
  of the host's, and an operator session had purged Docker from the host on 2026-10-12.
- `apt install docker-ce …` enabled and started the host's `docker.service`, `docker.socket` and
  `containerd.service`. `needrestart` then restarted `yulius.service` in the middle of the
  install, with no pause of the tasks.
- The base had to be rebuilt to pick up `dockerd`. By hand that broke running sandboxes (#618);
  automatically it took 10–40 minutes, and nothing said so.
- The kernel modules were loaded and persisted by hand, and a writable `yulius` binary gave no
  engine, which `yulius doctor` did not check.
- The checks needed flags nobody used, and nothing in the product showed whether sandboxes have
  Docker.

## Decision

- **`yulius host setup`** (root) is the install path: install the binary, run it. In one order
  it checks and fixes:
  - a needrestart rule (`/etc/needrestart/conf.d/yulius.conf`, `override_rc` 0 for
    `yulius.service`), written before any install;
  - the host packages (`git`, `uidmap`, `slirp4netns`, `bubblewrap`);
  - Docker Engine with buildx and compose from Docker's apt repository (Ubuntu's `docker.io`
    is kept, with its plugins);
  - the host's own Docker units disabled and stopped, unless they run containers or their
    containers cannot be counted;
  - the modules file `/etc/modules-load.d/yulius-docker.conf`, and the modules loaded;
  - the service user, its data directory and a subordinate id range;
  - `yulius.service`, written when missing (with `--public-url`), else given `Delegate=yes`,
    `DelegateSubgroup=server` and `KillMode=process` by a drop-in;
  - the update helper's write access (a drop-in);
  - the binary owned by root and writable by root alone;
  - a base built now, as the service user, which keeps live holders' bases (ADR-0161). Base
    builds take turns under a lock (`<data>/system/.build.lock`), so the setup's build never
    overlaps the server's;
  - then the contained doctor and the smoke test.

  Each item prints `ok`, `changed`, `manual` (with the reason) or `FAIL`. A second run changes
  nothing. Apt runs with needrestart suspended.
- **`yulius host check`** runs the same items with only the safe, idempotent fixes: the
  needrestart rule, the host's Docker units, the modules, the binary's owner. After a healthy
  install, the update helper runs the **installed** binary's `host check --json`. The report
  goes in the status (`HelperStatus.host_check`) and in `<state>/host-check.json`. A failed item
  or a "sandbox Docker" that does not hold is shown in the update's outcome and in Settings →
  Instance. The helper passes its unit's name and bounds every field of the report. A setup or
  check run as root also keeps its report there, so the card shows the latest root check.
- **One verdict.** `yulius_sandbox::docker_status::verdict` is the one "sandbox Docker" verdict
  for the doctor, the setup, the check and the server. It covers the setting, the delegation,
  the binary's owner, the modules, the host's `dockerd`, and the current base's `dockerd` and
  its rebuild. `yulius doctor` without `--contained` names that flag.
- **Settings → Instance** has a Sandbox Docker card (`GET /api/admin/sandbox-docker`). It shows
  the verdict, the base's rebuild with an estimate (the last build's duration), the sandboxes on
  their own Docker with the way back, and the last re-check. "Run smoke check" (`POST
  …/smoke`) runs `yulius doctor --docker-smoke --json` as the server's child, in its delegated
  cgroup. It runs one at a time, bounded to 40 minutes, and each check appears as it ends.
- **A base without the host's `dockerd`** is checked at every tick instead of every 30 minutes,
  so the rebuild starts after the 10-minute debounce.
- **A sandbox on its own `docker.io`** is listed when its upper layer's `dockerd` shadows the
  base's at another path. The way back is `apt-get install docker-ce …` in that sandbox; apt
  replaces `docker.io`. Yulius never removes anything, and `apt-get remove docker.io` would
  white out the base's files at the same paths.

## Alternatives

- **A shell script in `deploy/`.** It would duplicate the doctor's checks and module list, and
  it could not be tested against a fake host. It would also not ship with the binary, so the
  update helper could not run the new version's copy.
- **The helper re-checks with its own code.** The helper that installs version N+1 is version
  N, and only N+1 knows what N+1 needs.
- **Roll back an update whose re-check fails.** The server is healthy. Restoring the previous
  binary and database for a host prerequisite would lose more than it saves.
- **Run the smoke test after every update.** It pulls from Docker Hub and takes minutes, so a
  rate limit would read as a regression. The re-check is fast and deterministic; the smoke test
  runs on demand from the card.
- **Let needrestart restart `yulius.service`.** With `KillMode=process`, sessions survive a
  restart. But a restart outside the update flow freezes no task. #186's restart also came in
  the middle of an install whose base rebuild had not yet run.
- **Edit the operator's unit.** It holds local values. A drop-in adds only what sandboxes need,
  and a unit file that systemd does not report is never overwritten.
- **Stop host Docker whatever runs on it.** Yulius never uses it, but stopping someone's
  containers is not a safe automatic fix: such a host is reported as manual instead.

## Consequences

- A fresh Ubuntu 24.04 host needs: the binary, `sudo yulius host setup --public-url …`, the
  reverse proxy (§4), and the optional parts (Claude Code, `gh`, the browser's packages, the
  update helper). These stay manual, each for the reason the docs give.
- The first update to this version runs the previous helper, which does not re-check. The
  operator runs `sudo yulius host setup` once after it. Every later update re-checks.
- An existing `yulius-update.service` needs the setup's drop-in, or the shipped unit's new
  `ReadWritePaths`, before its re-check can write the modules file under
  `ProtectSystem=strict`. Without it the re-check reports the failure; it never passes it.
- A unit drop-in takes effect at the next restart, which the setup lists as manual.
- Not verified in a sandbox: systemd unit operations and a base build there (a sandbox engine's
  containers get a threaded cgroup and no nested user namespaces). These are covered against a
  fake host, and the operator runs them on the VM (SPEC-203 Evidence).
