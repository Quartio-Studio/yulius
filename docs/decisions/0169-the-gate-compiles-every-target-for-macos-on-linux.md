# 0169. The gate compiles every target for macOS on Linux

## Status

Accepted. Specified in [SPEC-214](../../.ai/specs/SPEC-214-i-got-this-from-my-agent.md).
Amends SPEC-002 ADR-21, where macOS became a manual check.

## Context

macOS is a supported development host, but Yulius CI runs only on Linux machines and the macOS
check was a manual `scripts/dev ci` on a Mac before promotion. Nobody ran it: #620 added Linux-only
calls to `yulius-sandbox` and `yulius-app`, and a person's agent could not install the `yulius`
CLI on a Mac at all (#636). There is no Mac runner to add.

Cross-compiling Rust for `aarch64-apple-darwin` needs only the target's standard library, but
`cargo check` still runs dependencies' build scripts, and `ring` and `libsqlite3-sys` compile
vendored C for the target. A Linux C compiler has no macOS headers.

## Decision

`scripts/dev macos`, run by `scripts/dev ci` after `lint`, is `cargo clippy --locked --workspace
--all-targets --all-features --target aarch64-apple-darwin -- -D warnings`. On Linux the vendored C
is compiled and archived by zig, whose release bundles macOS libc headers: bootstrap downloads the
official tarball pinned by version and SHA-256 in `.config/tools.toml` once per home cache and links
`.tools/bin/zig`, plus `macos-cc` and `macos-ar`, two names of `scripts/zig_macos.py`, which drops
the Apple target flags cc-rs passes (zig refuses `arm64`) and runs `zig cc -target aarch64-macos`
or `zig ar`. The xtask sets `CC_aarch64_apple_darwin` and `AR_aarch64_apple_darwin` to them. On a
Mac the stage uses the Mac's own compiler. The Rust target is listed in `rust-toolchain.toml`.

## Alternatives

- Keep the manual Mac check: it is how the regression shipped.
- A Mac runner for Yulius CI: no Mac machine exists, and the CI design runs jobs on the
  instance's own sandboxes.
- `cargo-zigbuild`: one more tool to compile per home, and it targets linking, which a check does
  not need.
- A stub compiler that writes empty objects: no real C is compiled, and cc-rs's compiler probes
  would see a compiler that accepts everything.
- `x86_64-apple-darwin` as well: the target-specific code differs by OS, not by architecture, and
  each target adds a full Clippy pass to every CI run.

## Consequences

- A Linux-only call outside a `cfg(target_os = "linux")` gate fails CI in Clippy with its site.
- Every CI run spends one more Clippy pass (about 3 minutes cold, less when warm) and
  `target/aarch64-apple-darwin` on disk; bootstrap downloads 57 MB once per home.
- Linking and running on macOS stay unchecked (no Apple SDK on Linux); a release still deserves a
  real Mac build.
- A Linux host architecture without a pinned tarball skips zig in bootstrap (it says so) and its
  `scripts/dev macos` fails; pinning one is adding its SHA-256 to `.config/tools.toml`.
- Listing the target in `rust-toolchain.toml` makes every rustup-managed checkout (the update
  helper's included) fetch the `aarch64-apple-darwin` standard library once.
- zig comes from ziglang.org itself, which prefers that automated users use mirrors; it is fetched
  once per home cache, so a ziglang.org outage fails only a fresh home's first bootstrap.
- `scripts/zig_install.py` holds the download (verified before it is unpacked, `tar` where Python
  has no `data` filter or lzma) and is unit-tested; `scripts/dev doctor` checks zig, the wrappers
  and the installed target.
