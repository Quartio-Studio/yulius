---
yulius-task: bec4e78a-38ca-448b-965f-c05492cbe528
status: done
---

# SPEC-456: Host setup makes bwrap able to create user namespaces (AppArmor)

Status: implemented
Date: 2026-11-11
Owner: Yulius task #438
Scope authorization: the owner's request (task #438): "Make the AppArmor rule that lets bubblewrap
create user namespaces part of installing Yulius (`yulius host setup`) and of every host check, so
no host ever needs a hand-written rule again." Extends SPEC-203 (host setup, HST-001 item order,
HST-002 idempotent, HST-012 the check's safe fixes) and SPEC-329 (IMP-006, the import's next
steps). ADR:
[0296](../../docs/decisions/0296-host-setup-writes-the-apparmor-profile-that-lets-bwrap-create-user-namespaces.md).

## Goal

`sudo yulius host setup` leaves every supported host (Ubuntu 24.04, Ubuntu 26.04, Debian) in a
state where `/usr/bin/bwrap` can create the user namespaces sandboxes run in, proves it by starting
one, and keeps it so over reboots and `apparmor` package upgrades. `yulius host check`, Settings →
Instance, `yulius doctor --contained` and `yulius instance import` say when a host lacks it and name
the fix. Nobody writes an AppArmor rule by hand again.

## Context

- **The request (2026-11-11).** Production (Ubuntu 24.04.5) runs contained sandboxes only because
  someone hand-wrote `/etc/apparmor.d/bwrap` (`profile bwrap /usr/bin/bwrap flags=(unconfined) {
  userns, }`). Nothing in Yulius creates or checks it. The new VM (Ubuntu 26.04.1) ships
  `/etc/apparmor.d/bwrap-userns-restrict`, a profile of the same name that confines what bwrap
  starts.
- **Measured on real hosts** (QEMU guests from the distributions' cloud images, 2026-11-11):

  | Host | AppArmor | Fresh host | With the fix |
  | --- | --- | --- | --- |
  | Ubuntu 24.04.5 (kernel 6.8, apparmor 4.0.1, bubblewrap 0.9.0) | `kernel.apparmor_restrict_unprivileged_userns = 1`, no profile for bwrap | `bwrap: setting up uid map: Permission denied` | `yulius-bwrap (unconfined)`, a mount inside works |
  | Ubuntu 26.04.1 (kernel 7.0, apparmor 5.0.2, bubblewrap 0.11.1) | same switch; ships `bwrap-userns-restrict` (profiles `bwrap` and `unpriv_bwrap`, enforce) | bwrap starts, its child runs as `bwrap//&unpriv_bwrap` and a mount is "permission denied" (every capability denied) | shipped profile disabled, `yulius-bwrap (unconfined)`, a mount inside works |
  | Debian 13 (kernel 6.12, apparmor 4.1.0), Debian 12 (kernel 6.1, apparmor 3.0.8) | AppArmor on, no such switch, no profile for bwrap | works | nothing to write |

  - Two profiles of different names for `/usr/bin/bwrap` (ours next to a hand-written or the
    shipped one): neither attaches, bwrap runs unconfined and fails as on a fresh 24.04.
  - Two files with the same profile name (hand-written `bwrap` next to the shipped one): the one
    loaded last wins, so a boot can go either way.
  - The supported way to disable a shipped profile: a link in `/etc/apparmor.d/disable/` plus
    `apparmor_parser -R <file>`. `-R` removes the profile by name, whoever loaded it, so a
    hand-written `bwrap` has to be loaded again after it. `apparmor_parser -r` skips a file that
    has a link in `disable/`.
  - Persistence: after a reboot of both Ubuntu guests and after `apt-get install --reinstall
    apparmor` on 26.04 the link stays, the shipped profile stays unloaded and bwrap works.
  - Under the update helper's hardening (`NoNewPrivileges`, `ProtectSystem=strict`, `PrivateTmp`)
    root can still run `bwrap --unshare-user --uid 0 --gid 0 …` as the service user; `newuidmap`
    (setuid) cannot run there. The helper's `PATH` has no `sbin`, where `runuser` and
    `apparmor_parser` live, and it cannot write `/etc/apparmor.d`.
  - A probe that only starts bwrap passes on a fresh 26.04: the child itself has to use a
    capability (a mount) to meet the shipped profile's denial.
- **Code.** `host_setup.rs` runs items over a `Host` (real or the tests' fake) in three modes
  (setup, dry run, check); `Run::user_and_ids` is the subset the import runs. The doctor's
  "sandbox isolation" line only resolves programs and id ranges; "bubblewrap" runs `bwrap
  --version`. `yulius_sandbox::userns::Mapper` starts bwrap with ids mapped by `newuidmap`, as the
  namespace holder does. The host check's lines reach Settings → Instance as they are
  (`HostCheckReport`, `sandboxdocker.js`).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| BNS-001 | `yulius host setup` has the item **bwrap namespaces**, after "subordinate ids" and before "yulius.service", in setup, `--dry-run` and `host check`. The import's `user_and_ids` subset is unchanged. | `host_setup::tests::bwrap_userns` (order), `an_import_makes_only_the_user_and_its_ids` |
| BNS-002 | **The proof.** When the service user exists and `bubblewrap` is installed, the item starts `bwrap --unshare-user --uid 0 --gid 0 --unshare-net --dev-bind / / --cap-add ALL --chdir / -- mount -t tmpfs none /tmp` as the service user (`/usr/sbin/runuser`, found without `sbin` in `PATH`): a namespace that bwrap creates and whose child uses a capability. It runs in every mode (it changes nothing), also under the update helper's hardening. Otherwise the item decides from files and says "not proven" with the reason. | fake-host tests; VM runs of `host setup`, `host check` and the helper's hardening |
| BNS-003 | **Missing rule.** With AppArmor on, `kernel.apparmor_restrict_unprivileged_userns` at 1 and no profile for `/usr/bin/bwrap`, when the proof fails or could not run, setup writes `/etc/apparmor.d/yulius-bwrap` (profile `yulius-bwrap`, `flags=(unconfined)`, `userns,`, `include if exists <local/yulius-bwrap>`), loads it with `apparmor_parser -r`, proves again and reports **changed**. A file that differs from this text is rewritten; one that is current but not loaded is loaded. | `a_fresh_noble_host_gets_the_profile`, `a_changed_profile_is_rewritten`, `an_unloaded_profile_is_loaded`; VM (24.04) |
| BNS-004 | **Present.** A host where the proof passes is **ok** and nothing is written or loaded: a second run, a rebooted host, a host without AppArmor, Debian (AppArmor on, no restriction). The line says which rule allows it. | `a_set_up_host_changes_nothing`, `a_rebooted_host_changes_nothing`, `no_apparmor_writes_nothing`, `debian_writes_nothing`; VMs |
| BNS-005 | **Shipped profile.** When `/etc/apparmor.d/bwrap-userns-restrict` is enabled, setup disables it the supported way (the link in `disable/`, then `apparmor_parser -R`), never editing or removing the file, then loads the profile that is to attach (ours, or a hand-written one again) and proves. It does so also when the proof passes at that moment (same-name race at boot). `apparmor_parser -R` may fail only with exit 254 (not loaded). A shipped profile that its link disables but the kernel may still hold (a person's link, an unload that failed) is unloaded when the proof fails. | `a_resolute_host_disables_the_shipped_profile`, `a_release_upgrade_is_reported_and_fixed`, `a_shipped_profile_beside_a_hand_written_one_is_disabled`, `a_disabled_shipped_profile_that_is_still_loaded_is_unloaded`; VM (26.04, reboot, `apparmor` reinstall) |
| BNS-006 | **Hand-written rule.** Another enabled file in `/etc/apparmor.d` whose profile attaches to `/usr/bin/bwrap` (a top-level header, whatever its layout: on one line, with the brace on the next line) (also through a variable set to bwrap's path) is never changed, disabled or removed, and Yulius's profile is never written next to it. When the proof passes the item is ok and names the adopted file. When it fails, setup loads that file again once (what a boot does); if it still fails, the item is **FAIL** with what was done and the commands that disable that file; if that load fails (the parser refuses the file, so it cannot unload it either), the item is **FAIL** with the parser's words and asks for a correct file. | `a_hand_written_rule_is_adopted`, `a_hand_written_rule_that_fails_is_named_not_touched`, `a_profile_header_is_found_in_any_layout`; VM (24.04 with the production rule) |
| BNS-007 | **Unsupported or broken.** When AppArmor cannot load Yulius's profile, the file is removed again and the item is FAIL with the parser's error and the word "unsupported". When the proof fails and AppArmor is off, or still fails after the fix, the item is FAIL with bwrap's error and, where they apply, the cause: `kernel.unprivileged_userns_clone = 0`, `user.max_user_namespaces = 0`, a link that disables Yulius's profile; with such a kernel limit a person's profile is not blamed and no command that disables it is printed. A failed command or write is named once; a step that fails after the write is undone (Yulius's file removed again, or given its earlier text when it existed before the run; the shipped profile enabled again) and the line says so, or says what could not be undone. | `a_parser_that_cannot_read_the_rule_is_unsupported`, `a_namespace_limit_is_named`, `a_failed_write_is_named`, `a_failed_step_is_undone_and_named` |
| BNS-008 | **Check and dry run change nothing.** `host check` reports what setup would do as FAIL "…; run `sudo yulius host setup`" (so the update helper's re-check and Settings → Instance show it, with no new contract), and ok only when nothing is to do. `--dry-run` says "would …". Neither writes, links nor runs `apparmor_parser`. | `a_check_reports_and_changes_nothing`, `a_dry_run_says_what_it_would_do`; VM (`host check` before and after) |
| BNS-009 | **Doctor.** `yulius doctor --contained`, once isolation resolves, starts bwrap as sandboxes do (`Mapper`: ids mapped by `newuidmap`/`newgidmap`, the same namespace arguments, the same mount) and reports **sandbox namespaces**: ok, or FAIL (exit 1) with bwrap's error and `sudo yulius host setup` as the fix. Host setup's "contained doctor" line therefore fails on such a host. | `doctor::tests` (the line for both results), `userns::tests::the_proof_mounts_in_the_holders_namespace` (arguments); VMs (before and after) |
| BNS-010 | **Import.** After a successful import, `yulius instance import` evaluates the item without changing anything and, when the host lacks it, names it first in step 1 of "Next:" (`sudo yulius host setup …` lets bwrap create user namespaces). It never tells anyone to write a rule by hand. | `import::tests` (text with and without); VM (import on fresh 24.04) if a copy can be made, else the text test |
| BNS-011 | **Documents.** The operations guide (§1 prerequisites, §2 host setup items, Sandbox isolation, the update helper's re-check) describes the rule, why it exists, what setup does on 24.04, 26.04 and Debian, and a release upgrade from 24.04 to 26.04. ADR-0296 records the decision and its security trade-off. | `python3 scripts/check_repository.py` |

## Design

- **Module.** `crates/yulius/src/host_setup/bwrap_userns.rs`: the profile's text and paths, the
  facts read through `Host` (AppArmor on: `/sys/module/apparmor/parameters/enabled` is `Y`; the
  restriction switch; the shipped file and its link; Yulius's file: missing, changed, current; the
  other enabled top-level files whose profile header names `/usr/bin/bwrap`), a pure plan
  (disable, load again, write, load) from the facts and the first proof, its execution, the second
  proof and the line. Tests in `host_setup/tests/bwrap_userns.rs` over the fake host, which gains
  an AppArmor model written from the measurements above (loaded profiles, `apparmor_parser -r/-R`,
  `disable/` links, a reboot that loads every enabled file).
- **Plan rules.**
  - Shipped profile enabled → disable it. Disabled by its link and the proof fails → unload it
    (the link alone does not).
  - Another file attaches to bwrap → never write ours; load theirs again when the shipped one was
    disabled or the proof fails.
  - Otherwise write and load ours when it is needed: AppArmor is on, the restriction switch is 1
    and the first proof fails or cannot say (not run, or the shipped profile is about to be
    disabled). A link that disables Yulius's own profile is a person's choice: nothing is
    written, the shipped profile is not disabled for it, and the cause is named.
  - Nothing to do and the proof fails → FAIL with the cause.
- **Why behavior first.** The files say what should happen and the proof says what does: adoption
  needs no parser for other people's profiles, and a host where bwrap already works is never
  touched.
- **Doctor.** `Mapper::probe` in `yulius-sandbox` (the namespace holder's arguments without the
  spec, then the mount), called by `doctor::isolation_checks` after "sandbox isolation" is ok; a
  pure function turns the result into the line.
- **Import.** `Run::namespaces` (the data directory, the service user, this item) in check mode on
  the real host after the copy; a pure `next_steps` builds the text.

## Plan

- [x] Spec and decision record for the bwrap namespaces item
- [x] Host setup writes and loads Yulius's AppArmor profile when bwrap cannot start a namespace
- [x] Host setup disables the shipped profile, adopts a hand-written rule and names what is unsupported
- [x] Host check and dry run report the item without changing the host
- [x] The contained doctor proves a mapped user namespace
- [x] Instance import names the step when the host lacks it
- [x] Operations guide, architecture and changes describe the rule
- [x] Proof on real Ubuntu 24.04, Ubuntu 26.04 and Debian hosts
- [x] Gates and independent review

## Acceptance criteria

- On a fresh Ubuntu 24.04 host `sudo yulius host setup` writes and loads
  `/etc/apparmor.d/yulius-bwrap`; a second run and a run after a reboot change nothing
  (BNS-003, BNS-004).
- On a fresh Ubuntu 26.04 host it also disables `bwrap-userns-restrict` by a link, and the state
  survives a reboot and an `apparmor` reinstall (BNS-005).
- On a host with the production hand-written rule it writes nothing and names the adopted file
  (BNS-006).
- On Debian and on a host without AppArmor it writes nothing (BNS-004).
- `yulius host check` on a host that lacks the rule exits 1 with a "bwrap namespaces" FAIL naming
  `sudo yulius host setup`, changing nothing; `yulius doctor --contained` fails with "sandbox
  namespaces" there (BNS-008, BNS-009).
- The import's "Next:" names the step only when the host lacks it (BNS-010).

## Test plan

- Unit tests over the fake host for every branch: missing, present, shipped conflict, shipped next
  to a hand-written rule, hand-written equivalent, hand-written rule that fails, no AppArmor,
  Debian, unsupported parser, changed and unloaded profile, failed write, a step that fails midway
  (undone), a disabled shipped profile still loaded, check, dry run, a host without service user,
  bwrap or `runuser` (not proven), reboot.
- Unit tests for the doctor's line, the probe's arguments and the import's text.
- Real hosts (QEMU guests): the built binary's `host setup`, `host check`, `doctor --contained` on
  Ubuntu 24.04 and 26.04 (fresh, second run, reboot, hand-written rule, the helper's hardening) and
  Debian 13.
- Gates: `scripts/dev ci`, `python3 scripts/check_repository.py`; the security scan is disabled for
  this project and reported so.

## Decisions

- **A Yulius-owned profile, `yulius-bwrap`, in its own file** (ADR-0296). Not the name `bwrap`: a
  shipped profile of that name would silently replace it or be replaced by it.
- **Disable the shipped profile by AppArmor's `disable/` link**, never by editing it: the package
  keeps owning its file and upgrades keep the link.
- **Adopt by behavior, never write next to another profile for bwrap**: two attachments break
  bwrap, and the first shows only after the next boot.
- **Check mode is report-only for this item**: the update helper cannot write `/etc/apparmor.d`
  and has no `apparmor_parser` in its `PATH`; widening the helper's write access for a one-time fix
  is not worth it.
- **The probe mounts with the host's `mount`**, no hidden subcommand and no shell: one fixed
  argument list, the same for root's proof and the doctor's.
- **The doctor's line is a failure, not a warning**: isolation resolved, so the server would start
  sandboxes that cannot start.
- **Not done:** setting `kernel.apparmor_restrict_unprivileged_userns=0` (weakens every program on
  the host), a private copy of bwrap (sandboxes exec `/usr/bin/bwrap` inside too), a `local/`
  override of the shipped profile (26.04 only, keeps its stacking).

## Evidence

2026-11-11. Real hosts are throwaway QEMU guests (TCG) booted from the distributions' cloud images
in the task's sandbox, fresh for these runs: Ubuntu 24.04.5 (kernel 6.8.0-147), Ubuntu 26.04.1
(kernel 7.0.0-38), Debian 13 and Debian 12. The binary is this branch built in a `debian:12`
container: at `b74138b1` for the first runs, at `39c85810` for the runs after review round 1
(marked "re-proof") and at `263c57bf` for the refused file.

- **Fresh host, `sudo yulius host setup --public-url …`** (base build, contained doctor, Docker
  smoke):
  - 24.04: `changed bwrap namespaces wrote and loaded /etc/apparmor.d/yulius-bwrap: bwrap starts a
    user namespace as yulius`; "sandbox namespaces" ok, contained doctor passed.
  - 26.04: `changed bwrap namespaces disabled Ubuntu's AppArmor profile bwrap-userns-restrict (a
    link in /etc/apparmor.d/disable; its file is untouched), wrote and loaded
    /etc/apparmor.d/yulius-bwrap`; the sandbox base built (apt inside the namespace), contained
    doctor passed, and the Docker smoke ran hello-world, a build and compose in sandboxes.
  - Debian 13 and 12: `ok bwrap namespaces … AppArmor does not restrict user namespaces here`,
    nothing written; contained doctor passed.
  - Before setup, `--dry-run` said "would write and load" (24.04), "would disable …, write and
    load" (26.04) and ok (Debian), changing nothing.
- **Second run, `host check`, the helper's hardening, the doctor** (all four hosts): the item is
  ok and names what allows it; `yulius host check` under `systemd-run -p NoNewPrivileges=yes -p
  ProtectSystem=strict -p PrivateTmp=yes` with the helper's `PATH` (no `sbin`) gives the same line;
  `yulius doctor --contained` as the service user: `ok sandbox namespaces`.
- **A host that lacks it** (24.04 and 26.04, Yulius's profile unloaded; 26.04 also with the shipped
  profile enabled again as a release upgrade leaves it): `host check` and the helper-style check
  exit 1 with `FAIL bwrap namespaces bwrap cannot start a user namespace as yulius (…): setup would
  [disable Ubuntu's AppArmor profile bwrap-userns-restrict …,] load /etc/apparmor.d/yulius-bwrap;
  run `sudo yulius host setup``, changing nothing; the doctor exits 1 with `FAIL sandbox
  namespaces sandboxes cannot start: …`. `host setup` repairs both (changed), then all are ok.
- **Persistence**: after a reboot of each host the profile state is the same, `host check` and the
  doctor are ok. On 26.04 after `apt-get install --reinstall -y apparmor` the shipped file is
  unchanged (`dpkg -V apparmor` clean), the link stays, the shipped profile stays unloaded and both
  checks are ok.
- **Hand-written rule** (24.04, the production file `/etc/apparmor.d/bwrap`, no profile of
  Yulius's): `ok bwrap namespaces … allowed by /etc/apparmor.d/bwrap (not written by Yulius, left
  as it is)`, file unchanged, nothing written. With its `userns,` line removed: FAIL naming the
  file and the two commands that disable it, file unchanged; after those commands setup writes and
  loads Yulius's profile.
- **Re-proof after review round 1** (the same two Ubuntu hosts; second setup, helper-style check
  and doctor ok first):
  - 24.04, a hand-written rule on one line that was never loaded: check FAIL "setup would load
    /etc/apparmor.d/bwrap again", setup `changed … loaded /etc/apparmor.d/bwrap again: bwrap
    starts a user namespace as yulius`, nothing of Yulius's written, then ok "allowed by
    /etc/apparmor.d/bwrap (not written by Yulius, left as it is)".
  - 24.04, the same rule confining bwrap: FAIL "loaded /etc/apparmor.d/bwrap again, but bwrap
    cannot start a user namespace as yulius with /etc/apparmor.d/bwrap (…); Yulius does not
    change a profile it did not write: disable it (…)", file unchanged.
  - 24.04, a file the parser refuses while an earlier version of it is loaded: the first run
    printed the disable commands, and `apparmor_parser -R` failed on that file (it reads it), so
    the old profile stayed loaded and Yulius's profile beside it did not help. Fixed test-first:
    the line now reads "`/usr/sbin/apparmor_parser -r /etc/apparmor.d/bwrap` failed: AppArmor
    parser error … ; Yulius does not change a profile it did not write: correct
    /etc/apparmor.d/bwrap, then run `sudo yulius host setup` again", file unchanged; with the
    file corrected setup loads it again (changed) and the check, the helper-style check and the
    doctor are ok.
  - 26.04, the shipped profile disabled by its link but still loaded: check, helper-style check
    and doctor fail ("setup would make sure Ubuntu's disabled AppArmor profile
    bwrap-userns-restrict is not loaded, load /etc/apparmor.d/yulius-bwrap"); setup unloads it
    (changed), then all ok.
  - 26.04, a step that fails: with `/etc/apparmor.d/disable` made immutable, FAIL "`ln -s …`
    failed: … Permission denied; /etc/apparmor.d/yulius-bwrap was removed again"; with `abi/4.0`
    moved away, FAIL "unsupported: this host's AppArmor cannot load Yulius's profile (… Could not
    open 'abi/4.0' …); /etc/apparmor.d/yulius-bwrap was removed again; Ubuntu's AppArmor profile
    bwrap-userns-restrict is enabled again"; files and loaded profiles as before each run, and
    setup on the restored host repairs it.
  - 26.04, Yulius's profile disabled by a link beside the enabled shipped profile: FAIL
    "/etc/apparmor.d/disable/yulius-bwrap keeps Yulius's profile from loading (remove that link,
    then run `sudo yulius host setup` again)", nothing changed; with the link removed setup
    disables the shipped profile and loads Yulius's.
- **Import** (an export of the 24.04 host imported on another fresh 24.04 host, bubblewrap not yet
  installed): step 1 of "Next:" reads "(lets bwrap start the user namespaces sandboxes run in,
  which this host does not allow yet; writes yulius.service, …)"; the `host setup` that follows
  writes and loads the profile.
- **Tests**: `cargo nextest run -p yulius -p yulius-sandbox` filters `bwrap_userns`, `host_setup`,
  `doctor`, `import`, `userns`: pass; each new test was seen failing first, and every mutation of
  the plan rule and its execution tried by hand was caught. `cargo clippy --all-targets -- -D
  warnings` clean for both crates.
- **Gates**: `scripts/dev ci` exit 0 at `3d74a9d0` (3311 tests) and again at `263c57bf` (3317
  tests run, 3317 passed, 16 skipped). After that `origin/dev` (#1161, a test fix in the turn
  code) was merged: `cargo check -p yulius --all-targets` and the filters above pass, and
  `python3 scripts/check_repository.py` passes. After round 2's fixes the same filters (115 tests),
  clippy and the repository check pass; the full gate on the final head is Yulius CI's run.
  Security scan: disabled (Vidoc is turned off for
  this project; `yulius security-scan` scanned nothing).
- **Review**: round 1 (fresh read-only session, `om-code-review`) asked for changes in own code:
  a failed step left the host half changed, a one-line profile header was not seen, a shipped
  profile disabled by its link but still loaded was not unloaded, Yulius's profile was written
  beside a link that disables it, a `runuser` that cannot start read as a failed proof, and the
  proof's arguments were not pinned. All fixed test-first (BNS-005, 006, 007, 009) and proven
  again on the hosts above. Round 2 approved `9d2e6ec8` with two should-fix items and nits in own
  code, fixed test-first after it (fake host; not run again on the VMs): the undo gives an
  earlier `yulius-bwrap` its text back instead of removing it; a hand-written profile is not
  blamed, and no disable commands are printed, when a kernel limit keeps bwrap out; a header
  like `{userns,}` and a path given through a variable are read; "not proven: the service user
  is not set up yet"; the ADR says why a working shipped profile is disabled too. Left as they
  are, with the reason: on 26.04 a person's loaded profile named `bwrap` whose file the parser
  now refuses is unloaded with the shipped one before its reload fails (the state the next boot
  gives anyway, and the line names the file to correct); the doctor's FAIL text names host setup
  for any namespace failure, with the failing command's own words in front. Round 3 reviews the
  head that carries this section.
- **Not tested**: a release upgrade from 24.04 to 26.04 itself (its end state, both profiles
  enabled, was made by hand on 26.04); a host whose AppArmor cannot parse a `userns` rule (fake
  host; on 26.04 a missing `abi/4.0` stood in for it); the production hosts themselves; Settings → Instance in a browser against such a host (it shows the host check's
  lines unchanged, and the check's lines are proven above).
- **Met on the way, outside this change**: on every fresh host with cgroup delegation the first
  `host setup` ends with `FAIL Docker smoke test` in the step "a sandbox without Docker answers
  why" (a live engine answers the Docker-off run; a later `doctor --docker-smoke` passes): issue
  #1160. Debian 12's systemd 252 does not know `DelegateSubgroup=`, so sandboxes there have no
  limits and no Docker (the guide already asks for systemd 254).
