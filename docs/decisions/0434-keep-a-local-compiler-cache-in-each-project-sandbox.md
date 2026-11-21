# 0434. Keep a local compiler cache in each project sandbox

## Status

Proposed for Yulius's own Rust builds in [SPEC-595](../../.ai/specs/SPEC-595-share-compiled-rust-across-worktrees-and.md).

## Context

The operator measured 195 s compiling and 151 s testing in one CI shard. Each checkout retained 18–22 GB of Cargo output. A task, an independent CI run and the merge train can build the same revision at three different absolute paths. CI accounts CPU and cancellation to each job. Many tests open fixtures relative to the compile-time value of `CARGO_MANIFEST_DIR`.

## Decision

Bootstrap the pinned mbx 1.30.0 Linux executable and verify its release archive against the repository's SHA-256. The project Cargo launcher uses one content-addressed store at `<sandbox>/rust-compile-cache` on the existing sandbox mount. Agent and CI homes can differ. Each invocation starts its own local cache session and runs the compiler in the calling job. Cargo keeps its ordinary checkout-local targets. The xtask launcher embeds its checkout path and costs about 0.6 s to build from scratch, so scripts/dev starts it with pinned Cargo as before. Cached artifacts are verified and restored as private copies on the instance's ext4 filesystem.

Preserve the compiler's original workspace-root and `OUT_DIR` paths. mbx normalizes paths in lookup keys where possible and tracks literal inputs such as `env!("CARGO_MANIFEST_DIR")`. We disable its optional workspace source-path remapping because it also changes `file!()` and panic locations. For absolute Rust source arguments, and the compiler working directory when debug information is enabled, an identity prefix mapping adds the literal path identity to mbx's key. Rust maps that prefix to itself. Registry sources used from different Cargo homes and debug builds in different source directories therefore keep their original paths. Build scripts execute normally. Rust does not track arbitrary file or environment reads during procedural macro expansion. The compiler launcher bypasses the cache for a macro consumer and its dependents, including macros re-exported through ordinary libraries. For each Cargo invocation it records which Cargo-produced rlibs and metadata have a known macro-free dependency closure. Pre-existing externs without that proof compile directly. The cache session disables nonportable incremental output. An explicit `CARGO_INCREMENTAL=1` selects plain Cargo with its private incremental directory, which agents can use for an edit/build loop. Foreground terminal input and inherited Make jobservers also select plain Cargo to retain the controlling terminal and the caller’s parallelism limit. The logs distinguish these bypasses from the cache's hits, misses and initially unknown keys.

Give the store a 16 GiB collection budget. mbx's disk upkeep evicts old entries after builds, with at most one scheduled collection per minute. Concurrent writes can temporarily exceed the budget. The instance-upkeep slice of SPEC-595 extends Yulius's five-minute upkeep to clear a marked, idle store above 20 GiB of allocated space if a CI job ended before collection. The launcher publishes the stamped directory with one rename. The stamp is the ownership contract for both builds and reclamation. Under quota or critical host pressure, Yulius can reclaim the entire idle store after pruning other rebuildable output. Builds take a shared lock on the sandbox directory and pressure reclamation takes it exclusively without waiting. A new build during reclamation proceeds directly through Cargo. The Cargo dispatch closes the lease before running user code. A termination signal to the launcher stops the cache command and its process group and escalates after three seconds. The ordinary sandbox quota includes the bytes in the store.

## Alternatives

- **sccache 0.19.0.** In a two-checkout experiment with Rust 1.99.0, the same non-incremental library and fresh targets, the first build took 2.39 s and the second 1.60 s. Both roots were listed in `SCCACHE_BASEDIRS` and remapped in rustc's arguments. Both cacheable requests missed. The released Rust key hashes the compiler working directory and Cargo environment literally. Its single local storage server can serve concurrent clients, but its client-side mode still has those key semantics.
- **A shared dependency target.** Cargo has one mutable fingerprint and build-script tree. Using one target across worktrees takes its build lock across compilation. Copying or rotating it requires a publication and invalidation protocol and must preserve embedded paths.
- **Cargo's build cache on the pinned toolchain.** Stable Cargo 1.99 can move intermediate outputs with `build.build-dir`. Two concurrent builds with separate targets and one shared build directory confirmed the lock. The first waited for a signal in its build script, the second reported a build-directory lock wait, and both exited 0 after release. On the pin, `cargo -Z fine-grain-locking metadata` refused the nightly-only option. Sharing that directory would serialize independent CI jobs.

## Measurements

On the instance, a lane-layout replay at `52b59ecb3` used two detached checkouts, independent empty targets and an initially empty private disk store. Both ran `scripts/cargo check --locked --offline -p yulius-domain --lib` with two Cargo jobs. The first lane took 9.49 s and populated 12 compilations. The second took 4.58 s and served every lookup (12/12). Across all compiler invocations the warm rate was 12/22 (54.5%), including seven mbx bypasses and three additional direct invocations for unknown or macro inputs each time. The cache copied 13.5 MiB to the warm target. No other build wrote this store.

## Consequences

A cold build pays for recording outputs and a warm build pays to validate and copy them. Tests still execute. The optional tool's absence or a cache startup failure leaves the ordinary Cargo command available. Repository CI and agent commands use `scripts/dev` and `scripts/cargo`. The project configuration leaves global Cargo installations alone. A different Rust project can adopt the launcher after measuring its own builds and agreeing on a sandbox disk budget.

The implementation and the path behavior are described in the upstream [mbx cache limits](https://mr-boxington.jdx.dev/limits), [sccache 0.19 Rust key](https://github.com/mozilla/sccache/blob/v0.19.0/src/compiler/rust.rs) and [Cargo build-cache reference](https://doc.rust-lang.org/cargo/reference/build-cache.html).
