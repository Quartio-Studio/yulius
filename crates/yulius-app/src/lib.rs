//! Yulius application layer: use cases, authorization decisions and ports implemented by
//! adapters (SPEC-002 §3). Use cases are `impl App` blocks grouped by area.

pub mod access;
pub mod admin;
pub mod auth;
pub mod automations;
pub mod background;
pub mod context;
pub mod coordination;
pub mod error;
pub mod events;
pub mod hooks;
pub mod hub;
pub mod lifecycle;
pub mod mcp;
pub mod mcp_config;
pub mod model;
pub mod ports;
pub mod sandboxes;
pub mod skills;
pub mod supervisor;
pub mod tasks;
pub mod terminal;
pub mod usage_json;
pub mod workflows;
