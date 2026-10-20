//! Composition of the `yulius` binary: configuration, adapter wiring and server startup.

pub mod cli;
pub mod docker_smoke;
pub mod doctor;
pub mod host_setup;
pub mod leftovers_cli;
pub mod remote_cli;
pub mod remove_cli;
pub mod sandbox_cli;
pub mod security_scan;
pub mod serve;
pub mod update_helper;
pub mod wait_cli;
pub mod wiring;
