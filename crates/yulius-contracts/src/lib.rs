//! Versioned external contracts of Yulius: HTTP API DTOs, UI events, terminal WebSocket frames,
//! the ptyhost wire protocol, MCP and hook payloads, import manifests, browser frames, the update
//! helper's and the move helper's file protocols, the sandbox namespace holder's records and the
//! credential wrapper every crate passes secrets in.

pub mod api;
pub mod browser;
pub mod events;
pub mod hooks;
pub mod import;
pub mod mcp;
pub mod migrate;
pub mod ptyhost;
pub mod redact;
pub mod sandbox;
pub mod scrub;
pub mod secret;
pub mod terminal;
pub mod update;
