# 0308. A running Docker smoke test's sandbox is the smoke's alone

## Status

Accepted. Specified in [SPEC-477](../../.ai/specs/SPEC-477-docker-smoke-fails-on-the-first-host-set.md);
task #459, issue #1160. Beside [0158](0158-docker-reaches-every-run-through-a-front-and-a-directory-moved-into-live-runs.md)
(the Docker front and its adoption) and
[0162](0162-one-host-setup-command-brings-and-keeps-docker-in-sandboxes.md) (`host setup` runs the
smoke test).

## Context

`yulius doctor --docker-smoke` proves Docker in sandboxes with a throwaway sandbox,
`docker-smoke-<pid>`, kept in the data directory next to the real ones (`sandboxes/`,
`sysroots/`, `runtime/sandboxes/`). It walks that sandbox through an upgrade with managers of its
own: first a holder whose Docker is off, whose front must refuse with the reason, then the same
holder adopted with Docker on.

A Yulius server adopts, when it starts, the holders "a previous server process started":
`Manager::adopt` took every entry of `runtime/sandboxes`. A server that started while a smoke ran
therefore took the smoke's holder too. With Docker on it replaced the holder's refusing front with
an engine front of its own and kept the holder in its upkeep. Two managers then owned one holder,
each taking the other's front for a dead one and starting its own at the same socket: the smoke's
step "a sandbox without Docker answers why" read a live engine, or the server's next upkeep
replaced the smoke's engine front under a running `docker compose up`. `yulius host setup` starts
`yulius.service` on its first run and runs the smoke a second later, so the first setup of a
fresh host failed its smoke (issue #1160), and so could the setup `yulius instance migrate` runs
on a new host. A restart
of the server during the smoke started from Settings → Instance met the same sandbox: that smoke
is the server's child, which a restart does not end (`KillMode=process`).

The leftovers scan, the smoke's own cleanup of interrupted smokes and the list of sandboxes on
their own Docker already read `docker-smoke-<pid>` as the smoke's while process `<pid>` lives.

## Decision

While process `<pid>` runs, the sandbox `docker-smoke-<pid>` and its production holder
`docker-smoke-<pid>.prod` belong to that smoke test alone. `Manager::adopt`, the scan a starting
server runs, skips them: no entry in the server's upkeep, no relay, and their Docker front is
neither stopped, replaced nor started. The rule is the name plus the live process
(`yulius_domain::leftovers::smoke_pid` and the manager's process check), the one the leftovers scan
uses. Once the process is gone the holders are like any other: the next server start adopts them,
the upkeep stops them when idle, and the paths that remove a stale smoke's files stay as they are.
`Manager::adopt_one` stays unconditional: the smoke adopts and removes its own sandbox through it.

Whatever reads the sandboxes of a data directory on its own (a scan, a cleanup, a migration)
keeps to the same rule.

## Alternatives

- **The smoke restores its refusing front when it finds another.** It hides the fight and does not
  end it: the server keeps the holder in its upkeep and starts its engine front again, and every
  later phase of the smoke shares the holder with a second manager.
- **`host setup` waits until the server answers before the smoke.** It narrows the window of the
  first run only; a migration's new host and a restart during a smoke stay as they were.
- **A marker file in the smoke's state directory.** A second way to say what the name already
  says, which the other readers would not know.
- **The smoke keeps its sandbox outside the data directory's sandbox trees.** The smoke proves the
  paths real sandboxes use (the base, the cgroup, the removal as root of a mapped namespace);
  moving it would prove something else and touch every path helper.

## Consequences

- The first `host setup` of a fresh host passes its smoke, and a smoke's result no longer depends
  on a server starting beside it.
- A smoke sandbox whose process id was taken by another long-lived process after the smoke was
  killed is skipped by a starting server as long as that process lives, as the leftovers scan
  already skips it; its holder then runs unmanaged until that process ends and a later server
  start adopts it or the next smoke removes it.
- A new scan of `runtime/sandboxes` must use the same rule; the manager's test
  `a_starting_server_leaves_a_running_smoke_tests_holders_alone` holds it for `adopt`.
