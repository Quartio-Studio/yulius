# 0001. One Rust binary with an embedded static web UI

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-01); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Yulius is a self-hosted tool for 2-5 users on one VM; it must be easy to deploy and to wrap in a future desktop shell.

## Decision

Ship one Rust binary `yulius` (axum/tokio) with subcommands `serve`, `ptyhost`, `hook`, `git-credential`, `import`, `login`, `backup`, `doctor`. The web UI is vanilla ES modules with vendored xterm.js, embedded in the binary.

## Alternatives

Leptos/Yew WASM UI; a Node SPA build.

## Consequences

One artifact to deploy and no Node toolchain at runtime. The JS UI has no Rust type checking; `scripts/check_repository.py` checks design tokens and links instead.
