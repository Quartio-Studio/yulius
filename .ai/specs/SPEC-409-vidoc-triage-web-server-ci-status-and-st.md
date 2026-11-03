---
yulius-task: 3140fc31-ace4-4995-ae6a-b77475cdd30b
status: in-progress
---

# SPEC-409: Vidoc triage of web, server, CI status and store findings (#817 #816 #820 #745 #737 #732)

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #391
Scope authorization: task #391 (a person asked that each of the six issues end closed). Fixes
#745. #817, #816, #737 and #732 are dismissed as false positives. #820 is unreachable code, which
is removed.

## Goal

Each of the six low Vidoc findings is checked against `dev` (d51b98ae). Each one is either fixed
test-first, or dismissed with file:line evidence and a narrow `[[ignore]]` entry for its wording,
so that a repeat scan does not reopen it.

## Context

- `delivery::read_repo_file` reads the task spec, `yulius.toml` and the specs found by discovery
  from the task worktree, which a contained agent writes. The server reads them outside
  containment. It canonicalized the path, checked that it was under the canonical root, and then
  read it again by path.
- `files::open_inside` (#657, ADR-0174) opens a resolved path with `O_NOFOLLOW | O_NONBLOCK` and
  accepts the handle only when its real path is still inside the root and out of `.git`.
  `onboarding::read_inside` (#744, SPEC-394) already reads checkout files that way. The
  `race::Point` pause points reproduce a swap between a check and its use deterministically.
- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring, optionally
  narrowed to a `function` (SPEC-262, SPEC-273, ADR-0202).

## Requirements

- VTW-001 (#745, real). A spec or configuration file read from the task worktree is read through
  a handle whose real path is inside the canonical worktree. A folder on the way swapped for a
  link after the containment check, or the file swapped for a link or a FIFO, yields an error.
  It does not yield the link target's text, and the server does not hang on the FIFO. The size
  check and the read use that same handle, and the read stops at `max` bytes.
- VTW-002 (#820, unreachable). `spec_format::spec_path` has no caller outside its own unit test.
  Discovery and allocation build spec paths in `yulius-app` (`dlv_discover_spec`,
  `allocate_spec_path`). The function is removed, so no later caller can pass it an unchecked
  slug.
- VTW-003 (#817, false positive). A draft thumbnail is a data URL drawn by this page from the
  person's own file (`drafts.js` `thumbnail`). It is restored from this browser's
  `localStorage` only when it starts with `data:image/`. No server or draft-sync response sets
  it. Dismissed with a narrow entry.
- VTW-004 (#816, false positive). A shell has one viewer kind: its creator, with `Work` access
  to the sandbox and owning the shell's task (`shell_open`). Any socket that reaches `pump` can
  write and controls its own shell. The periodic re-check closes the socket when that changes,
  and `shell_input` checks again. Dismissed with a narrow entry.
- VTW-005 (#737, false positive). Each part of the commit status `target_url` is server-made:
  `public_url` is operator configuration, the sandbox slug is `[a-z0-9-]` (`slug_base`, with a
  numeric suffix), and the run id is a UUID. The URL goes into a JSON body that is checked to be
  http(s), never into a header. Dismissed with a narrow entry.
- VTW-006 (#732, false positive). The interpolated `LIMIT` is `AnalyticsQuery::limit: u32`, an
  integer by type, as the finding itself says. Dismissed with a narrow entry.

## Plan

- [x] Spec SPEC-409 with the triage of all six findings
- [ ] Spec and config reads go through a handle checked inside the worktree (#745)
- [ ] Unused spec_path removed (#820)
- [ ] Narrow [[ignore]] entries for the dismissed wordings (#817 #816 #737 #732)
- [ ] Changes entry and gates
- [ ] Independent review answered
- [ ] Dismissed issues commented and closed (#817 #816 #820 #737 #732)

## Acceptance criteria

- `delivery::a_spec_folder_swapped_for_a_link_after_its_check_is_not_read` fails on dev, where
  the host spec is read through the link and judged valid. It passes with the fix: the spec is
  refused as pointing outside the worktree.
- `delivery::a_spec_swapped_for_a_fifo_after_its_check_does_not_block` shows that a FIFO swapped
  in reads as an error within the timeout.
- The existing delivery spec tests pass unchanged.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each issue ends closed: #745 by the merged pull request, and the others as not planned, each
  with a comment giving the evidence.

## Test plan

- Integration tests in `crates/yulius/tests/delivery.rs`, with the new `race::Point::RepoFileRead`.
- `scripts/dev ci` (the full gate) and `yulius security-scan`.

## Decisions

- #745 reuses `files::open_inside` and follows the handle-not-path rule of ADR-0174, as #744 did
  for onboarding. No new ADR. `open_inside` also refuses `.git`; no caller reads a file there.
  The canonicalization stays, so a spec that is a link to another file inside the worktree is
  still read as before, and the containment error stays as clear.
- #820: the function is removed rather than given its own validation. Nothing calls it, and an
  ignore entry would keep dead code.
- #732: binding `LIMIT` would renumber the parameters of every query that reuses the cohort CTE,
  with no gain for a `u32`. It stays as it is.
