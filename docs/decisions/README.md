# Decision records

Architecture decision records, numbered `NNNN-<slug>.md`. They summarise the ADR table in
[SPEC-002](../../.ai/specs/SPEC-002-yulius-architecture.md) section 2, which stays the
detailed source (ADR-NN numbers there are cited in each record). New decisions get the next
number here; a substantial one also gets an analysis section in the relevant spec.

- [0001](0001-single-rust-binary.md): One Rust binary with an embedded static web UI
- [0002](0002-detached-ptyhost-per-run.md): A detached ptyhost process owns every agent run
- [0003](0003-sandbox-isolation.md): Per-sandbox containment and namespace holder
- [0004](0004-sqlite-wal-store.md): SQLite in WAL mode as the only database
- [0005](0005-claude-only-accounts.md): Claude Code only; accounts are org secrets with per-sandbox homes
- [0006](0006-yulius-mcp-and-hooks.md): One internal MCP server and hook protocol for coordination
- [0007](0007-github-app-authority.md): GitHub App as the authority for repositories and permissions
- [0008](0008-streamed-sandbox-browser.md): The sandbox browser is the real Chrome streamed as H.264
- [0009](0009-builtin-monaco-editor.md): Built-in Monaco editor over a file API
- [0010](0010-delivery-profiles.md): Delivery profiles in yulius.toml
- [0011](0011-superadmin-recovery-code.md): Superadmin recovery by a six-digit code
- [0012](0012-task-intake-and-chains.md): Task intake with Jev and chained tasks
- [0013](0013-updates-from-main.md): Self-updates from main through a root helper
- [0014](0014-secrets-vault.md): Envelope-encrypted secrets vault with an egress proxy
- [0015](0015-yulius-ci.md): Yulius is the projects' only CI
- [0017](0017-sandbox-disk-upkeep.md): Disk is managed by Yulius: caches of idle tasks are pruned, switches check first
- [0018](0018-task-start-rules.md): Tasks start by a logical rule over other tasks' states
- [0019](0019-start-rule-editor.md): The composer builds a start rule from picked conditions, all or any
- [0020](0020-cockpit-as-a-plain-shell-session.md): The Cockpit is a plain Claude Code session on the shell runtime
- [0021](0021-no-messages-until-delivery-is-reliable.md): No messages until their delivery is reliable
