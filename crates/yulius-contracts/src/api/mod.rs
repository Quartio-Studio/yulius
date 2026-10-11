//! HTTP API request and response DTOs (docs/api.md, SPEC-002 §7.9).
//!
//! One submodule per area. Timestamps are RFC 3339 UTC strings; identifiers are opaque strings.

pub mod admin;
pub mod analytics;
pub mod auth;
pub mod automations;
pub mod ci;
pub mod common;
pub mod files;
pub mod git;
pub mod intake;
pub mod mcp;
pub mod sandboxes;
pub mod secrets;
pub mod sessions;
pub mod shells;
pub mod skills;
pub mod status;
pub mod tasks;
pub mod updates;
pub mod workflows;
