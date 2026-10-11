//! Versioned external contracts of Yulius: HTTP API DTOs, UI events, terminal WebSocket frames,
//! the ptyhost wire protocol, MCP and hook payloads, import manifests, browser frames, the update
//! helper's file protocol and the sandbox namespace holder's records.

pub mod api;
pub mod browser;
pub mod events;
pub mod hooks;
pub mod import;
pub mod mcp;
pub mod ptyhost;
pub mod sandbox;
pub mod scrub;
pub mod terminal;
pub mod update;
