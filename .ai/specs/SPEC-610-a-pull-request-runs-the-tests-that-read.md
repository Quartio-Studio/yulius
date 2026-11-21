---
yulius-task: a4a6603c-af95-4f93-8c03-fee246e93ef4
status: in-progress
---

# SPEC-610 Run repository file readers before merge

## Goal

Fixes #1652.

Catch stale assertions about repository content on the pull request and its merge train while keeping the full Rust and browser suites on dev.

## Context

PR #1645 changed the composer mock. The merge train passed and dev revision 13a632c40 failed one of 2,986 tests because the Antigravity composer assertion still expected the old line. The owner measured about 2.8 minutes for the train. [SPEC-598](SPEC-598-shorten-a-yulius-ci-run-parallel-quality.md) records the independent CI jobs. Rust contract tests open repository paths at runtime or embed them at compile time. Several tests share a directory reader.

## Requirements

- R1 Run all UI contract cases for any web tree change. Run the Rust text contracts for catalog and documentation changes, including readers in other crates.
- R2 Derive or verify coverage from test inputs. Cover edits, additions, deletions and both sides of renames. Keep unrelated full-suite work after merge.
- R3 Tell task agents to run every test in each test binary they touch.
- R4 Measure focused execution and the actual PR gate against the reported train. Request staging once after green delivery.

## Plan

- [x] Identify repository file readers and specify the pre-merge contract
- [x] Run the affected contract tests in the pull request gate
- [x] Require whole touched test binaries in task briefings
- [x] Verify reader coverage, timing and delivery evidence
- [x] Resolve named runtime paths in repository tests
- [x] Trace exported file constants into their consumer tests
- [ ] Approve the change for delivery to dev

## Acceptance criteria

A web-only mock edit executes the composer assertion and its siblings. A catalog or documentation edit executes its text readers. A newly introduced reader is selected or makes the coverage check fail. An unrelated edit avoids the Rust reader build. A rendered briefing includes the whole-binary instruction.

## Test plan

Exercise native job declarations and source discovery with real paths and synthetic readers. Exercise failure propagation with the reported stale composer assertion. Time the focused checks, run the touched binaries and quick checks, and record the CI and merge verdicts.

## Decisions

Use the existing path-filtered PR gate. It owns the checkout, timeout, exit status and candidate coverage, including outside PRs. Adding execution to delivery_merge would duplicate those responsibilities and miss independent CI runs. Group readers of shared inputs conservatively. No persistence or API changes are needed.

The selector reads Rust test sources in the workspace crates and tools. It resolves literal filesystem reads, compile-time includes and manifest-directory joins. Workspace-rooted policy tests collect their quoted file paths. Embedded workflow, automation and skill accessors cover tests that compare bundled data. Unit cases share their file’s inputs. A shared integration helper selects its sibling cases, while a direct integration read selects its case. A separate test helper with unidentified callers fails the repository check. CI checks the outer job globs against discovered tracked inputs.

The change diff uses the merge base with the integration branch and includes both names of a rename. An unavailable base selects all discovered readers. An unrelated change returns exit 78 before tool bootstrap. On a protected branch the full test shards execute the readers.

## Evidence

- On base 13a632c40, the initial web-path selection ran all 12 ui_contract tests. Eleven passed and the reported Antigravity assertion failed. Nextest reported 0.473 s of test execution. The first build in this worktree took 3m10s. PR #1651 then corrected the assertion on dev at 1f9314339.
- The Python regressions first failed on the absent job and selector. A new source-only reader, a workspace-root helper, a concatenated compile-time path and chained directory joins exercise discovery without adding test names to the implementation. A separate test-only helper with unknown callers fails the coverage check. A real Git fixture covers removal, rename and an unavailable base. The native path check compares the job globs with discovered tracked inputs. All seven cases passed.
- An unrelated Rust edit returned CI's skip code 78 in 0.43 s, before bootstrap or Cargo.
- The briefing assertion failed on the old rendered instructions. Running the entire domain test binary also caught three section-budget regressions in the first wording. The shorter discipline paragraph preserves the existing budget.
- The completed domain library run passed all 603 tests, including the rendered wording and section budgets. The combined web, catalog and docs probe on the corrected composer passed 165 tests in 1.06 s of nextest execution.
- Exact file includes retain their specific paths. A mock.js-only edit selects the 12 ui_contract cases. A changes-note edit selects the 22 skills unit cases. An operations-guide edit additionally selects its systemd examples. This avoids building extra package targets for the common single-file changes.
- On the integrated checkout, all 165 selected tests passed in 1.240 s. The build took 3m02s, and the command took 185.34 s including discovery and compilation. The repository quick checks passed. Vidoc reported disabled and performed no scan.
- Independent review identified two indirect catalog consumers, indirect literal filesystem paths, a segment-glob mismatch in the coverage check and rollout to older PR checkouts. Regression tests reproduced the findings. The revised selector covers the HTTP template list and workflow catalog accessor, bound path variables and literal concatenation. The native job uses only exact names and directory prefixes. An older-PR Git fixture runs the protected selector against the older checkout and propagates its failing test command. All nine Python regressions passed.
- The corrected integration binary ran the 12 UI contracts alone in 2.98 s. The hosted pull request gate and staging request are recorded by Yulius delivery.
- The second review reproduced a missed embedded HTTP security policy, an unqualified Rust file read and 112 unrelated cases selected by a rejected JSON path in a delivery test. Regression tests failed for all three. Header access now identifies policy consumers, imported reads resolve their literal inputs, and source edits select tests through their actual inputs. A delivery.rs-only change selects fixture_hygiene. All ten Python tests passed. The final unrelated-file probe returned 78 in 2.24 s.
- The third review found a typed path constant and the public SCANNER include. The follow-up work split runtime binding and exported data discovery. Both new fixtures failed first. The selector now derives public file constants at their definitions across the workspace and selects cases that use them, including the real scanner refusal test. Typed let, const and static path bindings resolve at filesystem reads. Eleven Python tests passed, and the compiled scanner refusal case passed.
