---
yulius-task: cee88c6b-57ca-4640-98e8-7e958163a913
status: in-progress
---

# SPEC-399: Vidoc triage of the web UI and sandbox path findings (#819 #741 #701 #683 #682)

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #380
Scope authorization: task #380 (a person asked that each of the five issues end closed). All five
are dismissed as false positives; nothing in the product changes.

## Goal

Each of the five Vidoc findings about the web terminal's clipboard, two links in the web UI and two
sandbox host paths is checked against `dev` (88a485ca) and either fixed test-first or dismissed
with file:line evidence and a narrow `[[ignore]]` entry for its wording, so a repeat scan does not
reopen it.

## Context

- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring; one narrow
  entry per wording (SPEC-262, ADR-0202). Each issue says Yulius reopens it when Vidoc reports the
  finding again after it was closed, so a dismissal needs its entry.
- Vidoc is turned off for this project in Yulius: `yulius security-scan` scans nothing, so the
  entries cannot be checked against a live scan.
- The same triage of other groups: SPEC-392 (auth and tokens), SPEC-393, SPEC-395.

## Requirements

- VWS-001 (#819, false positive: accepted by ADR-0130). A program's OSC 52 write reaches only a
  viewer who can write and pressed a key or mouse button in that terminal in the last 5 s;
  queries are never answered. The finding's remedy (match xterm's selection) cannot work: the
  program draws its own selection, xterm has none. The residual risk is the one ADR-0130 records
  (a native terminal's). Dismissed with a narrow entry.
- VWS-002 (#741, false positive). `sandbox.repo.url` is not stored repository data: the server
  builds it as `https://github.com/{owner}/{name}` (empty for a playground), so its scheme and
  host are fixed. Pinned by a new assertion; dismissed with a narrow entry.
- VWS-003 (#701, false positive). `needs_you` refuses any `link` that is not this instance's
  `public_url` or a page below it, so no `javascript:` (or other scheme) link is stored; the
  instance's own `public_url` is set by its operator. Pinned by a new `javascript:` case;
  dismissed with a narrow entry.
- VWS-004 (#683, false positive; the code changed in #651). The ids reaching the manager are
  validated at the trust boundary: the server passes a `SandboxId`, which only
  `SandboxId::parse` makes (ASCII letters, digits, `-` and `_`, at most 64;
  `yulius-domain/src/ids.rs:33`, `wiring/sandbox.rs:267`). Below that, `state_dir` and `sysroot`
  go through `path_name`, which maps a name that is not a holder name to the fixed component
  `.invalid`, and `HolderSpec::validate` refuses such a name before a holder starts.
  `production_runs_dir` joins the id itself, so it relies on the `SandboxId` boundary. Dismissed
  with a narrow entry.
- VWS-005 (#682, false positive). The path-based `make_writable` runs only from
  `base::remove_tree`, which ADR-0176 keeps for trees no sandbox run can change at that time:
  bases (built by the service user, mounted read-only), and system trees (`sysroots/`,
  `runtime/sandboxes/`, `system/`) removed only once no holder of them lives (leftovers.rs checks
  `holder_live` again at deletion). Runs join the holder's user, network and mount namespaces,
  not its PID namespace, so `holder_live` also counts a stopped holder's runs still in its network
  namespace (`manager.rs:1149`), and a removed sandbox is purged only once nothing of it runs
  (`still_running`). Trees runs can rearrange (`sandboxes/`, CD run files) go through the
  `O_NOFOLLOW` walk as mapped root (`reachable_by_runs`). The one other caller,
  `docker_smoke.rs:386`, is an operator-run diagnostic that removes its own fixed
  `docker-smoke-<pid>` sandbox by path after stopping its holders. Even a won race only adds
  owner `rwx` to a directory the service user owns; the `remove_dir_all` after it follows no link.
  Dismissed with a narrow entry.

## Plan

- [x] Spec SPEC-399 with the triage of all five findings
- [x] Pin the link guards of #741 and #701 with tests
- [x] Narrow [[ignore]] entries for the five wordings
- [ ] Gates, docs and changes entry
- [ ] Independent review answered
- [ ] Issues closed with their evidence (#819 #741 #701 #683 #682)

## Acceptance criteria

- A GitHub sandbox's API answer has `repo.url` `https://github.com/<owner>/<name>`
  (`playground_move.rs`), a playground's is empty (`playground.rs`, existing).
- `needs_you` with `link: "javascript:alert(document.cookie)"` is an error and records nothing
  (`cd.rs`).
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each issue ends closed as not planned with a comment giving the evidence on `dev`.

## Test plan

- New assertions in existing integration tests: `playground_move.rs` (the moved sandbox's
  `repo.url`) and `cd.rs` `a_needs_you_request_reads_what_happened_problem_and_to_do` (a
  `javascript:` link refused). They pin guards that already hold, so they pass on `dev` as well.
- Existing tests behind the other dismissals: `e2e/tests/terminal.e2e.ts` (OSC 52 without input
  and for a read-only viewer writes nothing), `manager.rs`
  `a_production_holder_has_its_own_trees_and_is_retired_with_its_sandbox` (`x.y` maps to
  `.invalid`), `leftovers.rs` `the_system_layer_waits_for_the_holder_to_stop` and `manager.rs`
  `only_a_sandbox_s_system_tree_is_removed`.
- `scripts/dev ci` (the full gate) and `yulius security-scan`.

## Decisions

- All five are dismissed, none fixed. For #682, a handle-based `make_writable` in `yulius-sandbox`
  would be a second copy of the leftovers walk, the alternative ADR-0176 rejected; the trees it
  serves have no writer when it runs. For #683, an early `valid_name` check in `retired()` would
  change no path (they are already `.invalid`) and only duplicate `path_name`. For #819, tighter
  rules (only after a mouse release) would break Claude Code's `/copy` and Ctrl+C copies, which
  ADR-0130 exists for.
- The entries are title-only, like the existing ones, not `function` entries: under ADR-0202 a
  `function` accepts any later finding in that function, broader than these dismissals.
- No new ADR: nothing changes; ADR-0130 and ADR-0176 already record the decisions behind #819 and
  #682.

## Evidence

(filled in as the work completes)
