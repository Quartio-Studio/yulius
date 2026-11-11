---
yulius-task: bec4e78a-38ca-448b-965f-c05492cbe528
status: in-progress
---

# SPEC-456: Host setup makes bwrap able to create user namespaces (AppArmor)

Status: proposed
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
| BNS-003 | **Missing rule.** With AppArmor on and no profile for `/usr/bin/bwrap`, when the proof fails (or, unproven, when `kernel.apparmor_restrict_unprivileged_userns` is 1), setup writes `/etc/apparmor.d/yulius-bwrap` (profile `yulius-bwrap`, `flags=(unconfined)`, `userns,`, `include if exists <local/yulius-bwrap>`), loads it with `apparmor_parser -r`, proves again and reports **changed**. A file that differs from this text is rewritten; one that is current but not loaded is loaded. | `a_fresh_noble_host_gets_the_profile`, `a_changed_profile_is_rewritten`, `an_unloaded_profile_is_loaded`; VM (24.04) |
| BNS-004 | **Present.** A host where the proof passes is **ok** and nothing is written or loaded: a second run, a rebooted host, a host without AppArmor, Debian (AppArmor on, no restriction). The line says which rule allows it. | `a_set_up_host_changes_nothing`, `a_rebooted_host_changes_nothing`, `no_apparmor_writes_nothing`, `debian_writes_nothing`; VMs |
| BNS-005 | **Shipped profile.** When `/etc/apparmor.d/bwrap-userns-restrict` is enabled, setup disables it the supported way (the link in `disable/`, then `apparmor_parser -R`), never editing or removing the file, then loads the profile that is to attach (ours, or a hand-written one again) and proves. It does so also when the proof passes at that moment (same-name race at boot). | `a_resolute_host_disables_the_shipped_profile`, `a_shipped_profile_beside_a_hand_written_one_is_disabled`; VM (26.04, reboot, `apparmor` reinstall) |
| BNS-006 | **Hand-written rule.** Another enabled file in `/etc/apparmor.d` whose profile attaches to `/usr/bin/bwrap` is never changed, disabled or removed, and Yulius's profile is never written next to it. When the proof passes the item is ok and names the adopted file. When it fails, setup loads that file again once (what a boot does); if it still fails the item is **FAIL** with the commands that disable that file. | `a_hand_written_rule_is_adopted`, `a_hand_written_rule_that_fails_is_named_not_touched`; VM (24.04 with the production rule) |
| BNS-007 | **Unsupported or broken.** When AppArmor cannot load Yulius's profile, the file is removed again and the item is FAIL with the parser's error and the word "unsupported". When the proof fails and AppArmor is off, or still fails after the fix, the item is FAIL with bwrap's error and, where they apply, the cause: `kernel.unprivileged_userns_clone = 0`, `user.max_user_namespaces = 0`, a link that disables Yulius's profile. A failed command or write is named once. | `a_parser_that_cannot_read_the_rule_is_unsupported`, `a_namespace_limit_is_named`, `a_failed_write_is_named` |
| BNS-008 | **Check and dry run change nothing.** `host check` reports what setup would do as FAIL "…; run `sudo yulius host setup`" (so the update helper's re-check and Settings → Instance show it, with no new contract), and ok only when nothing is to do. `--dry-run` says "would …". Neither writes, links nor runs `apparmor_parser`. | `a_check_reports_and_changes_nothing`, `a_dry_run_says_what_it_would_do`; VM (`host check` before and after) |
| BNS-009 | **Doctor.** `yulius doctor --contained`, once isolation resolves, starts bwrap as sandboxes do (`Mapper`: ids mapped by `newuidmap`/`newgidmap`, the same namespace arguments, the same mount) and reports **sandbox namespaces**: ok, or FAIL (exit 1) with bwrap's error and `sudo yulius host setup` as the fix. Host setup's "contained doctor" line therefore fails on such a host. | `doctor::tests` (the line for both results), `userns` test (arguments); VMs (before and after) |
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
  - Shipped profile enabled → disable it.
  - Another file attaches to bwrap → never write ours; load theirs again when the shipped one was
    disabled or the proof fails.
  - Otherwise write and load ours when it is needed: the first proof fails with AppArmor on; or,
    when the proof cannot say (not run, or the shipped profile is about to be disabled), when the
    restriction switch is 1.
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
- [ ] Host setup writes and loads Yulius's AppArmor profile when bwrap cannot start a namespace
- [ ] Host setup disables the shipped profile, adopts a hand-written rule and names what is unsupported
- [ ] Host check and dry run report the item without changing the host
- [ ] The contained doctor proves a mapped user namespace
- [ ] Instance import names the step when the host lacks it
- [ ] Operations guide, architecture and changes describe the rule
- [ ] Proof on real Ubuntu 24.04, Ubuntu 26.04 and Debian hosts
- [ ] Gates and independent review

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
  Debian, unsupported parser, changed and unloaded profile, failed write, check, dry run, a host
  without service user or bwrap (not proven), reboot.
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
