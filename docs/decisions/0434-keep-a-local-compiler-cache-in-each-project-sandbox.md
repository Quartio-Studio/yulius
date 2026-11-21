# 0434. Keep a local compiler cache in each project sandbox

## Status

Proposed for Yulius's own Rust builds in [SPEC-595](../../.ai/specs/SPEC-595-share-compiled-rust-across-worktrees-and.md).

## Context

The operator measured 195 s compiling and 151 s testing in one CI shard. Each checkout retained 18–22 GB of Cargo output. A task, an independent CI run and the merge train can build the same revision at three different absolute paths. CI accounts CPU and cancellation to each job. Many tests open fixtures relative to the compile-time value of `CARGO_MANIFEST_DIR`.

## Decision

Bootstrap the pinned mbx 1.30.0 Linux executable and verify its release archive against the repository's SHA-256. The project Cargo launcher uses one content-addressed store at `<sandbox>/rust-compile-cache` on the existing sandbox mount. Agent and CI homes can differ. Each invocation starts its own local cache session and runs the compiler in the calling job. Cargo keeps its ordinary checkout-local targets. The xtask launcher embeds its checkout path and costs about 0.6 s to build from scratch, so scripts/dev starts it with pinned Cargo as before. Cached artifacts are verified and restored as private copies on the instance's ext4 filesystem.

Enable workspace-root remapping for debug locations. mbx tracks values such as `env!("CARGO_MANIFEST_DIR")` and preserves the literal path a program can observe. Build scripts run normally and keep their original `OUT_DIR`. Cargo's existing job budget applies. The tool reports hits, misses, requests for which a key could not yet be predicted, and bypasses separately.

Give the store a 16 GiB collection budget. mbx's disk upkeep evicts old entries after builds, with at most one scheduled collection per minute. Concurrent writes can temporarily exceed the budget. The five-minute Yulius upkeep also clears a marked, idle store above 20 GiB of allocated space if a CI job ended before collection. An unmarked directory is left to its owner. Under quota or critical host pressure, Yulius can reclaim the entire idle store after pruning other rebuildable output. Builds take a shared lock on the sandbox directory and pressure reclamation takes it exclusively without waiting. A new build during reclamation proceeds directly through Cargo. The ordinary sandbox quota includes the bytes in the store.

## Alternatives

- **sccache 0.19.0.** In a two-checkout experiment with Rust 1.99.0, the same non-incremental library and fresh targets, the first build took 2.39 s and the second 1.60 s. Both roots were listed in `SCCACHE_BASEDIRS` and remapped in rustc's arguments. Both cacheable requests missed. The released Rust key hashes the compiler working directory and Cargo environment literally. Its single local storage server can serve concurrent clients, but its client-side mode still has those key semantics.
- **A shared dependency target.** Cargo has one mutable fingerprint and build-script tree. Using one target across worktrees takes its build lock across compilation. Copying or rotating it requires a publication and invalidation protocol and must preserve embedded paths.
- **Cargo's build cache on the pinned toolchain.** Stable Cargo 1.99 can move intermediate outputs with `build.build-dir`. Two concurrent builds with separate targets and one shared build directory confirmed the lock. The first waited for a signal in its build script, the second reported a build-directory lock wait, and both exited 0 after release. On the pin, `cargo -Z fine-grain-locking metadata` refused the nightly-only option. Sharing that directory would serialize independent CI jobs.

## Measurements

On the instance, a lane-layout replay at `7b8db254e` used two detached checkouts, independent empty targets and an initially empty private disk store. Both ran `scripts/cargo check --locked --offline -p yulius-domain --lib` with two Cargo jobs. The first lane took 8.76 s and populated 15 compilations whose keys were initially unknown. The second took 1.74 s and served 14 of 15 lookups (93.3%). Each also reported seven bypasses, including compiler queries. No other build wrote this store, so the first lane supplied every hit.

## Consequences

A cold build pays for recording outputs and a warm build pays to validate and copy them. Tests still execute. The optional tool's absence or a cache startup failure leaves the ordinary Cargo command available. Repository CI and agent commands use `scripts/dev` and `scripts/cargo`. The project configuration leaves global Cargo installations alone. A different Rust project can adopt the launcher after measuring its own builds and agreeing on a sandbox disk budget.

The implementation and the path behavior are described in the upstream [mbx cache limits](https://mr-boxington.jdx.dev/limits), [sccache 0.19 Rust key](https://github.com/mozilla/sccache/blob/v0.19.0/src/compiler/rust.rs) and [Cargo build-cache reference](https://doc.rust-lang.org/cargo/reference/build-cache.html).
