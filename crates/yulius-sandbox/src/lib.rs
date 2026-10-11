//! Sandbox isolation (SPEC-002 ADR-19): one namespace holder per sandbox, the writable system
//! layer over a shared base copy of the host system, the relays across the sandbox's network
//! boundary, per-sandbox cgroup limits and disk usage, and the seccomp filter of runs.
//!
//! | Module | Runs in |
//! | --- | --- |
//! | [`manager`] | the server: holders on demand, relays on the host, upkeep, base builds |
//! | [`holder`] | `yulius sandbox-host start` (helper) and `inner` (inside the namespaces) |
//! | [`base`] | `yulius sandbox-base build`/`chown` |
//! | [`enter`] | the ptyhost of a contained run, and `yulius sandbox-exec` |
//! | [`relay`], [`procnet`] | both sides of the network boundary |
//! | [`cgroup`], [`usage`], [`subid`], [`userns`], [`seccomp`] | building blocks |
//!
//! Namespaces, mounts and `setns` are Linux-only; elsewhere the functions that need them
//! return errors and the server keeps the previous containment.

pub mod base;
pub mod cgroup;
pub mod enter;
pub mod holder;
pub mod manager;
pub mod procnet;
pub mod relay;
pub mod seccomp;
pub mod subid;
pub mod usage;
pub mod userns;
