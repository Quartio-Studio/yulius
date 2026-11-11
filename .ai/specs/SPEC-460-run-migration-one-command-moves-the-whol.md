---
yulius-task: ac14490d-7f71-495d-8f11-10b0b42c2326
status: in-progress
---

# SPEC-460: Run migration: one command moves the whole instance to another host

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #442 (continues #439)
Scope authorization: task #442, asked by the project's owner: "One command 'run migration' that
moves a Yulius instance to another host completely … does every step below end to end, safely and
resumably, and reports one checklist; nothing is left for a human except what only a human can do
(a 2FA prompt, a DNS record), and those are asked for up front, before anything freezes." It keeps
the request of #439 ("Make moving a Yulius instance to another VM frictionless and provably
intact").

## Goal

`sudo yulius instance migrate root@new-host` on the old host moves the instance: it checks and
provisions the new host, carries every setting and key (transformed where it depends on the
machine), copies the data, proves the copy, starts the new instance, redirects the old URL and
cleans up after itself, as one resumable checklist. A person is asked only for what only a person
can do, before anything freezes, and the old instance runs again by itself whenever the move does
not complete.

## Context

- `yulius instance export --move | ssh new yulius instance import` (SPEC-329, ADR-0222) moves the
  data directory and nothing else. Moving production on 2026-11-11 (109.199.123.240, Ubuntu 24.04
  → 185.189.47.160, Ubuntu 26.04; 275.8 GiB, 6,388,083 entries, 54 minutes frozen) needed nine
  groups of hand steps; the task's request lists them and each is a way to break production.
- Task #439 was to design this but ended after 25 seconds (11:28 UTC cleanup): it wrote no spec
  and made no commit, so there is nothing of SPEC-457 to start from. This spec is written from its
  request, which task #442's request repeats in full.
- What exists and is built on: the export's freeze (`instance/freeze.rs`), exact-size stream and
  checksum (`archive.rs`, `export.rs`), the import's checks and id remapping (`import.rs`,
  `extract.rs`), and `yulius host setup` (SPEC-203): items over a `Host` trait, each one line
  `ok | changed | manual | FAIL`, tested against a fake host. The move reuses all of them.
- The server runs as the unprivileged service user and is frozen during the copy, so the move is a
  root command; a web button needs the update helper's pattern (a request file, a root unit, a
  status file; ADR-0013).
- Facts measured for this spec (2026-11-11):
  - GitHub's REST API changes an App's webhook only (`PATCH /app/hook/config`, App JWT). No
    endpoint changes callback URLs or the homepage; `GET /app` returns the homepage
    (`external_url`) and never the callbacks.
  - `POST https://github.com/login/oauth/access_token` with the client id and secret, a made-up
    code and a `redirect_uri` answers `redirect_uri_mismatch` when the URI is not registered and
    `bad_verification_code` when it is (tried with a public OAuth client). So an instance can ask,
    with its own credentials and nobody's session, whether its callback is registered.
  - Ubuntu 26.04 ships `coreutils-from-uutils` (default) and `coreutils-from-gnu`; the old host
    runs GNU coreutils, and sandboxes copy the host system.
  - On the hosts moved by hand Node and the global npm packages (Claude Code, Codex) live in
    `/usr/local`, not in apt.
- Parallel tasks this move relies on instead of repeating: #438 (SPEC-456: `host setup` lets bwrap
  create user namespaces), #440 (SPEC-458: restart with live sessions on systemd 256 and later),
  #441 (SPEC-459: host-sized limits; the import's mark makes the first start replace pins that no
  longer fit).

## Requirements

Command and checklist

- MIG-001 **One command.** `sudo yulius instance migrate <[user@]host>` on the old host runs the
  phases Plan, Prepare, Copy, Cutover and Finish and prints one checklist: a line per step with
  `ok`, `changed`, `manual` or `FAIL`, the step's name and what holds, changed or is missing (host
  setup's format), then a summary. Exit 0 when nothing failed, 1 when a step failed, 2 when it was
  refused before anything changed. It needs root and refuses a run from inside the unit (a Yulius
  terminal), an unknown unit and a running update helper. Options: `--public-url`, `--data-disk`
  (once or twice), `--ssh-port`, `--yes`, `--data-dir`, `--service`, `--update-service`,
  `--state-dir`, `--status`, `--abort`.
- MIG-002 **Asked up front.** Everything a person must do or decide appears in the Plan phase,
  before the target is changed (beyond the flow's own files) and before anything freezes: the one
  SSH sign-in to the target (password, 2FA); the blank disks to use; a DNS record when the public
  host name does not lead to the target; the GitHub App's callback when the URL changes (one deep
  link and the values to paste); and what the flow does not carry (another reverse proxy or
  firewall, packages without a candidate on the target's release). Then it asks once "Proceed?"
  on a terminal, unless `--yes`; with neither it ends after the plan (exit 0). After that it asks
  nothing.
- MIG-003 **Resumable.** The flow keeps `<state>/state.json` (default `/var/lib/yulius-migrate`,
  root, 0700): the move's id, target, options and how far the cutover got. Every step is
  idempotent: running the same command again evaluates each step again (`ok` when it is already
  done) and continues where it stopped. Another target than the recorded one is refused until
  `--abort`. `<state>/status.json` (0644, no secret values) holds the phase and the lines so far,
  for `--status` and for readers such as Settings → Instance.
- MIG-004 **Never two instances, never none for long.** The old instance is frozen only after the
  plan passed and the target is prepared. A failure or a signal before the new instance is started
  thaws the old one, which continues as before. The new instance is started only after the copy
  is proven (MIG-031). When it does not come up healthy it is stopped and disabled, then the old
  one is thawed, and the checklist says that the copy on the target was started and must be
  removed before another run. Only when the new instance is healthy does the old one stay frozen
  and disabled. Once the start was ordered no signal thaws the old instance.

Target

- MIG-010 **One-time key.** The flow makes an ed25519 key in its state directory and installs
  its public half in the target's `authorized_keys` through the operator's own SSH sign-in (the
  only interactive prompt); without a terminal it prints the line to add and stops as `manual`.
  Every later call uses only that key (`BatchMode`, the flow's own `known_hosts`, the host key
  kept from first contact). The key is removed from the target and deleted on the old host at
  Finish and at `--abort`. Remote commands are fixed strings; every value travels as JSON or as a
  stream on standard input, never in a command line.
- MIG-011 **The identical build.** The running `yulius` binary is copied to the target's state
  directory and must report the same commit and version there (else FAIL: another architecture or
  an older libc); Prepare installs it at the unit's `ExecStart` path, owned by root.
- MIG-012 **Preflight.** The plan reports the target's release, architecture, systemd, CPUs and
  memory, and fails when: the target runs a Yulius instance or its data directory holds one (an
  incomplete copy of this move is not one); the data directory's filesystem (or the disks named by
  `--data-disk`) has less free space or fewer free inodes than the old data directory uses; or the
  ports the proxy and Yulius need are held by another program.
- MIG-013 **Data disk.** The plan lists the target's blank disks (no partition table, no
  filesystem, no holder, not mounted). With `--data-disk` (one disk, or two for a mirror) Prepare
  refuses a disk that is not blank, makes ext4 on the disk or on an mdadm RAID1 of the two, mounts
  it at the data directory by UUID from `fstab`, records the array in `mdadm.conf` and the
  initramfs, writes the unit drop-in `RequiresMountsFor=<data directory>` and gives the mount
  point to the service user (`chown user:user`, 0700). Too little space without it fails in the
  plan and names the candidates.
- MIG-014 **Link and downtime.** The plan measures the link (64 MiB through the SSH connection)
  and shows the size to copy, the rate and the estimated time frozen.

Provisioning (sandboxes copy the host system, so the target matches the old host)

- MIG-020 **Apt sources.** Third-party source files of the old host and the key files they name
  are written on the target with the old release's codename replaced by the target's. A source
  that has no such suite keeps the old codename; one that still fails is disabled and named
  (`manual`). The distribution's own sources are never touched.
- MIG-021 **Packages.** The old host's `apt-mark showmanual` list is installed on the target in
  one non-interactive transaction. A name the target's release lacks is replaced by the one
  package with the same name but another version suffix (`libicu74` → `libicu78`); kernel and
  boot packages are skipped; names without a candidate are listed (`manual`, shown in the plan).
- MIG-022 **Coreutils.** After the packages, the target's coreutils flavour (GNU or uutils) is
  made the old host's, by the release's own selector package, and checked by `ls --version`.
- MIG-023 **Software outside apt.** `/usr/local`, the entries of `/opt` no package owns, and
  root's `~/.rustup` and `~/.cargo` are copied as they are (sizes in the plan); global npm
  packages are installed at the old host's versions when npm's global directory is not inside a
  copied tree.
- MIG-024 **Host setup.** Prepare runs `host setup`'s host items on the target (needrestart,
  packages, Docker engine and units, kernel modules, service user and ids, and the bwrap rule once
  SPEC-456 is merged); after the copy the whole `host setup` runs (unit drop-in, binary, sandbox
  base, start, verdict, contained doctor, Docker smoke). A failed item fails the step.

Settings and keys

- MIG-025 **The unit.** The old unit file, its drop-ins, the `EnvironmentFile`s and the key files
  its environment names outside the data directory are written on the target with the same modes,
  owned by root or the service user as before. The old public URL and host name are replaced by
  the new ones. Pinned resource limits travel unchanged and are named in the checklist; the first
  start replaces those that no longer fit (SPEC-459 HSL-005). No value of a variable whose name
  holds KEY, SECRET, TOKEN or PASSWORD is printed or kept in the status.
- MIG-026 **Update helper.** When the old host has the helper, its units and drop-ins and the
  record of the installed commit are carried and its path unit is enabled once the new instance
  runs; otherwise the checklist says there is none.
- MIG-027 **Reverse proxy.** A Caddy on the old host is carried: `/etc/caddy` and the service's
  drop-ins with the old host name replaced by the new one, the proxy's copy of the proxy secret
  included, validated (`caddy validate`) and enabled before the cutover, so its certificate is
  there when the instance starts. Another proxy is named in the plan (`manual`).
- MIG-028 **Firewall.** An enabled ufw is carried (its rule files and settings) and enabled as
  the flow's last act on the target, after the one-time key is removed. Another firewall is named
  in the plan (`manual`).

Copy and proof

- MIG-030 **Copy.** The old instance is frozen and its data directory streamed to the target with
  the export's exact-size archive and checksum, compressed with `zstd` when both hosts have it,
  and installed there by the import (its checks, id remapping and marker). An incomplete copy of
  this move found on the target is removed first; anything else there is refused.
- MIG-031 **Proof.** Both hosts compute the copy's inventory: entries and bytes for each
  top-level folder and one fingerprint over every entry's path, kind, size, owner, mode, time and
  restorable attributes in archive order, in the old host's ids. The new instance is started only
  when the two are equal and the import's checksum and length matched; a difference fails the
  step, names the first folder that differs and thaws the old instance.
- MIG-032 **Warm copy (slice B).** The data is copied while the old instance still runs, in one
  or more passes, into a staged copy on the target; the freeze then sends only entries that
  changed since they were last sent (found by the inode's change time and identity, never by size
  and time alone), removes what no longer exists and proves the result as in MIG-031. The plan and
  the checklist show the estimated freeze before it begins, from the measured rate and the
  changes outstanding; with `--max-downtime` the flow copies again instead of freezing while the
  estimate is above it. The sandbox base is then built before the freeze.

URL change

- MIG-040 **New URL.** The new public URL is `--public-url`; else the old one when its host is a
  name of its own; else, for an old URL made of the old address (`<ip>.sslip.io`), the same form
  for the target's address. After the start the flow waits (bounded) until `https://<host>/healthz`
  answers from the target. A name that does not lead to the target yet is checked by connecting
  to the target's address directly and stays in the checklist as the DNS record to change.
- MIG-041 **Old URL.** When the URL changed and the old host's proxy is Caddy, its site answers
  every request with a temporary redirect (307) to the same path at the new URL and the header
  `X-Yulius-Moved-To`; the earlier configuration is kept for `--abort`.
- MIG-042 **GitHub App.** When the URL changes the plan gives one deep link to the App's settings
  (`https://github.com/organizations/<owner>/settings/apps/<slug>`, or under `settings/apps` for a
  person's App) and the callback and homepage to register, before the cutover: an App accepts
  several callbacks, so the old one keeps working until the old host is retired. Yulius never
  uses a person's GitHub session. (Slice C: the instance asks GitHub whether its callback is
  registered, at start and on demand, and the plan and Settings → Instance show the answer.)
- MIG-043 **Remote clients (slice C).** `yulius remote` follows a redirect that carries
  `X-Yulius-Moved-To`: it moves the saved sign-in to the new URL, says so and repeats the call.

After the move

- MIG-050 **Tasks come back.** A copy installed by the move carries a mark. The first start that
  finds it puts every task whose run was lost with the old host back in the queue (same session,
  owner and subscription; the session limit applies), whoever owns it, except tasks a person had
  paused or was stopping. A task that cannot be queued is flagged for its owner under Needs you.
  The result is recorded for the checklist and the mark is removed.
- MIG-051 **Sandboxes on another release.** When the target's release differs, the checklist
  names the sandboxes whose package layer was built on the old release.
- MIG-052 **Finish.** The one-time key is removed from the target and deleted, the staged binary
  and the target's state are removed, and the checklist ends with what is left for people: the
  DNS record or the App's callback if still open, the command for laptops using `claude mcp add`,
  and that the old host still holds a frozen copy with every secret, with the abort command and
  when to retire the disk.
- MIG-053 **Abort.** `yulius instance migrate --abort` on the old host is the one way back: it
  stops and disables the new instance (through the one-time key, or after Finish it checks that
  the new URL no longer answers and refuses otherwise), restores the old proxy configuration,
  thaws and enables the old unit and removes the key and the state. It never leaves both running.

Settings → Instance (slice D)

- MIG-060 **The button.** A superadmin starts the same flow from Settings → Instance ("Run
  migration": target, URL, disks), follows the same checklist there and gets the same questions
  first; a root unit started by a request file runs the command, as for updates.

## Design

- **Processes.** The command runs as root on the old host. It reaches the target only through
  four fixed remote commands over the one-time key: install the staged binary from standard
  input; `yulius instance migrate-target` (one JSON request on standard input, one JSON reply:
  lines and facts); the same with `--receive` (the archive on standard input, options from the
  target's state file); and `tar` extracting copied trees. Target-side steps are Rust code run by
  the staged binary over `host_setup::Host`, so both sides are tested against fake hosts and no
  value is ever interpolated into a remote shell.
- **Steps, not flags.** A step reads the facts, acts if needed and returns a line; nothing but
  the move's identity and the cutover's progress is stored, so a rerun cannot disagree with the
  hosts.
- **Cutover order.** freeze → copy → proof → mark → order the start (no thaw by signal from
  here) → `host setup` on the target → health → `systemctl disable` on the old host → redirect →
  Finish. A failure before the order thaws; one after it stops and disables the target first.
- **States.** `planned` (owner: the operator; exit: confirm or rerun), `prepared` and `copying`
  (the flow; a failure leaves them for the rerun), `starting` (the flow; exit: healthy, or
  rollback when the bound passes; a killed flow leaves the old instance frozen and the rerun
  decides from the target's health), `moved` (terminal), `aborted` (terminal).

## Plan

- [x] Spec SPEC-460 and ADR-0297 for the whole command and its slices
- [ ] The copy's inventory: counted by the export, recomputed on the target, compared
- [ ] The host profile: the old host's facts read and transformed for the target
- [ ] Target steps over the one-time key: staged binary, preflight, data disk, sources and packages, trees
- [ ] Units, update helper, proxy and firewall carried to the target
- [ ] `yulius instance migrate`: plan, confirm, prepare, frozen copy, proof, start, redirect, finish, with resume, status and abort
- [ ] Tasks that were live come back by themselves at the first start after a move
- [ ] Operations guide §7, architecture, product summary, changes
- [ ] Chained tasks created for slices B, C and D
- [ ] Gates run and the independent review answered

## Acceptance criteria

- Against two fake hosts (an old Ubuntu 24.04 with a unit, drop-ins, key files, Caddy, ufw, a
  manual package list with `libicu74`, Node in `/usr/local`; a fresh Ubuntu 26.04) the command
  ends with exit 0 and a checklist without FAIL; on the target: the binary at the unit's path,
  the carried unit with the new URL, the drop-ins and key files with their modes, the sources
  with `resolute`, `libicu78` installed, GNU coreutils selected, Caddy's site for the new host,
  ufw enabled, no one-time key; on the old host: the unit frozen and disabled, the proxy
  redirecting, no key.
- The same run copies a real directory tree (files, links, hard links, attributes) through the
  real export and import, and the two inventories are equal; with one file changed on the target
  before the proof the step fails, names the folder, and the old unit is thawed and still
  enabled.
- A failure in Prepare leaves the old instance running and untouched; running the command again
  reports the finished steps as `ok` and completes.
- A target whose instance does not report healthy is stopped and disabled before the old unit is
  thawed; the checklist says the copy there must be removed, and a rerun is refused until it is.
- The plan with a public URL that changes names the GitHub App's link and both values; with
  neither a terminal nor `--yes` nothing is changed on the target beyond the flow's files, and
  nothing is frozen.
- `--data-disk` with a disk that carries a filesystem is refused in the plan; with two blank
  disks the target gets the mirror, the mount, `mdadm.conf`, the initramfs update and the drop-in,
  in that order, and the mount point belongs to the service user.
- `--abort` after a finished move refuses while the new URL answers; once it does not, it thaws
  and enables the old unit and restores the proxy configuration.
- A server started on a copy with the move's mark queues the tasks whose runs were lost (and
  starts them within the session limit), leaves a task with an open pause stopped, records the
  result and removes the mark; without the mark nothing is queued (as today).

## Test plan

- Unit tests (`instance::migrate`): destination and option parsing; the package rename rule; the
  codename, URL and host replacements in sources, units and Caddy files; secrets never in lines;
  the inventory's canonical form (foreign owners, attributes) and its equality across an id
  remap; the state file's round trip.
- Fake-host tests of every target step (each branch: already done, done now, refused, failed) and
  of the orchestrator's scenarios in the acceptance criteria, with the real export and import
  between two temporary directories.
- Application test (`crates/yulius/tests/`): the first start with the mark (queued, paused left
  alone, not without the mark).
- CLI test: `instance migrate` refuses without root and prints its help.
- Gates: `scripts/dev ci`; `yulius security-scan` (Vidoc is off for this project: disabled, not
  passed).
- Not testable here: a real freeze and a real move between two machines (the sandbox has no
  systemd); mdadm, apt and Caddy are exercised through the fake host only, unless two QEMU guests
  can be run within the task (recorded in Evidence either way).

## Decisions

- ADR-0297 records the lasting ones: a root command on the old host that drives the target over a
  one-time key with fixed remote commands; the target's steps run by the copied binary; nothing
  stored but identity and cutover progress; the start of the new instance as the point of no
  automatic return; the inventory as the proof instead of `git fsck` (the copy is proven equal to
  a source that worked; rechecking repositories would only find damage the old host already had,
  at many minutes frozen); GitHub's settings never driven with a person's session.
- **Slices.** This task delivers the command with a frozen full copy (what was done by hand,
  made one safe command) and the tasks' return. Chained tasks deliver: B, the warm copy and
  delta (MIG-032); C, the GitHub App check, the "moved" notes in Settings → Instance and the
  remote CLI following the move (MIG-042, MIG-043); D, the button (MIG-060). Each is a separate
  concern with its own tests, and B and D build on the command's state and status files.
- **Limits are not re-sized here.** SPEC-459 owns the formulas and adapts at the first start from
  the import's mark; the move carries the pins and names them.
- **Sandbox package layers are named, not rebuilt** (MIG-051): the layers keep working on the
  copied files, and reinstalling packages per sandbox is a product feature of its own.
- **307, not 302 or 308**: temporary, so an abort is not defeated by a cached redirect, and
  method-preserving, so a client that follows it keeps posting.
