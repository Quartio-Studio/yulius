# 0201. All sandboxes share a task limit below the service's

## Status

Accepted. Specified in [SPEC-272](../../.ai/specs/SPEC-272-all-sandboxes-together-keep-the-server-a.md);
task #260. Amends [0198](0198-sandbox-task-limits-are-sized-from-the-host-and-leave-sessions-a-reserve.md)
(its open consequence: two saturated sandboxes could fill the service's limit).

## Context

ADR-0198 sizes each sandbox's `pids.max` from the host, at most three quarters of
`yulius.service`'s own `pids.max` (systemd's `DefaultTasksMax`: 57 763 on the VM, so 43 320 per
sandbox). Nothing bounded the sandboxes together, so two busy ones could fill the service's limit
and the server in `<unit>/server` could no longer fork. #254's first common limit was reverted
because kernels with `pids.events.local` (6.13+) count a fork refused there only in
`<unit>/sandboxes`, which the limit counters did not read, and because the value stayed when the
service's limit was removed: `<unit>/sandboxes` survives restarts (`KillMode=process`).

## Decision

- **A common limit.** At every start, with the `pids` controller delegated, the server writes
  `<unit>/sandboxes/pids.max`: three quarters of the service's limit (`tasks_max_above`), or `max`
  when the service has none. The server keeps a quarter (14 443 on the VM) however many sandboxes
  are busy; a lone sandbox at its default loses nothing.
- **Its hits are counted.** Where the kernel has `pids.events.local`, a sandbox's task-limit counter
  adds `pids.events.local` `max` of its groups but CI's and the engine's subtrees, and of
  `<unit>/sandboxes`. A hit at the shared limit is attributed to every sandbox whose run ends
  crashed or killed meanwhile. On 6.8 the forking group counts the hit, as before.
- **An explicit `YULIUS_SANDBOX_PIDS` stays uncapped**, but when it is above the common limit the
  server warns: the common limit binds each sandbox too.
- **Counters never fall.** `freeze::thaw_all` keeps an empty run group whose counters show a hit,
  so a sandbox's total cannot drop during a run; systemd removes such groups when the service
  stops.

## Alternatives

- **No common limit, a bigger `TasksMax=` instead.** Leaves the server exposed on every default
  install; operators would have to know.
- **Partitioning the service's limit by sandbox count.** Wastes it when one sandbox works, the
  common case, and changes as sandboxes come and go.
- **A group per sandbox above the common one for attribution.** That group is the sandbox's own;
  the common limit is what one cannot attribute, and naming the limit already points at the knob.
- **Comparing counters per group across a run** instead of keeping groups with hits. Changes the
  port's two totals into maps for a rare case (a paused run's group with hits removed meanwhile).

## Consequences

- On the VM the server keeps 14 443 tasks whatever sandboxes do; a sandbox can still use the whole
  common share alone.
- On 6.13+ kernels a hit at the common limit by one sandbox can make another sandbox's run that
  crashed meanwhile say "the sandbox hit its process limit".
- An older binary (a rollback) leaves `sandboxes/pids.max` as it was; that only protects the
  server and is harmless. `echo max > .../sandboxes/pids.max` clears it by hand.
