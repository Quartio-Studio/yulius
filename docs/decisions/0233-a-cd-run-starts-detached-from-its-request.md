# 0233. A CD run starts detached from its request from its record on

## Status

Accepted. Specified in [SPEC-347](../../.ai/specs/SPEC-347-a-cd-run-whose-request-is-dropped-stays.md);
task #331, issue #948. Applies [0053](0053-shell-starts-run-detached-from-the-request.md) to CD
runs ([0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md)) and refines the deploy
permit of [0232](0232-an-update-waits-for-running-deploys-and-holds-new-ones.md).

## Context

`App::cd_start` recorded the run as `running`, then wrote its spans and its audit entry, and only
then registered its cancel channel and spawned its execution, all inside the request's future.
Axum drops that future when the client disconnects (a person's `POST .../cd/{env}/deploy`, an
agent's remote MCP `cd_deploy`). Dropped in between, the run stayed `running` with nothing
executing it until a restart's recovery or a person's Cancel: the environment was refused as
"already running" and an update waited its whole bound for it, then cancelled. Every store call is
a job on the database thread, so a drop while awaiting the insert's answer already leaves the row.

## Decision

- The section of a CD start from `insert_cd_run` to the spawn of `cd_execute` (the spans, the
  audit entry, ending the run as `error` when that entry cannot be written, the cancel channel)
  runs in `App::run_detached`, spawned by the request and awaited by it. A dropped request drops
  only the `JoinHandle`: the run is executing, or ended as `error`, never left `running`.
- The checks, the promotion and every refusal stay in the request, as before.
- `DeployPermit` owns its share of the gate (an `Arc`), so the permit moves into the detached
  section and is still held until the run is recorded and started (SPEC-342 DPU-002).
- Delivery environment runs (`dlv_start_environment_run`, #952) apply the same two pieces.

## Alternatives

- **Spawn right after the insert**: keeps the gap of a drop while the insert's answer is awaited.
- **A drop guard that ends the run as `error`**: needs a synchronous store write from `Drop` and
  throws away a deploy the person asked for.
- **Release the permit in the request**: an update could then close the gate, count nothing
  starting, read no run and restart over a deploy being recorded.
- **Rely on start-up recovery** (`reconcile_cd_runs`): only runs at a restart.

## Consequences

- A deploy reaches its executor whatever its client does; a server shutdown is different (the
  next server's recovery ends a run it recorded as `error`, as before).
- One more spawned task per deploy; a panic in it answers 500 (it already ended the request).
- Tests drop a deploy request by polling it by hand until its run is in the store
  (`tests/cd.rs`).
