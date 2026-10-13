//! The `yulius` integration tests, linked into this one test binary (SPEC-033): each file is a
//! module. Every test binary links the whole product (about 190 MB), so fifty of them filled
//! sandboxes' disks. Add a file next to this one and declare it below; the repository check fails
//! on a file that is not declared. Run one file's tests with
//! `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^<file>::/)'`
//! (the `cargo test` filter `<file>::` is a substring: `supervision::` also runs
//! `pool_supervision::`).

mod support;

mod account_usage;
mod accounts;
mod agent_hooks;
mod analytics;
mod auto_choice;
mod automations;
mod browser;
mod cli;
mod cockpit;
mod context_full;
mod delivery;
mod disk;
mod files;
mod first_message;
mod git_adapter;
mod github_admin;
mod github_checks;
mod http_auth;
mod import_server;
mod intake;
mod isolation;
mod jev_key;
mod live_claude;
mod live_display;
mod live_ui_browser;
mod live_ui_editor;
mod live_ui_filedrop;
mod live_ui_mcp;
mod live_ui_stream;
mod live_ui_terminal;
mod local_ci;
mod manage_tools;
mod mcp_coordination;
mod notes;
mod pool_supervision;
mod preview;
mod projects;
mod queued_messages;
mod recovery;
mod secrets;
mod secrets_redaction;
mod self_update;
mod session_sync;
mod sessions;
mod shells;
mod skills_mcp;
mod subscription_pool;
mod supertask;
mod supervision;
mod task_delete;
mod task_done;
mod task_ownership;
mod task_prompt;
mod terminal_ws;
mod trusted_proxy;
mod ui_contract;
mod update_supervision;
mod uploads;
