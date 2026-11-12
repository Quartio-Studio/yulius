# 0305. A root helper runs the move asked from the page, and the checklist travels with it

## Status

Accepted. Specified in
[SPEC-470](../../.ai/specs/SPEC-470-run-migration-the-button-in-settings-ins.md); task #447, slice
D of [0297](0297-one-root-command-moves-an-instance-and-drives-the-new-host-over-a-one-time-key.md)
(SPEC-460 MIG-060). Follows the pattern of [0013](0013-updates-from-main.md).

## Context

- `yulius instance migrate` is a root command on the old host; a superadmin should start it from
  Settings → Instance, see the same checklist and be asked the same things first.
- The server is the unprivileged service user. It is frozen during the copy and, after a move,
  frozen for good on the old host; its database is part of what is copied.
- The request names a host that root will give the whole instance to. The service user writes
  the request, so whoever takes over the server process can write one too.
- The page that started a move has to say what happened after it, on the old address or on the
  new one, to a browser whose session belongs to the old address.

## Decision

- **A request file and a root path unit, as for updates.** The server writes
  `<data>/migrate/request.json` (plan, move or abort, with the target, SSH port, public URL and
  data disks; no secret field exists). `yulius-migrate.path` starts `yulius migrate-helper`,
  which takes the file only from the data directory's owner, with the exact schema, fresh, and
  an id once; refuses while an update runs; and checks every value again with the rules the
  server used.
- **The helper runs the command as a child process**, with each value as one argument and
  never `--yes` for an abort. It reports the request, a heartbeat and the command's exit code in
  `helper.json`, and passes an abort that arrives meanwhile on as SIGTERM, which the command
  answers by its own rule (thaw before the new instance's start is ordered, ignore after). It
  does not touch the command's checklist: a run the command refuses changes nothing.
- **A move asked through the helper is nobody's sign-in.** The helper always runs the command
  with `--no-sign-in`: the operator's sign-in that adds the move's key is never tried, no ssh
  configuration is read, and every connection offers the one-time key alone (no agent, no
  other method). So the new host is reached only once a person there added the key's line,
  which the plan shows. Root's own keys, and the keys its ssh configuration names, never serve
  a request the service user wrote.
- **Root opts in, and may pin the targets.** `host setup` does not install the units. With them
  enabled the service user can have root move the instance to a host whose root added the
  move's key; `YULIUS_MIGRATE_TARGETS` in the root-owned unit limits the hosts further, and
  disabling the path unit closes the way. A move carries the units and enables the path unit on
  the new host only when it was enabled on the old one.
- **The view is computed from files, never stored.** The request, the helper's report and the
  command's `status.json` are read on every request; a request the helper took, or an old one,
  is removed, and no copy of the data directory takes the request along (`migrate/` is left
  out, as `update/` is). Whose the checklist is, is told by its time: written before the
  helper's last run started, it is an earlier run's. Nothing about a move is in the database,
  so nothing about it is copied as if the new host were moving.
- **The checklist travels with the Finish step.** The old host brings the lines so far and the
  lines it will add after the step; the new host's Finish adds its own and leaves the whole as
  `arrived.json` in the flow's directory there, root's and readable, where its server reads it.
  Nothing is written into the data directory, which the running instance's user owns. The page
  on the old address notices the redirect (a fetch that does not follow redirects) and links to
  the same page at the new URL.
- **A move needs a plan shown for the same values**, at most an hour old, without a failed line
  and without a key still to add; a move that stopped is continued with the same values.

## Alternatives

- **Calling the command inside the helper process**: one process less, but the helper could not
  stop it, outlive its signal handling or report how it ended.
- **A run row in the database, as updates have**: the row would say "running" on the frozen old
  host for ever and arrive with the copy on the new host as a move of that host.
- **Reading the status from the old host after the move**: its instance is frozen, so its proxy
  would have to serve the checklist, to anyone, without a session.
- **An abort from the page with `--yes`**: it would answer "is the new instance stopped?" with
  nobody having looked.
- **Running the command as from a root shell, sign-in included**: without a terminal `ssh`
  still offers root's keys, so the service user would reach every host that trusts root here
  (a backup host, another server), and a move would set that host up as root. Found by the
  independent review; the first version did this.
- **The helper removing the command's checklist before a run**, so that an earlier one is
  never read as this run's: a run the command then refuses (another move is recorded) would
  have wiped the recorded move's checklist. The server compares times instead.
- **Requiring a root-written list of targets always**: the superadmin could not name the host
  on the page, which is what was asked; the list stays an option of the unit.
- **Installing the units with `host setup`**: every instance would let its service user ask
  root for a move, wanted or not.
- **Following the redirect in the page's own requests**: a cross-origin redirect is a network
  error for a page, and the session cookie belongs to the old address.

## Consequences

- With the path unit enabled, a takeover of the server process can move the instance away and
  freeze it here, to a host whose root added the move's key and to no other. It could read the
  data already; what it gains is the host's files the move carries (the unit's environment
  files, the proxy's configuration, `/usr/local`, root's Rust toolchain) and the stop of this
  instance. The operations guide says so and how to limit it.
- The helper's unit has no start timeout and little sandboxing: the command reads the whole
  host, writes the proxy's configuration and freezes units, as it does from a root shell.
- A move asked from the page always needs a person on the new host and two plans: the first
  ends with the key line to add there. The new host is named so that it is reached without
  root's ssh configuration (a name that resolves, or an address).
- After a move to another URL the person signs in again at the new address; the page there
  shows the finished checklist until the next move replaces it.
- A move started on the host shows on the page as a recorded checklist; whether it still runs
  is known only for runs the helper started.
