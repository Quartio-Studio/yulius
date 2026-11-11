---
yulius-task: 7bccc121-07d2-4f42-852f-27fe6f1ce5f4
status: done
---

# SPEC-451: The /yulius/bin wrapper lets `yulius tool` and `yulius remove` through

Status: done
Date: 2026-11-11
Owner: Yulius task #433, issue #1109
Scope authorization: task #430 (works through open issues), issue #1109 opened by task #418.

## Goal

Every `yulius` command made for a task's session works there: `yulius tool <name> [<json>]`
(SPEC-432 MCR-005) and `yulius remove <paths>` (SPEC-192 RMV-002), which the briefing tells agents
to use, run instead of printing the wrapper's usage line.

## Context

- **Symptom.** In a task session `yulius tool steps_get '{}'` prints "In a task's session yulius
  offers: yulius wait [--minutes N], yulius security-scan [--all]" and exits 2, while
  `/usr/local/bin/yulius tool steps_get '{}'` prints the plan. Reproduced in task #433's own
  session on 2026-11-11; `yulius remove --help` gives the same usage line and exit 2.
- **Code path.** `PATH` of an agent run starts with `/yulius/bin` (`put_gh_first`,
  `crates/yulius-app/src/identity.rs`), which holds the `gh` shim and the `yulius` wrapper written
  from `agent_yulius_shim`. The wrapper is an allowlist (`case "$1" in wait|security-scan) exec …`)
  that offers a session its own commands and a usage line for anything else. It is a usability
  guard, not a security boundary: the real binary is bound read-only at its own path in every run
  and user space, so `/usr/local/bin/yulius serve` is reachable anyway, and the server
  authenticates each call.
- **Root cause.** The allowlist is a second list of the session's commands, kept by hand. It
  dates from SPEC-011 (`wait` only); SPEC-195 added `security-scan`, but `remove` (SPEC-192,
  #588, 2026-10-16) and `tool` (SPEC-432, #1099, 2026-11-07) were added to the CLI and the
  briefing without it. Nothing ties the two lists together.
- **Why the tests missed it.** `the_agent_yulius_shim_offers_wait_and_the_security_scan` only
  checks the commands the wrapper already had; `remove_cli` and `tool_cli` tests call the binary
  directly, never through the wrapper.
- **Existing sessions.** The server rewrites the wrapper when its text changes (`write_gh_shim`,
  atomically by rename) whenever it prepares an agent run or a user's space, into the host
  directory that every run binds as a directory at `/yulius/bin` (task #433's session:
  `/var/lib/yulius/runtime/gh` on `/yulius/bin`, read-only, in `/proc/self/mountinfo`). So after
  the update the first agent run any task starts rewrites `runtime/gh`, and agent sessions already
  running see the new file too (a directory bind shows a renamed-in entry); nothing needs a
  restart. A person's contained sandbox shell gets the same wrapper from `runtime/bin`, rewritten
  when the server prepares that person's space, so `tool` and `remove` pass there too (harmless:
  the server authenticates them exactly as a direct call to the binary).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| WRP-001 | The agent's `yulius` wrapper passes `wait`, `tool`, `security-scan` and `remove` with their arguments to the real binary and keeps its exit code; anything else prints a usage line naming `yulius wait [--minutes N]`, `yulius tool <name> [<json>]`, `yulius security-scan [--all]` and `yulius remove <paths>` and exits 2. | `identity::tests::the_agent_yulius_shim_offers_the_session_commands` |
| WRP-002 | Every CLI subcommand whose help starts "In a task's session" is one the wrapper passes, and every command the wrapper passes is a subcommand, so a session command documented that way cannot be left out again and a renamed one leaves no dead arm. | `tests::the_agent_wrapper_passes_every_session_command` (`crates/yulius/src/main.rs`) |

## Plan

- [x] Spec of the wrapper fix
- [x] Regression test: the wrapper passes tool and remove
- [x] The wrapper passes tool and remove and its usage names them
- [x] Docs and changes entry
- [x] Independent review answered and gates green

## Acceptance criteria

- Through the generated wrapper, `tool steps_get '{}'` and `remove a b` reach the binary with the
  same arguments and exit code; `serve` is refused with exit 2 and a usage line naming
  `yulius tool <name> [<json>]`.
- A test fails when a subcommand whose help starts "In a task's session" is not passed by the
  wrapper, or when the wrapper passes a name that is no subcommand.
- `yulius tool steps_get '{}'` in a real task session prints the plan (only checkable once the
  change is deployed; see Evidence).

## Test plan

- Regression (unit, `identity.rs`): the existing wrapper test, renamed, also runs `tool steps_get
  '{}'` and `remove a b` through the generated wrapper against a fake binary and checks the usage
  line names `yulius tool <name> [<json>]` (fails today: exit 2 with the old usage line).
- Guard (unit, `main.rs`): reads clap's subcommands, takes those whose about starts with "In a
  task's session" and checks each is in the wrapper's `case` (fails today on `tool` and `remove`),
  and that each name in the `case` is a subcommand.
- What else the cause touches: the wrapper still refuses every other subcommand; `tool` and
  `remove` were already meant for sessions and authenticate with the session's task token
  (`YULIUS_TASK_TOKEN`) or refuse paths outside the task (SPEC-192 RMV-005), so letting them
  through grants nothing new (the binary is reachable directly anyway, see Context). The briefing
  is unchanged.
- Gates: `scripts/dev ci`, `scripts/dev security`.

## Decisions

- **`remove` is fixed here too.** It fails the same way for the same cause; leaving it would ship
  a fix that still breaks a command the briefing names. Same file, same line.
- **Keep the allowlist; guard it with a test.** Passing every subcommand would offer sessions
  `serve`, `sandbox-base` and the internals as if they were theirs (a usability guard, not a
  security one); generating the `case` from clap would need the
  binary's CLI types in `yulius-app`, the wrong direction. A test in the binary that compares the
  two lists catches the drift at the gate, which is what failed twice.

## Evidence

- Reproduced (task #433's session, server one commit behind dev, 2026-11-11): `yulius tool
  steps_get '{}'` and `yulius remove --help` → the old usage line, exit 2;
  `/usr/local/bin/yulius tool steps_get '{}'` → the plan, exit 0.
- Test first: at `cf4a40a1` (tests, old wrapper)
  `cargo nextest run -p yulius-app -p yulius --lib --bins -E 'test(the_agent_yulius_shim_offers_the_session_commands) | test(the_agent_wrapper_passes_every_session_command)'`
  → 2 failed (`left: (2, "In a task's session yulius offers: …")`, `tool not in ["wait",
  "security-scan"]`); at `7813c204` (fix) → 2 passed.
- Mutations of the guard (committed first, undone with `git checkout`): adding a dead `gone` arm →
  "gone is no subcommand"; dropping `tool` → "tool not in [\"wait\", \"security-scan\",
  \"remove\"]". Both fail.
- Live, in this session: the new wrapper text rendered with `/usr/local/bin/yulius` → `tool
  steps_get '{}'` prints the plan (exit 0), `remove <scratch file>` removes it (exit 0),
  `remove /etc/hostname` is refused by `remove` itself (exit 1), `serve` → the new usage line
  (exit 2).
- `scripts/dev ci` at `05a0ffac` → exit 0, 3285 tests passed, 16 skipped. After the review fix
  (`test(cli)` commit): the guard test and `cargo clippy -p yulius --all-targets -- -D warnings`
  pass. Yulius CI run `f859b87c` at `d80c40ac` failed `quality` on `cargo fmt --check` (that
  commit's `assert!` was not formatted; the local run had skipped fmt); `cargo fmt --all` fixed it
  and `scripts/dev lint` (fmt check and clippy) → exit 0. Run `3f6de5ac` at `366946c0` failed
  `cargo deny` on a new advisory, RUSTSEC-2026-0486 (rustix 1.1.5), which fails every pull request
  (PR #1151 carries the same bump, `ada375dc`, with identical lock hunks, which this check
  missed; whichever merges second merges cleanly): `cargo update -p rustix@1.1.5 --precise 1.1.6`,
  in its own commit
  (the resolver also moves the Windows-only `windows-sys` users onto 0.52, which `ring` already
  locks); then `cargo deny --locked check` → all ok, `cargo check --locked --workspace
  --all-targets` and `scripts/dev macos` → exit 0.
- Yulius CI run `17f01b52` at `0bd3a591` failed `quality` on four `yulius::main` tests that fail
  for every pull request since the host move of 2026-11-11 (#1154; they touch nothing of this
  change). Fixed here, in two test-only commits, because no head merges without it:
  - `first_message` (three tests, "timed out waiting for the continued turn"): the helper moved
    the task to another subscription after its goal's turn had ended (the fake's turn has no
    length, and the faster host answers its hooks before the move); since #1141 a task moved while
    idle gets nothing typed (SPEC-447 MOV-001), so the new session never opened with a turn. A
    probe showed `activity = Waiting` at the move. The helper now holds the goal's turn open
    (`goal_delay_ms`) and moves the task while it works (`4f34df77`).
  - `merge_train::a_member_that_cannot_merge_is_told_instead_of_tested_again`: the owner was
    disabled after the train's run had ended, racing the background decision, which had merged the
    member already. The owner is now disabled while the run is queued (`0817eefb`).
  - `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(=merge_train::a_member_that_cannot_merge_is_told_instead_of_tested_again) | test(/^first_message::/))'`
    before → 12 passed, 4 failed; after, both modules → 37 passed; the four tests ten times in a
    row → 10 of 10.
  - Not proven: that test is not sensitive to the train's own owner check. Two mutations of it in
    `train_member` (skipped; `Step::Stop` for one by one) leave it green, since `merge_armed_task`
    drops the armed merge of a disabled owner in the same pass. So before this change; left as is.
- Yulius CI run `e753bd7a` at `0bd3a591` failed `e2e`: about ten tests that open a tool while the
  page still loads fail on the new host for every pull request (reproduced here with 2 and with
  10 workers; this change touches neither `web/` nor `e2e/`). Task #437 (PR #1155) fixes them.
- `scripts/dev security` → exit 0, "Security scan disabled: Vidoc is turned off in Yulius":
  disabled, not passed; nothing was scanned.
- Not tested: acceptance criterion 3 with the real `/yulius/bin/yulius` of a task session needs the
  change deployed to the instance (the server writes the wrapper); left for after the deploy.
- Independent review (fresh session, read-only, `om-code-review`, head `05a0ffac`): APPROVE, no
  blocker or major. Answers:
  1. Own code, minor: the spec called the wrapper a barrier to `serve`; it is a usability guard
     (the binary is reachable directly). Fixed in Context and Decisions.
  2. Own code, minor: the guard only sees subcommands whose help starts "In a task's session"
     (`security-scan` does not). Added the reverse check (every `case` name is a subcommand) and
     narrowed WRP-002 to what the test proves. `security-scan`'s help stays as is: it also serves
     CI and other projects.
  3. Own code, nit: "run or shell" was wrong for `runtime/gh`; corrected, and persons' shells
     (`runtime/bin`) are now described.
  4. Own code, nit: SPEC-395 (done) cites the old test name; left as written, since its Evidence
     records what ran then.
  5. Pre-existing (`write_file` unchanged): the fixed `yulius.tmp` temporary name can make two
     overlapping launches that both rewrite a changed wrapper fail one launch. Outside this change
     (every wrapper text change since SPEC-011 has the same window); reported on issue #1109 and
     in the task summary, since this task creates no issues.
