# 0294. Resource limits are sized from the host, and a pinned number remembers its host

## Status

Accepted (SPEC-459). Amends the fixed defaults of ADR-0045 (live sessions), ADR-0065 (sandbox disk
quota) and ADR-0216 (CI jobs and lanes); the sandbox memory default of ADR-0198 stays.

## Context

Six limits decide how much work an instance takes: live agent sessions, CI jobs at once, CI runs
at once per sandbox, a sandbox's memory, its soft disk quota and the disk bound of a CD run. Only
the memory followed the host. The others were constants tuned on one 12-CPU VM (8, 4, 2, 100 GB,
20 GB), so operators pinned other numbers in unit drop-ins, and those numbers travelled with the
instance.

On 2026-11-11 the instance moved from 12 CPUs, 47 GiB and a 387 GB disk to 64 CPUs, 755 GiB and a
3.5 TB mirror. The drop-ins (`MAX_SESSIONS=8`, `SANDBOX_MEMORY_GB=38`, `SANDBOX_DISK_GB=300`) came
along and kept 9 tasks queued on an idle machine until a person set 32 sessions, 1000 GB, 12 CI
jobs and 4 lanes by hand. Nothing in the product said that a number was sized for another
machine, where a value came from, or let anyone change it without a unit edit and a restart.

## Decision

**Formulas.** Without a pin a limit is computed from the host's logical CPUs, its memory and the
size of the data directory's filesystem (integers, rounded down):

| Limit | Host-sized value | 12 CPUs, 47 GiB, 387 GB | 64 CPUs, 755 GiB, 3.5 TB |
| --- | --- | --- | --- |
| Live sessions | `cpus / 2 + 2`, at most one per 2 GiB, at least 2 | 8 | 34 |
| CI jobs | `cpus / 6 + 2`, at most 64 | 4 | 12 |
| CI runs per sandbox | `cpus / 32 + 2`, at most 8 | 2 | 4 |
| Sandbox memory | memory minus an eighth (at least 4 GiB), at least 2 GiB | 41 GiB | 660 GiB |
| Sandbox disk quota | a quarter of the disk, at least 100 GB, at most half the disk | 100 GB | 875 GB |
| CD run disk | a twentieth of the disk, at least 20 GB, at most half the disk | 20 GB | 175 GB |

They are calibrated on the two machines we know: the old VM keeps exactly the numbers it ran with,
and the new host lands next to what was chosen for it by hand (32, 12, 4, 1000 GB). A fact the
host does not report leaves the limit at its earlier constant. The formulas live in
`yulius_domain::limits`, pure and tested at small and large hosts.

**Precedence.** A number saved in Settings wins; else "host-sized" saved in Settings (the unit's
number is ignored); else the unit's `YULIUS_*` value; else the formula. "Host-sized" is a setting
of its own and not the formula's result written down as a number: an adapted limit keeps following
the host through the next move or resize instead of becoming a new pin.

**A pin remembers its host.** The `instance` row gets one JSON document (`limits`): the settings
(value or host-sized, who, when, the host's facts then) and every unit value the server has seen
with the host's facts when it first saw that number. A record survives the unit dropping the
value, so a drop-in copied from the old machine days after the move is still recognised.

**"No longer fits".** A pin is flagged when a fact its formula uses differs from this host's by a
quarter or more, either way, and its number is not what this host would get anyway. The quarter
is measured on the smaller reading (the larger is at least 1.25 times the smaller), so the rule
answers the same on both hosts of a move. A quarter
ignores a few GiB of firmware or a filesystem reserve and catches any real resize. The instance
warns at start, shows it on Settings → Instance and offers two answers: take the host-sized
values, or keep the numbers (which records them for this host).

**The move adapts by itself.** `yulius instance import` leaves a mark in the data directory
(`.yulius-limits-adapt`). The first start that finds it saves "host-sized" for every pin that no
longer fits, including recorded unit values that are not in the new unit yet, audits it and
removes the mark. The adaptation runs in the server and not in the import because only the server
is started with the unit's environment; the migration command of task #442 gets it through the
import it already calls.

**Applied live where the state allows it.** Sessions, CI lanes and the CD run bound are read from
live state; the CI job semaphore is resized in place (grown at once, shrunk as running jobs end,
so the waiters keep their order); the disk quota is one number in the sandbox manager. The sandbox
memory applies at the next restart: each sandbox's `memory.max` is written when it is prepared and
the CI budget pools are sized from it once, and re-sizing pools with reservations out is not worth
a setting that changes once per host.

## Alternatives

- **Keep constants and document a sizing table.** It is what failed: the table is read once, at
  install, and never after a move.
- **Refuse to start, or drop pins, when the host changed.** A pin can be deliberate (a shared
  host); the instance must come up with the configured numbers and say what it thinks of them.
- **Write the adapted numbers into the unit.** The server cannot edit its unit, drop-ins are the
  operator's, and it would pin the limit again.
- **One recorded host for the whole instance.** Then a number set yesterday on the new host would
  be flagged because another one was set on the old; per-pin records need no "acknowledge all".
- **`YULIUS_SANDBOX_PIDS` and `YULIUS_SANDBOX_CPUS` too.** The task limit is already sized from the
  host and checked against the service's `TasksMax` at every start (ADR-0198, ADR-0201), and the
  CPU cap has no default to derive.

## Consequences

- A new host needs no limit lines in its unit; existing unit values keep working and show as
  `unit`.
- An instance that grows (a bigger VM) takes more work after a restart without anyone noticing a
  setting; the start log names every limit, its value and its source.
- An instance moved from a version before this one has no records: its unit values are recorded
  for the first host that runs this version and are not flagged there.
- The memory setting is the one that says "applies at the next restart".
- The numbers are heuristics. A host whose work differs (few CPUs, huge builds) pins its own in
  Settings, and the formulas are one module to retune.
