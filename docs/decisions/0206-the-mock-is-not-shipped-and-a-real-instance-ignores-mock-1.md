# 0206. The mock is not shipped, and a real instance ignores `?mock=1`

## Status

Accepted. Specified in [SPEC-277](../../.ai/specs/SPEC-277-remove-this-mock-thingy.md); task #263.

## Context

`assets/mock.js` is the web UI's in-browser fake backend: the e2e suite and local UI work open the
UI from the source tree with `?mock=1`. `build.rs` embedded it with every other file under `web/`,
so every real instance served it, and `/?mock=1` there showed fake sandboxes of private Quartio
repositories, fake users and environments, to anyone who could reach the instance, signed out
included. Nothing on a real instance needs it.

## Decision

- `build.rs` leaves `assets/mock.js` out of the embedded asset table: the server answers 404 for it.
- The server redirects (303) a page request whose query has `mock=1` to the same path without that
  parameter, other parameters kept, so a bookmarked or shared demo link opens the real app instead
  of failing on the missing module. `/api/` and `/assets/` paths are not touched.
- The mock stays in `web/assets/` and keeps running the e2e suite unchanged, served by
  `e2e/serve.py` from the source tree.

## Alternatives

- **Delete the mock.** It is the e2e suite's backend (about 80 test files); the UI would lose its
  browser tests.
- **Move it under `e2e/`.** Structurally clearer, but the static server, the contract test, the
  repository checks and the docs would all need a second asset root for one file, for the same
  outcome as leaving it out of the build.
- **Rename the Quartio fixtures** (e.g. `acme/web`). Touches every test, and still ships a demo
  backend a customer has no use for.
- **A client-side fallback** (app.js catches the failed import and reloads without `mock`). It
  works, but loads the shell twice, and a broken `mock.js` during development would look like
  the real app instead of an error.

## Consequences

- A real instance carries about 5 400 fewer lines of JavaScript and shows no demo data.
- Seeing the mock needs a checkout (`python3 -m http.server` in `crates/yulius-server/web`, see
  its README) or the e2e suite; `yulius serve` cannot show it.
- `mock.js` still lives under `web/`, so `scripts/check_repository.py`, the contract test and the
  syntax checks keep reading it as before.
