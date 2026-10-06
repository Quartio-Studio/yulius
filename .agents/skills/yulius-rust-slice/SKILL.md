---
name: yulius-rust-slice
description: Implement a specified Yulius Rust feature or bug fix across its domain, adapter and interface boundaries, with behavioral verification. Use for Rust code changes, not product discovery alone.
---

# Implement a Rust slice

Read `AGENTS.md`, the applicable spec and `docs/engineering/rust.md`. Read
`docs/engineering/architecture.md` when adding/changing boundaries.

Orient from the current worktree, not a prior handoff alone. Map the authorized
outcome to requirement IDs; if a material behavior lacks a contract, use the
repository's spec workflow first. Preserve other contributors' changes.

Implement one complete observable flow at a time. Put I/O behind an actual
application-owned port; keep domain decisions deterministic. Add crates only for
real boundaries, inherit workspace metadata/lints, and classify their layer.
Prefer a typed failure over a placeholder, panicking path or invented success.
Use `yulius-runner` for subprocess/provider changes and load runtime contracts then.

Add tests that distinguish correct behavior from the reported failure. Prove the
regression when practical in an isolated fixture/worktree. Do not mirror private
implementation details or use live provider credentials in normal tests.

Run targeted tests during iteration, then `scripts/dev ci`, inspect the final diff
and update the spec's evidence. If a gate fails, identify cause and repair it;
do not weaken policy to pass. Report what changed, actual test evidence and gaps.
No skill step authorizes remote publication or adds a product requirement.
