//! Deterministic Yulius domain: identifiers, permissions, task and attempt state machines,
//! rotation policy, documentation-format validation, schedules and delivery profiles.
//!
//! This crate performs no I/O and reads no clock or environment. Callers inject time as
//! [`time::Timestamp`] values and identifiers as already-generated strings.

pub mod briefing;
pub mod cron;
pub mod delivery;
pub mod harness;
pub mod ids;
pub mod intake;
pub mod permission;
pub mod rotation;
pub mod scope;
pub mod spec_format;
pub mod steps;
pub mod task;
pub mod time;
pub mod usage;
