---
yulius-task: 09d68ab2-678f-4e8e-a4f5-0fff81215881
status: in-progress
---

# SPEC-539 Guard a new sign-in against an older run's refusal

## Goal

Keep a freshly signed-in Claude subscription available when a run using its previous credential is refused ([#1365](https://github.com/Quartio-Studio/yulius/issues/1365)).

## Context

The watcher handles a live or replayed access refusal by passing the account ID and message to `record_access_lost`. That write takes the usage lock but has no record of which sign-in launched the attempt. The existing pool supervision test exercises a refusal without a concurrent sign-in.

## Requirements

- **REF-001** Only a refusal of the sealed credential selected for the attempt can mark its account `relogin` and set its error. This applies to live and reattached runs.
- **REF-002** The supervisor handles the interrupted run through its existing move or capacity wait in either case.

## Plan

- [x] Reproduce a refusal after sign-in in a synthetic run
- [ ] Bind the refusal write to the launch credential
- [ ] Verify account state and run recovery
- [ ] Review and deliver the fix on green CI

## Acceptance criteria

- **AC-1** A synthetic Claude run launches with T1, the subscription receives T2, and the run reports a refusal. The subscription still holds T2, is `ok` and has no error. With another subscription available the task moves there.
- **AC-2** With T1 still current, the subscription becomes `relogin` with the refusal message and the task moves.
- **AC-3** An older attempt with no evidence of its launch credential still handles the interrupted turn and leaves the account alone.

## Test plan

Drive the refusal from a synthetic agent after its task has started. Use the SQLite account store and observe both the next attempt and the original account. Exercise same-credential, replaced-credential and missing launch evidence cases. Run the existing usage-write race test.

## Decisions

Use a digest of the sealed launch credential in the private run directory, following the existing per-attempt markers for server recovery. This avoids retaining another copy of a credential, and survives a watcher reattach. Compare under the usage lock shared with Claude sign-in. A pre-upgrade run cannot identify its sign-in, so its refusal only affects the interrupted task.
