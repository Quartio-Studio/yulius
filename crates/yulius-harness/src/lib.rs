//! Claude Code and Codex adapters: launch specifications, hooks and MCP configuration, transcript
//! tailing, usage extraction, native transcript writers and login flows (SPEC-002 §7.2–§7.3).
//!
//! Provider wire formats stay inside this crate. Parsers are written against the structure of
//! Claude Code 2.1.292 and Codex CLI 0.160.1 transcripts (synthetic fixtures under
//! `tests/fixtures`); unknown record types are ignored and malformed lines are reported, never
//! fatal (SPEC-002 R-01, EVT-002).

pub mod claude;
pub mod claude_config;
pub mod codex;
pub mod hook_client;
pub mod launch;
pub mod login;
pub mod observe;
pub mod paths;
pub mod probe;
pub mod secret;
pub mod skills;
pub mod tail;
pub mod time;

mod ir;
mod json;
