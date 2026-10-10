# Yulius HTTP and WebSocket API (v1)

Contract between the web UI (and future desktop shells, SPEC-001 OPS-002) and `yulius serve`.
It refines [SPEC-002 §7.7–§7.9](../.ai/specs/SPEC-002-yulius-architecture.md). Rust DTOs live in
`crates/yulius-contracts/src/api.rs`; when they disagree with this page, fix both in one change.

## Conventions

- JSON bodies (`Content-Type: application/json`), ≤ 1 MiB. Timestamps are RFC 3339 UTC strings.
- Authentication: the session cookie (browser) or `Authorization: Bearer yul_…` (CLI
  tokens). The cookie is `__Host-yulius_session` when the public URL is HTTPS (host-only and
  `Secure`: no other host can plant or shadow it) and `yulius_session` over plain HTTP.
  State-changing requests from cookie sessions must send `X-Yulius-CSRF: <csrf_token>` from
  `GET /api/me`. A cookie session is refused (403) for a request that announces an `Origin` other
  than the public URL (or an allowlisted origin), or that the browser marks `Sec-Fetch-Site:
  same-site` or `cross-site`, unless it is a GET or HEAD navigation: a page on a live preview
  origin (same-site with Yulius) can never use the user's session.
- WebSockets require a valid session and an `Origin` equal to the public URL.
- Errors: HTTP status plus `{"error": {"code": "snake_case", "message": "human text"}}`. Codes:
  `unauthenticated` (401), `forbidden` (403), `not_found` (404; also for invisible sandboxes),
  `conflict` (409; racing state changes), `invalid` (422), `capacity` (429), `upstream` (502,
  GitHub/provider failure), `unavailable` (503; a local prerequisite such as openvscode-server
  is missing or disabled, the message says which), `internal` (500).
- Access levels: `read`, `write`, `admin` (SPEC-001 ACC-007). Endpoints note the minimum level.
  The superadmin has `admin` on every sandbox, with or without a GitHub link (ACC-012); everyone
  else has the level GitHub grants, and no GitHub identity means no sandbox.


Other frontends (for example a desktop app, OPS-002) use bearer tokens from the device flow (`POST /api/device/start`, approval in the web UI, `POST /api/device/poll`, as used by `yulius login`) and must be listed with `yulius serve --allowed-origin <origin>` (`YULIUS_ALLOWED_ORIGINS`, comma-separated): those origins get CORS responses (methods GET/POST/PUT/PATCH/DELETE; headers `Authorization`, `Content-Type`, `X-Yulius-CSRF`) and may open WebSockets; every other cross-origin WebSocket is refused with 403.

## Setup and session

| Method | Path | Body → Response |
| --- | --- | --- |
| GET | `/api/setup/status` | → `{"setup_required": bool}` |
| POST | `/api/setup` | `{"token", "password", "password_confirm", "confirm_saved": true}` → 201 `Me` + `"recovery_code": "123456"` (shown once) + cookie. Refused once a superadmin exists. |
| POST | `/api/auth/password` | `{"password"}` → `Me` + cookie (superadmin only; rate limited) |
| GET | `/api/auth/recovery` | → `{"available": bool, "locked": bool, "attempts_left": 0..3}` (ACC-011) |
| POST | `/api/auth/recover` | `{"code", "password", "password_confirm"}` → `{"recovery_code"}` (a new code, shown once); sets the superadmin password and ends every superadmin session. 422 for a wrong code (with the attempts left); 403 when no code is set or recovery is locked — the third wrong code locks it permanently |
| POST | `/api/admin/recovery-code` | superadmin session, `{"password"}` → `{"recovery_code"}`: replaces the code and re-enables a locked recovery; 403 for a wrong password |
| GET | `/api/auth/github/start?next=/[&prompt=select_account]` | → 303 to GitHub; `prompt=select_account` makes GitHub show its account picker (sign in with another GitHub account) |
| GET | `/api/auth/github/callback?code&state` | → 303 to `next`, sets cookie. Never answers with an error body: a refusal → 303 to `/signin?error=<reason>&next=…`, where `reason` is `not_admitted` (with `&login=<refused GitHub login>`), `disabled`, `expired` (unknown, used or expired state — e.g. a reload of the callback), `denied` (GitHub `error=access_denied`) or `failed` (GitHub or server error, logged) |
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
{"user_code","approve":bool}` (web session) → 204.

## Sandboxes

| Method | Path | Level | Body → Response |
| --- | --- | --- | --- |
| GET | `/api/sandboxes` | read | → `[Sandbox]` (only visible ones) |
| GET | `/api/github/repositories` | — | → `[{"owner","name","private","access","already_sandbox"}]` (the superadmin: every owner repository the App installation can access, `admin`; others need a GitHub identity, else 403) |
| GET | `/api/templates` | — | → `[{"id","name","description"}]` |
| POST | `/api/sandboxes` | admin on repo (superadmin: a repository the App can access) / owner | `{"mode":"import","repo":"owner/name"}` or `{"mode":"create","name","template","description"}` → 201 `Sandbox` |
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
           "require_checks": true,
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
natively on Continue. Only Claude Code sessions are imported.

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
| POST | `/api/sandboxes/{sid}/tasks` | write | `{"prompt","harness?":"claude","mode?":"goal|prompt","workflow_id?","model?","effort?","title?","after_task_id?"}` → 201 `TaskSummary`; Yulius runs Claude Code only (another `harness` is 422); `mode` defaults to `goal` (TSK-010); with `after_task_id` (a task of the sandbox that has not ended; 422 otherwise, 429 beyond 32 waiting) the task waits as `chained` until that task is Done (TSK-012) |
| POST | `/api/sandboxes/{sid}/intake/check` | write | `{"prompt"}` → `{"verdict":"one|several","checked":bool}`: whether a new task mixes separate workstreams (TSK-011). `checked` is false — and the verdict `one` — for prompts under 40 words, without a Jev key and when Jev fails or takes over 2.5 s |
| POST | `/api/sandboxes/{sid}/intake/proposal` | write | `{"prompt"}` → `{"tasks":[{"title","prompt","after_previous":bool}]}`: Claude's smallest split, 2–4 tasks grouped by area carrying their items word for word; `[]` means keep one task. 409 when proposals are not configured or no account is connected, 429 while 2 proposals run, 502 when Claude fails or its proposal is unusable (more than 4 tasks, one per item, an item missing) |
| POST | `/api/tasks/{tid}/start-now` | write | → `TaskSummary`; a `chained` task starts now whatever its predecessor does (409 otherwise). Cancel it with `/stop` |
| POST | `/api/tasks/{tid}/held-prompts/{hid}` | write | `{"action":"parallel|after|send|skip_after|revert|dismiss","skip_step_ids?":[…],"confirm?":bool}` → `{"task":TaskSummary,"created":TaskSummary|null,"note"}`; one decision per held prompt (409 for a second). `send` needs a running task; `skip_after` marks the given open steps `skipped`; `revert` needs `"confirm":true` (422 otherwise) — see "Task intake" below |
| GET | `/api/tasks/{tid}` | read | → `TaskDetail` |
| POST | `/api/tasks/{tid}/stop` | write | → `TaskSummary` |
| POST | `/api/tasks/{tid}/continue` | write | `{"account_id?"}` → `TaskSummary`; resumes the latest session natively, on the given Claude account or the best one |
| POST | `/api/tasks/{tid}/switch` | write | `{"account_id?"}` → `TaskSummary`; moves a running task to another Claude account (the best one when absent) with a native resume |
| POST | `/api/tasks/{tid}/archive` · `/restore` | write | → `TaskSummary` |
| DELETE | `/api/tasks/{tid}` | write | → 204; deletes a resting task permanently with its records, run directories, local worktree and branch (TSK-009); 409 while it runs |
| POST | `/api/tasks/{tid}/feedback` | read | `{"verdict":"correct|incorrect","comment"}` → 204 |
| GET | `/api/tasks/{tid}/messages` | read | → `[Message]`: messages between this task and other tasks (`message_send`) and its agent's messages to people (`to="people"`, `to_task_id` null). People talk to the agent in its terminal; there is no route that posts a person's message to it (removed 2026-10-10, `POST` is 405) |
| GET | `/api/archived-tasks` | read | → `[TaskSummary]` across visible sandboxes |
| WS | `/api/tasks/{tid}/terminal` | read (input: write) | terminal protocol below |

```json
TaskSummary = {
  "id": "…", "sandbox_id": "…", "number": 12, "title": "Checkout API",
  "harness": "claude", "status": "running|…(see domain wire names)",
  "activity": "working|waiting|needs_attention|null",
  "branch": "feat/checkout-api", "origin": "ui|automation|import|onboarding|agent",
  "created_by": {"login": "…"}, "created_at": "…", "updated_at": "…",
  "steps": {"done": 3, "total": 7, "current": "Implement API|null"},
  "unread_messages": 0, "archived": false, "account_label": "Claude #2|null",
  "mode": "goal|prompt",
  "resumes_at": "2026-10-09T08:20:00Z|null",
  "after": ChainLink|null, "next": [ChainLink],
  "held_prompt": {"id","class":"new_scope|drops_scope|redirect","excerpt","percent","typed_by|null","created_at"}|null
}
ChainLink = {"task_id|null","number","title|null","status|null","blocked":"reason|null"}
```

`after` is the task this one runs after (`task_id` null once that task was deleted), `next` the
tasks chained after it. On a `chained` task, `after.blocked` says why it waits for a person: its
predecessor stopped, failed, closed before it was done or was deleted (TSK-012).

`status` `waiting_capacity` means the task is **paused** for subscription capacity (ROT-010,
SPEC-002 ADR-17): every Claude subscription is at 98 % or limited in a window binding the task, so
the harness holds the agent at its turn boundary (or the task waits to start); nothing is sent to
the agent, and the task continues by itself. `resumes_at` is set only then: the earliest known
time a subscription has capacity for it again. The UI shows "Paused: waiting for subscription
capacity (resets ~14:20)" outside Needs you. A Continue with no capacity answers `200` with the
task paused, not an error.

```json
TaskDetail = TaskSummary + {
  "spec_path": "docs/specs/2026-10-07-checkout-api.md|null",
  "worktree": "…", "attempts": [{"seq","harness","account_label","account_id (proposed)","launch_kind","state",
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
`failed_start_timeout`, `failed_precondition`, `chained`.

### Task intake

Optional (SPEC-002 ADR-16): without a Jev key (`YULIUS_JEV_API_KEY_FILE`, docs/operations.md)
nothing is checked and nothing is held.

- **New tasks (TSK-011).** The composer calls `/intake/check` after Enter. On `several` it shows
  the split card and calls `/intake/proposal`, then creates the tasks with
  `POST /api/sandboxes/{sid}/tasks`: in parallel, or as a chain (each with `after_task_id` of the
  one before).
- **Chains (TSK-012).** A `chained` task starts by itself (`chained → queued`) when its predecessor
  is Done: a finished turn and a pull request merged, or open with checks green or none required.
- **Typed prompts (TSK-013).** The `UserPromptSubmit` hook classifies a person's prompt. `context`
  passes; `new_scope`, `drops_scope` and `redirect` are held. Claude Code shows the reason, and the
  text stays in its input box. The task's `held_prompt` is set and the UI shows the choices:
  - `parallel`: a new task with the prompt (same harness, model, effort and mode);
  - `after`: a new task chained after this one;
  - `skip_after`: the given steps become `skipped` (with the note "Ruled out by the user: …" by
    `user:<login>`), the agent gets a "Scope changed by the user" message, and a chained task gets
    the prompt;
  - `revert`: stops the task (reaped within 30 s, else 409 and nothing changes), commits its work to
    a new branch `yulius/reverted/<number>-<ms>`, resets its worktree to where it forked from the
    integration branch, archives it, and starts a new task with the prompt; `note` names the
    branch;
  - `send`: the identical prompt passes once (within 10 minutes), and Yulius presses Enter in the
    session to resubmit it;
  - `dismiss`: closes the card.

  Prompts that always pass: slash commands, at most 3 words, Claude Code's own tagged turns, the
  task's own prompt, prompts nobody typed into the terminal in the last 60 s, and any uncertain
  or failed check.

### Terminal WebSocket

Client → server text frames:

```json
{"type":"hello","last_seq":null,"cols":120,"rows":40}
{"type":"input","data":"text or escape sequences","seq":17}
{"type":"input","data":"\u001b[<65;10;5M","seq":18,"scroll":true}
{"type":"resize","cols":120,"rows":40}
{"type":"take_control"}
```

Input makes its viewer the controller (the last viewer to type sets the size), except
`"scroll":true`: a wheel turn forwarded to a full-screen app, which neither takes control nor
resizes the terminal.

The connection may carry the viewer's size: `WS /api/tasks/{tid}/terminal?cols=120&rows=40`. A
writer whom no other viewer controls gets it applied before the first `state` and snapshot (and
then controls the size); when that changed the size, the snapshot waits up to 400 ms for the
program's redraw. Both or neither; unparsable values are 400. `hello` and `resize` with the size
the terminal already has change nothing.

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
| GET | `/api/tasks/{tid}/changes` | read | → `TaskChanges`: the task worktree (committed, uncommitted and untracked work; symlinks listed, never followed) against the merge base with `origin/<integration>`; 409 when the worktree is gone |
| GET | `/api/tasks/{tid}/commits` | read | → `[TaskCommit]`: the task branch's commits not on the integration branch, newest first (at most 100) |
| POST | `/api/tasks/{tid}/delivery/pr` | write | `{"title?","body?"}` (body optional) → `Delivery` (refused without a valid spec or with uncommitted changes); opened with the caller's GitHub account, the superadmin's by the App |
| POST | `/api/tasks/{tid}/delivery/merge` | write | → `Delivery` (requires the domain gates: open PR at the checked head, valid spec, no open steps, checks per profile); acts as the caller on GitHub: 403 with a message for the superadmin without a GitHub identity |
| POST | `/api/sandboxes/{sid}/delivery/promote` | write | → `{"result":"promoted|pr_opened","sha"}`; acts as the caller on GitHub: 403 with a message for the superadmin without a GitHub identity |
| POST | `/api/sandboxes/{sid}/delivery/environments/{env}/runs` | write | `{"confirm":true}` → `EnvironmentRun` (includes `run_id`) |
| POST | `/api/sandboxes/{sid}/delivery/environments/{env}/confirm` | write | `{"confirm":true}` → `EnvironmentRun` (manual verification) |
| GET | `/api/sandboxes/{sid}/delivery/runs` | read | → `[EnvironmentRun]` (newest 20, output tails of 2 KiB) |
| GET | `/api/sandboxes/{sid}/delivery/runs/{run_id}` | read | → `EnvironmentRun` |

```json
GitOverview = {"integration_branch","production_branch","default_branch",
  "tasks":[{"task_id","title","branch","ahead","behind","files_changed","insertions","deletions","pr":PR|null}],
  "recent_commits":[{"sha","subject","author","at"}], "pulls":[PR], "notes":["…"],
  "checks_access":ChecksAccess|null}
PR = {"number","url","title","state":"open|merged|closed","base","head",
      "checks":"none|pending|success|failure|unknown"}
ChecksAccess = {"repo":"owner/name", "missing":["Checks","Commit statuses"],
  "read_from":["check runs","commit statuses","Actions runs"], "message":"…",
  "settings_url":"https://github.com/…/settings/apps/<slug>/permissions|null"}
Delivery = {"stage":"none|pr_open|merged_integration|env_done|promoted|production_done",
  "environment":"dev-vm|null", "pr":PR|null, "spec_path":"docs/specs/…|null",
  "spec_valid":bool, "spec_problems":["…"], "steps_open":0,
  "next_actions":["open_pr|merge|run_env:<name>|confirm_env:<name>|promote"],
  "blocked":[{"action","reason"}], "notes":["…"], "checks_access":ChecksAccess|null}
EnvironmentRun = {"run_id","environment","kind":"command|manual",
  "state":"running|succeeded|failed|error","command":"…|null","commit":"<sha>|null",
  "requested_by":"login","output":"…","exit_code":0|null,"started_at","ended_at":"…|null"}
```

Behavior (SPEC-001 GIT-001/002, DLV-001..005, DSC-002/007/008; SPEC-002 §6.7, §8):

- **Reads degrade.** Git or GitHub failures add a line to `notes` and leave the affected lists
  empty or partial (`checks: "unknown"` when checks cannot be read) instead of failing. GitHub
  reads use the caller's token, else the GitHub App installation token.
- **Checks and App permissions (#111).** A pull request head's checks come from three sources,
  each behind its own GitHub App permission: check runs (Checks: read), the combined commit
  status (Commit statuses: read) and, only when check runs cannot be read, the head's GitHub
  Actions workflow runs (Actions: read). A source answering 403 "Resource not accessible by
  integration" is skipped and named in `checks_access` (once per repository, not as a note);
  any other GitHub error still makes the checks `unknown` with a note. The readable sources
  decide what they report (a failure, a run in progress, all passed), but nothing reported while
  a source is unreadable, or no source readable, is `unknown`: never passed. `unknown` blocks a
  merge or release that requires checks ("the pull request's checks can't be read, so they don't
  count as passed"). `checks_access.settings_url` (the App's permission page) is set for
  instance administrators only; the UI shows the message to them as one line they can dismiss
  for the session. [Operations](operations.md#github-app-permissions) lists the App's permissions. Task branch statistics
  compare the local task branch with `origin/<integration>` of the sandbox clone (refreshed by
  merges, promotions and environment runs, not by reads).
- **Overview stages (proposed).** The Git page (UI-025) reads `GET /api/tasks/{tid}/delivery`
  once per overview row to learn each task's stage. Adding `stage`, `environment`, `next_actions`
  and `blocked` to `GitOverview.tasks[]` lets it skip those reads (it uses them when present);
  a `release` summary (production and integration heads, what is waiting, the last release:
  who, when, which tasks) would let it say *when* and *by whom* production last changed.
- **Task changes.** The Git page's review of a task (UI-031: Git → a task → Changes, Commits,
  Branch, then "Go to task") reads `GET /api/tasks/{tid}/changes` and `GET /api/tasks/{tid}/commits`.
  Changes are bounded: at most 300 files with hunks, 3 000 lines per file and 20 000 lines in all
  (`truncated: true` when cut; counts stay exact); untracked files over 256 KiB, binary files and
  symbolic links carry no hunks. A server without these routes answers 404 and the review shows
  its Branch tab alone.

  ```json
  TaskChanges = {"base":"<integration>","head":"<task branch>","base_sha","head_sha",
    "ahead":0,"behind":0,"uncommitted":0,"truncated":false,
    "files":[{"path","old_path":"…|null","status":"added|modified|deleted|renamed",
      "insertions":0,"deletions":0,"binary":false,
      "hunks":[{"header":"@@ -12,7 +12,19 @@","lines":[{"kind":"ctx|add|del","old":12|null,"new":12|null,"text":"…"}]}]}]}
  TaskCommit = {"sha","subject","author","at","files":0,"insertions":0,"deletions":0}
  ```
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
| GET | `/api/sandboxes/{sid}/skills/{name}` | read | `{"name","content"}` (`SKILL.md`, at most 512 KiB) |
| PUT | `/api/sandboxes/{sid}/skills/{name}` | write (instance skills: instance admin) | `{"enabled":bool}` → 204. Repository skills → 422 (always available inside the repository). |

The web UI edits and creates repository skills as files: a repository skill is
`.agents/skills/<name>/SKILL.md` (or `.claude/skills/…`) in the sandbox repository, so its skill page
(UI-033) reads and saves it with the built-in editor's files API (root `sandbox`, the save carrying
the read's `sha256`) and New skill (UI-035) creates the folder and the file there. Instance skills
and the Open Mercato pack have no write API and are read-only in the UI.
| GET/POST | `/api/sandboxes/{sid}/workflows` | read/write | `Workflow`; POST → 201. `"scope":"instance"` creates an instance workflow (instance admins). |
| PUT/DELETE | `/api/workflows/{id}` | write (instance: admin) | PUT replaces name, description, steps, instructions, gates → `Workflow`; DELETE → 204 |
| GET/POST | `/api/sandboxes/{sid}/automations` | read/write | `Automation`; POST → 201 |
| PUT/DELETE | `/api/automations/{id}` | write | PUT replaces the editable fields → `Automation`; DELETE → 204 (removes run history; tasks stay) |
| POST | `/api/automations/{id}/run` | write | creates one run now (also when disabled) → `AutomationRun` |
| GET | `/api/automations/{id}/runs` | read | `[AutomationRun]`, newest first (at most 100) |
| GET/POST | `/api/sandboxes/{sid}/mcp` | read/write | `McpServer` (sandbox + inherited instance servers); POST body `McpServerInput` → 201 |
| PUT/DELETE | `/api/mcp/{id}` | write (instance: admin) | PUT `McpServerInput` → `McpServer`; DELETE → 204 |
| POST | `/api/mcp/{id}/test` | write (instance: admin) | `{"ok","tools":["…"],"error"}`; HTTP ≤ 5 s, stdio ≤ 10 s then the process group is killed. On a contained instance stdio tests need an instance admin (they run outside the containment). |

```json
Workflow = {"id","name","description","steps":["Write spec","Implement","Test","Open PR"],
            "instructions":"…","gates":{"require_spec":true,"require_tests":true},"scope":"instance|sandbox"}
Automation = {"id","name","cron":"0 9 * * *","utc_offset_minutes":120,"prompt","workflow_id|null",
              "harness":"claude","enabled":true,"created_via":"ui|agent",
              "next_run_at":"…|null","last_run":AutomationRun|null}
AutomationRun = {"scheduled_for","state":"scheduled|task_created|skipped_previous_active|skipped_disabled|failed",
                 "task_id|null","error|null"}
McpServer = {"id","scope":"instance|sandbox","name","transport":"stdio|http","command|null",
             "args":[],"url|null","env_keys":["APIFY_TOKEN"],"header_keys":[],"enabled":true}
McpServerInput = {"name","transport","command?","args?","url?","env?":{"KEY":"value"},
                  "headers?":{"Name":"value"},"enabled?":bool,"scope?":"sandbox|instance"}
```

Two built-in instance workflows, `Feature` (`wf-feature`) and `Quick task` (`wf-quick-task`), are
seeded whenever the instance has no instance workflow. Automation `cron` is a five-field
expression (`yulius-domain::cron`), `utc_offset_minutes` is within ±1080, and at most 100
automations exist per sandbox. A run whose automation's previous task is still active is
recorded as `skipped_previous_active`.

MCP secrets are write-only: responses only list key names. On update, an omitted `env`/`headers`
keeps the stored values; a given map replaces all values of that map (`{}` clears it). Names are
1–64 letters, digits, `-` or `_`, unique per scope ignoring case and `-`/`_`; `yulius` is reserved
for the always-attached Yulius tool server. A sandbox server overrides an instance server of the
same name for that sandbox (also when disabled). Agent runs receive HTTP secrets only as an
`Authorization: Bearer` token; other headers are used by the test endpoint but not yet passed to
Claude Code.

## Browser, editor, shell

- `WS /api/sandboxes/{sid}/browser` (write): the sandbox's Chrome — the browser agents drive —
  streamed (SPEC-002 ADR-15). One browser per sandbox, started on first use; with a virtual
  display it runs headful on its own Xvfb and the connection receives its video and sends real
  input, without one (headless hosts) JPEG frames and CDP input.
  - Client: `{"type":"open","url"}` (`""` opens the welcome page), `{"type":"navigate","tab_id",
    "url"}`, `{"type":"activate","tab_id"}`, `{"type":"close","tab_id"}`, `{"type":"history",
    "tab_id","delta":-1|1}` (back, forward; nothing beyond the ends), `{"type":"reload","tab_id"}`,
    `{"type":"mouse","tab_id","event":"down|up|move|wheel","x","y","button":"left|middle|right|
    back|forward|none","dx","dy"}` (CSS pixels of the viewport; on a display moves, presses and
    releases are XTest input wherever the pointer is and the wheel scrolls the shown tab by
    `dx`/`dy` pixels), `{"type":"key","tab_id","event":"down|up|char","key","code","text",
    "modifiers"}` (on a display: printable ASCII by `key` as a US-layout key with Shift as needed,
    with Ctrl/Alt/Meta held the user's Shift is kept; other keys by `code`; a `key` no US key types
    and `char` text up to 64 characters are typed as real key presses in order; longer `char` text,
    a paste, is inserted with CDP `Input.insertText`; lock keys and AltGraph are ignored),
    `{"type":"copy","tab_id"}` (answered with `clipboard`), `{"type":"viewport","width",
    "height"}` (the panel size in CSS pixels; shared by every viewer, clamped to the configured
    maximum), `{"type":"view","mode":"video|pixels"}` (default `video`), `{"type":"watch",
    "tab_id"|null}` (the tab the panel shows: the display shows one tab, so this brings it to the
    front for every viewer; `null` stops this connection's stream).
  - Server: `{"type":"tabs","tabs":[{"id","title","url","active","action_requested":{"reason",
    "task_id"}|null}]}` (on connect and after every change; `active` is the tab the display
    shows — a page opening a page brings it to the front), `{"type":"viewport","width","height"}`
    (CSS pixels, on connect and after every change), `{"type":"stream","mode":"video|pixels",
    "reason"?}` (what this connection receives and, for `pixels` after asking for video, why),
    `{"type":"video_config","codec","description","width","height"}` (a WebCodecs
    `VideoDecoderConfig`: codec string such as `avc1.42c028`, the `avcC` record base64, the coded
    size in device pixels; before the first frame and after every encoder restart, ordered with the
    frames), binary video frames `u8 4`, `u8` flags (bit 0: keyframe), `u64` big-endian
    timestamp in µs, then one H.264 access unit as length-prefixed NAL units (the first frame after
    a configuration is a keyframe), binary JPEG frames (pixels) `u8 3` + 32-byte tab id (ASCII,
    space padded) + image, `{"type":"clipboard","text"}` (the selection, at most 1 MiB),
    `{"type":"agent_activity","tab_id","x","y","action":"click|type|open|navigate","label"}` (an
    agent acted; `x`/`y` in viewport CSS pixels or null), `{"type":"action_requested","tab_id",
    "reason","task_id"}`, `{"type":"error","code":"invalid|url_refused|not_found|capacity|
    upstream|internal","message"}`.
  - Bounds: a viewer that falls behind skips to the next keyframe (at most 300 queued frames,
    8 MiB, 30 frames behind); JPEG frames keep the newest 4; a client that stops reading JSON
    messages is closed with 4008. Input: 500 events per second (burst 1000); moves and wheel
    beyond are dropped, other input gets an `invalid` error. Keys and buttons a viewer held are
    released when it leaves. Access is re-checked every 60 s (close 4003). When Chromium is
    missing or the browser stops, the server sends an `upstream` error with the reason and closes
    with 1011. Addresses of Yulius itself (its public origin and its subdomains, every loopback
    form of its listeners) are refused with `url_refused`, and pages' own requests to them fail as
    blocked (SPEC-001 BRW-008).
- New tabs of the sandbox Chrome open `GET /browser/welcome?sandbox=<id>` on the internal
  (loopback) listener: the sandbox name and links to its listening dev-server ports (probed on
  127.0.0.1 every 5 s via `/browser/welcome/ports`; Yulius's own ports are never listed).
- Live previews (SPEC-002 ADR-14; since ADR-15 used by the panel's "Open live in a new window"
  for a sandbox dev server). Every port of a sandbox's range has a **preview origin**
  `<scheme>://<port>-<key>.<preview domain>[:<port>]` (`key`: 12 lowercase hex characters,
  stable per sandbox; preview domain: the public URL's host unless `--preview-domain` says
  otherwise). The public listener sends every request whose `Host` is a preview host to the
  preview proxy, never to the UI or API.
  - `GET /api/sandboxes/{sid}/previews` (write) → `{"enabled", "reason?", "origin_template?":
    "https://{port}-0123456789ab.yulius.example.com", "first_port", "last_port", "listening":
    [port], "excluded": [port]}` (`excluded`: ports of the range that belong to Yulius).
  - `GET /api/previews/authorize?host=<preview host>&to=<path>` (write; a navigation): 303 to
    `https://<host>/__yulius/handshake?t=<token>&to=<path>` with a single-use token valid for
    60 s on that host only. Unknown hosts 404; not signed in, no write access or a refused port:
    303 to the preview's `/__yulius/denied?why=signin|access`.
  - On a preview origin, `/__yulius/` is the proxy's: `handshake` (trades the token for the
    `__Host-yulius_preview` cookie — `yulius_preview` over HTTP —, host-only, `HttpOnly`,
    `SameSite=Lax`, 12 h; then 303 to `landing`), `landing?to` (303 to `to` when the cookie
    stuck, else an explanation), `bridge.js` (the script appended to proxied documents),
    `ping` (204, reachability), `denied`. Any other request needs the grant cookie: a GET
    navigation without one gets 302 to `/api/previews/authorize`, anything else 401. Grants are
    re-checked against the Yulius session and write access every 30 s and live in memory.
  - Refused: a host that names no sandbox (404), a port outside the sandbox's range or one of
    Yulius's own (403), credentialed requests and WebSockets from other origins than the
    preview itself and the same sandbox's other previews (403; navigations from anywhere are
    fine), an upstream that answers with this server's `X-Yulius` tag (508).
  - Forwarded to `127.0.0.1:<port>` (HTTP/1.1, WebSocket upgrades spliced, bodies streamed):
    `Host: localhost:<port>`; `Origin` and `Referer` of the sandbox's previews become
    `http://localhost:<port>`; the grant and Yulius cookies, hop-by-hop and `X-Forwarded-*`
    headers are removed; documents are requested without compression.
  - Responses: `X-Frame-Options`, `Strict-Transport-Security` and every CSP `frame-ancestors` are
    removed and `Content-Security-Policy: frame-ancestors 'self' <public URL>` added;
    `http(s)`/`ws(s)` URLs on the VM's loopback with a sandbox port in `Location`,
    `Content-Location`, `Refresh`, `Link`, `Access-Control-Allow-Origin` and CSP sources point at
    the preview origins; `Set-Cookie` loses `Domain` (and gains `Secure` over HTTPS); HTML
    documents get `<script src="/__yulius/bridge.js" async>` appended. The bridge posts
    `{"source":"yulius-preview","type":"location","href","title","back","forward"}` (and
    `type: "f6"`) to the public URL's origin only and accepts `{"source":"yulius","type":
    "back|forward|reload|hello"}` only from the window that frames it.
  - Bounds: connect 5 s, response head 120 s, 512 requests awaiting a head and 256 WebSockets
    (503 beyond).
- `GET /internal/preview-tls?domain=<host>` on the internal (loopback) listener: the `ask`
  endpoint of an on-demand TLS proxy; 200 for a preview host of an existing sandbox port,
  404 otherwise (docs/operations.md).
- Agents print `<public_url>/s/<slug>/browser?tab=<tab_id>` (returned by the
  `browser_request_user_action` MCP tool, which also marks the task `needs_attention` and
  publishes `browser.action_requested`); the web UI opens the browser panel on that tab.
- Built-in editor (write; EDT-001..004). `root` is `sandbox` (default, the repository) or
  `task:<task id>` (that task's worktree); `path` is relative (normal components only; `.git`
  is refused, 403; a path leading outside the folder, also through symbolic links, is 403).
  `GET /api/sandboxes/{sid}/files?root&path` → `{"path","entries":[{"name","path",
  "kind":"file|dir|symlink","size"}],"truncated"}` (directories first; `.git` hidden; at most
  2000 entries). `GET /api/sandboxes/{sid}/files/content?root&path` → `{"path","content":
  "…|null","binary","too_large","size","sha256"}` (UTF-8 text up to 2 MiB). `PUT
  /api/sandboxes/{sid}/files/content` `{"root?","path","content","base_sha256?"}` →
  `{"path","sha256","size"}`: atomic; 409 when the file changed since `base_sha256` or, without
  a base, when it exists. `POST /api/sandboxes/{sid}/files` `{"root?","path","kind":"file|dir"}`
  → 201 entry (409 when it exists). `DELETE /api/sandboxes/{sid}/files?root&path` → 204 (files,
  links and directory trees; never the folder itself). 409 before the repository is cloned.
  The editor UI (Monaco) keeps each user's open files and unsaved text in their browser only.
- Terminals (sandbox shells; UI-015, UI-016): each is private to the user who opened it and to its
  sandbox. `Shell = {"id","name","task_id|null","created_at"}`.
  - `GET /api/sandboxes/{sid}/shells` (write) → `[Shell]`: the caller's live shells in this
    sandbox, oldest first (never another user's or another sandbox's).
  - `POST /api/sandboxes/{sid}/shells` (write) `{"task_id?","name?"}` (or an empty body) → 201
    `Shell`: a login shell in the sandbox repository or that task's worktree, as the caller's
    sandbox identity (their home; uid 0 with `sudo` when contained). The name defaults to
    "Terminal N", the lowest number free among the caller's shells there. At most 8 live shells
    per user and sandbox (429); 409 before the repository is cloned.
  - `PATCH /api/sandboxes/{sid}/shells/{id}` (write) `{"name"}` → `Shell`: 1–40 characters, no
    control characters (422). `DELETE /api/sandboxes/{sid}/shells/{id}` (write) → 204: the shell
    leaves the list at once and its process is ended (Ctrl-C, SIGTERM, SIGKILL). Both are 404 for
    another user's shell and for a shell of another sandbox.
  - `WS /api/shells/{id}/terminal[?cols=&rows=]` (only the shell's creator; 404 for everybody
    else) speaks the terminal protocol (`state` has `attempt_seq` and `harness` `null`). A shell
    ends when its process exits (the server sends `exit` and closes), when it is closed, or after
    24 h without viewers; it keeps running across a server restart. Shells never change tasks.

## Administration, analytics, status

| Method | Path | Who | Notes |
| --- | --- | --- | --- |
| GET/POST | `/api/admin/users` | superadmin | GET → `[AdminUser]` (the superadmin is not listed); POST `{"github_login","display_name?","is_instance_admin?"}` → 201 `AdminUser` |
| PATCH | `/api/admin/users/{id}` | superadmin | `{"disabled?","is_instance_admin?"}` → `AdminUser` |
| GET | `/api/admin/accounts` | instance admin | `[Account]` |
| POST | `/api/admin/accounts` | instance admin | `{"provider":"anthropic","label"}` → 201 `{"account":Account,"login_id"}` (status `pending_login`) then use the login terminal; nothing is created when the login cannot start |
| PATCH | `/api/admin/accounts/{id}` | instance admin | `{"label"}` (1–80 chars) → `Account`; the pool has no priority, order or on/off switch (F 2026-10-10): any other field is `invalid` (422) |
| DELETE | `/api/admin/accounts/{id}` | instance admin | → 204; 409 while a live task uses it; cancels its running login |
| POST | `/api/admin/accounts/{id}/relogin` | instance admin | → `{"account":Account,"login_id"}`; replaces a running login of the account |
| WS | `/api/admin/accounts/login/{login_id}/terminal` | instance admin | login terminal (below) |
| POST | `/api/admin/accounts/{id}/usage` | instance admin | → `AccountView`: asks Claude for the account's usage windows (`/api/oauth/usage`) and stores them; at or above 98 % in a window its tasks move to the best other subscription at their next turn boundary (ADR-17); 502 with the reason (also kept as `last_error`) when the provider refuses or fails. `AccountView` also carries `tokens_24h` / `tokens_7d` = `{"runs","tasks","input|null","output|null","cache_read|null","cache_write|null"}` observed by Yulius (ROT-009) The server also refreshes every signed-in account in the background (every 10 min; on HTTP 429 it waits for `Retry-After`, else 30 min) and keeps the last known windows on failure; a limit the CLI reports fills its window at 100 % until the reset (ROT-009). |
| GET | `/api/capacity` | signed in | → `CapacityView`: the subscription pool, burn rate, forecast and recommendation (ROT-013, formula in SPEC-002 ADR-17 §2.4) |
| GET | `/api/admin/github` | superadmin | `{"configured","app_slug","owner_login","install_url|null","installed"}` |
| POST | `/api/admin/github/manifest` | superadmin | `{"owner_login","owner_kind":"org|user"}` → `{"post_url","manifest"}` (browser posts the manifest form to GitHub; `post_url` carries a one-time `state`, 30 min); 409 when an App is configured; 422 for a non-https public URL |
| GET | `/api/admin/github/manifest/callback?code&state` | superadmin | converts the code, stores the sealed App credentials and owner, 303 to the App's installation page; failures 303 to `/admin/github?error=…` |
| GET | `/api/analytics/summary?sandbox&from&to` | read | → `AnalyticsSummary` |
| GET | `/api/analytics/tasks?format=json|csv&sandbox&from&to` | read | → `[TaskRow]` or CSV |
| GET | `/api/status` | signed in | → `Status` |

```json
AdminUser = {"id","github_login","display_name","is_instance_admin","disabled",
             "github_linked"}
Account = {"id","provider","label",
  "status":"pending_login|ok|relogin|error|limited",
  "usage":{"primary":{"used_percent":42.0,"resets_at":"…","window_minutes":300}|null,
           "secondary":{…}|null,"weekly_opus":{…}|null,"weekly_sonnet":{…}|null,
           "observed_at":"…|null"},
  "last_error":"…|null","in_use_by":0,
  "email":"…|null","plan":"Pro|Max 5x|Max 20x|…|null"}
```

`primary` is the 5-hour window, `secondary` the weekly one, `weekly_opus` / `weekly_sonnet` the
per-model weekly caps when the plan has them. Unknown usage is `null`, never zero. Credentials are
never returned. `email` (the provider sign-in,
administrators only) and `plan` (the subscription tier) are planned for UI-022: the web UI shows
them when present, but **the server does not report them yet** (only the mock does), so today they
are absent and the UI falls back to the label and provider. A task's overview names each session's
account by that email when it has it, matching `TaskDetail.attempts[].account_label` to the
Status accounts; an `account_id` on attempts (proposed) would make the match independent of labels.

**Login terminal.** Each login runs Claude Code's own sign-in (`claude setup-token`) with a
temporary config directory in a server-owned PTY 500 columns
wide so tokens and URLs never wrap. It uses the terminal protocol above: `resize` and
`take_control` only re-send `state` (the size is fixed), `input` is forwarded while the login
runs. `state.status` is `running`, then `ok|error|cancelled`; the `exit` frame carries the same
status and the CLI's exit code. Output is masked before it reaches any viewer (Claude tokens
become `sk-ant-oat01-[REDACTED]`). On exit the credential (the Claude token from the output) is sealed
into the account (`ok`, `account.updated`), and the temporary directory
is overwritten and removed. A login that ends without a credential sets `error` with a message;
an account that still holds a working credential keeps its status and records
`re-login failed: …`. Bounds: one login per account, 8 at once (`capacity`), 16 viewers per
login, 15-minute timeout (the process is stopped), finished logins stay viewable for 10 minutes.

**Analytics.** `sandbox` is optional (default: every sandbox the caller can see; an invisible
one is `not_found`); `from`/`to` are inclusive UTC days `YYYY-MM-DD` that select tasks by
creation time; every figure describes that cohort (at most 50 000 newest tasks, `truncated`).
Token figures sum known values only and count runs without usage in `unknown_usage_runs`;
a figure no run reported is `null`. Wall time comes from attempts, active time from metrics.

```json
AnalyticsSummary = {
  "range": {"sandbox": "…|null", "from": "2026-10-01|null", "to": "2026-10-07|null"},
  "tasks": {"total": 3, "closed": 1, "by_status": {"closed": 1}, "by_harness": {"claude": 2},
            "by_origin": {"ui": 3}},
  "tasks_per_day": [{"date": "2026-10-01", "count": 1}],
  "duration_ms": {"median": 1800000, "p90": 2400000, "total_wall": 3600000,
                  "total_active": 600000},
  "tokens": Tokens,
  "tokens_by_model": [{"model": "claude-opus|unknown", …Tokens}],
  "tokens_by_harness": [{"harness": "claude", …Tokens}],
  "tokens_by_account": [{"account_id": "…|null", "account_label": "…|null", …Tokens}],
  "switches": {"total": 2},
  "feedback": {"correct": 1, "incorrect": 1},
  "truncated": false
}
Tokens = {"runs": 3, "tasks": 2, "input": 100, "output": 50, "cache_read": 10,
          "cache_write": null, "unknown_usage_runs": 2}
TaskRow = {"task_id","sandbox","number","title","status","origin","harness","harnesses",
  "models","accounts","created_at","closed_at|null","attempts","wall_ms","active_ms|null",
  "input_tokens|null","output_tokens|null","cache_read_tokens|null","cache_write_tokens|null",
  "unknown_usage_runs","switches","feedback":"correct|incorrect|null","feedback_correct",
  "feedback_incorrect"}
```

`tasks_per_day` lists every day of a range up to 366 days (zero-task days included), otherwise only
days with tasks. `harnesses`, `models` and `accounts` are `;`-separated. `format=csv` returns
`text/csv` (attachment `yulius-tasks-<from>-<to>.csv`) with the `TaskRow` keys as the header,
CRLF line ends, RFC 4180 quoting, empty cells for unknown values, and a leading `'` on text
cells that start with `=`, `+`, `-`, `@`, tab or CR (spreadsheet formula injection).

```json
Status = {
  "vm": {"cpu_percent": 12.5, "cpu_count": 4, "memory_used_bytes": 1, "memory_total_bytes": 2,
         "disk_used_bytes": 1, "disk_total_bytes": 2, "load": [0.5, 0.3, 0.1], "load_1m": 0.5,
         "uptime_s": 60},
  "sessions": {"live": 2, "limit": 8, "queued": 1},
  "sandboxes": [{"id", "slug", "name", "live_sessions": 1}],
  "accounts": [{"label", "provider", "status", "usage",
                "id?", "last_error?", "in_use_by?"}],
  "contained": true
}
```

VM fields are `null` when unknown (CPU needs two readings). Disk figures describe the volume
holding the data directory. `sandboxes` lists only sandboxes the caller can see; `sessions` is
instance-wide. Account `id`, `last_error` and `in_use_by` are present only for instance
administrators.

```json
CapacityView = {
  "threshold": 98.0, "subscriptions": 3, "unknown_usage": 1, "available": 2,
  "weekly_left": 46.0, "five_hour_left": 166.0,
  "burn_24h": 2.0, "burn_7d": 1.6,
  "runs_out_at": "2026-10-11T02:00:00Z|null",
  "paused_hours_7d": 8.0, "active_hours_7d": 32.0,
  "needed": 4, "add": 1, "recommendation": "Add about 1 subscription to stop pausing",
  "paused_tasks": 0, "resumes_at": "…|null",
  "accounts": [{"label", "status", "usage", "available", "id?", "in_use_by?"}]
}
```

Points are percentage points of one subscription's weekly window: `weekly_left` / `five_hour_left`
sum the headroom below 98 % over subscriptions with known use (`null` when none is known);
`burn_*` is points per hour from the provider readings of the last 24 h / 7 days (`null` without an
hour of readings); `runs_out_at` is when the weekly headroom runs out at `burn_24h`, walking through
each subscription's weekly reset, if within 7 days; `paused_hours_7d` / `active_hours_7d` are
task-hours; `needed` = ⌈max(burn_24h × 168 ÷ 98, subscriptions × (active + paused) ÷ active)⌉ and
`add` = needed − subscriptions (SPEC-002 ADR-17 §2.4). `resumes_at` is set while no subscription is
available. `id` and `in_use_by` are for instance administrators only.

## UI event stream

`WS /api/events` → JSON: `{"type":"task.updated","task":TaskSummary}`, `{"type":"task.removed",
"task_id"}`, `{"type":"steps.updated","task_id"}`, `{"type":"message","task_id","message":Message}`,
`{"type":"sandbox.updated","sandbox":Sandbox}`, `{"type":"browser.action_requested","sandbox_id",
"tab_id","reason","task_id"}`, `{"type":"automation.run","automation_id","state","task_id"}`,
`{"type":"account.updated","account":Account}` (instance administrators only), `{"type":"resync"}`.

Sandbox views depend on the viewer's access level, so after a sandbox change (import, create,
onboarding transitions, removal) the server publishes `resync` to that sandbox's viewers (or
everyone) instead of `sandbox.updated`; clients refetch `/api/sandboxes`. Sandbox `counts` are
not pushed: clients refetch them (the web UI does so once after a burst of `task.updated`).

## Agent hooks (internal listener)

`POST /hooks/claude/{event}` on the loopback listener (`YULIUS_URL`), called by `yulius hook` with
the run's task token (SPEC-002 §7.5). `200` carries the hook output (an empty body is no
decision). `Stop` and `UserPromptSubmit` may answer **`202 Accepted` with `Retry-After: 10`** and no
body while no Claude subscription has capacity (ROT-010, SPEC-002 ADR-17): the agent waits at that
boundary and the client sends the same request again after `Retry-After` seconds (1–60), until a
`200`, a 4xx (it fails open) or 8 days. While held it retries a refused connection or a 5xx every
2 s, so a server restart does not release the agent. `401` means the run is unknown or finished.
