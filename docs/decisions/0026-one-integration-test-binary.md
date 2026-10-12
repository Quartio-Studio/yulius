# 0026. The product's integration tests are one test binary

## Status

Accepted. Specified in [SPEC-033](../../.ai/specs/SPEC-033-recently-i-got-dozens-of-ci-errors-comin.md).

## Context

Cargo builds every file at the root of a crate's `tests/` as its own test executable. The `yulius`
crate (the composition layer) had 50 of them, and each links the whole product: about 190 MB
each, 9.5 GB per build directory. A worktree keeps several copies (the gate's `--all-features`
build, ad-hoc `cargo test -p yulius` builds, and new hashes after dependency or toolchain
changes), so task `target/` directories reached 21 to 49 GB. Working tasks' caches are never
pruned (ADR-0017), so a few working tasks passed the sandbox's 100 GB quota and Yulius refused
every CI run with "this sandbox uses 114.7 GB of its 100.0 GB disk quota" (task #31).

## Decision

`crates/yulius` sets `autotests = false` and declares one test target, `main`
(`tests/main.rs`), whose modules are the test files, which stay where they were. The repository
check (`integration_test_errors` in `scripts/check_repository.py`) fails when a composition-layer
crate has more than one integration test target, or when a `.rs` file next to its `main.rs` is
not declared as a module (it would silently never run). One file's tests run with
`cargo test -p yulius --test main <file>::`.

## Alternatives

- Strip or split debug info: saves about a third of each executable, but keeps fifty copies of
  the code and symbols, and backtraces lose line numbers.
- Prune working tasks' caches too: breaks a running agent's next build; ADR-0017 keeps working
  tasks untouched on purpose.
- Raise the quota: hides the growth; the disk behind the sandboxes is shared.
- One binary for every crate: the other crates' test executables are 10 to 45 MB, so it would add
  churn for little disk.

## Consequences

A gate build's test executables drop from 9.2 GB to 1.3 GB (`target/debug/deps` from 11 GB to
2.6 GB), and linking one executable instead of fifty is faster. Tests of different files now
share a process under `cargo test` (nextest still runs each test in its own process), so a test
must not rely on process-global state another file changes. New test files must be declared in
`tests/main.rs`; the repository check says so.
