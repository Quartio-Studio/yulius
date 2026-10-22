---
yulius-task: 5942b8c4-4413-4a95-96a4-2e5ab6888fac
status: done
---

# SPEC-266: Transcript step-back and home creation go through handles (#661)

Status: done
Date: 2026-10-22
Owner: Yulius task #252
Scope authorization: task #252 (nightly "Work through open issues"), fixes #661. Applies
[ADR-0174](../../docs/decisions/0174-host-access-to-agent-owned-folders-goes-through-handles.md)
and the walk of [ADR-0193](../../docs/decisions/0193-the-claude-config-dir-is-walked-from-above-home-and-the-main-repository-is-read-from-the-git-layout.md).

## Goal

The usage-limit step-back (SPEC-034 LIM-001) and the creation of a task's home read and write the
agent's folders only through handles opened without links, so a link or a FIFO the agent puts in
`<sandbox>/home/.claude/projects/<slug>/<session>.jsonl`, or on the way to it, can neither steer
the server outside the sandbox nor hang it.

## Context

- Symptom (verified on `dev` at 589b082c):
  - `step_back_file` (`crates/yulius-harness/src/step_back.rs`) reads the transcript with
    `std::fs::read(path)`, takes its mode with `std::fs::metadata(path)`, replaces it with
    `PlannedFile::write` (a predictable `<name>.tmp-<pid>` at `parent.join`, `std::fs::rename`) and
    sets the mode with `std::fs::set_permissions(path)`: every step follows links, and the read
    blocks on a FIFO.
  - Callers: a task's resume after a limit (`supervisor.rs` `step_back_transcript`, an awaited
    `spawn_blocking`, through the `HarnessPort::step_back_transcript` port that gets the sandbox
    home), and the Superagent (cockpit) resume (`wiring/harness.rs` `plan_cockpit`, on
    `config_dir_host`).
  - `Supervisor::start_run` makes the home with `std::fs::create_dir_all(&home)`. A dangling
    `home` link creates nothing (`mkdir` gets `EEXIST`, then `is_dir` is false: an error), contrary
    to the issue's text; a `home` linked to a host folder is accepted as the home (`is_dir`
    follows it). The test's dangling case is a guard, the linked-folder case the regression.
- Root cause: the code predates ADR-0174; #657 (SPEC-220) and #634 (SPEC-253) moved the other
  host accesses to agent folders onto handles and left these two as "still by path (#661)". The
  existing test wrote a plain transcript in a temp dir, so links and FIFOs were never exercised.
- A task's home is `<sandbox>/home`, the Superagent's config dir `<user folder>/home/.claude`
  (`cockpit.rs`): in both, the folder above the home is out of the agent's reach, and
  `ensure_claude_config` already anchors there (SPEC-253 CFG-001).

## Requirements

- **STB-001** The step-back opens the folder above the home by path, then `home`, the config dir,
  `projects` and the project slug one at a time with `O_DIRECTORY | O_NOFOLLOW`; nothing is
  created. A missing folder or transcript is "unchanged"; a link or a file on the way fails,
  naming it, and nothing outside the sandbox is read or written.
- **STB-002** The transcript is opened `O_NOFOLLOW | O_NONBLOCK` in the project folder's handle
  and used only when `fstat` says regular file: a link fails (`ELOOP`), a FIFO fails at once.
- **STB-003** The rewrite goes to a random `O_CREAT | O_EXCL | O_NOFOLLOW` temporary file in the
  project folder's handle, given the old transcript's permission bits with `fchmod`, synced and
  renamed over the transcript with `renameat` within the same handle, then the folder synced; the
  temporary file is removed on failure.
- **STB-004** A task's home is made with `mkdirat` from its sandbox directory and opened
  `O_DIRECTORY | O_NOFOLLOW`: a `home` link (dangling or not) fails the start and creates nothing.
- **STB-005** Stepping back a normal transcript behaves as before: same result, same permissions,
  no temporary file left.

## Plan

- [x] Spec written and checked
- [x] Step-back reaches the transcript through handles (regression tests first)
- [x] The task home is made with mkdirat from the sandbox directory (regression test first)
- [x] Docs record the step-back and the home on handles
- [x] Independent review answered and gates green

## Acceptance criteria

- A linked project folder (or `home`, `.claude`, `projects`) and a linked transcript leave the
  link's target untouched, and the call errors naming the link (STB-001, STB-002).
- A FIFO at the transcript returns an error at once instead of blocking (STB-002).
- A dangling `home` link creates nothing outside the sandbox and fails the start (STB-004).
- `step_back_file_rewrites_only_when_needed` passes, mode 0600 kept, no temporary left (STB-005).

## Test plan

- `yulius-harness` unit tests in `step_back.rs`: a normal transcript (existing test, moved to the
  real layout), a linked slug folder, a linked `home`, a linked transcript (its target's content
  unchanged), a FIFO at the transcript (returns; the test thread would hang otherwise), a
  non-default mode (0640) kept.
- `yulius-app` unit test of the home creation: a dangling `home` link to a path outside the
  sandbox fails and the path stays absent; a missing home is made; an existing one is reused.
- Full gate (`scripts/gate` or the AGENTS.md command list).

## Decisions

- **Anchor derived from the config dir, not a new port argument.** The issue suggests passing the
  sandbox directory through the port. The step-back instead takes the config dir, its slug and
  session, and opens `<config>/../..` (the folder above the home) by path, as `ensure_claude_config`
  does (ADR-0193). Both callers already hold that config dir; the anchor is the same folder in
  both, and no port signature changes (other tasks of this run touch neighbouring code).
- **Reuse the harness's handle helpers.** `open_dir_at`, `open_regular` and `replace` in
  `claude_config.rs` become `pub(crate)`; `replace` gains a mode (applied with `fchmod`) through a
  sibling function so its callers stay as they are. No new module, no copy.
- **Home through `files::open_dirs_below`.** Its signature stays; the sandbox directory itself is
  still made with `create_dir_all` (it is the host's, above the mount).
- No new ADR: this applies ADR-0174 and ADR-0193 to two more places; ADR-0193's "not covered"
  line is updated.

## Evidence

- Regression tests first (commits `d00c0ae6`, `8781981f`), failing on the old code:
  `step_back_file_follows_no_linked_folder` and `step_back_file_follows_no_linked_transcript`
  (the old code followed the link and returned `Ok(true)`), `step_back_file_returns_at_once_on_a_fifo`
  (hit its 30 s bound, the old read blocked on the FIFO), `the_home_is_made_without_following_a_link`
  (a `home` linked to a host folder was accepted). Its dangling-link case already passed on `dev`
  (see Context); it stays as a guard.
- `cargo test -p yulius-harness --lib`: 78 passed. `cargo test -p yulius-app --lib
  the_home_is_made`: passed.
- `scripts/dev ci` at `14ff4e35`: exit 0 (fmt, clippy, macOS clippy, tests: 2414 nextest tests,
  doctests, deps). The first run at `29092e3d` failed on `cargo fmt --check`. `scripts/dev macos`
  then reproduced the reviewer's blocker (`mode_t` is `u16` on macOS), fixed in `14ff4e35`.
- `scripts/dev security` at `14ff4e35`: exit 0. Nothing in the change; two low findings in
  untouched `supervisor.rs` lines (483 `write_private_file`, 960 `profile_of`), filed by CI.
- Independent review, round 1 at `29092e3d`: request changes. Answered:
  - blocker (own code), macOS compile: fixed test-first (the macOS gate stage failed, then passed);
  - minor (own code), the dangling-home root cause was wrong in Context: corrected;
  - nit (own code), `.claude` and `projects` links were not covered: added to the test;
  - nit (own code), the changes entry overstated the linked-transcript case: reworded;
  - minor (pre-existing, outside #661's links and FIFOs), the transcript read has no size cap, as
    on `dev`: reported in a comment on #661 (this task creates no issues).
- Not tested here: a live resume after a real usage limit (the step-back's callers are unchanged
  and only the function body moved to handles).
