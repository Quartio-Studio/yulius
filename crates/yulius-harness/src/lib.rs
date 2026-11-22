//! The Claude Code, Codex and Antigravity adapters: launch specifications, hooks and MCP
//! configuration, transcript tailing, usage extraction and login flows (SPEC-002 §7.2). Codex is
//! the second harness again (SPEC-503, ADR-0355; accepted 2026-11-16, reversing the Codex part of
//! F 2026-10-08); Google's Antigravity CLI is the third (SPEC-505, ADR-0361).
//!
//! Provider wire formats stay inside this crate. Parsers are written against the structure of
//! Claude Code 2.1.292 transcripts, Codex CLI 0.167.0 and 0.169.0 rollouts and Antigravity CLI
//! 1.5.10 transcripts (fixtures under `tests/fixtures`); unknown record types are ignored and
//! malformed lines are reported, never fatal (SPEC-002 R-01, EVT-002).

pub mod antigravity;
mod antigravity_hooks;
pub mod claude;
pub mod claude_config;
pub mod claude_plugins;
mod cleanup_permission;
pub mod codex;
pub mod guard_client;
pub mod hook_client;
pub mod launch;
pub mod login;
pub mod observe;
pub mod paths;
pub mod recount;
pub mod scheduled;
pub mod scrub_client;
pub mod secret;
pub mod skills;
pub mod step_back;
pub mod subagents;
pub mod tail;
pub mod time;

mod json;
