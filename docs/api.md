# Yulius HTTP and WebSocket API (v1)

Contract between the web UI (and future desktop shells, SPEC-001 OPS-002) and `yulius serve`.
It refines [SPEC-002 §7.7–§7.9](../.ai/specs/SPEC-002-yulius-architecture.md). Rust DTOs live in
`crates/yulius-contracts/src/api.rs`; when they disagree with this page, fix both in one change.

## Conventions

- JSON bodies (`Content-Type: application/json`), ≤ 1 MiB. Timestamps are RFC 3339 UTC strings.
- Authentication: the `yulius_session` cookie (browser) or `Authorization: Bearer yul_…` (CLI
  tokens). State-changing requests from cookie sessions must send `X-Yulius-CSRF: <csrf_token>`
  from `GET /api/me`.
- WebSockets require a valid session and an `Origin` equal to the public URL.
- Errors: HTTP status plus `{"error": {"code": "snake_case", "message": "human text"}}`. Codes:
  `unauthenticated` (401), `forbidden` (403), `not_found` (404; also for invisible sandboxes),
  `conflict` (409; racing state changes), `invalid` (422), `capacity` (429), `upstream` (502,
  GitHub/provider failure), `internal` (500).
- Access levels: `read`, `write`, `admin` (SPEC-001 ACC-007). Endpoints note the minimum level.

## Setup and session

| Method | Path | Body → Response |
| --- | --- | --- |
| GET | `/api/setup/status` | → `{"setup_required": bool}` |
| POST | `/api/setup` | `{"token", "password", "password_confirm", "confirm_saved": true}` → 201 `Me` + cookie. Refused once a superadmin exists. |
| POST | `/api/auth/password` | `{"password"}` → `Me` + cookie (superadmin only; rate limited) |
| GET | `/api/auth/github/start?next=/` | → 302 to GitHub |
| GET | `/api/auth/github/callback?code&state` | → 302 to `next`, sets cookie; refuses users not admitted |
| POST | `/api/auth/logout` | → 204 |
| GET | `/api/me` | → `Me` |

```json
Me = {
  "user": {"id": "…", "login": "octocat", "display_name": "…", "avatar_url": "…|null",
           "is_superadmin": false, "is_instance_admin": false, "github_linked": true},
  "csrf_token": "…",
  "instance": {"owner_login": "Quartio-Studio", "owner_kind": "org|user|null",
               "public_url": "https://yulius.quartio.ai", "github_configured": true,
               "contained": true}
}
```

CLI device approval: `POST /api/device/start` → `{"device_code","user_code","verification_url",
"interval_s","expires_in_s"}`; `POST /api/device/poll {"device_code"}` →
`{"status":"pending|approved|denied|expired","token":"yul_…|null"}`; `POST /api/device/approve
{"user_code","approve":bool}` (web session).

## Sandboxes

| Method | Path | Level | Body → Response |
| --- | --- | --- | --- |
| GET | `/api/sandboxes` | read | → `[Sandbox]` (only visible ones) |
| GET | `/api/github/repositories` | — | → `[{"owner","name","private","access","already_sandbox"}]` |
| GET | `/api/templates` | — | → `[{"id","name","description"}]` |
| POST | `/api/sandboxes` | admin on repo / owner | `{"mode":"import","repo":"owner/name"}` or `{"mode":"create","name","template","description"}` → 201 `Sandbox` |
| GET | `/api/sandboxes/{sid}` | read | → `Sandbox` |
| PATCH | `/api/sandboxes/{sid}` | admin | `{"profile": Profile}` → `Sandbox` |
| POST | `/api/sandboxes/{sid}/onboarding/skip` | admin | → `Sandbox` (`pending`/`preparing` → `skipped`) |
| POST | `/api/sandboxes/{sid}/onboarding/approve` | write | → `Sandbox`; starts the preparation task (`pending` → `preparing`); 409 when nothing awaits approval or the task cannot start |
| DELETE | `/api/sandboxes/{sid}` | admin | → 204; stops live/queued tasks; GitHub repository, clone and task records kept |

`GET /api/github/repositories` lists the App installation's repositories of the instance owner
that the caller can at least read (needs a GitHub-linked account). Import mode requires a
repository of the instance owner (`invalid` otherwise) and `admin` or `maintain` on GitHub; it
clones the repository, picks the profile (`yulius.toml` `[delivery]`, else `product`/`studio` for
`Quartio-Studio/product`/`studio`, else `standard`) and validates the documentation format: a
failing project gets `onboarding: "pending"` and a resting preparation task (`origin:
"onboarding"`). While `pending`/`preparing`, creating other tasks is a 409 (U-08). Create mode
(instance admins) creates a private repository, pushes the template scaffold to `main` and `dev`,
and adds rulesets requiring pull requests on both.

```json
Sandbox = {
  "id": "…", "slug": "studio", "name": "studio",
  "repo": {"owner": "Quartio-Studio", "name": "studio", "url": "https://github.com/…"},
  "access": "read|write|admin", "default_branch": "main",
  "profile": Profile,
  "onboarding": "not_needed|pending|preparing|ready|skipped",
  "port_base": 20000, "port_count": 20,
  "counts": {"running": 1, "needs_attention": 0, "queued": 0}
}
Profile = {"preset": "standard|product|studio|custom", "integration_branch": "dev",
           "production_branch": "main", "merge": "squash|merge|rebase",
           "promote": "merge_commit|fast_forward", "auto_promote": false,
           "environments": [{"name": "dev", "after": "integration|production",
                             "command": "…|null", "verify": "manual|command"}]}
```

## Import (`yulius import`)

Bearer CLI token or cookie session. The full protocol, bounds and snapshot layout are documented
in `crates/yulius-contracts/src/import.rs`.

| Method | Path | Body → Response |
| --- | --- | --- |
| POST | `/api/import` | `{"manifest": ImportManifest}` → `{"job_id", "missing": [{"name", "received_bytes"}]}`. Links the repository as a sandbox when needed (admin/maintain; owner repositories only); an unfinished job with the same manifest is reused; files already stored for the sandbox (same `sha256`) are not listed. |
| PUT | `/api/import/{job}/files/{name}?offset=N` | raw chunk (≤ 8 MiB) with `X-Chunk-Sha256` → `{"name", "received_bytes"}`. `name` is one percent-encoded segment. Repeats are acknowledged; an offset beyond the received length is 409; a wrong digest is 422 (a wrong whole-file digest resets the file to 0). |
| POST | `/api/import/{job}/complete` | → `{"sandbox_id", "sandbox_url", "tasks_created", "sessions_skipped_existing", "onboarding"}`; 409 while files are incomplete. Idempotent. |

Applying: bundle branches become `import/<login>/<branch>`; the snapshot is committed on
`import/<login>/snapshot-<YYYY-MM-DD>[-n]` created from the client's `HEAD`; each new session
becomes an archived task (`origin: "import"`) whose transcript is kept immutably and resumed
natively on Continue (same or other harness).

## Tasks

| Method | Path | Level | Body → Response |
| --- | --- | --- | --- |
| GET | `/api/sandboxes/{sid}/tasks?archived=false` | read | → `[TaskSummary]` |
| POST | `/api/sandboxes/{sid}/tasks` | write | `{"prompt","harness":"claude|codex","workflow_id?","model?","effort?","title?"}` → 201 `TaskSummary` |
| GET | `/api/tasks/{tid}` | read | → `TaskDetail` |
| POST | `/api/tasks/{tid}/stop` | write | → `TaskSummary` |
| POST | `/api/tasks/{tid}/continue` | write | `{"harness?","account_id?"}` → `TaskSummary` |
| POST | `/api/tasks/{tid}/switch` | write | `{"harness?","account_id?"}` → `TaskSummary` |
| POST | `/api/tasks/{tid}/archive` · `/restore` | write | → `TaskSummary` |
| POST | `/api/tasks/{tid}/feedback` | read | `{"verdict":"correct|incorrect","comment"}` → 204 |
| POST | `/api/tasks/{tid}/messages` | write | `{"body"}` → `Message` (user → agent, delivered like task messages) |
| GET | `/api/tasks/{tid}/messages` | read | → `[Message]` |
| GET | `/api/archived-tasks` | read | → `[TaskSummary]` across visible sandboxes |
| WS | `/api/tasks/{tid}/terminal` | read (input: write) | terminal protocol below |

```json
TaskSummary = {
  "id": "…", "sandbox_id": "…", "number": 12, "title": "Checkout API",
  "harness": "claude|codex", "status": "running|…(see domain wire names)",
  "activity": "working|waiting|needs_attention|null",
  "branch": "feat/checkout-api", "origin": "ui|automation|import|onboarding|agent",
  "created_by": {"login": "…"}, "created_at": "…", "updated_at": "…",
  "steps": {"done": 3, "total": 7, "current": "Implement API|null"},
  "unread_messages": 0, "archived": false, "account_label": "Claude #2|null"
}
TaskDetail = TaskSummary + {
  "spec_path": "docs/specs/2026-10-07-checkout-api.md|null",
  "worktree": "…", "attempts": [{"seq","harness","account_label","launch_kind","state",
                                  "started_at","ended_at","exit_code","exit_reason"}],
  "steps_list": [Step], "metrics": Metrics, "feedback": [{"login","verdict","comment","at"}],
  "delivery": Delivery, "switches": [{"at","reason","from","to"}]
}
Step = {"id","ordinal","title","status":"pending|in_progress|done|skipped|blocked","note","updated_by","updated_at"}
Message = {"id","from":{"task_id|null","login|null","title"},"to_task_id|null","kind":"info|question|request|scope","subject","body","created_at","read":bool}
Metrics = {"wall_ms","active_ms|null","input_tokens|null","output_tokens|null","cache_read_tokens|null","cache_write_tokens|null","models":["…"],"switches":0,"turns|null"}
```

Status wire names (from `yulius-domain::task`): `queued`, `starting`, `running`, `switching`,
`waiting_capacity`, `stopping`, `closed`, `stopped_cancelled`, `stopped_crashed`, `stopped_lost`,
`stopped_switch_failed`, `stopped_switch_timeout`, `stopped_server_lost`, `failed_spawn_error`,
`failed_start_timeout`, `failed_precondition`.

### Terminal WebSocket

Client → server text frames:

```json
{"type":"hello","last_seq":null,"cols":120,"rows":40}
{"type":"input","data":"text or escape sequences","seq":17}
{"type":"resize","cols":120,"rows":40}
{"type":"take_control"}
```

Server → client:

- Binary frame: `u8 kind` + `u64 big-endian seq` + bytes. `kind` 1 = output, 2 = snapshot (clear
  the terminal and write the bytes; a full screen redraw).
- Text frames: `{"type":"state","attempt_seq":n,"harness":"claude","cols":c,"rows":r,
  "can_write":bool,"controller":bool,"status":"running"}`, `{"type":"notice","text":"↻ switched
  to …"}`, `{"type":"exit","status":"closed","code":0}`, `{"type":"error","code","message"}`.
- Close code 4008: viewer lagged; reconnect with `last_seq`.

The account-login terminal (`/api/admin/accounts/login/{lid}/terminal`) and sandbox shells
(`/api/shells/{id}/terminal`) use the same protocol.

## Git and delivery

| Method | Path | Level | Body → Response |
| --- | --- | --- | --- |
| GET | `/api/sandboxes/{sid}/git` | read | → `GitOverview` |
| GET | `/api/tasks/{tid}/delivery` | read | → `Delivery` |
| POST | `/api/tasks/{tid}/delivery/pr` | write | `{"title?","body?"}` → `Delivery` (refused without a valid spec) |
| POST | `/api/tasks/{tid}/delivery/merge` | write | → `Delivery` (requires green checks per profile) |
| POST | `/api/sandboxes/{sid}/delivery/promote` | write | → `{"result","sha"}` |
| POST | `/api/sandboxes/{sid}/delivery/environments/{env}/runs` | write | `{"confirm":true}` → `{"run_id"}` |
| GET | `/api/sandboxes/{sid}/delivery/runs/{run_id}` | read | → `{"state","output","exit_code"}` |

```json
GitOverview = {"integration_branch","production_branch","default_branch",
  "tasks":[{"task_id","title","branch","ahead","behind","files_changed","insertions","deletions","pr":PR|null}],
  "recent_commits":[{"sha","subject","author","at"}], "pulls":[PR]}
PR = {"number","url","title","state":"open|merged|closed","base","head","checks":"none|pending|success|failure"}
Delivery = {"stage":"none|pr_open|merged_integration|env_done|promoted|production_done",
  "pr":PR|null,"spec_valid":bool,"spec_problems":["…"],"steps_open":0,
  "next_actions":["open_pr|merge|run_env:dev|promote|run_env:production"]}
```

## Skills, workflows, automations, MCP

| Method | Path | Level | Notes |
| --- | --- | --- | --- |
| GET | `/api/sandboxes/{sid}/skills` | read | `[{"name","description","source":"repository|instance|open-mercato","path","enabled"}]` |
| GET | `/api/sandboxes/{sid}/skills/{name}` | read | `{"name","content"}` |
| PUT | `/api/sandboxes/{sid}/skills/{name}` | write | `{"enabled":bool}` |
| GET/POST | `/api/sandboxes/{sid}/workflows` | read/write | `Workflow` |
| PUT/DELETE | `/api/workflows/{id}` | write | |
| GET/POST | `/api/sandboxes/{sid}/automations` | read/write | `Automation` |
| PUT/DELETE | `/api/automations/{id}` | write | |
| POST | `/api/automations/{id}/run` | write | creates one run now |
| GET | `/api/automations/{id}/runs` | read | `[{"scheduled_for","state","task_id","error"}]` |
| GET/POST | `/api/sandboxes/{sid}/mcp` | read/write | `McpServer` (sandbox + inherited instance servers) |
| PUT/DELETE | `/api/mcp/{id}` | write (instance: admin) | |
| POST | `/api/mcp/{id}/test` | write | `{"ok","tools":["…"],"error"}` |

```json
Workflow = {"id","name","description","steps":["Write spec","Implement","Test","Open PR"],
            "instructions":"…","gates":{"require_spec":true,"require_tests":true},"scope":"instance|sandbox"}
Automation = {"id","name","cron":"0 9 * * *","utc_offset_minutes":120,"prompt","workflow_id|null",
              "harness":"claude|codex","enabled":true,"created_via":"ui|agent",
              "next_run_at":"…|null","last_run":{"scheduled_for","state","task_id|null"}|null}
McpServer = {"id","scope":"instance|sandbox","name","transport":"stdio|http","command|null",
             "args":[],"url|null","env_keys":["APIFY_TOKEN"],"header_keys":[],"enabled":true}
```

Secrets are write-only: create/update bodies may include `"env": {"KEY": "value"}` and
`"headers": {…}`; responses only list keys.

## Browser, editor, shell

- `WS /api/sandboxes/{sid}/browser` (write): client `{"type":"open","url"}`, `{"type":"navigate",
  "tab_id","url"}`, `{"type":"activate","tab_id"}`, `{"type":"close","tab_id"}`,
  `{"type":"mouse","tab_id","event":"down|up|move|wheel","x","y","button","dx","dy"}`,
  `{"type":"key","tab_id","event":"down|up|char","key","code","text","modifiers"}`,
  `{"type":"viewport","width","height"}`; server `{"type":"tabs","tabs":[{"id","title","url",
  "active","action_requested":{"reason","task_id"}|null}]}`, binary JPEG frames prefixed by
  `u8 3` + 32-byte tab id (ASCII, space padded) + image, `{"type":"action_requested",…}`.
- `GET /s/{sid}/editor/` (write): reverse-proxied VS Code (iframe in the editor panel).
- `POST /api/sandboxes/{sid}/shells` (write) → `{"id"}`; `WS /api/shells/{id}/terminal`.

## Administration, analytics, status

| Method | Path | Who | Notes |
| --- | --- | --- | --- |
| GET/POST | `/api/admin/users` | superadmin | `{"github_login","display_name?","is_instance_admin?"}` |
| PATCH | `/api/admin/users/{id}` | superadmin | `{"disabled?","is_instance_admin?"}` |
| GET | `/api/admin/accounts` | instance admin | `[Account]` |
| POST | `/api/admin/accounts` | instance admin | `{"provider":"anthropic|openai","label"}` → `{"account":Account,"login_id"}` then use the login terminal |
| PATCH/DELETE | `/api/admin/accounts/{id}` | instance admin | `{"enabled?","priority?","label?"}` |
| POST | `/api/admin/accounts/{id}/relogin` | instance admin | → `{"login_id"}` |
| GET | `/api/admin/github` | superadmin | `{"configured","app_slug","owner_login","install_url|null"}` |
| POST | `/api/admin/github/manifest` | superadmin | `{"owner_login","owner_kind"}` → `{"post_url","manifest"}` (browser posts the manifest form to GitHub) |
| GET | `/api/admin/github/manifest/callback?code` | superadmin | stores App credentials, 302 to install |
| GET | `/api/analytics/summary?sandbox&from&to` | read | aggregates |
| GET | `/api/analytics/tasks?format=json|csv` | read | per-task rows |
| GET | `/api/status` | signed in | VM, sandboxes, accounts usage |

```json
Account = {"id","provider","label","enabled","priority","status":"ok|relogin|error|limited",
  "usage":{"primary":{"used_percent":42.0,"resets_at":"…","window_minutes":300}|null,
           "secondary":{…}|null,"observed_at":"…|null"},
  "last_error":"…|null","in_use_by":0}
```

## UI event stream

`WS /api/events` → JSON: `{"type":"task.updated","task":TaskSummary}`, `{"type":"task.removed",
"task_id"}`, `{"type":"steps.updated","task_id"}`, `{"type":"message","task_id","message":Message}`,
`{"type":"sandbox.updated","sandbox":Sandbox}`, `{"type":"browser.action_requested","sandbox_id",
"tab_id","reason","task_id"}`, `{"type":"automation.run","automation_id","state","task_id"}`,
`{"type":"account.updated","account":Account}`, `{"type":"resync"}`.
