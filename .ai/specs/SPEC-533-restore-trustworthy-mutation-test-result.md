---
yulius-task: 73c95420-044f-49f0-8574-c9b5476f7ebf
status: in-progress
---

# SPEC-533. Restore trustworthy mutation-test results

## Goal

A reviewer can tell whether the selected tests detected a mutation in the branch.

## Context

On 8d05c25bf the xtask missing-key test fails at cli.rs line 49 in a Git-free copy.
The scanner exits 2 while checking for a repository before it checks for a key.
cargo-mutants 27.1.0 selects mutated packages for the baseline, even when
`--test-workspace true` selects every member for mutant scenarios. The existing fixture
has one package, so it cannot expose the difference. [Issue #1364](https://github.com/Quartio-Studio/yulius/issues/1364)

## Requirements

- MUT-001. The security CLI test must reach its missing-key assertions in a Git-free build.
- MUT-002. The baseline and mutants run the same workspace packages, features and nextest filter. A failing baseline stops classification and reports failure.
- MUT-003. A surviving mutation in a fixture appears as missed. Older counts carry a warning about false catches.

## Plan

- [x] Reproduce the Git-free failure and specify the baseline contract
- [x] Prove baseline failure and missed mutants across workspace members
- [ ] Run the same workspace tests on clean and mutated code
- [ ] Document historical results and verify the reviewed change

## Acceptance criteria

- AC-1. Run the missing-key test from a copy with no checkout metadata. Both normal and broad scan assertions pass.
- AC-2. A failing test in an unmutated workspace member fails the clean baseline before any mutant is classified.
- AC-3. With that test passing, the deliberately untested changed function yields a missed mutant.
- AC-4. The engineering guide and ADR-0347 explain how to interpret catches recorded before this fix.

## Test plan

Exercise the real pinned cargo-mutants and nextest in a two-member Git fixture. Verify an
unmutated member participates in both scenarios and inspect baseline failure output.
Run the original security test in a Git-free copy, the xtask suite and the repository quick checks.

## Decisions

Give the CLI test its own Git fixture so it exercises the real scanner's missing-key contract.
Pass workspace selection to Cargo for both scenarios. Keep the branch diff, product integration
filter and campaign bounds defined in [ADR-0347](../../docs/decisions/0347-mutation-testing-runs-once-per-pull-request.md).
