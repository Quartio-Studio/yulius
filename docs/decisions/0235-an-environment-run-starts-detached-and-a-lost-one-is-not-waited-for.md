# 0235. An environment run starts detached, and a lost one is not waited for

## Status

Accepted. Specified in [SPEC-348](../../.ai/specs/SPEC-348-an-environment-run-whose-request-is-drop.md);
task #332, issue #952. Applies [0233](0233-a-cd-run-starts-detached-from-its-request.md) to
delivery environment runs and refines the wait of
[0232](0232-an-update-waits-for-running-deploys-and-holds-new-ones.md).

## Context

`App::dlv_start_environment_run` recorded the run as `running`, wrote its audit entry and only
then spawned its execution, all inside the request's future (a person's
`POST .../delivery/environments/{env}/runs`, an agent's remote MCP `delivery_deploy`). Dropped in
between, the run stayed `running` with nothing executing it. An environment run has no Cancel:
only its lazy lost-run expiry (job timeout + 35 minutes + 5 minutes, applied when the run is read)
or a restart's recovery ended it. The update's wait read running environment runs straight from
the store, so such an orphan kept an update `building` until its bound, then cancelled it.

## Decision

- The section of an environment-run start from `insert_environment_run` to the spawn of
  `dlv_execute_environment_run` (the audit entry, ending the run as `error` when that entry cannot
  be written) runs in `App::run_detached`, the deploy permit moved in, as in ADR-0233. The checks,
  the confirmation and every refusal stay in the request; the answer's view is built there too.
- The update's wait (`listed_deploys`) passes each running environment run through the existing
  lost-run expiry (`dlv_expire_lost_run`) before counting it: a run past its limit is ended as
  `error`, as a read would end it, and is not waited for.

## Alternatives

- **A Cancel for environment runs** (the issue's fallback): with the start detached no request
  leaves an orphan, and stopping a command run on a host is a feature of its own.
- **Skip runs past the limit in the wait without ending them**: the row would stay `running` for
  the "already in progress" refusal, and the limit would be applied in two places.
- **A drop guard, or spawning right after the insert**: rejected in ADR-0233 for CD runs, for the
  same reasons.

## Consequences

- An environment run reaches its executor whatever its client does; a server shutdown is still
  ended by the next server's recovery.
- The update never waits for an environment run past its lost-run limit, which is at most the
  wait's bound for a run that started before the wait. A store failure while expiring a run is a
  failed read, which the wait already counts as one deploy still running.
- The update's wait now writes: it may end a lost run as `error` (audited only in the run's
  output, as a read's expiry is).
