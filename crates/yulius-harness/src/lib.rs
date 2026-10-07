//! The Claude Code adapter: launch specifications, hooks and MCP configuration, transcript
//! tailing, usage extraction and login flows (SPEC-002 §7.2). Yulius runs Claude Code only
//! (F 2026-10-08); another harness would get its own module here.
//!
//! Provider wire formats stay inside this crate. Parsers are written against the structure of
//! Claude Code 2.1.292 transcripts (synthetic fixtures under `tests/fixtures`); unknown record
//! types are ignored and malformed lines are reported, never fatal (SPEC-002 R-01, EVT-002).

pub mod claude;
pub mod claude_config;
pub mod hook_client;
pub mod launch;
pub mod login;
pub mod observe;
pub mod paths;
pub mod secret;
pub mod skills;
pub mod tail;
pub mod time;

mod json;
