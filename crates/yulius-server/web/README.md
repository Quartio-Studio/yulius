# Yulius web UI

Static single-page UI for `yulius serve`. It implements the workspace interface from
[SPEC-001 §6](../../../.ai/specs/SPEC-001-yulius-product.md) against the HTTP/WebSocket
contract in [docs/api.md](../../../docs/api.md). Per SPEC-002 ADR-01 it is plain ES modules
with vendored xterm.js: no framework, no build step and no Node toolchain at runtime.

## Serving contract (for the Rust server)

- Serve `index.html` at `/` **and** for every UI path (history API routing): `/setup`,
  `/signin`, `/device`, `/archived`, `/settings`, `/design`, `/admin/*` and `/s/*` (including `/s/<slug>/sessions[/<id>]`) — except anything under `/api/`.
- Serve `assets/**` at `/assets/...` with the usual content types (`.js` and `.mjs` as
  `text/javascript`, `.css`, `.png`). Assets are referenced by absolute `/assets/` paths.
- Nothing is loaded from other origins: no CDNs, remote fonts or external images (a brand font is
  self-hosted under `assets/fonts/`). A strict `Content-Security-Policy` works:

  ```
  default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
  img-src 'self' data: blob:; font-src 'self' data:; worker-src 'self' blob:;
  connect-src 'self' https://*.<host>; frame-src 'self' https://*.<host> data:; frame-ancestors 'none'; base-uri 'none'
  ```

  There are no inline scripts (`boot.js` is a file). Inline `style` attributes carry computed values
  only (bar widths, indentation). The editor (Monaco) needs `font-src data:` for its icon font and
  `worker-src blob:` for its language workers. The browser panel draws the sandbox Chrome's video
  (WebCodecs) and JPEG frames (`blob:`) on a canvas and opens preview origins only in new windows,
  so it needs nothing beyond the policy above. The server sends only `frame-ancestors 'none'` so
  far, and `boot.js` makes the app refuse to run inside any frame.
- `README.md` files are documentation and need not be embedded.

## Layout of the code

| File | Responsibility |
| --- | --- |
| `index.html` | Page shell; loads `assets/tokens.css`, `assets/styles.css` and `assets/app.js` (`type=module`) and preloads the terminal typeface (IBM Plex Mono, regular). xterm and its stylesheet load with the first terminal (`terminal.js`). |
| `assets/app.js` | Entry point: boot (setup → sign-in → workspace), route dispatch, composition of the layout, link handling. |
| `assets/api.js` | HTTP client (`X-Yulius-CSRF` on every non-GET, `{"error":{code,message}}` → `ApiError`, 30 s timeout) and the transport abstraction. |
| `assets/router.js` | Routes and links; history API paths, hash paths in mock mode; recognizes Yulius links. |
| `assets/store.js` | State store (sandboxes, tasks per sandbox, browser action requests) and the `/api/events` client. |
| `assets/termsocket.js` | WebSocket protocol code: binary frame parsing, `ReconnectingSocket` (backoff 0.5–10 s, bounded attempts, 4008 → immediate resync), `TerminalSocket`. |
| `assets/terminal.js` | Terminal views on demand: returns the view at once and loads `terminal-core.js`, xterm, `xterm.css` and the terminal typeface together with the first terminal (or on idle after sign-in, `preloadTerminal()`). |
| `assets/terminal-core.js` | xterm view: fit/resize, controller handover, read-only banner, floating reconnect notices, exit banners, web links and OSC 8 links. It appears once at its final size and font: fitted before connecting (`?cols=&rows=`), hidden under a placeholder until the first state and screen are drawn (F 2026-10-09). |
| `assets/layout.js` | Sandbox tab row (All sandboxes, the sandboxes, +, Status panel, notifications, tools toggle) and the sidebar (wordmark, navigation, active task list, user menu at its foot); the sidebar's width is dragged on its edge, kept per user and clamped so the rail and a 360 px centre stay in the window (with a panel open it gives way first, down to 200 px). |
| `assets/splitter.js` | Column splitters (UI-044) for the sidebar, the editor's explorer and the tool panel: a quiet grip that is a focusable separator (←/→, Shift for 64 px steps, Home/End; double-click resets), and the widths kept per user in `localStorage` (`yulius.layout.<user id>`: `sidebar` and `explorer` in px, `panel-<tool>` as a share of the work area). Phones have none. |
| `assets/pages/home.js` | Home (UI-045), the default view at `/`: every sandbox as a card (repository, running / needs you / queued, last activity from its tasks, Open and New task), Import and Create; with no sandbox, the first steps and the GitHub notes. |
| `assets/notifications.js` | Notification center (UI-020, UI-028): the bell, its panel with the All / Unread filter, and entries derived from the event stream; kept per user in `localStorage`, bounded to 100; optional OS notifications while the tab is hidden. |
| `assets/pages/gittask.js` | Git → a task's review (UI-031): Changes (file tree, unified or split diff, wrap), Commits, Branch and Go to task; reads `/api/tasks/{tid}/changes` and `/commits`. |
| `assets/pages/sessions.js` | Sessions (UI-049): the sandbox's task and imported Claude Code sessions with kind and person filters and full-text search (matches marked), and one session's read-only transcript — prompts, Claude's messages, folded thinking and tool calls with their results inside, edits as diffs, a search match opened at its line; Continue in a new task and Delete for imported sessions; "Add your sessions" explains `yulius onboard`. |
| `assets/pages/taskinfo.js` | Tasks → a task's overview (UI-032): state, times, plan, changes, usage, details and sessions, with Review changes and Go to task. |
| `assets/mdeditor.js` | A Notion-like block editor for Markdown (UI-033): formatted blocks, frontmatter as properties, "/" menu, Markdown shortcuts, selection toolbar, source view; untrusted text, no HTML parsing. |
| `assets/pages/skilledit.js` | A skill's page: its SKILL.md in the block editor with Save (⌘S), read-only where it may not be edited, and an unsaved-changes guard. |
| `assets/shortcuts.js` | Keyboard shortcuts (UI-029): platform key names (`NEW_TASK_KEYS`, `HOME_KEYS`, `SAVE_KEYS`, `WRAP_KEYS`, `IS_MAC`) and the ⌘/ sheet listing every shortcut — update it when a handler changes. |
| `assets/taskview.js` | Task view (step strip with the "Auto → Goal · Feature" label, terminal, phone quick-input bar) and the new-task composer (rounded prompt box with mode and workflow — Auto by default — run after, model and effort pickers, Preview and the ↑ Start button; Enter adds a line, ⌘/Ctrl+Enter starts; the working status and the split card below the box). |
| `assets/mdinput.js` | Markdown while typing in a plain textarea (UI-021): list continuation and ending on Enter, Tab / Shift+Tab indent (Esc, then Tab, leaves), ⌘B / ⌘I wrap, code fences close, ⌘/Ctrl+Enter submits; edits through the editing command so undo works; IME-safe. |
| `assets/intake.js` | Task intake (TSK-011..013): the split check, the split card (shimmering rows, then compact rows with collapsed prompts and one primary action), the working status line, "Run after" candidates and the held-prompt card. |
| `assets/quotes.js` | 40 quotes about building great things (well-documented authors only); one shows at random above the composer. |
| `assets/models.js` | Models (grouped by provider, each naming its harness) and effort levels the composer offers. |
| `assets/panels.js` | Right rail and resizable panel: Editor, Shell (the user's own terminals in this sandbox as tabs: opens one at once, "+" for more, rename with F2 or a double-click; new ones start in the sandbox repository or the task worktree), Messages (notices from Yulius and the agent's messages to people, no composer: an agent's question offers "Reply in the terminal"); hosts the browser panel. One tool is open at a time (`aria-pressed`); each tool's width is remembered per user (`splitter.js`) and re-fitted when the window narrows or the sidebar widens, so it never pushes the rail out of the window. |
| `assets/editorview.js` | The built-in editor (EDT-001..005): explorer (its width dragged on its edge and kept per user), tabs and Monaco over the files API; ⌘S / Ctrl+S saves; word wrap is one toolbar toggle (or ⌥Z / Alt+Z, as in VS Code) for every open file, off by default and kept per user in this browser (`localStorage` `yulius.editorWordWrap.<user id>`). |
| `assets/filedrop.js` | Dropped and pasted files (EDT-006, UI-048): files or folders dropped on the explorer go into the folder under the pointer, on the editor into the open file's folder (text files open); an upload button picks files on phones; a file already there is replaced only after asking. Files dropped on a terminal, or an image pasted into it, are uploaded as attachments and their paths typed in as a paste (xterm's bracketed paste), quoted when needed. Drop overlays, progress and errors; a file dropped elsewhere never makes the browser leave the app. Hook points: `attachEditorDrop` (editorview.js), `attachTerminalDrop` (taskview.js, panels.js); text pastes stay with the terminal. |
| `assets/browser.js` | Browser panel (ADR-15): the sandbox Chrome's tabs in the strip, address bar, back/forward/reload, the action-request banner, notices for agents acting in another tab, "Open live in a new window" for a sandbox dev server (ADR-14 preview origins). |
| `assets/browserstream.js` | The streamed sandbox Chrome: its socket and tab list, H.264 video decoded with WebCodecs onto a canvas (every frame acknowledged for the server's flow control and decoded, the newest drawn; JPEG frames as the labelled fallback), the shared viewport and its notice, pointer, wheel, touch and keyboard input (a hidden textarea for IME, dead keys and phone keyboards; ⌘ as Ctrl on macOS; paste and copy), the agent's cursor. |
| `assets/actions.js` | Task actions and dialogs: stop, continue, feedback, delivery details and deploy output (read-only: agents deliver, GIT-002), sandbox add/remove. No account choice: tasks are subscription-agnostic. |
| `assets/status.js` | Status → glyph/tone/label mapping and permitted actions per access level. |
| `assets/dom.js`, `assets/format.js` | DOM builder (text only, never `innerHTML` with data), icons, toasts, modals, menus; formatting helpers. |
| `assets/pages/*.js` | Pages: auth (setup, sign-in), tasks (+ archived, device approval), task overview (`taskinfo.js`), git and a task's review (`gittask.js`), CI (`ci.js`: runs, job logs, repository hygiene), workflows, skills, automations, MCP and their editors (`workflowedit.js`, `skilledit.js`, `skillnew.js`, `automationedit.js`, `mcpedit.js`), dashboard, admin (users, accounts, GitHub, instance), settings, design. |
| `assets/spotlight.js` | The search dialog (Tasks page `/`): results with ↑/↓, Enter opens, ⌘/Ctrl+Enter goes to the task. |
| `assets/intake.js` | Task intake in the composer: the split proposal card and held prompts (SPEC-002 ADR-16). |
| `assets/termkeys.js`, `assets/termlinks.js`, `assets/mousefilter.js` | Terminal key table per platform (copy, paste, interrupt), links in terminal output (web links and file paths to the editor), mouse-report filtering. |
| `assets/updates.js` | Instance updates (OPS-006..009, ADR-18): the calm banner (an update running for everyone; "Update available" and a failed update for the superadmin; "Reload" once the server runs newer code) and the superadmin's notification, from `GET /api/update` and `update.updated`; polls while an update runs. |
| `assets/jevkey.js` | The Jev key (ADM-001/002): the setup wizard's optional last step and the instance page's card (set, replace, test, remove, save a configured key here); the key is never shown back. |
| `assets/pages/instance.js` | Settings → Instance (superadmin): the build, the update on offer with its commits, Update now with its confirmation, the progress (build → pause tasks → restart → resume) and the outcome; the Jev key card. |
| `assets/mock.js` | In-memory backend and fake WebSockets used with `?mock=1`. |
| `assets/brand.js` | Brand identity in one place: product name, tagline and mark (the SVG path used by the sidebar, sign-in, favicon and tab title). |
| `assets/boot.js` | The only classic script: runs in `<head>` after the stylesheets and before the first paint, applies the stored theme (no dark flash for light or system) and sets the browser's theme colour from the tokens; notes when the page is framed, and app.js then shows only a notice (Yulius never runs inside a frame). |
| `assets/theme.js` | Theme choice (dark, light, system) per browser: sets `data-theme`, the colour-scheme and theme-color metas, and re-colours terminals and editors from the tokens. |
| `assets/tokens.css` | Design tokens: brand and palette, colour roles per theme (dark, light), derived tints, and the type, control, radius, elevation and motion scales. |
| `assets/styles.css` | Components and layout (including the phone layout < 768 px), using only roles and scales from `tokens.css`. A page is a size container (`container: page`), so its lists and cards follow the width the page has, an open tool panel included (UI-046); page content is centred up to its maximum width (UI-047). |
| `assets/vendor/` | Vendored xterm.js; see [vendor/README.md](assets/vendor/README.md). |
| `assets/logos/` | Logos of well-known services shown on the MCP page; sources in [logos/README.md](assets/logos/README.md). |

## Theming and branding

Everything visual is driven by tokens, so a new brand, palette or theme is a token edit — never a
component change. See it live at **`/design`** (Settings → Design system), which renders every token
and component in the current theme and computes contrast as you change things.

### Rebrand in three steps

1. **`assets/brand.js`** — product `name`, `tagline` and `mark`: SVG path data with its `markViewBox`
   and `markStyle` (`'fill'` for a solid logo such as the current one from
   `assets/brand/logo-lockup.svg`, `'stroke'` for a line drawing). The mark's colour is the
   `--brand-mark` token (monochrome by default) and the wordmark's case and tracking are
   `--wordmark-case` / `--wordmark-tracking`.
   The sidebar wordmark, sign-in/setup screens, browser tab title, favicon and every sentence that
   names the product follow (`BRAND.name`; the check rejects the name written anywhere else).
2. **`assets/tokens.css`, Layer 1** — the brand ramp `--brand-200 … --brand-800` and `--brand-ink`
   (text on a `--brand-500` fill), then the state palette. The current brand, **Roma — smoke and
   chalk**, has no accent hue: the ramp runs from chalk to smoke, chalk is the accent on the dark
   theme (primary buttons, focus, links, charts) and smoke ink on the light one; the themes' roles
   (Layer 2) hold the matte smoky black, its one lighter intermediate dark and the chalk inks. The
   state palette is Roman pigments — verdigris, yellow ochre, cinnabar, Egyptian blue ("your
   turn") and Tyrian purple — independent of the brand on purpose, so a brand never makes "your
   turn" read as a warning or an error. Terminal ANSI colours are a fixed palette on purpose
   (agents' output colours mean something). Each theme also sets `--texture` and `--light` (a fine
   grain and a soft light over the canvas, pages, sidebar and rail).
3. **Sync and check** — run `python3 scripts/check_repository.py --sync-brand`: it rewrites the
   brand parts of `index.html` (title, theme colour, favicon, no-script text — what shows before the
   app's scripts run) from `brand.js` and the dark tokens, and its `modulepreload` links from
   `app.js`'s static imports (so the browser fetches every start-up module in one wave; run it after
   adding or removing an import too). Then open `/design` in both themes and run
   `python3 scripts/check_repository.py --ui` (no Rust needed; `scripts/dev check` runs it too): it fails if `index.html` and the brand disagree, or if any text or control
   edge in any theme drops below WCAG contrast (text 4.5:1; state marks, focus, control edges 3:1),
   listing what to fix.

The assets are compiled into the server binary (`build.rs` embeds `web/`), so a rebrand ships with
the next server build and deploy.

### Brand details beyond tokens

A few shapes are the brand's own and live at the end of `styles.css` ("Brand details"): the mark
in an arched niche over the composer, the composer's quote set like an inscription, the sidebar's
group heads as softly rounded bands of spaced capitals. In the niche, `brandfx.js` lets smoke rise
(WebGPU, the vendored `shaders` library without its telemetry; only with a GPU adapter, never under
reduced motion or data saving, paused in hidden tabs). A brand-lab proposal can add its own details
keyed on `data-brand`.

### Brand lab: preview whole brand proposals

Open the app with `?lab=1` (mock mode: `/?mock=1&lab=1`) and a panel restyles it live on the real
screens, in both themes, without a rebuild:

- **Proposals** are complete brands: any tokens of `tokens.css` — both themes' colours, state
  colours, typefaces, type scale, control heights, radii, motion. Built-in ones live in
  `assets/lab-proposals.js` (`?lab=<id>` opens one). **Paste proposal…** previews a JSON proposal at
  once (kept in this browser); **Copy template** gives the JSON shape filled with the values in view,
  for a designer or an assistant to fill in.
- **Directions and sliders** generate a brand from a few values (accent hue, strength and
  brightness, the greys' tint, dark depth, typefaces, radius, wordmark).
- **Checks** measure the repository's contrast pairs (text 4.5:1, edges 3:1) in both themes as you
  go; directions also warn when the accent sits next to a state colour.

Typefaces come from Google Fonts in the lab only. **Copy settings** gives the tokens to write into
`tokens.css` (and the typefaces to self-host). `lab.js` loads only when the URL asks for it and is
not part of the app's start-up.

### Brand typeface

Fonts are self-hosted (no CDNs). The brand's are Instrument Sans (UI text), Tenor Sans (titles and
the wordmark, for its Roman capital proportions) and IBM Plex Mono (code, terminals, the editor):
`.woff2` files from Fontsource 5.3.0 in `assets/fonts/` (Latin and Latin Extended, one per weight,
with their SIL Open Font License texts), declared at the end of `tokens.css`. To change one, put
its `.woff2` files in `assets/fonts/`, replace its `@font-face` rules, and put the family first in `--font` (UI text),
`--font-display` (page, dialog and sign-in titles and the wordmark; it follows `--font` until set)
and/or `--font-mono` (code, terminals and the editor read it too). Keep the weights the scale uses (400,
500, 600): a static font needs one `@font-face` per weight; a weight range is only for a variable
font (on a static file it makes every weight identical). Then check `/design`.

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
| Layers | `--z-raised`, `--z-sticky`, `--z-float`, `--z-drawer-scrim`, `--z-drawer`, `--z-panel`, `--z-popover`, `--z-menu`, `--z-toast` | Every `z-index`; modal dialogs use the browser's top layer above them |

### What the checks enforce

`scripts/check_repository.py` (`scripts/dev check`) rejects in `assets/styles.css`: colour literals
(hex, `rgb()`/`hsl()`/…, named colours), font sizes, line heights, families and weights outside the
type scale, radii outside the tokens, padding/margin/gap off the spacing scale, and literal transition
timings. It resolves `tokens.css` per theme and checks the contrast of every text/background role
pair and of state marks and control edges, and that buttons, inputs, selects and tabs use a
`--control-*` height (at least 24 px targets, WCAG 2.5.8). Scripts may not set fixed lengths or
colours in inline styles, nor show requirement IDs in UI text, nor write the product name in a
string (use `BRAND.name` from `brand.js`). For the scripts it checks that every imported name is
exported by its module and used, and (when Node is installed) the syntax of every file. There is no
JS linter or type checker yet: that would add an npm dependency, which is a project decision.

### Components

Shared building blocks live in `assets/dom.js`: `iconButton`, `toggle`, `badge`, `iconBadge`,
`statusMark` (task state icon + tone + label), `itemCard` (clickable list card with icon, badges,
description, controls), `meter`, `openMenu` (icons, descriptions, check marks), `openModal`,
`toast`. Prefer them to new one-off markup; anything new should appear on `/design`.

One pattern per kind of state (SPEC-001 UI-050): `emptyState` for every empty list (with the action
that fills it), `loadError` for a failed load said in place (`loader(container, 'the users', …)` in
`pages/common.js` names what failed: "Couldn’t load the users: …"), `skeleton` while loading, and
`notFound` (`pages/common.js`) for a missing page, sandbox, task or item: a heading and the way
back. Page headers come from `pageFrame`: a title, a subtitle that says what the page is for, and
actions; `createButton` is the primary "add" button with its "+". "Needs you" is `taskInfo`'s
group `needs` (`status.js`), the rule the server counts with too.

## Routes

| Path | View |
| --- | --- |
| `/setup` | Setup wizard (token, password twice, two non-recoverable-password confirmations). |
| `/signin?next=…&error=…` | Sign in with GitHub (`/api/auth/github/start?next=`) or the superadmin password. |
| `/` | Home: every sandbox as a card (the default view; ⌘⇧H / Ctrl+Shift+H, the logo or the All sandboxes tab return to it); empty state with Import/Create when there is none. |
| `/s/<slug>` | New-task composer. |
| `/s/<slug>/t/<number>` | Task: step strip + native terminal. |
| `/s/<slug>/browser?tab=<id>` | Opens the browser panel on that tab (Yulius links, BRW-004). |
| `/s/<slug>/tasks`, `/git`, `/ci`, `/workflows`, `/skills`, `/automations`, `/mcp`, `/dashboard` | Sandbox pages. `tasks?state=needs|working|done|stopped|closed&author=a,b&origin=automation&q=text` is a filtered task list (shareable). `ci?run=<id>` opens a CI run (the link of its `yulius/ci` commit status). `skills?source=instance` and `mcp?scope=instance` are the admin "Skills/MCP for every sandbox" entries. |
| `/s/<slug>/tasks/<number>`, `/s/<slug>/git/t/<number>` | A task's overview; a task's changes and commits. |
| `/s/<slug>/skills/<name>`, `/workflows/<id>`, `/mcp/<id>`, `/automations/<id>` | A skill, workflow, MCP server or automation in its editor (`new` for a new one). |
| `/archived` | Archived tasks across sandboxes, with restore. |
| `/settings` | Settings: your account, this browser's preferences, administration tiles (user menu → Settings). |
| `/design` | Design system: brand, colour roles with live contrast, type, spacing, radii, controls, elevation, motion, icons, task states and components, in the current theme (Settings → Design system). |
| `/device?code=<user_code>` | CLI device approval. |
| `/admin/users`, `/admin/accounts`, `/admin/github`, `/admin/instance` | Administration (superadmin / instance admin): Users, Subscriptions, GitHub connection, Instance. Like Home, Settings, Archived, Device and Design, these pages are outside any sandbox: no sandbox navigation or tools. |

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
`role=superadmin` (the default), `contained=0` (shows "Not isolated"), `github=0` (GitHub App not configured), `github_linked=0`
(the account has no GitHub link: the superadmin still sees every sandbox, other roles see none and
the home page asks them to sign in with GitHub), `sandboxes=0` (a fresh instance without sandboxes: the
home screen's first steps), `tasks=0` (sandboxes without tasks yet: no start rule to offer), `othercols=N&otherrows=M` (another, active viewer sets task terminals to
N×M until this one types or clicks into the terminal), `update=1` (a newer `main` is on offer; Update
now walks through its phases in about six seconds) or `update=fail` (the same, rolled back),
`helper=0` (no update helper), `jev=config` (a Jev key file is configured), `fail=<path>` (GET
requests of that exact API path answer 500, to see a view say what failed to load).

The mock mirrors the real response shapes (it is the UI's offline test bed and
documentation; the [end-to-end tests](../../../e2e/README.md) run against it). It has sandboxes with admin, write and read access (`studio` with the Studio
preset, `product` with a custom profile whose `dev-vm` environment has a command, read-only
`sandbox-3` whose Git page shows a degraded-read note), tasks in every status, steps,
messages, accounts with usage (some unknown), Git data and delivery stages, environment runs,
CI runs with job logs and repository hygiene (`studio` has jobs and runs, `product` has no CI and
leftover GitHub Actions workflows), workflows, skills, automations, MCP servers, analytics, the
superadmin recovery code (mock code `123456`) and the GitHub App connection. Importing a repository creates a
sandbox whose onboarding is `pending` with a resting preparation task (Start preparation /
Skip). Uploads report progress in steps, a dropped file lands in the editor's in-memory
repository and attachments get server-like paths and names. Task terminals print TUI-like ANSI output (with file references in every form the terminal links
to the editor, and some it must not), echo input and answer on Enter; type
`/exit` to close a task, `/lag` to force a 4008 reconnect, `/help` for the list. The
browser panel draws fake pages and sends them as video (VP8 through the browser's own
WebCodecs encoder; the real server sends H.264) or as JPEG frames when the browser cannot encode
or decode video; click "Sign in" on the AWS tab, type into its field.

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
- **Known gaps.** None: every documented route the UI calls is routed (`KNOWN_UNROUTED` in the
  test is empty and fails if an entry goes stale).
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
- **Delivery.** Shown, never driven (GIT-002): agents open, merge, deploy and release through
  their workflow. Next steps come from the server (`open_pr`, `merge`, `run_env:<name>`,
  `confirm_env:<name>`, `promote`) and show as status, `blocked` ones with their reason. Profile
  presets come from `GET /api/delivery/presets`; a running deploy's output is polled via
  `GET …/delivery/runs/{id}`.
- **Uploads.** `api.put|post(path, file, {onProgress})` sends a `Blob` as the raw body; the real
  transport uses XMLHttpRequest for upload progress (`transport.upload`), and the timeout then
  counts from the last progress. Dropped files use `PUT /api/sandboxes/{sid}/files/upload` and the
  attachment endpoints (docs/api.md); the limit is `instance.upload_max_bytes` from `/api/me`.
- **Browser input.** `button` is `left|middle|right|back|forward|none`; `modifiers` is the CDP
  bitmask (Alt 1, Ctrl 2, Meta 4, Shift 8); keys send `down`/`up` with `key` and `code`, composed
  text and pastes one `event: "char"` with the `text`; ⌘C/Ctrl+C also sends `copy`, and the
  `clipboard` answer goes to the user's clipboard.

Syntax check without a browser: `node --check` on each file (Node 24 detects ES modules).
