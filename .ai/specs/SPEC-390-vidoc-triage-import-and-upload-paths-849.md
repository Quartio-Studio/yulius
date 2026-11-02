---
yulius-task: 8f10972e-402d-40ed-b5cb-175aeb312753
status: in-progress
---

# SPEC-390: Triage of the Vidoc findings on import and upload paths

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #372
Scope authorization: task #372 (close #849, #836, #827, #838 and #750, each as a false positive
or as solved).

## Goal

Each of the five Vidoc findings about client file names becoming filesystem paths is checked
against `dev` (4e07ce4e) and either fixed test-first or dismissed with its guard, a test that pins
that guard and a narrow `[[ignore]]` entry (one per wording) in `.config/vidoc.toml`, so Yulius
does not reopen the issue on the next scan.

## Context

- The import manifest (SPEC-001) names every uploaded file. `FileRef::validate`
  (`yulius-contracts/src/import.rs:543`) refuses a name that fails `is_safe_file_name` (`:564`):
  `/`-separated components of `[A-Za-z0-9._-]`, none empty and none starting with `.`, which rules
  out `..`, `.git`, hidden names, absolute paths, backslashes and NUL.
- `create_import` (`yulius-app/src/imports.rs:534`) validates the manifest first and stores its
  canonical form in the one `insert_import_job` (`:616`); the store never updates
  `import_jobs.manifest` (`yulius-store/src/imports.rs` changes only `received`, `state` and
  `result`). Every later read (`parse_manifest`, `:310`) therefore returns validated names.
- The chunk route's `{name}` (`yulius-server/src/routes/mod.rs:278`) only selects a manifest entry
  (`imports.rs:670-675`); an unknown name is `NotFound`, and only the entry's own name is joined.
- Editor uploads (`yulius-app/src/uploads.rs`): `begin_file_upload` runs `relative_components`
  (`files.rs:89`: normal components only, no `.git`, no NUL, not absolute) on the request path
  (`uploads.rs:632`); `file_target` builds `target.relative` from the same function
  (`files.rs:519-520`); `check_components` (`uploads.rs:234`) only adds length and control-character
  rules to that; `ensure_dirs` (`:345`) re-checks every resolved folder with `files::inside`
  (`files.rs:127`: inside the root, no `.git` component).
- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring, and with
  `function` and `line` (within 10 lines) a reworded repeat at that place (SPEC-273).

## Requirements

- VTI-001 (#849, #836, #827, dismissed). A manifest naming any file outside its job directory
  (`..`, an absolute path, a `.git` or hidden component, a backslash) is refused with 422 before a
  project, job or file is created; a chunk addressed to a name outside the manifest (including a
  traversal) is 404 and writes nothing. An end-to-end test pins both.
- VTI-002 (#838, dismissed). Editor uploads refuse `..` (also nested) and `.git`; the existing
  end-to-end test `uploads::…` traversal table pins it, extended with a nested `..`.
- VTI-003 (#750, dismissed). Covered by VTI-001's chunk half.
- VTI-004. One narrow `[[ignore]]` per dismissed wording, with its `function` and `line`, and a
  reason naming the guard and its test.

## Plan

- [ ] Spec SPEC-390 with the triage of all five findings
- [ ] Import file names pinned by an end-to-end refusal test
- [ ] Narrow [[ignore]] entries for the five dismissed wordings
- [ ] Issues commented and closed as false positives
- [ ] Gates, changes entry and independent review answered
- [ ] Merged into dev on green

## Acceptance criteria

- `import_server::unsafe_manifest_names_never_reach_the_disk` passes on `dev`'s code: each unsafe
  name is 422, no project exists afterwards, and a traversal chunk on a valid job is 404 with
  nothing written beside the job directory.
- The editor upload traversal table includes a nested `..` and still passes.
- `.config/vidoc.toml` has five new entries, each read by the CI config check.
- Issues #849, #836, #827, #838 and #750 are closed as not planned with their evidence.

## Test plan

- New integration test in `crates/yulius/tests/import_server.rs`; one more row in
  `crates/yulius/tests/uploads.rs`.
- `scripts/dev ci` (the full gate). The security scan is disabled for this project (reported as
  disabled, not as passed).

## Decisions

- All five are dismissed, not "hardened" with a second validator at join time: the names are
  validated once at the only point they enter (the manifest at `create_import`, the path at
  `relative_components`), the stored manifest is never rewritten, and a duplicate check would be
  a second version of the same rule to keep in step. The pinning tests fail if either entry point
  stops validating.
- #836's wording also names `.git` components and NUL; `is_safe_file_name` refuses both (no
  component may start with `.`, only `[A-Za-z0-9._-]` bytes).
- No issue is a duplicate of another: each has its own wording, place and fingerprint, and each
  gets its own `[[ignore]]` entry (an entry matches one wording).
