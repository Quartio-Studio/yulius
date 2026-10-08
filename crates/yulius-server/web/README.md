# Yulius web UI

Static single-page UI for `yulius serve`. It implements the workspace interface from
[SPEC-001 §6](../../../.ai/specs/SPEC-001-yulius-product.md) against the HTTP/WebSocket
contract in [docs/api.md](../../../docs/api.md). Per SPEC-002 ADR-01 it is plain ES modules
with vendored xterm.js: no framework, no build step and no Node toolchain at runtime.

## Serving contract (for the Rust server)

- Serve `index.html` at `/` **and** for every UI path (history API routing): `/setup`,
  `/signin`, `/device`, `/archived`, `/settings`, `/design`, `/admin/*` and `/s/*` — except anything under `/api/`.
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
| `index.html` | Page shell; loads `assets/vendor/xterm/xterm.css`, `assets/tokens.css`, `assets/styles.css` and `assets/app.js` (`type=module`). |
| `assets/app.js` | Entry point: boot (setup → sign-in → workspace), route dispatch, composition of the layout, link handling. |
| `assets/api.js` | HTTP client (`X-Yulius-CSRF` on every non-GET, `{"error":{code,message}}` → `ApiError`, 30 s timeout) and the transport abstraction. |
| `assets/router.js` | Routes and links; history API paths, hash paths in mock mode; recognizes Yulius links. |
| `assets/store.js` | State store (sandboxes, tasks per sandbox, browser action requests) and the `/api/events` client. |
| `assets/termsocket.js` | WebSocket protocol code: binary frame parsing, `ReconnectingSocket` (backoff 0.5–10 s, bounded attempts, 4008 → immediate resync), `TerminalSocket`. |
| `assets/terminal.js` | xterm view: fit/resize, controller handover, read-only banner, reconnect/exit banners, web links and OSC 8 links. |
| `assets/layout.js` | Sandbox tab row (+, Status panel, notifications, tools toggle) and the sidebar (wordmark, navigation, active task list, user menu at its foot). |
| `assets/notifications.js` | Notification center (UI-020): the bell, its panel, and entries derived from the event stream; kept per user in `localStorage`, bounded to 100. |
| `assets/taskview.js` | Task view (step strip, terminal, phone quick-input bar) and the new-task composer (rounded prompt box with mode, workflow, model and effort pickers). |
| `assets/quotes.js` | 40 quotes about building great things (well-documented authors only); one shows at random above the composer. |
| `assets/models.js` | Models (grouped by provider, each naming its harness) and effort levels the composer offers. |
| `assets/panels.js` | Right rail and resizable panel: Editor (iframe, task worktree via `?worktree=`), Shell (sandbox or task worktree), Messages; hosts the browser panel. |
| `assets/browser.js` | Sandbox browser panel: tabs, address bar, JPEG canvas, mouse/keyboard forwarding. |
| `assets/actions.js` | Task actions and dialogs: stop, continue, switch, feedback, delivery (PR, merge, environment runs and confirmations, promotion), sandbox add/remove. |
| `assets/status.js` | Status → glyph/tone/label mapping and permitted actions per access level. |
| `assets/dom.js`, `assets/format.js` | DOM builder (text only, never `innerHTML` with data), icons, toasts, modals, menus; formatting helpers. |
| `assets/pages/*.js` | Pages: auth (setup, sign-in), tasks (+ archived, device approval), git, workflows, skills, automations, MCP, dashboard, admin (users, accounts, GitHub), settings. |
| `assets/mock.js` | In-memory backend and fake WebSockets used with `?mock=1`. |
| `assets/brand.js` | Brand identity in one place: product name, tagline and mark (the SVG path used by the sidebar, sign-in, favicon and tab title). |
| `assets/theme.js` | Theme choice (dark, light, system) per browser: sets `data-theme`, the colour-scheme and theme-color metas, and re-colours terminals and editors from the tokens. |
| `assets/tokens.css` | Design tokens: brand and palette, colour roles per theme (dark, light), derived tints, and the type, control, radius, elevation and motion scales. |
| `assets/styles.css` | Components and layout (including the phone layout < 768 px), using only roles and scales from `tokens.css`. |
| `assets/vendor/` | Vendored xterm.js; see [vendor/README.md](assets/vendor/README.md). |
| `assets/logos/` | Logos of well-known services shown on the MCP page; sources in [logos/README.md](assets/logos/README.md). |

## Theming and branding

Everything visual is driven by tokens, so a new brand, palette or theme is a token edit — never a
component change. See it live at **`/design`** (Settings → Design system), which renders every token
and component in the current theme and computes contrast as you change things.

### Rebrand in three steps

1. **`assets/brand.js`** — product `name`, `tagline` and `mark` (SVG path data on a 24-unit grid).
   The sidebar wordmark, sign-in/setup screens, browser tab title and favicon all follow.
2. **`assets/tokens.css`, Layer 1** — the brand ramp `--brand-200 … --brand-800` and `--brand-ink`
   (text on the brand colour). Dark theme uses 300–500, light theme 600–800. Optionally adjust the
   state palette (green, amber, red, violet) the same way.
3. **Check** — open `/design` in both themes, then run `scripts/dev check`: it fails if any text or
   control edge in any theme drops below WCAG contrast (text 4.5:1; state marks, focus, control edges
   3:1) and lists the failing pairs.

### How the tokens are layered (`assets/tokens.css`)

| Layer | What | Who edits it |
| --- | --- | --- |
| 1 · Brand and palette | Raw hues: `--brand-*`, `--green-*`, `--amber-*`, `--red-*`, `--violet-*` | Rebranding |
| 2 · Roles per theme | `:root, [data-theme="dark"]` and `[data-theme="light"]`: surfaces, lines, ink, accent, states, data, overlays, terminal and ANSI colours | New theme or a role tweak |
| 3 · Derived roles | `--accent-soft`, `--warn-border`, `--danger-track`… mixed from a role with `color-mix()`, so tints follow the brand and the theme | Rarely |
| Scales | Type, control heights, radii, elevation, focus, icon stroke, motion, layout | Design-system changes |

Components (`styles.css`, page modules) use **roles and scales only**. To add a theme, add a
`[data-theme="name"]` block that sets the Layer 2 roles and a choice in `theme.js`; terminals and the
editor re-colour from the same tokens (`theme.js` → `terminalColors()`, the Monaco theme).

### Scales

| Scale | Tokens | Rule |
| --- | --- | --- |
| Type | `--fs-xs/sm/(base)/md/lg/xl/2xl` with `--lh-*`: 11/16, 12/16, 13/20, 14/22, 16/24, 20/28, 24/32 | Captions, secondary, body, prose/item titles, section titles, page titles, display numbers; weights 400/500/600 only; `--font` and `--font-mono` only |
| Controls | `--control-sm/md/lg/xl`: 24, 28, 32, 40 | Small actions; default buttons, inputs, rows; top bar, pickers, nav; prominent and touch |
| Radii | `--radius-sm` 4, `--radius` 6, `--radius-lg` 10, `--radius-xl` 16, `--radius-full` | Small things; controls; containers; the composer and sign-in card; pills (circles use 50%) |
| Spacing | 2, 4, 6, 8, 12, 16, 20, 24, 32, 40, 48 px | Padding, margin, gap (a 1 px margin may nudge an icon) |
| Elevation | `--shadow-sm`, `--shadow`, `--shadow-raised`, `--focus` | L1 cards use borders only; L2 menus/popovers/toasts; L3 modals |
| Motion | `--duration-fast` 120 ms, `--duration` 200 ms, `--duration-slow` 300 ms, `--ease` | One rule turns all motion off under `prefers-reduced-motion` |
| Icons | `--icon-stroke` 1.4 px | Same optical stroke at every size (non-scaling) |

### What the checks enforce

`scripts/check_repository.py` (`scripts/dev check`) rejects in `assets/styles.css`: colour literals
(hex, `rgb()`/`hsl()`/…, named colours), font sizes, line heights, families and weights outside the
type scale, radii outside the tokens, padding/margin/gap off the spacing scale, and literal transition
timings. It resolves `tokens.css` per theme and checks the contrast of every text/background role
pair and of state marks and control edges. Control heights are a convention, not checked yet.

### Components

Shared building blocks live in `assets/dom.js`: `iconButton`, `toggle`, `badge`, `iconBadge`,
`statusMark` (task state icon + tone + label), `itemCard` (clickable list card with icon, badges,
description, controls), `meter`, `openMenu` (icons, descriptions, check marks), `openModal`,
`toast`. Prefer them to new one-off markup; anything new should appear on `/design`.

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
| `/settings` | Settings: your account, this browser's preferences, administration tiles (user menu → Settings). |
| `/design` | Design system: brand, colour roles with live contrast, type, spacing, radii, controls, elevation, motion, icons, task states and components, in the current theme (Settings → Design system). |
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

The mock mirrors the real response shapes (it is the UI's offline test bed and
documentation). It has sandboxes with admin, write and read access (`studio` with the Studio
preset, `product` with a custom profile whose `dev-vm` environment has a command, read-only
`sandbox-3` whose Git page shows a degraded-read note), tasks in every status, steps,
messages, accounts with usage (some unknown), Git data and delivery stages, environment runs,
workflows, skills, automations, MCP servers and analytics. Importing a repository creates a
sandbox whose onboarding is `pending` with a resting preparation task (Start preparation /
Skip). Task terminals print TUI-like ANSI output, echo input and answer on Enter; type
`/exit` to close a task, `/lag` to force a 4008 reconnect, `/help` for the list. The
browser panel renders fake pages as JPEG frames (click "Sign in" on the AWS tab).

## Contract with the server

The source of truth for paths and methods is the route table in
`crates/yulius-server/src/routes/mod.rs`; shapes are the DTOs in
`crates/yulius-contracts/src/api/` (documented in [docs/api.md](../../../docs/api.md)).

- **Checked automatically.** `crates/yulius/tests/ui_contract.rs` reads these sources at test
  time: every `/api/...` literal (HTTP calls, WebSocket `path:`s, links) must reach a real route with the method used (`api.get|post|put|patch|del`; WebSocket
  paths and links are `GET`), every route `assets/mock.js` fakes must exist on the server,
  every imported module must be embedded, and UI routes must fall back to `index.html`.
  Keep API paths **literal** at the call site (``ctx.api.post(`/api/tasks/${enc(id)}/stop`)``):
  an interpolation may only fill a whole path segment, never choose the endpoint.
- **Known gaps.** `GET /api/admin/github` and `POST /api/admin/github/manifest` are documented
  but not routed yet; the GitHub page then shows the connection facts from `/api/me`
  (`instance.github_configured`, `owner_login`). The test keeps them in an allowlist that
  fails once the routes exist.
- **Events.** The server sends `resync` (not `sandbox.updated`) after sandbox changes and does
  not push sandbox counters; the store reloads `/api/sandboxes` on `resync` and once
  (debounced) after task events, and leaves a sandbox that disappeared.
- **Unknown is not zero.** Usage, token and VM figures that the server reports as `null` render
  as "—" or "unknown"; analytics show runs without usage separately.
- **Errors.** `{"error": {code, message}}` becomes `ApiError` (`status`, `code`); bodies without
  it fall back to the status (`503` → `unavailable`). The editor panel (`editorview.js`, Monaco
  from `assets/vendor/monaco`) shows the 409 of an uncloned repository as a message.
- **Terminals.** Task terminals, sandbox shells and account logins share one protocol client
  (`termsocket.js`). Writers are controllers (the server sets `controller = can_write`); a
  4003 close (access revoked) is final; the login terminal renders the fixed 500-column PTY and
  scrolls horizontally instead of resizing it; a shell's `exit` frame ends it for good.
- **Browser.** An `upstream` error followed by a 1011 close (Chromium missing or stopped) shows
  the reason with a Retry button instead of reconnecting in a loop.
- **Delivery.** Next actions come from the server (`open_pr`, `merge`, `run_env:<name>`,
  `confirm_env:<name>`, `promote`); `blocked` actions show disabled with the reason. Profile
  presets come from `GET /api/delivery/presets`; command environments are run with
  `POST …/environments/{env}/runs {"confirm":true}` (polled via `GET …/delivery/runs/{id}`),
  manual verifications confirmed with `POST …/environments/{env}/confirm {"confirm":true}`.
- **Browser input.** `button` is `left|middle|right|none`; `modifiers` is the CDP bitmask
  (Alt 1, Ctrl 2, Meta 4, Shift 8); printable keys send `event: "down"` with `text`, paste sends
  one `event: "char"` with the pasted `text`.

Syntax check without a browser: `node --check` on each file (Node 24 detects ES modules).
