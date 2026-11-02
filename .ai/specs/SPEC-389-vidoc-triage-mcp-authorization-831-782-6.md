---
yulius-task: 26e0474f-5481-43e8-a4d6-f793a4e41c78
status: in-progress
---

# SPEC-389: Triage of the Vidoc findings on MCP tool authorization (#831 #782 #695 #694 #846)

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #371
Scope authorization: task #371 (a person asked for each of the five issues to end closed, as a
false positive or as solved, without creating new issues).

## Goal

Each of the five Vidoc findings filed as #831, #782, #695, #694 and #846 is checked against `dev`
(4e07ce4e) and either fixed test-first or dismissed with its reason, the guard that stops it, the
test that pins that guard, and one narrow `[[ignore]]` entry for its wording in
`.config/vidoc.toml`, so Yulius does not reopen the issue on the next scan.

## Context

- Vidoc files each finding once as an issue and reopens it when it reports the finding again after
  it was closed; a false positive needs an `[[ignore]]` entry (path exact, title a
  case-insensitive substring, SPEC-217, SPEC-273). Vidoc rewords findings between scans, so each
  entry holds the wording on record and stays narrow (SPEC-262 VTR-005): an entry with `function`
  would accept any later finding in `mcp_call` or `tool_task_create`, so none is given.
- `.config/vidoc.toml` already accepts the `secret_verify` oracle under another wording ("Secret
  value verification oracle", SPEC-217's audit).
- The MCP server (`crates/yulius-app/src/mcp.rs`) serves two callers: a task's agent (task token)
  and a person's Cockpit (`ck_` token, the Superagent), whose tool set is `COCKPIT_TOOLS`
  (SPEC-037 MGT-006, ADR-0029, ADR-0144, ADR-0150, ADR-0178, ADR-0227).

## Requirements

- VMA-001 (#831, dismissed). `mcp_call` keeps the `McpCaller` through the whole dispatch: its first
  arm refuses a Cockpit any tool outside `COCKPIT_TOOLS` (`mcp.rs:585-588`, "A Superagent calls
  only the tools it is served, whatever the arms below allow"); Cockpit-only tools match only
  `McpCaller::Cockpit` (`mcp.rs:623-649`), and an agent falls to `task_tool`, which has none of
  them and answers "unknown tool" (`mcp.rs:650`, `mcp.rs:720`). `project` is only the project both
  callers act in. Pinned by `cockpit.rs` `a_cockpit_manages_the_project_for_its_owner`
  (`steps_set` refused) and `the_superagent_lists_its_sandboxs_agent_secrets_without_values`
  (`browser_fill_secret`, `secret_state_restore`, `secret_state_save`, `delivery_merge`,
  `scope_claim` refused) and `secrets.rs` `agents_are_not_served_secret_verify`.
- VMA-002 (#782, dismissed). Task, session and automation tools are the Cockpit's by design:
  `COCKPIT_TOOLS` lists them (`mcp.rs:31-55`; SPEC-037 MGT-006, ADR-0029, SPEC-066 MGT-012). A
  `ck_` token is the token of a live Cockpit that resolves, at every call, to its creator only
  while they are enabled and may work in the project (`cockpit.rs:249-280`, MGT-005/MGT-007), and
  each creating or changing tool checks `manage_authority` again (`manage.rs:98-110`, its call
  sites). Sessions of the project are readable by any member who may view it; the Cockpit's person
  may work in it. Pinned by `cockpit.rs` `a_cockpit_manages_the_project_for_its_owner`,
  `a_cockpit_whose_owner_lost_access_manages_nothing`, `a_disabled_owners_cockpit_manages_nothing`.
- VMA-003 (#695, dismissed). `secret_create_from_file` reads no arbitrary host file: `source`
  (`vault/create.rs:78-105`) refuses absolute paths, `..`, `.git`, `~user` and a bare folder; the
  file is reached from an anchor Yulius chooses (the caller's work folder or home in the sandbox,
  the Cockpit person's own home; `vault/create.rs:353-392`) one folder at a time without following
  a link (`open_folder`, `vault/create.rs:271`), and only a regular, non-link file of at most
  `MAX_FIELD_BYTES` is read (`read_secret_file`, `vault/create.rs:110-177`). The person must be
  able to configure the project (`Action::Configure`, `vault/create.rs:237`), and the value never
  reaches the caller. Pinned by `secrets_create.rs` `files_yulius_must_not_read_are_refused`,
  `a_root_swapped_for_a_link_and_reserved_names_are_refused` and `create.rs`
  `paths_name_the_work_folder_or_the_home`.
- VMA-004 (#694, dismissed). `agent_workflow` (`automations.rs:1329-1357`) only reads the
  workflows of the caller's own project and changes nothing; its one error says whether a named
  workflow exists there. The same caller gets the whole list from `workflows_list`
  (`mcp.rs:621`, `workflows.rs:500-514`), which by design needs no write access, so resolving it
  before `manage_authority` (`manage.rs:160-164`, `manage.rs:179`) discloses nothing; a Cockpit is
  checked for write access before any tool runs (`cockpit.rs:268`). Nothing is created before
  `manage_authority` passes. Not changed: reordering would only change which error a caller
  without write access reads first.
- VMA-005 (#846, dismissed). `secret_verify` is the Cockpit's only (`mcp.rs:634`, refused to
  agents) and an accepted design (ADR-0144): Yulius never hands out a hash of a stored value, so no
  offline attack is possible from what it answers; a salt would not change an online equality
  check. It refuses production secrets (`verify.rs:181`), compares in constant time and audits each
  distinct candidate (`verify.rs:191`). ADR-0144's Consequences accept that the oracle adds nothing
  beyond the redaction a person's processes already see (which also tells a guessed value), and
  that verified secrets are keys, not short passwords.

## Plan

- [x] Spec with the triage of all five findings
- [ ] Narrow [[ignore]] entries for the five dismissed wordings
- [ ] Gates, changes entry and evidence
- [ ] Independent review answered
- [ ] Issues commented and closed, merged into dev on green

## Acceptance criteria

- `.config/vidoc.toml` has one `[[ignore]]` entry per dismissed wording (five, the exact titles of
  #831, #782, #695, #694 and #846 at their files), each with its reason; no entry uses `function`
  or `line`.
- No product code changes; the tests named in VMA-001..005 pass on this branch.
- Each issue carries a comment with the reason and file:line evidence on `dev` and is closed as not
  planned; no issue is created.

## Test plan

- `cargo test -p yulius --test cockpit a_cockpit_manages_the_project_for_its_owner
  the_superagent_lists_its_sandboxs_agent_secrets_without_values a_cockpit_whose_owner
  a_disabled_owners_cockpit`, `--test secrets agents_are_not_served_secret_verify`, `--test
  secrets_create`: the guards cited are pinned.
- `python3 -c 'import tomllib; …'` parses `.config/vidoc.toml`; the scanner's ignore tests
  (`scripts/vidoc_scan.py` tests in the gate) still pass.
- `scripts/dev ci`: the full gate.

## Decisions

- All five findings are false positives on `dev`; nothing is fixed. Each guard is already pinned by
  an integration or unit test, so no new test is added (a test that cannot fail without a change
  would prove nothing new).
- Dismissed with title-only `[[ignore]]` entries, one per wording (SPEC-262 VTR-005): a `function`
  entry on `mcp_call` would accept any later finding in the whole dispatch.
- #694 is not "fixed" by reordering: changing product behaviour to silence a false positive is
  refused (SPEC-262), and the order is harmless (VMA-004).
