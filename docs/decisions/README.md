# Decision records

Architecture decision records, numbered `NNNN-<slug>.md`. They summarise the ADR table in
[SPEC-002](../../.ai/specs/SPEC-002-yulius-architecture.md) section 2, which stays the
detailed source (ADR-NN numbers there are cited in each record). New decisions get the next
free number here, also in their title; a substantial one also gets an analysis section in the
relevant spec. The repository check refuses two records with one number (#317): when a parallel
task merged the same number first, take the next free one after merging `dev`. It also refuses a
title whose number differs from the file name, and a record this index does not list.

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
- [0016](0016-apm-for-agent-primitives.md): Agent primitives are managed with APM
- [0017](0017-sandbox-disk-upkeep.md): Disk is managed by Yulius: caches of idle tasks are pruned, switches check first
- [0018](0018-task-start-rules.md): Tasks start by a logical rule over other tasks' states
- [0019](0019-start-rule-editor.md): The composer builds a start rule from picked conditions, all or any
- [0020](0020-cockpit-as-a-plain-shell-session.md): The Cockpit is a plain Claude Code session on the shell runtime
- [0021](0021-composer-drafts-in-the-browser.md): New-task drafts live in the browser, per user and sandbox
- [0022](0022-native-task-attachments.md): Attached files reach Claude Code as `@path` mentions in its first message
- [0023](0023-no-messages-until-delivery-is-reliable.md): No messages until their delivery is reliable
- [0024](0024-default-model-and-effort.md): Tasks default to Opus 5.5 at high effort
- [0025](0025-sandbox-navigation-more-group.md): Six sandbox pages in the sidebar, the rest under "More"
- [0026](0026-attached-images-are-pasted.md): Attached images are pasted into the first message, not mentioned
- [0027](0027-step-back-past-a-usage-limit.md): A usage limit steps the transcript back; Claude Code continues the cut turn
- [0028](0028-one-integration-test-binary.md): The product's integration tests are one test binary
- [0029](0029-cockpit-manages-through-mcp.md): The Cockpit and task agents manage the project through the Yulius MCP server
- [0030](0030-menus-filter-when-long.md): Menus are compact and filter themselves when long
- [0031](0031-deleting-a-done-task-closes-it.md): Deleting a Done task closes its session first
- [0032](0032-shared-notes-with-revisions.md): Shared notes are stored on the server and saved by revision
- [0033](0033-original-prompt-on-demand.md): A task's original prompt is read on demand; its files only from its own attachment folders
- [0034](0034-ready-made-automations-catalog.md): Ready-made automations ship as a catalog, enabled in one click and matched by name
- [0035](0035-queued-messages-join-the-request.md): Messages queued into a waiting task join its request
- [0036](0036-agent-in-the-right-panel.md): The Cockpit is shown as the Agent, a tool in the right panel
- [0037](0037-dead-code-tools.md): Dead code is cleaned with each language's established tool; Yulius's gate uses cargo-shear
- [0038](0038-onboarding-starts-in-the-browser.md): Onboarding starts in the browser, never from the CLI
- [0039](0039-ci-jobs-get-the-cpu-first.md): CI jobs get the CPU first through a weighted cgroup
- [0040](0040-done-tasks-release-their-subscription.md): Done tasks release their subscription
- [0045](0045-ci-waits-for-disk-under-the-shared-quota.md): CI waits for disk under the sandbox's shared quota
- [0046](0046-compact-then-restart-a-full-session.md): A full session is compacted, then restarted fresh, then handed to a person
- [0041](0041-administrators-reset-a-lost-two-factor.md): Administrators reset a lost two-factor; a code confirms only the seed it was checked against
- [0047](0047-device-sign-in-start-is-limited-and-bounded.md): Device sign-in start is limited per client, its grants bounded and purged
- [0050](0050-a-run-ends-only-when-its-host-is-proven-gone.md): A stopped or closed run ends only when its host is proven gone
- [0042](0042-personal-two-factor-guards-the-vault.md): A personal two-factor code guards the vault; a secret is a title, a value and notes
- [0043](0043-archive-all-runs-the-per-task-archive.md): "Archive all" runs the per-task archive; no bulk endpoint
- [0044](0044-task-status-follows-claude-codes-own-records.md): A task's status follows Claude Code's own records
- [0048](0048-armed-merge-state-under-one-lock.md): An armed merge, its decisions and its outcome are one value under one lock
- [0049](0049-management-tools-cover-the-task-and-automation-lifecycle.md): Management tools cover the task and automation lifecycle, never the caller's own task
- [0051](0051-only-keystrokes-count-as-typing.md): Only keystrokes count as typing; a typed first message stays pending until submitted
- [0051](0051-the-merge-gate-enforces-the-branch-policy.md): Yulius's merge gate enforces the branch policy
