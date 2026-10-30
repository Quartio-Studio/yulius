---
yulius-task: c2ab97f7-0fb6-4a21-920a-49b7d1201574
status: in-progress
---

# SPEC-353: Leftover processes pile up in a sandbox, and a session's commands stop with "fingerprints cannot be loaded"

Status: in progress
Date: 2026-10-30
Owner: Yulius task #335
Scope authorization: task #335, two defects seen on the production host on 2026-10-30 in the
quartio-v2 sandbox `7c46cdac-…`.

## Goal

A live session keeps running its commands without a server restart, even when processes in its
sandbox hold many connections to Yulius. When its commands cannot run, the session says why and
what to do, and never blames a restart that is not happening. Processes that no shell of the agent
owns any more (strays) are reported, and an idle task's strays are ended after a bound. Shells that
a live turn owns are never touched.

## Context

### Defect 1: "the agent-secret fingerprints of this run cannot be loaded"

- Symptom: at 21:37 CET, every Bash command and hook of `claude --resume cec9f680-…` in quartio-v2
  failed in `run_scrub_exec`'s `Fingerprints::Unavailable` arm. `yulius serve` had been up since
  2026-10-28, and only `systemctl restart yulius` at 21:40 brought the session back.
- Which session: only task runs (`vault_run_setup`) and the Superagent (`cockpit.rs`) get the
  `CLAUDE_CODE_SHELL_PREFIX=yulius --scrub-exec` wrapper. Yulius starts a task run with
  `--session-id`, and with `--resume` only to continue an earlier run. It always starts the
  Superagent with `--resume <its newest conversation>`.
- Why only this session failed: `load_fingerprints` falls back to the run's `scrub.json` when
  Yulius cannot answer (since SPEC-352 SRF-003, not when it refuses the token). A task run
  always has that file, read-only-bound into its bwrap, and nothing deletes it while the run
  lives. Measured here: with Yulius unreachable, a task run's
  `--scrub-exec` still runs the command from the file. The Superagent has no file (SPEC-147 chose
  that), so it failed whenever `POST /internal/scrub/fingerprints` did not answer 200 within 2 s.
  The failing session was therefore the person's Superagent. Task #334 (SPEC-352, merged into this
  branch) gives the Superagent a fingerprint file; this spec does not repeat that.
- Why the server path failed while Yulius was up: a sandbox reaches Yulius on one port, 8081,
  which serves both the internal API and the egress proxy, through the sandbox relay
  (`yulius-sandbox/src/relay.rs`). The relay takes at most 256 connections per sandbox and
  direction (`MAX_CONNECTIONS`). The egress proxy takes at most 256 connections over **all**
  sandboxes (`EgressConfig::max_connections`) and holds a tunnel for up to 24 h
  (`CONNECTION_MAX`). Both refuse past their limit without a log line: the relay drops the
  connection, and the proxy answers 503 "too many connections". The fingerprints route logs a
  refusal at debug only. Long-lived processes left in quartio-v2 (a headless Chrome, scripts) hold
  connections through the proxy. Once one sandbox holds the proxy's slots, every sandbox's
  downloads fail, and that sandbox's own relay fills, so its internal API calls (fingerprints,
  hooks, MCP) are dropped. Only a restart closed those connections. Independent evidence: from
  21:36 to 21:40, task #334 saw `Yulius proxy: too many connections; try again` from another
  sandbox, and the proxy recovered at the 21:40 restart. The host's connection table was gone
  after the restart, so the exact holders cannot be counted now.
- The message names a restart for every cause, including a token Yulius refuses because the run
  ended.

### Defect 2: shells and busy loops pile up

- What launches the `.jobs/job-*.sh` scripts: quartio-v2's own unit tests, not Claude Code
  (2.1.292 contains neither `.jobs` nor `sleep 0.05`) and not Yulius.
  `crates/quartio/src/sandbox/tools/tests.rs` has a `Local` backend whose `jobs_dir()` is
  `<tempfile::tempdir()>/.jobs`, giving `/tmp/.tmpXXXXXX/.jobs`. `Tools::start` writes the command
  to `job-<16 hex>.sh` and runs it detached through `sh -c "if command -v bash >/dev/null 2>&1;
  then b=bash; else b=sh; fi; $b \"$1/$2.sh\" …"`. The test backend spawns that child without
  `kill_on_drop` and never waits on it.
- Where the `sleep 0.05` loop comes from: the same tests start
  `while [ ! -f go ]; do sleep 0.05; done` as a background job
  (`background_commands_report_running_then_their_result`,
  `running_jobs_and_long_reads_never_show_part_of_a_secret` and the shift loop at line ~1129). If a
  test fails or is killed before it writes `go` (an assertion, `expect("the job wrote its output")`,
  or Claude Code's Bash timeout), its `TempDir` is dropped. `go` can then never appear and the job
  polls every 50 ms forever. Each such test run leaves one `sh` plus `bash` pair. `sleep 60` is
  quartio's hook-timeout test (`hooks.rs`), and `sleep 600` and `timeout 90 tail -f /dev/null` are
  agents' own commands. `node driver.mjs` is in neither repository; it is a script or package inside
  the sandbox.
- Why they outlive their turn: their parent (the test binary) exits, so the kernel hands them to
  the sandbox's PID-namespace init (bwrap). The background-shell probe skips everything that init
  adopted (SPEC-130 DMD-001, so that a preview server does not read Working for good), and nothing
  else looks at it. They live as long as the session. Long-lived sessions such as task deced849
  (armed merge, idle for hours) and the Superagent keep their bwrap for days.
- What already holds: when a run ends, is stopped, deleted or archived, the ptyhost kills the
  child's process group and session (`reap`, `signal_child`). In a contained run the death of the
  namespace init kills every process in the namespace, strays included.

## Requirements

- **LAP-001** `--scrub-exec` and the tool-result hook tell why the fingerprints are unavailable.
  When Yulius refuses the token (a 4xx), the message says that the session's run has ended
  (or the person may no longer work in the project) and that the person should close the session
  and continue from Yulius, and it does not say "restarting". When Yulius cannot be reached or does
  not answer in time, the message says so, says a restart is one cause, and names leftover
  processes using up the sandbox's connections as the other (with the operations guide). When
  Yulius answers with another error, the message gives the status. Each message also says what
  is wrong with the fallback file: there is none, it cannot be read, or it is not valid.
- **LAP-002** The egress proxy gives each sandbox at most half of its connections
  (`EgressConfig::max_per_sandbox`, default 128 of 256). A sandbox past its share is answered
  `503` saying that this sandbox's processes hold its share, and other sandboxes keep theirs. Half
  of a sandbox's 256 relay connections therefore stay free for its own calls to Yulius. Every
  refusal at a limit is logged as a warning (at most once a minute per limit and sandbox), and so
  is a relay that drops a connection at its limit.
- **LAP-003** The background probe also lists a run's strays: the processes the run's own
  namespace init adopted (a child of an init that is a child of the probe's root), other than the
  agent (the init's earliest-started child), each with its whole subtree and start times. An
  unsandboxed run has no such init and reports none, even below a namespace one of its commands
  made. An init with a child whose start time is unknown gives none in that probe.
- **LAP-004** An idle task's strays are logged as a warning when first seen. Once a stray has been
  seen for `stray_reap_after_ms` (default 2 h), Yulius kills its subtree, and logs it, at the next
  probe while the task is idle, checked again after the probe. A process is only signalled
  while its pid still has the recorded start time. Processes under the agent (its shells, a live turn's commands) are never strays.
- **LAP-005** `docs/operations.md` explains how an operator finds and clears leftover agent
  processes and saturated connections without a restart.

## Plan

- [x] Spec and ADR for leftover processes and connection shares
- [x] Failing regression tests for the message, the proxy share and the strays
- [x] The fingerprint message names its cause
- [x] The egress proxy keeps a share per sandbox and logs refusals at its limits
- [x] The probe lists strays and an idle task's old strays are reaped
- [ ] Operations guide, architecture, changes and every gate green
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- With a server that answers 401 and no fallback file, `--scrub-exec` exits 126 and says that the
  run has ended and to continue from Yulius, and it does not say "restarting".
- With a server that cannot be reached, the message names unreachability, a restart and leftover
  processes, and the state of the file.
- One sandbox holding its whole share of the proxy is refused `503` with a message about its share,
  while another sandbox's tunnel opens.
- A process table with the sandbox init, the agent and 60 stray job pairs lists 60 strays. The
  agent and its shells are not among them.
- An idle task whose probe reports strays reaps them once they are older than the bound, and a
  stray reused by another process (different start time) is not signalled.

## Test plan

- Unit (`yulius-harness`): `load_fingerprints` and the message against a one-shot local server
  (401, 500) and a closed port, with no file, an unreadable file and an invalid file.
- Integration (`yulius-egress/tests/proxy.rs`): raw CONNECT tunnels held open from two sandboxes.
- Unit (`yulius-ptyhost/src/shells.rs`): `strays` on tables shaped like the incident and like
  DMD-001, plus nested namespaces. Real processes: `reap_strays` kills a spawned process tree and
  skips an identity whose start time differs.
- Integration (`yulius/tests/strays.rs`): a session host that reports strays for a seeded idle
  task, with the test clock: reported first, reaped after the bound, a reused pid is a new stray, a
  working task's are left alone, a turn that starts during the probe keeps them.
- Real sandbox (`shells.rs`, skipped where bwrap cannot create namespaces): an orphan under
  `bwrap --unshare-pid` is found and reaped.

## Decisions

- ADR-0240 (strays of a live run are reaped after a bound; one sandbox gets at most half of the
  egress proxy): reaping strays at the end of a turn would kill preview servers that a person is
  still looking at (DMD-001). 2 h of idle bounds the waste of a busy loop to hours, not days.
  Splitting the internal API from the proxy onto another port would change every run's
  environment and the relay. A per-sandbox share protects both with one counter.
- The Superagent's fallback file is task #334's (SPEC-352). Not doing it twice.
- quartio-v2's leaking tests are reported on its repository, not fixed here.
