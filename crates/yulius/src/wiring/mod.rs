//! Adapter wiring: implementations of application ports over adapter crates.

pub mod browser;
pub mod claude_cli;
pub mod claude_models;
pub mod claude_service;
pub mod clock;
pub mod egress;
pub mod git;
pub mod github_client;
pub mod harness;
pub(crate) mod http;
pub mod jev;
pub mod migrate_files;
pub mod pinned_cli;
pub mod proxy;
pub mod release;
pub mod sandbox;
pub mod session;
pub mod split;
pub mod system;
pub(crate) mod tarball;
pub mod telegram;
#[cfg(target_os = "linux")]
pub(crate) mod tunnel_handoff;
pub mod update_files;
pub mod usage;
pub mod vidoc;
