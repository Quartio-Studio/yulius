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
| `sign-in.e2e.ts` | Signed out: a wrong superadmin password is refused, the right one signs in (to the home screen) |
| `access.e2e.ts` | Without a GitHub link the superadmin sees every sandbox (home cards and tabs); an instance admin sees none and is asked to sign in with GitHub |
| `shortcuts.e2e.ts` | The shortcut list opens from the keyboard and closes with Escape |
| `editor.e2e.ts` | A file from the tree opens in a selected editor tab; a folder expands; word wrap turns on and off from the toolbar and Alt+Z without typing, the open editor has no WCAG violations, and wrapping stays after a reload |
| `browser.e2e.ts` | The browser panel streams the sandbox Chrome (mock: a drawn page as VP8 video or JPEG frames) onto its canvas with the tab's address; keys typed after a click reach the page; the agent's sign-in tab asks for action and the click signs in; a new tab opens the welcome page; no WCAG violations |
| `filedrop.e2e.ts` | Files dropped on an explorer folder show where they go and are added there; dropping an existing file asks (Keep existing, Replace); a text file dropped on the editor opens; files dropped on the task terminal and a pasted image are typed in by path, a text paste stays text; a read-only terminal refuses drops |
| `terminal.e2e.ts` | The task terminal: the clipboard key table and the file-path forms (unit, in the page); Ctrl+C copies a selection and interrupts without one; paste types, a read-only viewer's paste is refused; the right-click menu; another viewer's size fills the box with a chip until a click takes it; file paths in the task terminal and a shell's output open the editor at the file and line |
| `terminals.e2e.ts` | The Shell panel shows a working terminal without a button; "+" opens another tab; tabs switch, rename (F2) and close (Delete); each sandbox has its own terminals |
| `rail.e2e.ts` | The tools rail: items and counts inside the rail with one open tool (and no WCAG violations); the rail stays in the window when it narrows with a panel open; at 390 px it sits in the top bar without covering it |
| `messages.e2e.ts` | The Messages panel lists messages between tasks and the agent's question to people, has no text box or Send button (and no WCAG violations); "Reply in the terminal" focuses the task's terminal |
| `git.e2e.ts` | A GitHub App permission Yulius lacks is one quiet line on the Git page for administrators, with a link to the App's permissions, no warning banner, hidden for the session once dismissed (also after a reload); members don't see it. The Git page, a task's delivery details and its strip show state without merge, release, deploy, verify or open-PR buttons (no WCAG violations) |
| `mcp.e2e.ts` | With every server removed, the empty state's Add server opens the add page (the reported "nothing happens"); a server added from the catalog opens tested, an edited command is listed and read back, Remove removes it |
| `workflows.e2e.ts` | The defaults come first, read-only (no Delete, no Save, a banner) with when to use them; Duplicate opens a custom copy with the same steps, which Create adds to Custom |
| `skills.e2e.ts` | Skills shows the included set (with the Open Mercato thank-you), then custom skills, without tabs or search; an included skill opens read-only |
| `phone.e2e.ts` | At 390 px: no horizontal scroll, the quick input, the navigation drawer |
| `home.e2e.ts` | `/` is the home screen: a card per sandbox with its repository, counts and last activity; a card's New task focuses that sandbox's composer, Open opens it; deep links still open directly; the logo, the All sandboxes tab and ⌘⇧H / Ctrl+Shift+H go home (listed in the sheet); with no sandbox, Import and Create; no WCAG violations |
| `layout.e2e.ts` | The sidebar, the editor's explorer and the tool panel resize by dragging their edge, by keyboard (sidebar) and back with a double-click, and keep their widths after a reload; at 1024 px with a panel open the sidebar gives way so the centre keeps 360 px; no splitters at 390 px |
| `responsive.e2e.ts` | Every main page at 768 and 390 px: no two header, toolbar or list controls overlap (bounding boxes) and nothing scrolls sideways; Tasks with a tool panel open wraps its state tabs; a task beside an open Editor, Browser or Shell keeps its step strip and the panel head apart; at 1920 px every page is centred (no empty band on the right) |
| `task-done.e2e.ts` | A task whose goal is achieved is Done with a green check in the header, the sidebar and on Tasks (no WCAG violations); one with a background shell still running stays under Working with "1 background shell still running" |
| `quality.e2e.ts` | Every main page: no WCAG 2.x A/AA violations (axe-core) |
| `subscriptions.e2e.ts` | The accounts page shows the pool's capacity, burn rate, forecast and recommendation with only add, sign-in-again and remove (no priority or on/off controls); a paused task reads "Paused: waiting for subscription capacity (resets ~…)"; the dashboard shows the subscriptions card; no WCAG violations |
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
