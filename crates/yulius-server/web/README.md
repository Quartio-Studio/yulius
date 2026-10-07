# Yulius web UI

Static single-page UI for `yulius serve`. It implements the workspace interface from
[SPEC-001 §6](../../../.ai/specs/SPEC-001-yulius-product.md) against the HTTP/WebSocket
contract in [docs/api.md](../../../docs/api.md). Per SPEC-002 ADR-01 it is plain ES modules
with vendored xterm.js: no framework, no build step and no Node toolchain at runtime.

## Serving contract (for the Rust server)

- Serve `index.html` at `/` **and** for every UI path (history API routing): `/setup`,
  `/signin`, `/device`, `/archived`, `/admin/*` and `/s/*` — except the reverse-proxied
  editor under `/s/{sid}/editor/` and anything under `/api/`.
- Serve `assets/**` at `/assets/...` with the usual content types (`.js` and `.mjs` as
  `text/javascript`, `.css`, `.png`). Assets are referenced by absolute `/assets/` paths.
- Nothing is loaded from other origins: no CDNs, fonts or external images. A strict
  `Content-Security-Policy` such as `default-src 'self'; img-src 'self' data: blob:;
  style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-src 'self'` works (inline
  `style` attributes are used for dynamic widths; there are no inline scripts).
- `README.md` files are documentation and need not be embedded.

## Layout of the code

| File | Responsibility |
| --- | --- |
| `index.html` | Page shell; loads `assets/styles.css`, `assets/vendor/xterm/xterm.css` and `assets/app.js` (`type=module`). |
| `assets/app.js` | Entry point: boot (setup → sign-in → workspace), route dispatch, composition of the layout, link handling. |
| `assets/api.js` | HTTP client (`X-Yulius-CSRF` on every non-GET, `{"error":{code,message}}` → `ApiError`, 30 s timeout) and the transport abstraction. |
| `assets/router.js` | Routes and links; history API paths, hash paths in mock mode; recognizes Yulius links. |
| `assets/store.js` | State store (sandboxes, tasks per sandbox, browser action requests) and the `/api/events` client. |
| `assets/termsocket.js` | WebSocket protocol code: binary frame parsing, `ReconnectingSocket` (backoff 0.5–10 s, bounded attempts, 4008 → immediate resync), `TerminalSocket`. |
| `assets/terminal.js` | xterm view: fit/resize, controller handover, read-only banner, reconnect/exit banners, web links and OSC 8 links. |
| `assets/layout.js` | Sandbox tab row (+, Status panel, user menu) and the sidebar (navigation, active task list). |
| `assets/taskview.js` | Task view (step strip, terminal, phone quick-input bar) and the new-task composer. |
| `assets/panels.js` | Right rail and resizable panel: Editor (iframe), Shell, Messages; hosts the browser panel. |
| `assets/browser.js` | Sandbox browser panel: tabs, address bar, JPEG canvas, mouse/keyboard forwarding. |
| `assets/actions.js` | Task actions and dialogs: stop, continue, switch, feedback, delivery, deploy runs, promote, sandbox add/remove. |
| `assets/status.js` | Status → glyph/tone/label mapping and permitted actions per access level. |
| `assets/dom.js`, `assets/format.js` | DOM builder (text only, never `innerHTML` with data), icons, toasts, modals, menus; formatting helpers. |
| `assets/pages/*.js` | Pages: auth (setup, sign-in), tasks (+ archived, device approval), git, workflows, skills, automations, MCP, dashboard, admin (users, accounts, GitHub). |
| `assets/mock.js` | In-memory backend and fake WebSockets used with `?mock=1`. |
| `assets/styles.css` | Design tokens (CSS variables) and all layout, including the phone layout (< 768 px). |
| `assets/vendor/` | Vendored xterm.js; see [vendor/README.md](assets/vendor/README.md). |

## Routes

| Path | View |
| --- | --- |
| `/setup` | Setup wizard (token, password twice, two non-recoverable-password confirmations). |
| `/signin?next=…&error=…` | Sign in with GitHub (`/api/auth/github/start?next=`) or the superadmin password. |
| `/` | Redirects to the last used sandbox; empty state with Import/Create when there is none. |
| `/s/<slug>` | New-task composer. |
| `/s/<slug>/t/<number>` | Task: step strip + native terminal. |
| `/s/<slug>/browser?tab=<id>` | Opens the browser panel on that tab (Yulius links, BRW-004). |
| `/s/<slug>/tasks`, `/git`, `/workflows`, `/skills`, `/automations`, `/mcp`, `/dashboard` | Sandbox pages. `skills?source=instance` and `mcp?scope=instance` are the admin "Instance skills/MCP" entries. |
| `/archived` | Archived tasks across sandboxes, with restore. |
| `/device?code=<user_code>` | CLI device approval. |
| `/admin/users`, `/admin/accounts`, `/admin/github` | Administration (superadmin / instance admin). |

## Run the mock locally

```sh
cd crates/yulius-server/web
python3 -m http.server --bind 127.0.0.1 8765
# open http://127.0.0.1:8765/?mock=1   (stop the server with Ctrl-C afterwards)
```

`?mock=1` replaces the HTTP client and every WebSocket with `assets/mock.js` and switches
routing to `#/…` paths so a plain static server can reload deep links. The protocol code
(request building, error parsing, binary frames, reconnect) is the same code used against
the real server. Options (combine with `&`): `setup=1` (setup required; token
`YUL-SETUP-1234`), `signedout=1` (password `correct horse battery`), `role=member|admin`,
`contained=0` (shows "Not contained"), `github=0` (GitHub App not configured).

The mock has sandboxes with admin, write and read access, tasks in every status, steps,
messages, accounts with usage, Git data, workflows, skills, automations, MCP servers and
analytics. Task terminals print TUI-like ANSI output, echo input and answer on Enter; type
`/exit` to close a task, `/lag` to force a 4008 reconnect, `/help` for the list. The
browser panel renders fake pages as JPEG frames (click "Sign in" on the AWS tab).

Syntax check without a browser: `node --check` on each file (Node 24 detects ES modules).

## Assumptions about unspecified response shapes

docs/api.md does not define these bodies; the UI renders them defensively (missing fields
show "—", never zero) and the integrator should confirm or adjust:

- `GET /api/status`: `{"vm": {"cpu_percent", "cpu_count", "memory_used_bytes",
  "memory_total_bytes", "disk_used_bytes", "disk_total_bytes", "load": [1, 5, 15],
  "uptime_s"}, "sessions": {"live", "limit", "queued"}, "sandboxes": [{"id", "name",
  "live_sessions", "disk_bytes"}], "accounts": [Account]}`.
- `GET /api/analytics/summary?sandbox&from&to` (dates `YYYY-MM-DD`): `{"tasks": {"total",
  "by_status": {status: n}, "by_harness": {harness: n}}, "tasks_per_day": [{"date",
  "count"}], "duration_ms": {"median", "p90", "total_active", "total_wall"}, "tokens":
  {"input", "output", "cache_read", "cache_write"}, "tokens_by_model": [{"model", "input",
  "output"}], "tokens_by_harness": [{"harness", "input", "output"}], "tokens_by_account":
  [{"account_label", "input", "output"}], "switches": {"total", "account", "provider"},
  "feedback": {"correct", "incorrect"}}`. Export links add `sandbox`, `from`, `to` to
  `/api/analytics/tasks?format=csv|json`.
- `GET /api/admin/users`: `[{"id", "github_login", "display_name", "is_instance_admin",
  "disabled", "is_superadmin", "last_seen_at"}]`.
- Delivery runs: `state` values `running` → `succeeded | failed | error | cancelled`.
- Browser input: `button` is `left|middle|right|none`; `modifiers` is the CDP bitmask
  (Alt 1, Ctrl 2, Meta 4, Shift 8); printable keys send `event: "down"` with `text`,
  paste sends one `event: "char"` with the pasted `text`. The `action_requested` frame is
  read as `{"type", "tab_id", "reason", "task_id"}`.
- Instance-scope MCP servers and workflows are created through the sandbox endpoints with
  `"scope": "instance"` (instance admins only). MCP updates omit `env`/`headers` to keep
  stored secrets; sending them replaces all values of that map.
- `POST /api/sandboxes/{sid}/shells` is sent without a body; the editor iframe opens a
  task worktree with `?folder=<worktree>` (openvscode-server convention).
