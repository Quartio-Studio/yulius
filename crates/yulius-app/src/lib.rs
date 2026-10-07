//! Yulius application layer: use cases, authorization decisions and ports implemented by
//! adapters (SPEC-002 §3). Use cases are `impl App` blocks grouped by area.

pub mod access;
pub mod admin;
pub mod auth;
pub mod context;
pub mod error;
pub mod events;
pub mod model;
pub mod ports;
pub mod sandboxes;
