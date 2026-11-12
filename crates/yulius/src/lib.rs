//! Composition of the `yulius` binary: configuration, adapter wiring and server startup.

pub mod cli;
pub mod docker_smoke;
pub mod doctor;
pub mod host_setup;
pub mod instance;
pub mod leftovers_cli;
pub mod mcp_bridge;
pub mod migrate_helper;
pub mod remote_cli;
pub mod remove_cli;
pub mod restart_safety;
pub mod sandbox_cli;
pub mod security_scan;
pub mod serve;
pub mod tool_cli;
pub mod update_helper;
pub mod wait_cli;
pub mod wiring;
