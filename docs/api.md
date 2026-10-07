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
| POST | `/api/sandboxes/{sid}/onboarding/skip` | admin | → `Sandbox` |
| DELETE | `/api/sandboxes/{sid}` | admin | → 204 (GitHub repository untouched) |

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
           "require_checks": true,
           "environments": [{"name": "dev", "after": "integration|production",
                             "command": "…|null", "verify": "manual|command"}]}
```

`PATCH /api/sandboxes/{sid}` validates the profile (branch names, distinct branches, environment
names, single-line commands ≤ 2000 bytes, `verify: command` needs a command, ≤ 8 environments) and
relabels it: `preset` stays a named preset only while every setting equals that preset's
defaults, otherwise it becomes `custom` (so switching Studio's `auto_promote` off yields `custom`).
`require_checks` absent or null means the preset's default (`false` for `studio`, else `true`);
responses always carry it; blank commands mean "no command". The sandbox profile is the only
source of environment commands (never the repository). `GET /api/delivery/presets` (signed in)
returns the three presets `standard`, `product` and `studio` as `[Profile]`.

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
| POST | `/api/tasks/{tid}/delivery/pr` | write | `{"title?","body?"}` (body optional) → `Delivery` (refused without a valid spec or with uncommitted changes) |
| POST | `/api/tasks/{tid}/delivery/merge` | write | → `Delivery` (requires the domain gates: open PR at the checked head, valid spec, no open steps, checks per profile) |
| POST | `/api/sandboxes/{sid}/delivery/promote` | write | → `{"result":"promoted|pr_opened","sha"}` |
| POST | `/api/sandboxes/{sid}/delivery/environments/{env}/runs` | write | `{"confirm":true}` → `EnvironmentRun` (includes `run_id`) |
| POST | `/api/sandboxes/{sid}/delivery/environments/{env}/confirm` | write | `{"confirm":true}` → `EnvironmentRun` (manual verification) |
| GET | `/api/sandboxes/{sid}/delivery/runs` | read | → `[EnvironmentRun]` (newest 20, output tails of 2 KiB) |
| GET | `/api/sandboxes/{sid}/delivery/runs/{run_id}` | read | → `EnvironmentRun` |

```json
GitOverview = {"integration_branch","production_branch","default_branch",
  "tasks":[{"task_id","title","branch","ahead","behind","files_changed","insertions","deletions","pr":PR|null}],
  "recent_commits":[{"sha","subject","author","at"}], "pulls":[PR], "notes":["…"]}
PR = {"number","url","title","state":"open|merged|closed","base","head",
      "checks":"none|pending|success|failure|unknown"}
Delivery = {"stage":"none|pr_open|merged_integration|env_done|promoted|production_done",
  "environment":"dev-vm|null", "pr":PR|null, "spec_path":"docs/specs/…|null",
  "spec_valid":bool, "spec_problems":["…"], "steps_open":0,
  "next_actions":["open_pr|merge|run_env:<name>|confirm_env:<name>|promote"],
  "blocked":[{"action","reason"}], "notes":["…"]}
EnvironmentRun = {"run_id","environment","kind":"command|manual",
  "state":"running|succeeded|failed|error","command":"…|null","commit":"<sha>|null",
  "requested_by":"login","output":"…","exit_code":0|null,"started_at","ended_at":"…|null"}
```

Behavior (SPEC-001 GIT-001/002, DLV-001..005, DSC-002/007/008; SPEC-002 §6.7, §8):

- **Reads degrade.** Git or GitHub failures add a line to `notes` and leave the affected lists
  empty or partial (`checks: "unknown"` when checks cannot be read) instead of failing. GitHub
  reads use the caller's token, else the GitHub App installation token. Task branch statistics
  compare the local task branch with `origin/<integration>` of the sandbox clone (refreshed by
  merges, promotions and environment runs, not by reads).
- **Stage reconciliation.** Reading a task's delivery aligns the stored stage with GitHub: a PR
  opened, merged or closed outside Yulius moves the stage (`none`→`pr_open`, →`merged_integration`,
  `pr_open`→`none`). `env_done` carries the environment in `environment`. `blocked` lists the
  actions of the current stage that are refused now, with the domain's reason.
- **Spec.** The spec is read from the task worktree at the registered `spec_path`; if none is
  registered, the first Markdown file under the project's specs directory (`yulius.toml`
  `[docs]`, default `docs/specs`) whose frontmatter names the task is registered. Paths escaping
  the worktree (also via symlinks) and files over 256 KiB are refused.
- **Open PR** pushes the task branch with an installation token, then opens the PR into the
  integration branch with the caller's token (title: task title unless given; body: the spec's
  Goal, spec path, Plan checklist and steps unless given, plus a link back to the task).
- **Merge** uses the profile's merge method and passes the head that was checked as the
  expected head; a PR head that differs from the task worktree's head is refused as stale.
  With `auto_promote` (Studio), the integration branch is promoted right after a merge done
  through Yulius once no integration environment is pending; the outcome is added to `notes`
  and audited. No other path merges, promotes or deploys on its own; agents only have the
  read-only `delivery_status` tool.
- **Promote** refuses while a merged task still has an integration environment pending, when
  production already equals integration, or when a fast-forward is impossible.
  `fast_forward`: production is fast-forwarded to the integration head (`result: "promoted"`,
  `sha` = new head). `merge_commit`: the first call opens the promotion PR integration →
  production (`result: "pr_opened"`, `sha` = PR URL); a later call merges it with a merge commit
  once its checks allow (`result: "promoted"`). Tasks waiting for promotion then advance to
  `promoted` (or `production_done` without production environments).
- **Environment runs** need `{"confirm": true}` on every request. For an environment with a
  command, the run starts in the background and `GET …/runs/{run_id}` reports progress; only one
  command run per sandbox runs at a time (`conflict` otherwise). An environment without a command
  is a manual checkpoint: the same request (or `…/confirm`) records the person's confirmation as
  a `manual` run. `…/confirm` also confirms command environments whose `verify` is `manual`.
  Environments must follow the profile order for tasks in their stage (`conflict` otherwise);
  when no task is in that stage the run is standalone. A successful command with `verify:
  command`, or a confirmation, advances every task for which the environment is due.
- **Environment command trust contract.** `command` is shell text configured only by repository
  administrators through the sandbox profile. Each run is audited (actor, environment, command)
  and executed as `/bin/sh -c <command>` in the sandbox's primary clone, checked out (detached)
  at the head of `origin/<integration>` (`after: integration`) or `origin/<production>` (`after:
  production`), with only `PATH`, `HOME` (the sandbox home), `LANG`, `YULIUS_ENVIRONMENT`,
  `YULIUS_BRANCH` and `YULIUS_COMMIT` set, stdin closed, the last 1 MiB of combined output kept,
  and a 30-minute limit after which the whole process group is killed and the shell reaped
  (`state: "failed"`). It runs with the server's privileges, without sandbox containment; the
  clone's `.git` and the sandbox home are writable by the sandbox's agents, so the checkout
  ignores global/system Git config and disables hooks and fsmonitor (repository-local config
  such as filter drivers is still read). Commands are visible to every reader of the sandbox:
  keep secrets out of them. Runs left `running` by a server restart become `error`.

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
