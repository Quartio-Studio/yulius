# 0122. The e2e server keeps up with a page under load, and Monaco is used once it has loaded

## Status

Accepted. Specified in [SPEC-148](../../.ai/specs/SPEC-148-fix-354-349-356-394-the-e2e-suite-holds.md);
task #134, issues #354, #349, #356 and #394. Keeps [ADR-0039](0039-ci-jobs-get-the-cpu-first.md)
(CI's CPU weight) and SPEC-082's worker bound.

## Context

On the shared VM under load (load average 40–78) the e2e suite failed first-load waits with no
product defect: headings, regions, the notes' save state, xterm rows, the Editor's first tab.
- The suite served the UI with `python3 -m http.server`. Its listen queue holds 5 connections, and
  it answers HTTP/1.0, closing the connection after every answer. A page opens about 75 modules at
  once and Monaco about 120 more files. When the loaded machine does not schedule the server's
  accept loop in time, the kernel drops the SYNs beyond the queue and the browser tries again
  after 1 s, then 3 s. A passing full run at load 35–53 overflowed the queue 491 times.
- Monaco threw "depends on UNKNOWN service ICodeLensCache" (and four more services) when the
  Editor opened under load. Monaco's main chunk sets `window.monaco` before the chunk that
  registers those services has run, and the first call into its API fixes the services every
  editor gets. The editor view's loader handed out that half-loaded `window.monaco` to a second
  file opened during the load, and tests polled `monaco.editor.getEditors()` during it.

## Decision

- The runner serves the UI with `e2e/serve.py`: Python's static file handler over HTTP/1.1
  keep-alive, with a listen queue of 1024 connections (the kernel caps it at
  `net.core.somaxconn`). It serves the same files with the same answers.
- `loadMonaco()` resolves only when all of `editor.main` has run; `window.monaco` is no sign of
  that. An editor view makes its one editor once, however many files are opened while Monaco loads.
- Tests touch `window.monaco.editor` only after the Editor shows a Monaco editor
  (`editorShown`). The Editor's first opening in a test is waited for with the cold load's own
  budget (`MONACO_LOAD_MS`, 60 s, which two files already used); every other wait keeps the
  runner's default budget.
- No retries and no general budget increase: a red run stays a failure.

## Alternatives

- **Longer budgets or retries.** They hide the stall instead of removing it, slow every real
  failure, and were removed on purpose before (SPEC-053).
- **CPU priority for the suite's browser and server.** Raising a process's priority needs
  `CAP_SYS_NICE`, which a task sandbox has not; CI already runs in the weighted `ci` leaf
  (ADR-0039). Priority would not have stopped the dropped SYNs either: the accept loop still has
  to run before the queue overflows.
- **Fewer workers.** Workers are already bounded by the session's CPU budget (SPEC-082). Fewer
  would make each run longer and leave the server's queue of 5 as small.
- **A Node or Rust static server.** Node is test-only and would need a dependency or more code;
  `yulius serve` needs a build and an account. Python's handler with two settings changed is the
  smallest change that removes the cause.
- **Filter Monaco's errors as vendor noise (#394's option).** They came from our loader and our
  tests calling Monaco early; a filter would also hide a real broken editor.

## Consequences

First loads on a loaded machine no longer wait out SYN retransmissions, and a Monaco chunk is no
longer refused by an overflowing queue. Idle keep-alive connections hold a server thread each until
the browser closes them, which the runner's one browser per worker bounds. A test that adds a
Monaco call must wait for `editorShown` first. Opening two files while Monaco loads creates one
editor; which of them is in front still depends on which file's read answers last.
