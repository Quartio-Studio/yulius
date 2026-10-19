---
yulius-task: dc274028-c9f6-4710-b429-8bcdb5ef4607
status: in-progress
---

# SPEC-235: A Monaco worker ended while its script loads raises no uncaught error

Status: in-progress
Date: 2026-10-19
Owner: task #221
Scope authorization: task #221, "Fix GitHub issue #509". Fixes #509. Builds on SPEC-127 (the
Editor's presence, the test that failed) and SPEC-148 (the e2e suite under load, `serve.py`).

## Goal

Closing the last file, switching folder or leaving the Editor while Monaco's editor worker is still
loading its script reports no uncaught error on the page. A worker that fails while it is still in
use stays an uncaught error, and `failOnPageErrors()` stays strict: no allowlist.

## Context

The symptom (#509, CI run `6e5b9a59`, and once in #523's loaded proof): `editor.e2e.ts` › "a file
closed, a folder switched or a page gone says the person left, even with an update on its way"
passes its own steps, then `failOnPageErrors()` finds

```
uncaught: Uncaught NetworkError: Failed to execute 'importScripts' on 'WorkerGlobalScope': The script at 'http://127.0.0.1:<port>/assets/vendor/monaco/vs/assets/editor.worker-_vAIFJDs.js' failed to load.
```

The code path (vendored Monaco, AMD build):

1. `editor.main.js` sets `self.MonacoEnvironment.getWorker`, which makes every worker a classic
   `new Worker(blob)` whose script is `importScripts(<worker url>)` followed by a ready message.
2. Opening a file makes a model and an editor; Monaco's `EditorWorkerService` starts the editor
   worker for it (`withWorker()`), and the worker begins loading `editor.worker-*.js` (275 KB).
3. Closing the last file (`editorview.js` disposes its model), switching folder or leaving the
   Editor disposes every model; Monaco's `_checkStopEmptyWorker` then disposes the worker client,
   which calls `worker.terminate()`, also while the worker is still in `importScripts`.
4. Ending the worker cuts its script's load: `importScripts` throws a NetworkError inside the
   worker, and the browser fires an `error` event at the page's `Worker` object. Monaco's client
   (`kJ`) only forwards it and never calls `preventDefault()` (and removes its listener before
   `terminate()` anyway), so the browser reports the error on `window` as uncaught.

The failing test closes a file, switches folder and hides the page within seconds of each file
opening, three new workers in a row. On a loaded machine the worker script takes longer to arrive
than the gap between opening a file and closing it, so the cut lands mid-load. The other editor
tests keep their file open, so their worker finishes loading.

Reproduced (task #221):

- a probe page that loads Monaco, starts `MonacoEnvironment.getWorker(..., 'editorWorkerService')`
  and calls `terminate()` while a test server holds `editor.worker-*.js` (before the headers, or
  after half the body) gives exactly the error above on `window` at every delay inside the hold,
  and none once the script has loaded;
- in the app, `browser.route` holding `editor.worker-*.js` and the person closing README.md gives
  the same `afterEach` failure as CI, with every step of the test passing.

Why the tests missed it: no test held the worker script, and a quiet machine loads it before a
test closes its file. #523 tried holding it 8 s at the server and saw no error; why is not
recorded (an earlier worker from the HTTP cache, or the close landing outside the hold, could
explain it). A server reset is not needed to explain the CI failure: an ended worker reproduces it
every time.

What else the cause reaches: every Monaco worker goes through the same `getWorker`, so a language
worker (JSON, CSS, HTML, TypeScript) ended while it loads (Monaco stops idle language workers)
would do the same, and so would a person's browser closing a file on a slow network. Only the
console sees it there; the product loses nothing, as the worker was no longer wanted.

## Requirements

- R1: An `error` event from a Monaco worker that the page has already ended is cancelled
  (`preventDefault()`), so it is not reported on `window`.
- R2: An error from a worker the page has not ended still reaches `window` as uncaught.
- R3: `failOnPageErrors()` and `support.ts` are unchanged: no allowlist.
- R4: The Editor still works after such an ended worker: the next file starts a new worker.

## Plan

- [x] Root cause reproduced and spec written
- [ ] Regression test: an editor worker ended while its script loads (fails on dev)
- [ ] Ended Monaco workers raise no uncaught error
- [ ] Loaded proof: editor suite repeated at a 0.2 CPU share
- [ ] Gates, docs and independent review
- [ ] Delivered: pull request merged into dev on green

## Acceptance criteria

- The new e2e test holds `editor.worker-*.js`, closes the file while it is held, sees the worker's
  NetworkError happen (a test-side listener on each `Worker`), and fails on `dev` in `afterEach`
  with the error of #509; it passes with the fix.
- The test then opens a file again, the held script is released, and a new editor worker loads.
- `editor.e2e.ts` passes `--repeat-each` at a held 0.2 CPU share.

## Test plan

- `e2e/tests/editor.e2e.ts`: "an editor worker ended while its script loads raises no error
  (#509)": route-held worker script, close the file, wait for the worker's error event at the
  `Worker` (positive condition, no sleep), release; open the file again and wait for the new worker
  to be ready. `failOnPageErrors()` is the assertion that fails without the fix.
- Loaded proof: `editor.e2e.ts --repeat-each` with a controller holding a 2 s CPU-bound probe at a
  0.20 share (SPEC-148 method), logged.
- The full e2e suite and `scripts/dev ci`.

## Decisions

- **Cancel the errors of an ended worker, in `editorview.js`.** `loadMonaco` wraps
  `MonacoEnvironment.getWorker`: each worker it returns notes when `terminate()` is called and
  cancels its `error` events from then on. Chosen because the error is the expected end of a
  load the page itself cut, and an ended worker's errors can mean nothing to the person; errors of
  a live worker still surface (R2).
- Rejected: an allowlist in `support.ts` (hides real errors, against the issue); making the test
  wait for the worker before closing (the product would still report an error for a person who
  closes a file quickly, and every other test closing files would need it); cancelling every
  worker load error by its message (would hide a worker script that really fails to load);
  patching the vendored Monaco bundle (lost on the next update).
