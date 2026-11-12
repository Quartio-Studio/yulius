# 0296. Host setup writes the AppArmor profile that lets bwrap create user namespaces

## Status

Accepted. Specified in
[SPEC-456](../../.ai/specs/SPEC-456-host-setup-makes-bwrap-able-to-create-us.md) (task #438).
Extends SPEC-203 (host setup) and [ADR-0003](0003-sandbox-isolation.md) (sandboxes are bubblewrap
user namespaces).

## Context

Ubuntu since 24.04 sets `kernel.apparmor_restrict_unprivileged_userns = 1`: a program without an
AppArmor profile that allows `userns` gets a user namespace in which every capability is denied.
Sandboxes are such namespaces, created by `/usr/bin/bwrap` as the service user.

- On Ubuntu 24.04 nothing allows it: `bwrap: setting up uid map: Permission denied`. Production
  worked only because someone wrote `/etc/apparmor.d/bwrap` by hand. Yulius neither wrote nor
  checked it, so a fresh host passed `host setup` and its sandboxes failed.
- Ubuntu 26.04 ships `/etc/apparmor.d/bwrap-userns-restrict`: a profile named `bwrap` that allows
  the namespace but runs what bwrap starts under `unpriv_bwrap`, which denies every capability.
  A sandbox mounts its system, maps ids and runs a Docker engine there, so it cannot start.
- Debian (12, 13) has AppArmor without that switch: bwrap works with no profile.

Measured on real hosts (SPEC-456, Context): two profiles for one program under different names
cancel each other (bwrap runs unconfined and fails); two files with one profile name replace each
other in load order, which a boot does not fix.

## Decision

- **A profile Yulius owns.** `yulius host setup` (item "bwrap namespaces") writes
  `/etc/apparmor.d/yulius-bwrap`: profile `yulius-bwrap` for `/usr/bin/bwrap`,
  `flags=(unconfined)`, the rule `userns,` and `include if exists <local/yulius-bwrap>`. It is
  what Ubuntu ships for other programs that need user namespaces. It is loaded with
  `apparmor_parser -r` and by AppArmor's own unit at every boot.
- **Only when it is needed, proven by behavior.** The item starts bwrap as the service user in a
  user namespace whose child mounts a tmpfs. If that works, nothing is written (Debian, a host
  without AppArmor, a host with a working hand-written rule, a second run).
- **The shipped profile is disabled the supported way.** A link in `/etc/apparmor.d/disable/` and
  `apparmor_parser -R`; the package's file is never edited or removed, and the link survives
  package upgrades. An enabled shipped profile is disabled even where bwrap works at that
  moment (a person's override in `local/bwrap-userns-restrict`, say): Yulius's sandboxes are
  proven against one known state, not against an override nobody at Yulius has seen.
- **A profile a person wrote is theirs.** Another enabled file that attaches to `/usr/bin/bwrap`
  is adopted when bwrap works with it, and never changed, disabled or joined by Yulius's profile.
  When bwrap does not work with it, the item fails and prints the commands that disable it.
  The item reads only the plain spellings of such a profile. One in any other spelling is found
  by what it does: Yulius's profile is proven with every profile of the directory loaded as a
  boot loads them; where bwrap does not start, it is proven once more without Yulius's
  profile, and a person's profile that lets it in is adopted in place of Yulius's, whichever
  run wrote that (SPEC-456 BNS-012). The same proof decides beside a profile the item reads,
  where a profile of Yulius's from an earlier run stands next to it (BNS-006): nothing of
  Yulius's is removed because of what a file's text looks like. A profile of Yulius's that a
  person's `disable/` link names is only unloaded where AppArmor still holds it; its file
  stays. While both files are on disk and bwrap starts, the item names both and settles
  nothing before the boot that loads them.
- **Checks report and never fix.** `yulius host check` (the update helper's re-check, Settings →
  Instance) runs the same proof and reports what setup would do. `yulius doctor --contained`
  starts bwrap with ids mapped by `newuidmap`, as a sandbox does, and fails when that does not
  work. `yulius instance import` names the step when the new host lacks it.

## Alternatives

- **The profile name `bwrap`** (as the hand-written rule). On 26.04 it shares its name with the
  shipped profile: whichever loads last wins, silently. Rejected; a distinct name makes the
  conflict visible and the shipped profile's state explicit.
- **`kernel.apparmor_restrict_unprivileged_userns = 0`.** One line, but it lifts the restriction
  for every program on the host. Rejected.
- **A private copy of bwrap with its own profile.** Sandboxes also run `/usr/bin/bwrap` inside
  (nested containment), where the shipped profile would attach again. Rejected.
- **Extending the shipped profile through `local/bwrap-userns-restrict` or a higher `priority=`.**
  Works only on 26.04, keeps the stacked child profile and depends on its text. Rejected.
- **Applying the fix in `host check`.** The update helper cannot write `/etc/apparmor.d`
  (`ProtectSystem=strict`) and has no `sbin` in its `PATH`; widening both for a change a host
  needs once is not worth it. Rejected.
- **Deciding from files only.** Cannot tell a working hand-written rule from a broken one, nor a
  profile that is written from one that is loaded. Rejected in favour of the proof.
- **Reading every spelling AppArmor takes for a profile's path**, or asking `apparmor_parser`'s
  debug dumps for it. A reader after the parser's grammar went through three reviews, each of
  which found one more corner, and what a pattern weighs beside an exact path only the kernel
  says; the dumps do not tell a profile's attachment from a child's, and before AppArmor 5.0 not
  a profile named by its path from a rule. Rejected for the proof beside the whole directory.

## Consequences

- Any local user can create an unconfined user namespace through `/usr/bin/bwrap` on a Yulius
  host, as with the hand-written rule and as on Debian. The kernel's namespace attack surface is
  reachable again for that one program; the restriction stays for every other program. A Yulius
  host is a dedicated machine whose agents already run in such namespaces.
- On Ubuntu 26.04 other programs that run `/usr/bin/bwrap` (Flatpak, for one) lose the shipped
  confinement of bwrap's children.
- A release upgrade from 24.04 to 26.04 installs the shipped profile next to Yulius's: sandboxes
  fail until `sudo yulius host setup` runs again. `host check` and the doctor say so.
- A host whose AppArmor restricts user namespaces but cannot load a `userns` rule is reported as
  unsupported, and Yulius's file is removed again (an earlier version of it gets its text back)
  so AppArmor's unit keeps loading cleanly.
- The proof needs the host's `mount` and `/usr/sbin/runuser` (util-linux, on every supported
  host).
- Whenever setup loads Yulius's profile it loads every profile of `/etc/apparmor.d` again
  (`apparmor_parser -r /etc/apparmor.d`, what `systemctl reload apparmor` loads): edits of
  other profiles that were waiting for a boot or a reload become live then, and a profile
  that was written and never loaded is loaded. The load may take up to 300 s; one that does
  not end fails the item, which undoes its steps.
- Where a person's profile lets bwrap in, setup removes `/etc/apparmor.d/yulius-bwrap`, also
  one an earlier run wrote and that worked until the person's profile came: the proof decides,
  and Yulius never disables a person's profile itself. For the length of that proof Yulius's
  profile is out of AppArmor, so a sandbox that starts in that moment fails. A person's
  profile that only AppArmor holds (loaded by hand from outside `/etc/apparmor.d`) is adopted
  as well and is gone after a boot: the check fails then, and the next setup writes Yulius's
  profile again.
