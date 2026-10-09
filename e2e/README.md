# Web UI end-to-end tests

End-to-end tests of the [web UI](../crates/yulius-server/web/README.md), written with
[e2e by TesterArmy](https://github.com/tester-army/e2e) (Apache-2.0, Playwright underneath).
The runner serves `crates/yulius-server/web` with Python's static file server and opens it in
mock mode (`?mock=1`), so no `yulius serve`, account or model is needed. Node is a
**test-only** prerequisite: the UI itself stays plain ES modules with no build step and no
Node at runtime (SPEC-002 ADR-01).

## Run

Node 24.8+ (`.node-version` pins the version CI uses) and Python 3:

```sh
cd e2e
npm ci --ignore-scripts
npx e2e-web install chromium     # once per Playwright version
E2E_TELEMETRY_DISABLED=1 npx e2e run
```

`npx e2e run tests/editor.e2e.ts` runs one file, `--headed` shows the browser. A failed test
writes a trace page (steps, screenshot, accessibility tree) under `.e2e/results/`. The runner
sends anonymous usage data unless `E2E_TELEMETRY_DISABLED=1` is set (or `npx e2e telemetry
disable`); CI sets it.

## What the suites check

| File | Flow |
| --- | --- |
| `navigation.e2e.ts` | Open a task from the sidebar, every sandbox page from the sandbox navigation, another sandbox from the tabs |
| `new-task.e2e.ts` | The new-task shortcut focuses the composer; Enter starts the task and opens it |
| `sign-in.e2e.ts` | Signed out: a wrong superadmin password is refused, the right one signs in |
| `shortcuts.e2e.ts` | The shortcut list opens from the keyboard and closes with Escape |
| `editor.e2e.ts` | A file from the tree opens in a selected editor tab; a folder expands |
| `terminals.e2e.ts` | The Shell panel shows a working terminal without a button; "+" opens another tab; tabs switch, rename (F2) and close (Delete); each sandbox has its own terminals |
| `phone.e2e.ts` | At 390 px: no horizontal scroll, the quick input, the navigation drawer |
| `quality.e2e.ts` | Every main page: no WCAG 2.x A/AA violations (axe-core) |
| `agent.e2e.ts` | An agent reaches a goal written in plain English (needs a model, see below) |

Every test also fails when the page reports an error: an uncaught exception, an unhandled
rejection or `console.error` (`failOnPageErrors()` in `tests/support.ts`). Prefer locators by
role and accessible name (`screen.getByRole('button', 'Start task')`): they test what assistive
technology sees, and they survive markup changes.

## Agent steps

`agent.act('…')` lets a model drive the UI towards a goal; `agent.assert('…')` lets it judge the
screen. With `ANTHROPIC_API_KEY` set, agent tests run against Anthropic (`E2E_MODEL` picks the
model); without it they are skipped. When a step passes and a later check confirms it, the
runner records the actions under `replays/`, and later runs replay them without model calls
until the UI changes. Commit `replays/` and review it like test data. CI gets no model key, so
agent tests are skipped there; giving CI a key would hand it to every pull request's code.
