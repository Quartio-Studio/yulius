//! Deterministic Yulius domain: identifiers, permissions, task and attempt state machines,
//! subscription pool policy and capacity forecast, documentation-format validation, the default
//! workflow file format, schedules, delivery profiles, instance updates, the secrets vault's
//! rules, Yulius-run CI and secret redaction of transcripts.
//!
//! This crate performs no I/O and reads no clock or environment. Callers inject time as
//! [`time::Timestamp`] values and identifiers as already-generated strings.

pub mod auto_choice;
pub mod automation_format;
pub mod branch_policy;
pub mod briefing;
pub mod capacity;
pub mod ci;
pub mod context_full;
pub mod cron;
pub mod delivery;
pub mod disk;
pub mod harness;
pub mod ids;
pub mod intake;
pub mod permission;
pub mod redact;
pub mod request;
pub mod resources;
pub mod rotation;
pub mod rule;
pub mod scope;
pub mod spec_format;
pub mod steps;
pub mod task;
pub mod time;
pub mod update;
pub mod usage;
pub mod vault;
pub mod workflow_format;
