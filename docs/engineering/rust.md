# Rust engineering rules

These are Yulius engineering conventions.
MUST means a repository requirement; SHOULD permits a recorded reason to differ.
Do not import every upstream guideline as a new mandatory rule.

| ID | Rule | Enforcement |
| --- | --- | --- |
| RUST-001 | Every crate MUST inherit edition, rust-version, license and workspace lints; the lockfile MUST be committed | Repository check, locked Cargo commands |
| RUST-002 | First-party unsafe code is forbidden | Workspace `unsafe_code = forbid`; local lint suppression is a review violation |
| RUST-003 | Expected failures MUST use Result/Option, not panic/unwrap/expect; never use todo/unimplemented as shipped behavior | Clippy plus review; tests return Result or use assertions |
| RUST-004 | Validate at external boundaries; encode IDs and constrained values as types instead of interchangeable strings | API/constructor tests and review |
| RUST-005 | Domain code MUST remain deterministic and free of I/O, clocks, environment and provider types | Dependency-layer gate plus review; inject clocks/IDs at use-case boundary |
| RUST-006 | Borrow inputs by default; clone only for an ownership need. Use Arc for actual shared ownership, not to silence the compiler | Review and measured profiling when performance matters |
| RUST-007 | Typed errors MUST retain cause/context internally; public errors MUST redact paths/credentials where sensitive | Error conversion and redaction tests |
| RUST-008 | Avoid holding synchronous locks across await; bounded channels, cancellation and owned task handles are required for long-running async work | Runtime contract tests and code review |
| RUST-009 | Blocking Git/disk/CPU work MUST not block an async executor thread indefinitely | Adapter design, bounded blocking tasks, shutdown tests |
| RUST-010 | Public contracts MUST have docs; examples should compile. Persisted formats MUST carry a version/migration policy | rustdoc warnings, doctests, compatibility fixtures |
| RUST-011 | Dependencies MUST be scoped to importing crates, have compatible licenses/MSRV, and use a committed lockfile | cargo-deny, cargo-shear and review |
| RUST-012 | Tests MUST assert externally meaningful behavior and failure paths, with reproducible fixtures | Review; nextest with no automatic retries |

Prefer enums for closed state sets and newtypes for domain IDs. Do not expose
persistence structs as API contracts or duplicate provider wire types in the UI.
Keep public interfaces small. Traits are justified at real substitution/testing
boundaries, not mechanically for every struct. Use constructor injection; no
global mutable service locator.

Avoid blanket Clippy allows, broad lint removals and ignored tests. A narrow
exception must state the concrete reason, applicable scope, owner and removal
condition; review it with the affected spec. `unsafe_code` remains forbidden until
a dedicated architecture change changes that policy and its proof obligations.

Use unit tests for pure transitions, adapter contract fixtures for providers,
temporary repositories for Git, and integration tests for externally visible
flows. Inject time, identifiers and transport fakes. Avoid wall-clock sleeps,
shared ports, home-directory credentials and live paid models in the ordinary gate.
Prove a regression test detects the old behavior using an isolated fixture or
worktree; do not stash another contributor's work to manufacture a red run.

No performance claim without measurement. No blanket line-coverage percentage
before product risks and meaningful assertions are identified.
