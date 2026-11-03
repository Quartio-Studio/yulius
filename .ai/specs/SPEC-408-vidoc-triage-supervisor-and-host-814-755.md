---
yulius-task: 1ca9a981-c9f8-400d-b143-8df16d921b1d
status: in-progress
---

# SPEC-408: Vidoc triage of the supervisor and host findings (#814 #755 #698 #756 #678 #690)

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #390
Scope authorization: task #390 (a person asked that each of the six issues end closed). Fixes
#678. #814, #755, #698, #756 and #690 are dismissed as false positives.

## Goal

Each of the six low Vidoc findings is checked against `dev` (d51b98ae). Each one is either fixed
test-first or dismissed with file:line evidence and a narrow `[[ignore]]` entry for its wording,
so that a repeat scan does not reopen it.

## Context

- `yulius serve` is one process per data directory (SPEC-002, its component diagram). `build_app` makes the data
  directory owner-only (0700) at every start, before anything else is opened
  (`serve.rs:405-412`, called at `:435`).
- An agent run's directory is `<data>/runs/<attempt>` (`supervisor.rs:1361-1362`). The process
  host makes it 0700 before it writes `launch.json` (`yulius-ptyhost/src/spawn.rs:167-181`,
  reached through `PtySessionHost::spawn`, `wiring/session.rs:181`).
- A contained run sees none of the run directory: only the run's generated files are bound in,
  read-only (`supervisor.rs:1429-1446`). Uncontained is the documented development mode, where
  agents and shells already run on the host as the service user (#715, SPEC-403).
- The server's `PATH` is the operator's: `yulius host setup` pins
  `Environment=PATH=/usr/local/bin:/usr/bin:/bin` in the unit (`host_setup.rs:256`, #716,
  SPEC-403). A run in an isolated sandbox gets the fixed `SANDBOX_PATH` instead
  (`supervisor.rs:1411-1425`, `isolation.rs:14`). A contained run without isolation sees the
  host's `/usr`, `/bin`, `/lib`, `/etc` and `/opt` read-only (`containment.rs:12`, `:87`).
- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring. There is
  one narrow entry per wording (SPEC-262, ADR-0202).

## Requirements

- VTS-001 (#678, real, hardening). `needrestart_rule` escapes every character of the unit name
  that is not an ASCII letter, digit, `_` or `-`, so the Perl regex in the rule matches the name
  literally on its own, whatever `service_name_ok` accepts. `-` stays as it is: outside a
  character class it is literal, and existing rules for hyphenated units keep their text.
- VTS-002 (#814, false positive). The run directory's default mode exposes nothing: the data
  directory above it is 0700, the process host sets the run directory to 0700 before it writes
  the launch, and every file Yulius writes there is 0600 (`write_private_file`).
- VTS-003 (#755, false positive). The `PATH` an agent run inherits is the operator's, which no
  agent sets. An isolated run gets the fixed `SANDBOX_PATH`, and a contained one sees the host's
  system read-only. The same decision as #716.
- VTS-004 (#698, false positive). Nobody but the server can plant a link in the run directory:
  it is 0700 under a 0700 data directory, a contained run does not see it, and an uncontained
  agent already runs as the service user.
- VTS-005 (#756, false positive). The `--scrub-exec` wrapper raises its own `oom_score_adj` to
  500. A positive value makes the agent's command the kernel's preferred victim, which protects
  co-located services (the opposite of the finding's denial of service). An unprivileged process
  can only raise it. A failed write (no `/proc`, as on macOS) must not stop the agent's command,
  so it is ignored on purpose.
- VTS-006 (#690, false positive). The hourly bound on tool-created tasks lives in memory by
  design (MGT-003, SPEC-037; `manage.rs:8-9`). There is one server per data directory, only the
  operator restarts it, and the window is a rolling hour pruned at every reservation
  (`manage.rs:43-62`, tested by
  `manage::tests::the_window_holds_twenty_an_hour_and_frees_slots_as_they_age`).

## Plan

- [x] Spec SPEC-408 with the triage of all six findings
- [x] needrestart rule escapes every non-word character (#678)
- [ ] Narrow [[ignore]] entries for the dismissed wordings (#814 #755 #698 #756 #690)
- [ ] Changes entry and gates
- [ ] Independent review answered
- [ ] Dismissed issues commented and closed (#814 #755 #698 #756 #690)

## Acceptance criteria

- `host_setup::tests::the_needrestart_rule_matches_the_name_literally` fails on dev, where `+`,
  `(`, `)` and `$` reach the regex unescaped, and passes with the fix. The rule for
  `yulius.service` is unchanged.
- The existing `host_setup::` tests pass unchanged.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason and
  function.
- Each issue ends closed: #678 by the merged pull request, and the others as not planned, each
  with a comment giving the evidence.

## Test plan

- The new unit test in `crates/yulius/src/host_setup/tests.rs`, and the `host_setup::` tests.
- `scripts/dev ci` (the full gate) and `yulius security-scan`.

## Decisions

- #678 is fixed, not dismissed, though `service_name_ok` already guards its only caller
  (`host_setup.rs:457` runs before `needrestart` at `:463`). `needrestart_rule` is public and its
  correctness depended on a check elsewhere. Escaping by what is safe instead of by what is
  special ties the two together, as the finding asks, for three lines.
- #814 is not "fixed" with a second chmod in the supervisor: the directory is already 0700 at
  the one place that creates its launch, under a 0700 data directory. A redundant mode adds a
  third copy of the same rule.
- #755 keeps the inherited `PATH` outside a sandbox. A fixed list there would miss the operator's
  own tools, and whoever can change the server's `PATH` already controls the server (#716, #806).
