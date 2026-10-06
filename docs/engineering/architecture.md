# Rust dependency boundaries

Use explicit domain, application and I/O boundaries. Add crates only when a real
feature needs a separate boundary; these classifications are not a product roadmap.
The current workspace has only the engineering task runner.

| Layer | Responsibility | Allowed internal dependencies |
| --- | --- | --- |
| domain | Deterministic types, invariants and transitions | domain |
| contracts | Versioned external DTOs/events | contracts |
| application | Use cases, authorization decisions and ports | domain, contracts, application |
| adapter | OS, network, storage, Git and provider I/O | domain, contracts, application, adapter |
| interface | CLI/HTTP parsing and presentation conversion | contracts, application |
| composition | Construction and configuration | all product layers |
| tooling | Engineering commands | any layer |

Every crate declares `[package.metadata.yulius] layer`. The repository checker
validates Cargo dependency directions, including renamed, build, dev and
platform-specific dependencies. Product crates cannot depend on tooling.
Domain crates permit only `thiserror` as an external dependency by default; update
the documented convention and checker together if a concrete need justifies more.
Intra-crate I/O and semantic purity remain review responsibilities.

Inject clocks, identifiers and side effects through application-owned boundaries.
Provider wire types remain in adapters. External contracts and persistence records
are separately versioned; a storage struct is not automatically a public API.
Use small typed constructors and traits at actual substitution points rather than
a global service locator or a trait for every struct.

Choose runtime, database, frontend, providers and deployment through future feature
specs. This file does not select those technologies or authorize product scope.
