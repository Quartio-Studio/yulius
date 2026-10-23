---
yulius-task: 363f9443-870a-4d86-8245-a4805507ca71
status: in-progress
---

# SPEC-277: Demo mode (`?mock=1`) is no longer part of a real instance

Status: in-progress
Date: 2026-10-23
Owner: task #263
Scope authorization: task #263 (requested by MrFishPL): "remove this mock thingy", about demo
mode (`/?mock=1`), which shows fake `Quartio-Studio/studio` and `Quartio-Studio/product`
sandboxes on any instance, even to a signed-out visitor. Decision: ADR-0206.

## Goal

A customer's instance shows nothing of the demo backend: `assets/mock.js` is not in the server
binary, and opening any page with `?mock=1` on a real instance opens the real app. The mock stays
what the e2e suite and local UI work run on, served from the source tree.

## Context

- `crates/yulius-server/build.rs` embeds every file under `web/`, `assets/mock.js` (about 5 400
  lines of fake sandboxes, tasks, users and repositories) included. `assets.rs` serves it to
  anyone, signed in or not.
- `app.js` loads it with `import('./mock.js')` when the URL has `mock=1`, so `/?mock=1` on a
  real instance shows the fake Quartio data under the customer's own address.
- The e2e suite (`e2e/`, about 80 files) and local UI work run the UI from the source tree with
  `e2e/serve.py` or `python3 -m http.server`, never `yulius serve`, and open it in mock mode. The
  UI contract test (`crates/yulius/tests/ui_contract.rs`) reads `mock.js` from disk to check it
  fakes only real routes.
- Deleting the mock would delete the e2e suite; renaming its fixtures (e.g. `acme/web`) touches
  every test and still ships a demo backend nobody uses on a real instance.

## Requirements

- MCK-001: the server binary embeds no `assets/mock.js`: `GET /assets/mock.js` answers 404.
- MCK-002: a page request (any path outside `/api/` and `/assets/`, `index.html` included) whose
  query has `mock=1` is redirected (303) to the same path with that parameter removed and every
  other parameter kept in order, so the real app opens instead of a broken mock start.
- MCK-003: the mock keeps working where the UI is served from the source tree (e2e suite, local
  UI work); the contract test still checks it fakes only real routes.

## Plan

- [ ] Spec and ADR: demo mode is test-only
- [ ] Server binary no longer serves mock.js (contract test first)
- [ ] ?mock=1 on a real instance drops to the normal app (redirect, test first)
- [ ] Docs, changes and gates green
- [ ] Independent review and delivery

## Acceptance criteria

- `GET /assets/mock.js` from the real router is 404; every other module the UI imports is still
  served with its content type.
- `GET /?mock=1` redirects to `/`; `GET /s/x/tasks?a=1&mock=1&b=2` to `/s/x/tasks?a=1&b=2`;
  `GET /index.html?mock=1` to `/index.html`; `/api/...?mock=1` is untouched.
- The e2e suite passes unchanged.

## Test plan

- `ui_contract.rs`: the import test expects `/assets/mock.js` to be 404 and every other import
  200; a new test drives the redirect cases above, written first and failing before the change.
- The full gate (`scripts/dev check`) including the e2e suite, which runs on `mock.js` from disk.

## Decisions

- ADR-0206: the mock is a test fixture served from the source tree, never by the server; the
  server answers `?mock=1` with a redirect rather than leaving the app to fail on a missing
  module. Alternatives (delete the mock, move it under `e2e/`, rename the Quartio fixtures, a
  client-side fallback) are in the ADR.
