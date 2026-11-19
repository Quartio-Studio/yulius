---
yulius-task: 09d68ab2-678f-4e8e-a4f5-0fff81215881
status: done
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
- [x] Bind the refusal write to the launch credential
- [x] Verify account state and run recovery
- [x] Review and deliver the fix on green CI

## Acceptance criteria

- **AC-1** A synthetic Claude run launches with T1, the subscription receives T2, and the run reports a refusal. The subscription still holds T2, is `ok` and has no error. With another subscription available the task moves there.
- **AC-2** With T1 still current, the subscription becomes `relogin` with the refusal message and the task moves.
- **AC-3** An older attempt with no evidence of its launch credential still handles the interrupted turn and leaves the account alone.

## Test plan

Drive the refusal from a synthetic agent after its task has started. Use the SQLite account store and observe both the next attempt and the original account. Exercise same-credential, replaced-credential and missing launch evidence cases. Run the existing usage-write race test.

## Decisions

Use a digest of the sealed launch credential in the private run directory, following the existing per-attempt markers for server recovery. This avoids retaining another copy of a credential, and survives a watcher reattach. Compare under the usage lock shared with Claude sign-in. A pre-upgrade run cannot identify its sign-in, so its refusal only affects the interrupted task.

## Evidence

- Before the fix, `cargo test --locked -p yulius --test main subscription_moves::a_refused_run_` failed both regressions. Each task moved to the second subscription and the original account read `Relogin` where `Ok` was expected.
- After the fix, six synthetic integration tests passed with `cargo nextest run --locked -p yulius --test main -E 'test(/subscription_moves::a_refused_/) | test(/pool_supervision::a_refused_subscription/) | test(/usage_credits::a_subscription_refused_while/)'`. They cover the pasted replacement token, a missing launch digest, a current refusal and its error, the usage-write race, the move notice and the last subscription's capacity wait.
- `scripts/dev quick` passed. `scripts/dev security` reported Vidoc disabled for this project. The independent reviewer approved `42411b5d` after tracing launch, refusal, reattachment and sign-in locking.
- `scripts/dev mutants` reported 12 listed, 4 caught, 0 missed, 0 timed out, 6 unviable and 2 not reached within 30 minutes. The four caught results failed in the unrelated xtask Vidoc-key test in the Git-less scratch checkout, so they provide no evidence about this fix. This limitation is recorded on #1365.
- A separate `cargo mutants` run inverted only the credential comparison and selected the two supervision tests for #1365. Both failed at their account-state assertions: current read `Ok` instead of `Relogin`, and stale read `Relogin` instead of `Ok`. The tool reported one mutant tested and caught. The source matched the reviewed commit again when the run ended.
