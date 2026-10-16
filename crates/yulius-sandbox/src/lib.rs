//! Sandbox isolation (SPEC-002 ADR-22): one namespace holder per sandbox, the writable system
//! layer over a shared base copy of the host system, the relays across the sandbox's network
//! boundary, per-sandbox cgroup limits and disk usage, and the seccomp filter of runs.
//!
//! | Module | Runs in |
//! | --- | --- |
//! | [`manager`] | the server: holders on demand, relays on the host, upkeep, base builds |
//! | [`holder`] | `yulius sandbox-host start` (helper) and `inner` (inside the namespaces) |
//! | [`base`] | `yulius sandbox-base build`/`chown` |
//! | [`enter`] | the ptyhost of a contained run, and `yulius sandbox-exec` |
//! | [`engine`] | the server (the engine's file view) and the engine (`sandbox-host engine`) |
//! | [`docker_front`] | `yulius sandbox-host docker`: the sandbox's Docker socket beside its holder (ADR-0158) |
//! | [`docker_bind`] | `yulius sandbox-host bind-docker`: the Docker directory into live runs (ADR-0158) |
//! | [`engine_supervisor`] | the Docker front: the engine on demand (ADR-0151) |
//! | [`relay`], [`procnet`] | both sides of the network boundary |
//! | [`cgroup`], [`freeze`], [`usage`], [`subid`], [`userns`], [`seccomp`] | building blocks |
//!
//! Namespaces, mounts and `setns` are Linux-only; elsewhere the functions that need them
//! return errors and the server keeps the previous containment.

pub mod base;
pub mod cgroup;
pub mod docker_bind;
pub mod docker_front;
pub mod engine;
pub mod engine_supervisor;
pub mod enter;
pub mod freeze;
pub mod holder;
pub mod manager;
pub mod procnet;
pub mod relay;
pub mod seccomp;
pub mod subid;
pub mod usage;
pub mod userns;
