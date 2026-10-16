# Yulius HTTP and WebSocket API (v1)

Contract between the web UI (and future desktop shells, SPEC-001 OPS-002) and `yulius serve`.
It refines [SPEC-002 §7.7–§7.9](../.ai/specs/SPEC-002-yulius-architecture.md). Rust DTOs live in
`crates/yulius-contracts/src/api.rs`; when they disagree with this page, fix both in one change.

## Conventions

- JSON bodies (`Content-Type: application/json`), ≤ 1 MiB; uploads (import chunks, dropped files)
  are raw bodies with their own limits. Timestamps are RFC 3339 UTC strings.
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
| POST | `/api/setup` | `{"token", "password", "password_confirm", "confirm_saved": true}` → 201 `Me` + `"recovery_code": "123456"` (shown once) + cookie. Refused once a superadmin exists. The web UI's last setup step offers the optional Jev key with this session (`PUT /api/admin/jev`, ADM-001). |
| POST | `/api/auth/password` | `{"password"}` → `Me` + cookie (superadmin only; 5 attempts per client per minute, then 429; the client is the TCP peer, or the reverse proxy's `X-Yulius-Client-Ip` when `X-Yulius-Proxy-Secret` matches the configured secret, ACC-013, [operations](operations.md#per-client-sign-in-limits-trusted-proxy-header)) |
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
Device start needs no session and is limited per client: 5 starts a minute, at most 10 live
grants per client and 1000 per instance; past either it answers 429.

## Sandboxes

| Method | Path | Level | Body → Response |
| --- | --- | --- | --- |
| GET | `/api/sandboxes` | read | → `[Sandbox]` (only visible ones) |
| GET | `/api/github/repositories` | — | → `[{"owner","name","private","access","already_sandbox"}]` (the superadmin: every owner repository the App installation can access, `admin`; others need a GitHub identity, else 403) |
| GET | `/api/templates` | — | → `[{"id","name","description"}]` |
| POST | `/api/sandboxes` | admin on repo (superadmin: a repository the App can access) / owner; a playground: anyone signed in | `{"mode":"import","repo":"owner/name"}`, `{"mode":"create","name","template","description"}` or `{"mode":"playground","name","template"}` → 201 `Sandbox` (an import from the browser starts onboarding at once; with a bearer token it stays `pending`, SPEC-051; a playground needs no GitHub, SPEC-099) |
| POST | `/api/playgrounds?name=<name>` | anyone signed in | raw body: a gzip tar of the folder (`files/…` its working tree, `git/…` its `.git`, of which only `HEAD`, `packed-refs`, `shallow`, `objects/` and `refs/` are taken) → 201 `Sandbox` (`playground: true`); 409 for a person who keeps 10 playgrounds (the superadmin is not counted); at most 1 GiB (refused from `Content-Length` before it is read) and the import bounds once unpacked (2 GiB, 200 000 entries); 422 for an empty, unreadable or over-limit archive, nothing kept (SPEC-099) |
| GET | `/api/sandboxes/{sid}` | read | → `Sandbox` |
| PATCH | `/api/sandboxes/{sid}` | admin | `{"profile": Profile}` → `Sandbox` |
| GET | `/api/sandboxes/{sid}/onboarding/plan` | read | → `{"title", "steps": [..], "task_number"}`: what onboarding does, its task's title and steps, or while that task does not exist yet (a `pending` sandbox, SPEC-097) the ones it would be created with (`task_number: null`); reads only |
| POST | `/api/sandboxes/{sid}/onboarding/skip` | admin | → `Sandbox` (`pending`/`preparing` → `skipped`); an operator's recovery path for an onboarding that cannot finish: the web UI offers no skip (ADR-0076) |
| POST | `/api/sandboxes/{sid}/onboarding/approve` | write, browser session | Start onboarding → `Sandbox`; creates the onboarding task if it has none and starts it (`pending` → `preparing`); idempotent while `preparing`; 403 for a bearer token (the CLI only uploads, SPEC-051); 409 when nothing awaits preparation or the task cannot start |
| DELETE | `/api/sandboxes/{sid}` | admin | → 204; stops live/queued tasks; GitHub repository, clone and task records kept |
| POST | `/api/sandboxes/{sid}/move-to-github` | admin of a live playground, who could import the repository (`admin`/`maintain` on GitHub; the superadmin: a repository the App can access) | `{"name"}` → `Sandbox` (now a GitHub sandbox, same id and slug). Pushes every branch and tag of the playground to the empty repository `<instance owner>/<name>` in one atomic push, points the clone's `origin` at it, records the repository and deletes the playground's bare `origin`; tasks, sessions and steps stay, access then mirrors GitHub. 409 without the GitHub App, for a GitHub sandbox, while a task is live or queued or a CI run of it is active, for a repository that is a sandbox or holds anything but this playground's refs (at or behind its commits); 403/422 as an import; a failure leaves the playground as it was (SPEC-176, ADR-0142) |
| POST | `/api/sandboxes/{sid}/purge` | admin of a live playground; the superadmin for a removed one | → `{"state": "purged"\|"waiting", "waiting_for": ["1 agent run", …]}`; removes the playground as `DELETE` does (when still live) and asks for its code to be deleted for good: its `origin` and sandbox directory go once no agent run, shell, browser, CI run or other process of its sandbox is left (at once, else with the disk upkeep); task records, the export and the audit stay, and the quota no longer counts it; 409 for a GitHub sandbox (SPEC-171, ADR-0139) |

A **playground** (SPEC-099) is a sandbox without GitHub: `Sandbox.playground` is true, its `repo`
has an empty `owner` and `url`, its onboarding is `not_needed`, and only its maker and the
superadmin see it (admin). Its `origin` is a bare repository on the server outside the sandbox, so
nothing is pushed anywhere; pull requests, hygiene's GitHub checks and the automatic pull request skip
it, and its delivery view and Git page carry one note saying so. Yulius CI tests its integration
head and, when a task delivers, the task head merged into it (runs `task/<n>`, labelled `Task #n`);
it publishes no commit status (SPEC-163). Nothing issues a GitHub token for
it (`/internal/git-credential` answers 403), promotion and `issue_create` answer 409, and since its
maker may be anyone signed in, it has no deploy: a profile with an environment command is 422 and
an environment run 409.

`GET /api/github/repositories` lists the App installation's repositories of the instance owner
that the caller can at least read (needs a GitHub-linked account). Import mode requires a
repository of the instance owner (`invalid` otherwise) and `admin` or `maintain` on GitHub; it
clones the repository, picks the profile (`yulius.toml` `[delivery]`, else `product`/`studio` for
`Quartio-Studio/product`/`studio`, else `standard`) and validates the documentation format: a
failing project gets a preparation task (`origin: "onboarding"`) that starts at once (`onboarding:
"preparing"`; through `yulius onboard` the sandbox stays `pending`, with no preparation task yet, until someone
clicks Start onboarding on the sandbox page, SPEC-051, SPEC-097). While
`pending`/`preparing` no other task starts, but none is refused: creating or chaining a task (also an
automation run or a held prompt's Start in parallel / Run after) returns it `chained` after the
preparation task (created then, resting, when the sandbox has none yet), and it starts by itself once the sandbox is `ready` or `skipped`
(`POST /api/tasks/{tid}/start-now` starts one at once); continuing a task and continuing an
imported session are 409 (U-08, IMP-010). The web UI's new-task page shows no task input
meanwhile, only what onboarding will do and its button (SPEC-096). Create mode
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
                             "command": "…|null", "verify": "manual|command"}],
           "gate": "scripts/ci.sh|null"}
```

The counts are three of the web UI sidebar's groups, by its rule (SPEC-001 UI-050,
`AttentionFacts::group` in the domain); `counts.running` + `counts.queued` is the sidebar's Working
group. `counts.queued` counts queued tasks and chains waiting for another task; `counts.running`
counts the rest of the Working group: running tasks that do not wait for a person (working, kept
going by the agent's background shells, or a finished turn whose pull request's checks still run
or whose Yulius CI run could not run),
starting, switching and paused (`waiting_capacity`) ones. Done tasks, tasks being stopped and ended
ones count nowhere; the status page's `live_sessions` counts live processes.
`counts.needs_attention` counts the tasks that need a person by the web UI sidebar's "Needs you"
rule (SPEC-001 UI-050, `AttentionFacts` in the domain): a permission prompt (`needs_attention`),
a finished turn waiting for input (`waiting`), a pending held prompt, a pending request to act in
the sandbox browser (read from the running browser's tabs), a chain whose predecessor ended
otherwise, and failing checks of the pull request of a finished turn. A tested, reviewed or merged
pull request of a finished turn whose plan is complete counts as Done or In review, not as needing
a person. The pull request facts are those the sandbox's Git overview
(`GET /api/sandboxes/{sid}/git`) last read; until it is read after a start, only a merge already
recorded counts (the UI reads it with the sidebar and reloads the counters after each read). An
ended task or one being stopped never counts; a paused or queued one only for a held prompt or a
browser request.

## Import (`yulius onboard` / `yulius import`)

Bearer CLI token or cookie session. The full protocol, bounds and snapshot layout are documented
in `crates/yulius-contracts/src/import.rs`.

| Method | Path | Body → Response |
| --- | --- | --- |
| POST | `/api/onboard/plan` | `{"owner","name","sessions":[{"harness":"claude","session_id","sha256","size"}]}` (≤ 2000) → `OnboardPlan`. Changes nothing. With `"playground": true` (and an empty `owner`), the caller's playground named `name` (or a taken-name variant, `name-2`), else whether they may make one (the limit of 10, SPEC-171). |
| POST | `/api/import` | `{"manifest": ImportManifest}` → `{"job_id", "missing": [{"name", "received_bytes"}]}`. Links the repository as a sandbox when needed (admin/maintain; owner repositories only; write for an existing sandbox); an unfinished job with the same manifest is reused; files already stored for the sandbox (same `sha256`) are not listed. A session file is at most 128 MiB; at most 2000 sessions, each once. A manifest with `"playground": true` (empty owner and remote URL, `bundle` every branch) goes to the caller's playground of that name, else to a new one made when the job completes (its branches as they are, the snapshot one commit on `head_branch`; 422 without a bundle or a branch, 409 at the limit of 10). A person has one such upload open at a time: the same manifest resumes it, another one supersedes it (its files go, and completing it is 409). Without that flag an empty owner is refused (SPEC-171). |
| PUT | `/api/import/{job}/files/{name}?offset=N` | raw chunk (≤ 8 MiB) with `X-Chunk-Sha256` → `{"name", "received_bytes"}`. `name` is one percent-encoded segment. Repeats are acknowledged; an offset beyond the received length is 409; a wrong digest is 422 (a wrong whole-file digest resets the file to 0). |
| POST | `/api/import/{job}/complete` | → `ImportResult`; 409 while files are incomplete. Idempotent. |

```json
OnboardPlan = {"login": "alice",
  "sandbox": {"id", "slug", "url", "sessions_url", "onboarding": OnboardingStatus} | null,
  "can_create": false,              // no sandbox yet: whether the caller may add it
  "refusal": "…" | null,            // why the caller cannot upload here
  "sessions": [{"session_id", "state": "new|changed|unchanged|taken"}]}
OnboardingStatus = {"state": "not_needed|pending|preparing|ready|skipped", "ready": false,
  "summary": "Preparing: task #1 is adapting the repository (step 2 of 5).",
  "pending": ["task #1 finishes", "the repository passes the Yulius format check"],
  "task": {"number", "title", "status", "steps_done", "steps_total", "url"} | null}
ImportResult = {"sandbox_id", "sandbox_url", "sessions_url", "onboarding", "onboarding_status": OnboardingStatus,
  "sessions_imported", "sessions_updated", "sessions_unchanged",
  "sessions_skipped_existing",      // = sessions_unchanged (older CLIs)
  "tasks_created": 0}               // sessions are documents, not tasks
```

`state` of a session: `unchanged` = the server holds this exact (redacted) transcript, `changed` =
it holds the caller's earlier copy, `taken` = another person uploaded a session with this id (kept),
`new` otherwise. `SessionRef` carries `redactions` (secrets the CLI redacted). Applying: bundle
branches become `import/<login>/<branch>`; the snapshot is committed on
`import/<login>/snapshot-<YYYY-MM-DD>[-n]` created from the client's `HEAD`; each session is
stored as a read-only session document of the sandbox attributed to the caller (a changed one
replaces the caller's copy). It never starts onboarding: a `pending` sandbox waits for Start
onboarding in the browser (SPEC-051). Only Claude Code sessions are imported.

`PATCH /api/sandboxes/{sid}` validates the profile (branch names, distinct branches, environment
names, single-line commands ≤ 2000 bytes, `verify: command` needs a command, ≤ 8 environments) and
relabels it: `preset` stays a named preset only while every setting equals that preset's
defaults, otherwise it becomes `custom` (so switching Studio's `auto_promote` off yields `custom`).
`require_checks` absent or null means the preset's default (`false` for `studio`, else `true`);
responses always carry it; blank commands mean "no command". The sandbox profile is the only
source of environment commands (never the repository). `GET /api/delivery/presets` (signed in)
returns the three presets `standard`, `product` and `studio` as `[Profile]`. `gate` (single line,
≤ 2000 bytes; blank or null clears it) is a CI command an administrator sets for the sandbox: it
overrides the CI of `yulius.toml` (see "CI" below) and does not change the preset label.

## Sessions

Task sessions and imported Claude Code sessions as read-only documents (SES-001..005). Every text
returned is redacted by the server.

| Method | Path | Level | Body → Response |
| --- | --- | --- | --- |
| GET | `/api/sandboxes/{sid}/sessions?kind=all\|task\|imported&author=<login>&q=<text>&limit=N` | read | → `{"sessions": [SessionDoc], "authors": ["login"], "total", "query"}`; newest first, or best match first with `q` (`hits` filled); `limit` ≤ 500; 422 for another `kind` |
| GET | `/api/sessions/{id}` | read | → `SessionDoc` |
| GET | `/api/sessions/{id}/transcript?from=<line>&limit=N` | read | → `{"session": SessionDoc, "entries": [TranscriptEntry], "next": line \| null, "skipped_lines"}`; `limit` ≤ 500 (default 200), about 2 MiB of text per page |
| POST | `/api/sessions/{id}/continue` | write | imported only → `TaskSummary` of a new task (origin `import`) resuming the session natively; a continued session returns its task; 409 while the sandbox is being prepared; 422 for task sessions |
| DELETE | `/api/sessions/{id}` | its uploader (write) or admin | imported only → 204; files and index removed; audited `session.deleted` |

```json
SessionDoc = {"id", "sandbox_id", "kind": "task|imported", "title", "harness": "claude", "session_id",
  "author": {"login"} | null, "task": SessionTask | null, "continued_in": SessionTask | null,
  "git_branch", "started_at", "ended_at", "imported_at", "bytes", "redactions",
  "can_continue", "can_delete",
  "hits": [{"line", "role": "user|assistant|tool|summary", "snippet": [{"text", "hit"}]}]}
SessionTask = {"id", "number", "title", "status"}
TaskTranscript = {"session_id" | null, "doc_id" | null, "entries": [TranscriptEntry], "next": line | null,
  "end": line, "bytes", "earlier": bool, "skipped_lines"}
TranscriptEntry = {"line", "role": "user|assistant|system|summary", "at", "sidechain", "blocks": [
  {"type": "text", "text", "truncated"} | {"type": "thinking", "text", "truncated"} |
  {"type": "tool_call", "id", "name", "summary", "input", "diff": {"path", "hunks": [{"old", "new"}], "truncated"} | null} |
  {"type": "tool_result", "tool_use_id", "text", "is_error", "truncated"} |
  {"type": "image", "media_type"} | {"type": "meta", "text", "truncated"}]}
```

Texts are bounded: a message at 32 Ki characters, thinking at 8 Ki, a tool result and a meta text at
16 Ki (a `system` line's meta at 32 Ki), each side of a diff hunk at 16 Ki and an edit at 20 hunks,
and a tool call's input at 8 Ki (cut without a flag). `truncated: true` says the server cut it; on a
diff, that a side was cut or hunks were dropped, so its line counts are not the edit's (SPEC-177).
`truncated` on diffs and meta blocks is additive: a payload without it reads as `false`.

Agents use the same library through the internal MCP: `sessions_search {query, kind?, limit?}`
(≤ 20 sessions, ≤ 3 passages each, the asking task's own sessions excluded) and `session_read {id |
task_number, from?, limit?, max_chars?}` (≤ 200 entries, ≤ 100 000 characters, `next_from`), both
limited to the agent's sandbox.

## Tasks

| Method | Path | Level | Body → Response |
| --- | --- | --- | --- |
| GET | `/api/sandboxes/{sid}/tasks?archived=false` | read | → `[TaskSummary]` |
| POST | `/api/sandboxes/{sid}/tasks` | write | `{"prompt","harness?":"claude","mode?":"goal|prompt|auto","workflow_id?":"<id>|auto","model?","effort?","title?","after_task_id?","start_rule?","supertask?":false,"prepared?":false,"start_at?":"<RFC 3339>"}` → 201 `TaskSummary`; Yulius runs Claude Code only (another `harness` is 422); `mode` defaults to `goal` (TSK-010); without `model` the task runs `claude-opus-5-5` and without `effort` at `high` (none for Haiku; SPEC-023); `auto` lets Jev choose the mode and/or the workflow before the task is created (TSK-015, ≤ 2.5 s; see "Task intake"); an unknown workflow or mode is 422 before Jev is asked; with `after_task_id` (a task of the sandbox that has not ended; 422 otherwise, 429 beyond 32 waiting) the task waits as `chained` until that task is Done (TSK-012); with `start_rule` (a logical sentence such as `#12 done and (#13 done or #14 ended)` over tasks of the sandbox; 422 for a malformed rule, an unknown task or together with `after_task_id`) it waits until the rule is true and needs a person when it can no longer be (TSK-016, SPEC-017); the rule shows as `after.rule`; with `"supertask": true` the task waits `queued` for the project's working tasks, then runs alone while the project's other tasks wait for it (TSK-018, SPEC-071; 422 together with `after_task_id` or `start_rule`). With `"prepared": true` the task waits as `prepared` until Start now, and with `start_at` (implies prepared; at most 60 s past and 366 days ahead, else 422) until that time, shown as `start_at` (TSK-019, SPEC-105; 422 together with `after_task_id`, `start_rule` or `supertask`, 429 beyond 32 prepared tasks in the sandbox). A queued task's `held_by` lists the task numbers a supertask rule makes it wait for (`[]` otherwise), and `supertask` says whether it is one |
| POST | `/api/sandboxes/{sid}/intake/check` | write | `{"prompt"}` → `{"verdict":"one|several","checked":bool,"supertask":bool}`: whether a new task mixes separate workstreams (TSK-011; prompts of 40 words or more), and whether to propose it as a supertask (SPEC-188, ADR-0152): Jev is sure it is broad work and the verdict is not `several`. Both questions are asked at once, each within 2.5 s. `checked` says whether Jev answered either; it is false — the verdict `one`, `supertask` false — for prompts under 3 words, without a Jev key and when Jev fails or is slow |
| POST | `/api/sandboxes/{sid}/intake/proposal` | write | `{"prompt"}` → `{"tasks":[{"title","prompt","after_previous":bool}]}`: Claude's smallest split, 2–4 tasks grouped by area carrying their items word for word; `[]` means keep one task. 409 when proposals are not configured or no account is connected, 429 while 2 proposals run, 502 when Claude fails or its proposal is unusable (more than 4 tasks, one per item, an item missing) |
| POST | `/api/tasks/{tid}/start-now` | write | → `TaskSummary`; a `chained` task starts now whatever its predecessor does, a `prepared` one whatever its time (409 otherwise, also while onboarding blocks the sandbox's tasks). Cancel it with `/stop` |
| PUT | `/api/tasks/{tid}/schedule` | write | `{"start_at":"<RFC 3339>"|null}` → `TaskSummary`; when a `prepared` task starts by itself, or null: when a person starts it (SPEC-105; 422 outside the window of `CreateTask.start_at`, 409 when the task is not prepared). A time already due starts it at once |
| PUT | `/api/tasks/{tid}/start` | write | `ChangeStart` (`after_task_id`, `start_rule`, `supertask`, `prepared`, `start_at`, as `CreateTask`; none: Start now) → `TaskSummary`; how a task waiting to start (`chained` or `prepared`) starts instead (SPEC-126). 409 once the task has started (also `queued`), 422 for a choice `CreateTask` refuses or a wait on itself, 429 beyond the sandbox's bound of chained or prepared tasks; the owner only (SPEC-070) |
| POST | `/api/tasks/{tid}/held-prompts/{hid}` | write | `{"action":"parallel|after|send|skip_after|revert|dismiss","skip_step_ids?":[…],"confirm?":bool}` → `{"task":TaskSummary,"created":TaskSummary|null,"note"}`; one decision per held prompt (409 for a second). `send` needs a running task; `skip_after` marks the given open steps `skipped`; `revert` needs `"confirm":true` (422 otherwise) — see "Task intake" below |
| POST | `/api/tasks/{tid}/queued-messages` | write | `{"text"}` → `{"request"}`; adds the message to the request of a task that has never started (`queued`, `chained`, `prepared` or `waiting_capacity`, no run, no provider session) under "**Added by {login} before the task started ({UTC time}):**", so the agent gets it when the task starts (SPEC-052 / SPEC-001 TSK-017, ADR-0035). 409 once the task has started (type into its session instead), or, rarely, when it changed between the check and the update (try again); 422 for an empty message or a request past 64 KiB; publishes `task.updated` |
| GET | `/api/tasks/{tid}` | read | → `TaskDetail` |
| GET | `/api/tasks/{tid}/prompt` | read | → `{"text","attachments":[{"name","image":bool,"available":bool}],"parts":[{"text"}\|{"attachment":n}]}`: the prompt the task started with, without its attached files (an inline `@'path'` mention or an `@path` line each), its attached files in order (SPEC-043), and `parts`, the prompt in order with each file where it sits (`n` indexes `attachments`; SPEC-124). `available` is false for a file that is gone (composer drafts are removed after a week) or not in the task's own attachment folders (a path mentioned by hand, another task's or user's file); at most 500 are checked |
| GET | `/api/tasks/{tid}/prompt/attachments/{n}` | read | → the bytes of attachment `n` (from 0) of that list, only when the prompt names it directly in one of the task's own attachment folders (its creator's composer drafts, `attachments/drafts/<user>/`, or the task's own attachments, dropped on its terminal or queued with a message, `attachments/tasks/<task>/`), reached without a link, a regular file within the upload limit and, on Linux, opened at its real path as `/proc/self/fd` says (404 otherwise, also out of range or not a number; 429 when none of the 16 read slots frees within 30 s; uploads have their own bound). The body is streamed in chunks of at most 64 KiB with its `Content-Length`, holding its read slot; left unread for 60 s it gives the slot back and closes the file, and the response breaks off at the client's next read (SPEC-080, ADR-0064). An image (`.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`) is served with its image type, any other file as `application/octet-stream` with `Content-Disposition: attachment`; the content security policy includes `sandbox` (ADR-0033) |
| GET | `/api/tasks/{tid}/transcript?from=<line>&limit=N&seen=<bytes>&session=<id>` | read | → `TaskTranscript`: the task's latest Claude Code session as the people watching someone else's task read it (SPEC-179). Without `from` the tail: the last `limit` entries (≤ 500, default 200, about 2 MiB of text; `earlier` when entries were left out) and `doc_id`, the session's document; with `from` the entries from that line (`next` when the page is full). `end` is the line after the last complete line (a line still being written waits), `bytes` the transcript's size: a reader continues with `from=end&seen=bytes&session=session_id`, answered without reading the file while that session's size is unchanged. Redacted as session pages are. No session yet: `session_id` null, no entries |
| POST | `/api/tasks/{tid}/pause` | write | → `TaskSummary`; its owner pauses a `running` task (SPEC-191, ADR-0154): its run is frozen, or held at its turn boundary when it cannot be (`pause.pending` until then); `pause: {"by":"person","user","since","pending"}`; pausing again changes nothing; 409 for any other state; audited `task.paused_by_person`; publishes `task.updated` |
| POST | `/api/tasks/{tid}/resume` | write | → `TaskSummary`; ends a person's pause and thaws the run (while an update holds the task, the update thaws it); nothing for a task that is not paused; audited `task.resumed_by_person` |
| POST | `/api/tasks/{tid}/stop` | write | → `TaskSummary`; a `stopping` task is stopped again: its escalation is armed anew (a run whose processes could not be ended is retried, SPEC-065) |
| POST | `/api/tasks/{tid}/continue` | write | `{"account_id?"}` → `TaskSummary`; resumes the latest session natively, on the best Claude subscription (the pool's choice; `account_id` is an operator override the web UI never sends); 403 for a bearer token on a `pending` sandbox's onboarding task (Start onboarding is a browser action, SPEC-051) |
| POST | `/api/tasks/{tid}/switch` | write | `{"account_id?"}` → `TaskSummary`; operator override: moves a running task to another Claude subscription with a native resume. Tasks are subscription-agnostic (F 2026-10-10): the web UI offers no account choice; the pool switches by itself |
| POST | `/api/tasks/{tid}/archive` · `/restore` | write | → `TaskSummary` |
| DELETE | `/api/tasks/{tid}` | write | → 204; deletes a resting task permanently with its records, run directories, local worktree and branch (TSK-009); a Done running task (TSK-014) is closed first and deleted once its run has ended; the close and its wait take at most ~25 s (SPEC-039); 409 while any other task runs (at once, also for a task that turned switching meanwhile), if a Done task's session is still closing then, or if its session could not be ended (its processes survived termination; SPEC-065) |
| POST | `/api/tasks/{tid}/control-request` | write | → `TaskSummary`; asks the task's owner for control (SPEC-070, ADR-0060): it would act on GitHub as you. One request waits per task: 409 for the owner and while another person's request waits (naming them); asking again changes nothing. With the owner disabled, deleted or without write access, control passes at once (`owner` is you), whatever request waits; a waiting asker who can no longer work gives way (their request dropped, `task.control_dropped`). Audited `task.control_requested` |
| DELETE | `/api/tasks/{tid}/control-request` | read (asker) · write (owner) | → `TaskSummary`; the asker withdraws (`task.control_withdrawn`) or the owner declines (`task.control_declined`); 409 when no request waits, 403 for anyone else |
| POST | `/api/tasks/{tid}/control-request/accept` | write (owner) | → `TaskSummary` with the asker as `owner`; the task acts on GitHub as them from its next call, its worktree commits as them, its terminal says so, audited `task.control_passed`; 409 when no request waits, or when the asker can no longer work in the project (the request is dropped) |
| POST | `/api/tasks/{tid}/feedback` | read | `{"verdict":"correct|incorrect","comment"}` → 204 |
| GET | `/api/archived-tasks` | read | → `[TaskSummary]` across visible sandboxes |
| WS | `/api/tasks/{tid}/terminal` | read (input: write, owner) | terminal protocol below |

Every task route marked write also needs the task's owner (SPEC-070): anyone else gets 403 "Task
#N is @owner's: it acts on GitHub as them. Ask them for control." — also an administrator or the
superadmin, and the Cockpit's and task agents' `task_stop`, `task_archive` and `task_delete` for
their person. The terminal opens read-only (`can_write: false`) for anyone but the owner. The
file API's writes and uploads with `root=task:<id>` and `POST /api/sandboxes/{sid}/shells` with
a `task_id` need the owner too; reading a worktree does not.

```json
TaskSummary = {
  "id": "…", "sandbox_id": "…", "number": 12, "title": "Checkout API",
  "harness": "claude", "status": "running|…(see domain wire names)",
  "activity": "working|waiting|needs_attention|done|null",
  "branch": "feat/checkout-api", "origin": "ui|automation|import|onboarding|agent",
  "created_by": {"login": "…"}, "owner": {"login": "…"},
  "control_request": {"by": {"login": "…"}, "at": "…"}|null,
  "created_at": "…", "updated_at": "…", "state_since": "…", "working_since": "…|null",
  "steps": {"done": 3, "total": 7, "current": "Implement API|null", "percent": 42, "active_percent": 14},
  "archived": false, "account_label": "Claude #2|null",
  "mode": "goal|prompt",
  "resumes_at": "2026-10-09T08:20:00Z|null",
  "supertask": false, "held_by": [3, 5],
  "start_at": "2026-10-15T09:00:00Z|null",
  "after": ChainLink|null, "next": [ChainLink],
  "held_prompt": {"id","class":"new_scope|drops_scope|redirect","excerpt","percent","typed_by|null","created_at"}|null,
  "needs_you": {"action","why|null","link|null","at"}|null,   // SPEC-134: what the agent asked a person (`needs_you`)
  "background_shells": "0|1|…|null",
  "auto": {"mode":"jev|default|null","workflow":"jev|default|null","workflow_name":"Feature|null"}|null
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

`steps.done` counts finished steps (done or skipped, STP-005); `steps.percent` is the finished
share of the plan's **effort**, 0-100, rounded down (STP-006): steps declare an effort of 1 (the
default) to 3, so 9 of 10 equal steps is 90 and a plan whose one big step is open shows less.
`steps.active_percent` is the share of the steps **in progress** (SPEC-159), so that `percent +
active_percent` is the finished and in-progress share rounded down; the task strip draws it as a
second segment in the working colour. Older servers omit it (read 0).

`updated_at` is the last change of anything (title, owner, archive, status …). `state_since` is
the last change of the task's **status or activity** (setting the same value again does not move
it); `working_since` is the start of its working stretch: it starts when the task starts working,
goes on through idle turns while background work wakes it and through subscription switches, and
is null while the task waits for a person, is queued or has ended (SPEC-087). The UI shows, per
group, "working 1h 20m" from `working_since` and "waiting 12m", "queued 2h", "done 3h ago" from
`state_since`.

`activity` is `done` when Claude Code recorded the task's goal as achieved and the turn ended
cleanly (TSK-014); the session stays open, and any new prompt makes it `working` again.
`background_shells` is set while the agent is idle (`waiting` or `done`): the Bash-tool shells it
left running (Claude Code's "· 1 shell", probed every 5 s) and the background agents its last turn
end said it waits for (`turn_duration.pendingBackgroundAgentCount`, SPEC-032); null when not known.
The UI shows a `done` task as Done only when no such background work is running ("Working · 1 background task still running"
otherwise), and applies the same shell condition to Done by a tested pull request.

```json
TaskDetail = TaskSummary + {
  "spec_path": "docs/specs/2026-10-07-checkout-api.md|null",
  "worktree": "…", "attempts": [{"seq","harness","account_label","account_id (proposed)","launch_kind","state",
                                  "started_at","ended_at","exit_code","exit_reason"}],
  "steps_list": [Step], "metrics": Metrics, "feedback": [{"login","verdict","comment","at"}],
  "delivery": Delivery, "switches": [{"at","reason","from","to"}],
  "request": "the whole request, queued messages included", "started": false
}
Step = {"id","ordinal","title","status":"pending|in_progress|done|skipped|blocked","note","updated_by","updated_at","effort":1}
Metrics = {"wall_ms","active_ms|null","input_tokens|null","output_tokens|null","cache_read_tokens|null","cache_write_tokens|null","models":["…"],"switches":0,"turns|null"}
```

`wall_ms` is how long the task's sessions were open, idle included. `active_ms`, the tokens and
`turns` come from each run's transcript (SPEC-087): active time is the union of its turns' spans
(prompt to answer, and Claude Code's turn records, which include a turn's wait for its own
background agents); tokens include the session's sub-agent transcripts; turns are Claude Code's
turn records. A restart or a resume reads the run's own lines again, so they never reset.

Status wire names (from `yulius-domain::task`): `queued`, `starting`, `running`, `switching`,
`waiting_capacity`, `stopping`, `closed`, `stopped_cancelled`, `stopped_crashed`, `stopped_killed`,
`stopped_lost`, `stopped_switch_failed`, `stopped_switch_timeout`, `stopped_server_lost`,
`failed_spawn_error`, `failed_start_timeout`, `failed_precondition`, `chained`, `prepared`.

How a session's end moves its task (F 2026-10-10, SPEC-002 §6.1): leaving the session (`/exit`,
Ctrl+C twice, Ctrl+D, `/logout` — Claude Code's `SessionEnd` reason `prompt_input_exit`/`logout`)
or any clean exit → `closed`; ended by a signal from outside Yulius (SIGTERM, SIGKILL, SIGHUP,
SIGINT, or the exit codes 129/130/137/143 they give) → `stopped_killed`; any other exit →
`stopped_crashed`; Stop in the UI → `stopping` at once, then `stopped_cancelled`. Interrupting a
turn (Esc, or Ctrl+C while the agent works) keeps the session: `activity` becomes `waiting`.

### Task intake

Optional (SPEC-002 ADR-16): without a Jev key (`YULIUS_JEV_API_KEY_FILE`, docs/operations.md)
nothing is checked and nothing is held, and Auto uses its defaults.

- **New tasks (TSK-011).** The composer calls `/intake/check` when a task is started (the ↑ button
  or ⌘/Ctrl+Enter; Enter adds a line) other than as a supertask or prepared. On `several`, without
  a chosen task or rule to run after, it shows the split card and calls `/intake/proposal`, then
  creates the tasks with `POST /api/sandboxes/{sid}/tasks`: in parallel, or as a chain (each with
  `after_task_id` of the one before). On `supertask` (SPEC-188) it shows "Run it as a
  supertask?": Run as a supertask creates it with `"supertask": true` (and no `after_task_id` or
  `start_rule`), Start as chosen as the person chose.
- **Auto mode and workflow (TSK-015).** With `"mode":"auto"` and/or `"workflow_id":"auto"` (the
  composer's defaults) the server asks Jev before creating the task, both questions at once within
  2.5 s: `goal` or `prompt` (followed at 75 %), and one of the sandbox's workflows from the store,
  or none (followed at 60 %). Otherwise the defaults apply: Goal, and the workflow named "Feature"
  (the sandbox's own first; none without one, and none for a prompt-mode task). A prompt-mode task
  never gets a workflow that requires a spec from Auto. The summary's `auto` says who chose what
  (`jev`, `default`, or null for a value the request picked) and the workflow's name; it is null
  when nothing was on Auto. Without a Jev key Auto still works, with the defaults.
- **Chains (TSK-012).** A `chained` task starts by itself (`chained → queued`) when its predecessor
  is Done: a finished turn and a pull request merged, or open with checks green or none required.
- **Typed prompts (TSK-013).** The `UserPromptSubmit` hook classifies a person's prompt. `context`
  passes; `new_scope`, `drops_scope` and `redirect` are held. Claude Code shows the reason, and the
  text stays in its input box. The task's `held_prompt` is set and the UI shows the choices:
  - `parallel`: a new task with the prompt (same harness, model, effort and mode);
  - `after`: a new task chained after this one;
  - `skip_after`: the given steps become `skipped` (with the note "Ruled out by the user: …" by
    `user:<login>`; the agent sees them in its steps, no message is sent, SPEC-028), and a chained
    task gets the prompt;
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
{"type":"active"}
{"type":"idle"}
```

The PTY has one size: the controlling viewer's (F 2026-10-10). Every writer sends the size its own
window fits (`hello`, `resize`, whether or not it controls) and whether it is `active` (its page
visible and focused, the terminal used within the last minute) or `idle`. Input (typing) and
`take_control` (a click into the terminal) make that viewer the controller at once, except
`"scroll":true` input: a wheel turn forwarded to a full-screen app, which neither takes control nor
resizes the terminal. An `active` viewer takes control from an idle controller; when the
controller sends `idle` or leaves, the most recently active viewer takes over (on leaving, else the
most recently seen one). A viewer alone keeps control while idle. Whenever control moves, the
terminal takes the new controller's last reported size at once and every viewer gets a `state`.
Read-only viewers never control (their `resize`, `active` and `idle` change nothing).

The connection may carry the viewer's size: `WS /api/tasks/{tid}/terminal?cols=120&rows=40`. A
writer whom no viewer controls, or whose controller is idle, gets it applied before the first
`state` and snapshot (and then controls the size); when that changed the size, the snapshot waits
up to 400 ms for the program's redraw. Both or neither; unparsable values are 400. `hello` and `resize` with the size
the terminal already has change nothing.

Server → client:

- Binary frame: `u8 kind` + `u64 big-endian seq` + bytes. `kind` 1 = output, 2 = snapshot (clear
  the terminal and write the bytes; a full screen redraw).
- Text frames: `{"type":"state","attempt_seq":n,"harness":"claude","cols":c,"rows":r,
  "can_write":bool,"controller":bool,"status":"running"}`, `{"type":"notice","text":"↻ switched
  to …"}`, `{"type":"exit","status":"closed","code":0}`, `{"type":"error","code","message"}`.
- Close code 4008: viewer lagged; reconnect with `last_seq`.

Write access is confirmed when the terminal opens and then held as the viewer's grant
([decision 0096](decisions/0096-terminal-input-rides-a-write-grant.md)): `input`, `resize`,
`active` and `take_control` go to the terminal without a permission lookup. When something that
decides access changes (a hand-over, a removed sandbox, a disabled user, a changed GitHub level), the
next one is checked first and refused with `forbidden` when access is gone. A grant older than 30 s
is re-confirmed in the background. Access is re-checked every 60 s beside the stream, which keeps
flowing meanwhile: losing view access closes with 4003, and losing write access sends a `state`
with `"can_write":false`.

The account-login terminal (`/api/admin/accounts/login/{lid}/terminal`) and sandbox shells
(`/api/shells/{id}/terminal`) use the same protocol; for them `active` and `idle` change nothing
and `take_control` moves nothing (a login terminal's size is fixed; a shell follows the last
`resize`).

## Git and delivery

The web UI only reads delivery (GIT-002): the Git page, a task's delivery details and its strip
use the `GET` routes below. The `POST` routes (open PR, merge, promote, environment runs) stay for
the agents' delivery workflow and other clients; no page of the web UI calls them.

| Method | Path | Level | Body → Response |
| --- | --- | --- | --- |
| GET | `/api/sandboxes/{sid}/git` | read | → `GitOverview` |
| GET | `/api/tasks/{tid}/delivery` | read | → `Delivery` |
| GET | `/api/tasks/{tid}/changes` | read | → `TaskChanges`: the task worktree (committed, uncommitted and untracked work; symlinks listed, never followed) against the merge base with `origin/<integration>`; 409 when the worktree is gone |
| GET | `/api/tasks/{tid}/commits` | read | → `[TaskCommit]`: the task branch's commits not on the integration branch, newest first (at most 100) |
| POST | `/api/tasks/{tid}/delivery/pr` | write | `{"title?","body?"}` (body optional) → `Delivery` (refused without a valid spec or with uncommitted changes); opened with the caller's GitHub account, the superadmin's by the App |
| POST | `/api/tasks/{tid}/delivery/merge` | write | → `Delivery` (requires the domain gates: open PR at the checked head, valid spec, no open steps, checks per profile); acts as the caller on GitHub: 403 with a message for the superadmin without a GitHub identity. In a playground: squashes the task branch onto its integration branch in the playground's own `origin` (valid spec where required, steps resolved, a clean worktree, commits to deliver; 409 naming the files on a conflict), no GitHub identity needed (SPEC-099), once Yulius CI passed on the head merged into the integration branch: 409 while that run goes (it never arms) or when it failed (SPEC-163) |
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
- **Open PR** pushes the task branch and opens the PR into the integration branch with the
  caller's token (the superadmin without a GitHub identity: the App's), so both are theirs on
  GitHub (title: task title unless given; body: the spec's Goal, spec path, Plan checklist and
  steps unless given, plus a link back to the task). The pull request Yulius opens by itself for
  a ready task, and later pushes to it, use the task creator's token (GIT-003).
- **Merge** uses the profile's merge method and passes the head that was checked as the
  expected head; a PR head that differs from the task worktree's head is refused as stale.
  With `auto_promote` (Studio), the integration branch is promoted right after a merge done
  through Yulius once no integration environment is pending; the outcome is added to `notes`
  and audited. No other path merges, promotes or deploys on its own. Agents deliver their own
  work through the MCP tools `delivery_merge` (passed checks and `reviewed_head`, the commit an
  independent review approved, equal to the branch head) and `delivery_deploy` (integration
  environments with a command and `verify = "command"` only), which take these same paths with
  the task creator's authority and are audited as `task:<n>` too (DLV-008). Agents never promote:
  production is always a person's (ADR-0110; there is no `delivery_promote` any more), and they
  deploy with CD (below). `issue_create`
  opens a GitHub issue only when it meets the Definition of Ready (one `type:`, one `priority:`,
  at least one `area:` label and an `## Acceptance criteria` checklist). On GitHub these tools
  act with the task creator's own token; a creator without a usable one (no linked account, an
  expired sign-in) gets the App's, and the task's terminal says so once (GIT-003).
- **Promote** needs an administrator and `{"confirm": "<slug>"}` (a release is a production deploy,
  ADR-0110). It refuses while a merged task still has an integration environment pending, when
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

## CI (Yulius-run) and repository hygiene

Yulius is the projects' only CI (SPEC-001 DLV-006/007, SPEC-002 ADR-21). The jobs come from
`yulius.toml` on the integration branch, or from the profile's `gate`:

```toml
[[ci.jobs]]                 # one per independent check; the jobs of a run run in parallel
name = "test"               # [a-z0-9][a-z0-9_-]*, at most 32 bytes, unique
command = "cargo test"      # one line, run as /bin/sh -c in the job's CI checkout
matrix = "1/2 2/2"          # optional: one job per value, as $YULIUS_CI_MATRIX ("test (1/2)")
cpus = 4                    # optional (1-1024): the CPUs it uses; default: an equal share of the
memory_gb = 8               #   sandbox among the run's jobs; memory_gb (1-4096) follows the CPUs
# or, for one job: [commands] gate = "…"   (not both)

[resources.env]             # optional: the budget in this repository's tools' terms (SPEC-082)
CARGO_BUILD_JOBS = "{cpus}" # placeholders {cpus}, {memory_mb}, {memory_gb}; at most 32 variables,
                            # not what Yulius sets or what changes how the agent CLI, Git, TLS,
                            # proxies, shells or interpreters run (PATH, HOME, CI, YULIUS_*, GIT_*,
                            # ANTHROPIC_*, CLAUDE_*, NODE_*, PYTHON*, *_PROXY, LD_* and similar)
```

Jobs name their stages with `::yulius-stage <name>` lines ("Stages" below). Jobs get `CI=true`, `HOME` (persistent), `YULIUS_CI_CACHE` (persistent), `YULIUS_CI_RUN`,
`YULIUS_CI_JOB`, `YULIUS_CI_SHA`, `YULIUS_CI_REF`, `YULIUS_CI_MATRIX` and no secrets. A job
starts once its CPUs and memory are free: the CI jobs of all lanes never ask for more than the
sandbox has, and each is told its budget in `YULIUS_CPUS`, `YULIUS_MEMORY_MB` and the variables of
`[resources.env]` in its checkout's `yulius.toml` (its output starts with
`[yulius] budget: 4 CPUs, 8192 MiB …`). Agent sessions, terminals and the Cockpit get the agent
budget the same way, from the `yulius.toml` where they start.

| Method and path | Access | Result |
| --- | --- | --- |
| `GET /api/sandboxes/{sid}/ci` | read | `CiOverview` |
| `POST /api/sandboxes/{sid}/ci/sync` | write | looks for new heads now; `CiOverview` |
| `GET /api/sandboxes/{sid}/ci/runs/{rid}` | read | `CiRun` with each job's `output` |
| `POST /api/sandboxes/{sid}/ci/runs/{rid}/cancel` | write | `CiRun`; `conflict` when it ended |
| `POST /api/sandboxes/{sid}/ci/runs/{rid}/rerun` | write | the new queued `CiRun`; `conflict` while a run of that commit is active or without CI |
| `GET /api/sandboxes/{sid}/hygiene` | read | `HygieneReport` (the last scan; scans when none) |
| `POST /api/sandboxes/{sid}/hygiene/scan` | write | `HygieneReport` |
| `POST /api/sandboxes/{sid}/hygiene/cleanup` | write | `{"removed": [Target], "skipped": [Target + "reason"], "report": HygieneReport}` |

```text
CiOverview = {"gate": {"source": "profile|yulius_toml", "jobs": [{"name", "command", "matrix"}]} | null,
              "gate_problem": "yulius.toml line 7: …" | null,
              "runs": [CiRun] (newest 30, without outputs), "can_run": true,
              "status_access": "for administrators: commit statuses cannot be published …" | null,
              "actions_workflows": ["ci.yml"]}   // GitHub Actions still on the integration branch
CiRun = {"id", "reference": "pr/12|branch/dev", "label": "PR #12|dev", "pr_number": 12|null,
         "sha", "state": "queued|running|passed|failed|error|cancelled" (never skipped: skipped jobs count as passed),
         "source": "profile|yulius_toml|pull_request",
         "jobs": [{"id", "name", "command", "matrix",
                   "state": "queued|running|passed|failed|error|cancelled|skipped", "reason", "exit_code",
                   "duration_ms", "output" (run detail only: last 512 KiB, escapes removed)}],
         "reason": "superseded by 0d7e5f3|…" | null, "queued_at", "started_at", "ended_at",
         "duration_ms", "requested_by": "yulius|<login>|task:<n>",
         "status": "published|pending|not published: …"}
HygieneReport = {"scanned_at": "…" | null, "items": [{"kind": "remote_branch|local_branch|worktree",
                 "name", "reason_code": "pull_merged|pull_closed|merged|local_copy|idle|task_deleted|task_archived|task_merged|missing",
                 "reason", "safe": true}], "notes": ["…"]}
Cleanup request = {"items": [{"kind", "name"}] (empty: every safe item), "force": false}
```

- **Runs.** Every open pull request into the integration branch (same repository, at most 30) and
  every new integration and production head is tested once per commit. The integration branch's
  configuration applies; while it has none, a pull request that adds CI is tested with its own
  (`source: pull_request`). A newer head cancels the active run of the same reference ("superseded
  by …"). One run per sandbox runs at a time; its jobs share the instance's job slots.
- **Commit status** `yulius/ci` on the commit: `pending` (queued, running), `success`, `failure`
  (names the failed job and exit code), `error` (errors and cancellations), linking to
  `/s/<slug>/ci?run=<id>`. It needs the App's "Commit statuses: Read and write"; without it `status`
  says so and the run still gates merges.
- **Merge gate.** Once a sandbox has CI, the latest run at the pull request head alone decides the
  delivery checks (`pending` / `success` / `failure`); GitHub's other checks are not read.
- **Hygiene.** After a merge through Yulius the head branch is deleted (not the integration,
  production or default branch, nor one another open pull request uses). Cleanup never forces a
  worktree removal, never removes a live task's worktree, a protected branch or one with an open
  pull request, and removes `idle` branches (unmerged, no pull request) only with `force`.

## CD (deployment)

A project's CD (SPEC-134, ADR-0109/0110) is two environments in `yulius.toml`, read only from
protected branches: `[cd.dev]` at the integration branch head, `[cd.prod]` at the commit it deploys
(the integration head when that head's `[cd.prod]` promotes, else the production head).

```toml
[cd.dev]                            # missing: "not defined"; no deploy/verify/promote: "empty"
about = "the team's dev VM"         # one line for people (at most 300 characters)
deploy = "scripts/cd/deploy.sh dev" # one line, /bin/sh -c in the environment's own checkout
verify = "scripts/cd/verify.sh dev" # after a successful deploy
secrets = "DEV_SSH_KEY DEV_HOST"    # at most 16 agent secrets, as environment variables

[cd.prod]                           # the same keys, and:
promote = true                      # a person's deploy first promotes the integration branch
```

A run deploys the commit GitHub reports for the branch, taken into a mirror only Yulius writes
(every object checked, replacement refs ignored). It writes the commit's files into the run's own
directory and runs `deploy` then `verify` through the process host (dev inside the sandbox's
containment, production on the sandbox's network only and the host's read-only system), for at most the CI job
timeout, with `YULIUS_CD=1`, `YULIUS_CD_ENV`, `YULIUS_CD_RUN`, `YULIUS_CD_SHA`,
`YULIUS_CD_BRANCH`, `YULIUS_CD_CACHE` (persistent, per environment, out of the agents' reach), a
`HOME` made for the run and removed after it, and the real value of each named secret allowed for
that environment: dev, allowed outside the proxy (`env_files = allowed`, ADR-0105); production,
allowed for production deploys (`cd_prod`). No task token, provider credential or GitHub token. Its stored output is the last 512 KiB with every
agent secret of the sandbox redacted. One run per environment at a time; a run left running by a
restart is recorded as `error` and never run again by itself.

| Method and path | Access | Result |
| --- | --- | --- |
| `GET /api/sandboxes/{sid}/cd` | read | `CdOverview` |
| `POST /api/sandboxes/{sid}/cd/dev/deploy` | write | the running `CdRun` |
| `POST /api/sandboxes/{sid}/cd/prod/deploy` | admin, `{"confirm": "<slug>", "expect": "<sha>", "confirm_ms": 4200}` | the running `CdRun` after the promotion |
| `GET /api/sandboxes/{sid}/cd/runs/{rid}` | read | `CdRun` with its `output` |
| `POST /api/sandboxes/{sid}/cd/runs/{rid}/cancel` | write (a production run: admin) | `CdRun`; `conflict` when it ended |
| `POST /api/agent-secrets/{id}/cd-prod` | manage the secret, `{"allow": true}` | `AgentSecretView` with `cd_prod`; production runs get it |

```text
CdOverview = {"slug", "environments": [CdEnvironment (dev, then prod)], "runs": [CdRun] (newest 20),
              "can_deploy_dev": true, "can_deploy_prod": false}
CdEnvironment = {"name": "dev|prod", "label": "dev|production", "state": "not_defined|empty|ready",
                 "branch", "head", "problem": "yulius.toml line 3: …"|null, "about", "deploy", "verify",
                 "promote", "promotes_from": "<integration head>"|null,
                 "secrets": [{"name", "state": "ready|not_allowed|missing"}],
                 "deployed": "<sha of the last passed run>"|null, "last_run": CdRun|null}
CdRun = {"run_id", "environment": "dev|prod", "state": "running|passed|failed|error|cancelled",
         "branch", "commit", "requested_by": "<login>|task #<n>", "deploy", "verify",
         "note": "promoted 'dev' to 'main' at …"|null, "reason", "exit_code", "started_at", "ended_at",
         "output" (detail only)}
```

`expect` (optional, any deploy) is the commit the person was shown: the one deployed, or with a
promotion the integration commit promoted; the deploy refuses (`conflict`) when the branch moved
since. `confirm_ms` (optional, production) is how long the person kept the confirmation open, as the
page measured it; up to an hour it is recorded as the run's confirmation wait (SPEC-137). A production secret (`cd_prod`) is exclusive: allowing it is refused while it has `.env`
copies allowed or proxy hosts, and it then refuses both (`conflict`); dev deploys never get it.

Refusals: `forbidden` for production below administrator access, `invalid` for an unknown
environment or a production deploy without the slug typed exactly, `conflict` for an empty or
undefined environment, a missing or not-allowed secret ("nothing ran: …"), a running run, or a
promotion that cannot complete now (its pull request's checks). Every run is audited before it
starts (`cd.deploy`), its end (`cd.finished`), cancellations (`cd.cancel`) and refused production
deploys (`cd.refused`). Agents use the MCP tools `cd_status` and `cd_deploy` (dev only; production is
refused whatever they pass); `yulius wait` returns when the task's CD run ends (`cd_run`, `cd_state`,
`cd_detail` in its state).

## Stages (pipeline measurements)

Every stage of a task, a CI run and a CD run is a span (SPEC-137, ADR-0112), recorded by Yulius as
it happens: a task's states (`queued`, `chained`, `prepared`, `starting`, `working`, `idle`,
`needs_you`, `done`, `paused`, `switching`, `stopping`), its `needs_you` requests, steps (with the
tokens and agent working time its runs recorded meanwhile), runs, pauses, delivery stages,
armed-merge waits and updates onto the integration branch; a CI run's queue (with why it waited),
preparation, job-slot waits, jobs, checkouts and the stages its output names; a CD run's
confirmation, checks, promotion, deploy, verify and the stages their output names. Spans recorded
before migration 28 from older tables are `backfilled`; a CI result reused from an identical tree
is `detail.reused` and not counted. Spans are kept 180 days. Nothing writes them through the API.

A CI job or CD script names its stages and figures with lines Yulius reads as it runs; Yulius
times them by its own clock:

```text
::yulius-stage build            # a stage starts (the one before ends); 1-48 characters
::yulius-measure cache_hits=3   # figures of the open stage (or the job): [a-z][a-z0-9_]{0,31}=<number>
::yulius-stage-end              # the open stage ends (the job's end ends it too)
```

Test summaries of cargo nextest, cargo test, pytest and Playwright add `tests_passed`,
`tests_failed`, `tests_skipped` and `tests_flaky` to the open stage. A figure never overwrites what
Yulius records of the span: `tests_…`, `…_start` and the names Yulius writes itself
(`RESERVED_FIGURES` in `yulius-domain/src/stages.rs`: `sha`, `reason`, `exit_code`, `cpus`,
`input_tokens`…) are ignored, as are new names past 64 per span. Figures are written once per span
each half second the output is read (counts added up, the last value kept).

A CI job's `job` span and a CD run's `run` span also carry what its processes used (SPEC-154,
ADR-0127), measured by the process host when it reaped them and written with the job's end:
`cpu_ms` (user + system CPU time of the job's processes that ended and were reaped within it),
`memory_peak_mb` (the largest peak resident memory of one of them, in MiB, at least the host's own
few MiB a fork starts with) and `disk_written_mb` (what they wrote to storage, rounded to the
nearest MiB, on Linux hosts that account for it). Background processes still running when the job
ends are killed and not counted. A job Yulius killed (cancelled, timed out, too much output), one
that never started, one whose host was lost, or an older host's: the figures are absent, never `0`.
The three names are reserved.

| Method and path | Access | Result |
| --- | --- | --- |
| `GET /api/sandboxes/{sid}/stages/timeline?subject=<task|ci_run|cd_run>:<id>` | read | `TimelineView` |
| `GET /api/sandboxes/{sid}/stages/summary?from&to&subject&kind&stage&environment&job&trend` | read | `StageSummary` |
| `GET /api/sandboxes/{sid}/stages/spans?format=json|csv&from&to&subject&kind&stage&environment&job` | read | `[SpanView]` or CSV (`yulius-stages-<from>-<to>.csv`) |

`from`/`to` are UTC days (`YYYY-MM-DD`, default the last 30 days, at most 366); `trend` is
`<subject>:<kind>:<name>` (default `ci_run:run:run`).

```text
SpanView = {"subject_kind", "subject_id", "key", "parent_key", "kind", "name", "environment", "job",
            "started_at", "ended_at"|null, "duration_ms"|null, "outcome"|null,
            "source": "recorded|backfilled", "detail": {…figures}}
TimelineView = {"subject_kind", "subject_id", "spans": [SpanView] (the newest 2,000), "related": [SpanView]
                (a task of this sandbox: its PR's CI runs), "truncated": bool (it has more)}
StageSummary = {"from", "to", "stages": [StageRow], "trend": {"subject_kind", "kind", "name",
                "days": [{"date", "count", "p50_ms", "p90_ms"}]}, "delivery": [DeliveryFigures (dev, prod)],
                "options": {"jobs", "environments", "kinds"}, "reused_runs", "truncated"}
StageRow = {"subject_kind", "kind", "name", "job", "environment", "count", "failed", "running",
            "backfilled", "p50_ms", "p90_ms", "max_ms", "total_ms", "cpu_p50_ms"?, "cpu_p90_ms"?,
            "memory_peak_p50_mb"?, "memory_peak_p90_mb"?}   # ? only when an ended span measured it
DeliveryFigures = {"environment", "deployments", "passed", "per_week", "lead_time_p50_ms",
                   "lead_time_p90_ms", "lead_time_count", "change_failure_rate",
                   "time_to_restore_p50_ms", "restores"}
```

## Skills, workflows, automations, MCP

| Method | Path | Level | Notes |
| --- | --- | --- | --- |
| GET | `/api/sandboxes/{sid}/skills` | read | `[{"name","description","source":"repository|instance|yulius|open-mercato|superpowers","path","enabled"}]`; `yulius`, `open-mercato` and `superpowers` are the bundled packs (SKL-007) |
| GET | `/api/sandboxes/{sid}/skills/{name}` | read | `{"name","content"}` (`SKILL.md`, at most 512 KiB) |
| PUT | `/api/sandboxes/{sid}/skills/{name}` | write (instance skills: instance admin) | `{"enabled":bool}` → 204. Repository skills → 422 (always available inside the repository). |

The web UI edits and creates repository skills as files: a repository skill is
`.agents/skills/<name>/SKILL.md` (or `.claude/skills/…`) in the sandbox repository, so its skill page
(UI-033) reads and saves it with the built-in editor's files API (root `sandbox`, the save carrying
the read's `sha256`) and New skill (UI-035) creates the folder and the file there. Instance skills
and the bundled packs (`yulius`, `open-mercato`, `superpowers`) have no write API and are
read-only in the UI.
| GET/POST | `/api/sandboxes/{sid}/workflows` | read/write | `Workflow`; POST → 201. `"scope":"instance"` creates an instance workflow (instance admins). |
| PUT/DELETE | `/api/workflows/{id}` | write (instance: admin) | PUT replaces name, description, steps, instructions, gates → `Workflow`; DELETE → 204. A default workflow (`builtin`) → 409 `conflict` ("duplicate it to customise"). |
| GET/POST | `/api/sandboxes/{sid}/automations` | read/write | `Automation`; POST → 201 |
| PUT/DELETE | `/api/automations/{id}` | write | PUT replaces the editable fields → `Automation`; DELETE → 204 (removes run history; tasks stay) |
| POST | `/api/automations/{id}/run` | write | creates one run now (also when disabled) → `AutomationRun`; an optional body `RunAutomation` is this run's start choice (checked like `Automation`'s, 422 when invalid, not stored; SPEC-093), none uses the stored one |
| GET | `/api/automations/{id}/runs` | read | `[AutomationRun]`, newest first (at most 100) |
| GET | `/api/sandboxes/{sid}/automation-templates` | read | `[AutomationTemplate]`, the ready-made automations (SPEC-042) |
| POST | `/api/sandboxes/{sid}/automation-templates/{id}/enable` | write | creates the automation from the template, enabled → 201 `Automation`; when the sandbox already has an automation of that name, enables it unchanged → 200 `Automation`; unknown template → 404 |
| GET/POST | `/api/sandboxes/{sid}/mcp` | read/write | `McpServer` (sandbox + inherited instance servers); POST body `McpServerInput` → 201 |
| PUT/DELETE | `/api/mcp/{id}` | write (instance: admin) | PUT `McpServerInput` → `McpServer`; DELETE → 204 |
| POST | `/api/mcp/{id}/test` | write (instance: admin) | `{"ok","tools":["…"],"error"}`; HTTP ≤ 5 s, stdio ≤ 10 s then the process group is killed. On a contained instance stdio tests need an instance admin (they run outside the containment). |

```json
Workflow = {"id","name","description","steps":["Write the spec","Implement","Test"],
            "instructions":"…","gates":{"require_spec":true,"require_tests":true},"scope":"instance|sandbox",
            "builtin":false,"when_to_use":""}
Automation = {"id","name","cron":"0 9 * * *","utc_offset_minutes":120,"prompt","workflow_id|null",
              "harness":"claude","enabled":true,"supertask":false,"after_task_id":"…|null",
              "after_task_number":12|null,"start_rule":"#12 done|null","prepared":false,
              "created_via":"ui|agent",
              "next_run_at":"…|null","last_run":AutomationRun|null}
RunAutomation = {"supertask":false,"after_task_id":"…|null","start_rule":"#12 done|null","prepared":false}
AutomationTemplate = {"id":"docs-up-to-date","name","description","cron","utc_offset_minutes","prompt",
                      "workflow_id|null","automation_id|null"}
AutomationRun = {"scheduled_for","state":"scheduled|task_created|skipped_previous_active|skipped_disabled|failed",
                 "task_id|null","error|null"}
McpServer = {"id","scope":"instance|sandbox","name","transport":"stdio|http","command|null",
             "args":[],"url|null","env_keys":["APIFY_TOKEN"],"header_keys":[],"enabled":true}
McpServerInput = {"name","transport","command?","args?","url?","env?":{"KEY":"value"},
                  "headers?":{"Name":"value"},"enabled?":bool,"scope?":"sandbox|instance"}
```

The list starts with the **default workflows** (AUT-006): one per file of `catalog/workflows/`
in the running release (`wf-<file stem>`, for example `wf-feature`, `wf-fix`, `wf-research`),
instance scope, `builtin: true` and a one-line `when_to_use`; they are never stored, so an upgrade
changes them, and they cannot be changed or deleted (409): POST a copy to customise one. Then the
instance's and the sandbox's own workflows (`builtin: false`, `when_to_use` empty; both fields are
ignored on input). The rows `wf-feature` "Feature" and `wf-quick-task` "Quick task" seeded by
earlier releases are retired at startup: unchanged, they are deleted (`wf-feature` then means the
default; automations using `wf-quick-task` continue without a workflow); an edited "Feature" is
kept as the instance workflow "Feature (custom)". The file format is SPEC-002 §7.10 "Workflow
catalog format". Automation `cron` is a five-field
expression (`yulius-domain::cron`), `utc_offset_minutes` is within ±1080, and at most 100
automations exist per sandbox. A run whose automation's previous task is still active (also
waiting `chained`, `queued` or `prepared`) is recorded as `skipped_previous_active`. `supertask`,
`after_task_id`, `start_rule` and `prepared` say how each created task starts, as in `CreateTask`
(SPEC-078; `prepared`, "When I decide": each task waits `prepared` until Start now, with no start
time, SPEC-155): none of them, it starts now (queued); at most one is given (else 422), the task to run after is a
task of the sandbox and the rule names only its task numbers (422 otherwise; the rule is stored
canonically). A PUT replaces them like the other fields; the stored choice sent back unchanged is
kept without that check (its task may have been deleted since). A run after a task that has ended waits
for the rule `#N done` instead; a run whose task to run after was deleted, or that would prepare a
33rd prepared task of the sandbox, is `failed`.
`after_task_number` is read-only. The ready-made automations are one file each in
`catalog/automations/` of the running release; `automation_id` is the sandbox's automation with
the same name (case-insensitive), which counts as the template's: a template is never linked or
run by itself.

Task agents and Cockpits manage the same things through the Yulius MCP tools (SPEC-002 §7.4,
SPEC-037, SPEC-066), acting for the task's creator or the Cockpit's owner, who must still have
write access; every changing call is audited with the caller (`task:<n>` or `cockpit:<login>`):

| Tool | Same as | Notes |
| --- | --- | --- |
| `task_create` | New task | origin `agent`; at most 20 per project and rolling hour |
| `task_stop`, `task_archive` (`archived`: false restores), `task_delete` | `POST /api/tasks/{tid}/stop`, `…/archive` (`…/restore`), `DELETE /api/tasks/{tid}` | by `task_id` or `number`; never the caller's own task; audited `task.stopped`, `task.archived`/`task.unarchived`, `task.deleted` |
| `automation_create`, `automation_update` | `POST …/automations`, `PUT /api/automations/{id}` | `created_via: agent` |
| `automation_delete`, `automation_run_now` | `DELETE /api/automations/{id}`, `POST /api/automations/{id}/run` | audited `automation.deleted`, `automation.run_now` |
| `automations_list` | `GET …/automations` and `GET …/automation-templates` | `{"automations":[Automation],"ready_made":[AutomationTemplate without "prompt"]}` |
| `automation_template_enable` | `POST …/automation-templates/{id}/enable` | `{"template":"docs-up-to-date"}` → `{"automation","created":bool,"note"}`; `created_via: agent`; an unknown id is an error naming the known ones |

MCP secrets are write-only: responses only list key names. On update, an omitted `env`/`headers`
keeps the stored values; a given map replaces all values of that map (`{}` clears it). Names are
1–64 letters, digits, `-` or `_`, unique per scope ignoring case and `-`/`_`; `yulius` is reserved
for the always-attached Yulius tool server. A sandbox server overrides an instance server of the
same name for that sandbox (also when disabled). Agent runs receive HTTP secrets only as an
`Authorization: Bearer` token; other headers are used by the test endpoint but not yet passed to
Claude Code.

Every run gets the effective servers in its own MCP configuration, whichever Claude subscription
it runs on, also after a switch of subscription (MCP-004). A secret value may be a reference to an
agent secret instead of the secret itself: `${secret:NAME}` (NAME: 1–128 letters, digits, `_`,
`-`, `.`; for the token, `"Authorization": "Bearer ${secret:NAME}"`). It is stored and listed like
any value (key names only) and resolved when a run starts by the agent-secrets vault; while it
resolves to nothing, the run gets the server without that variable or token.

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
    front for every viewer; `null` stops this connection's stream), `{"type":"ack","frames"}`
    (flow control: the number of binary video frames received on this connection so far, sent as
    each arrives; once a viewer acknowledges, the server keeps only about a round trip of video
    on the way to it and adapts the stream's bitrate to what it receives; a viewer that never
    acknowledges is not paced), `{"type":"keyframe"}` (the viewer's decoder needs a keyframe:
    it failed or fell far behind; rate-limited).
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
  - Bounds: an acknowledging viewer has at most its base round trip plus 120 ms of video
    unacknowledged; video that waited on the server 250 ms (or beyond 300 frames, 8 MiB) is
    dropped and the viewer resumes at a fresh keyframe, which the server asks the encoder for
    (keyframes come only on demand: at the start, for a viewer that joins, falls behind or asks,
    and on a bitrate change); JPEG frames keep the newest 4; a client that stops reading JSON
    messages is closed with 4008. Input: 500 events per second (burst 1000); moves and wheel
    beyond are dropped silently (they leave 100 of the burst to other input), other input gets an
    `invalid` error. Pointer moves are coalesced (the newest position is applied; a press, release
    or key lands after the position before it) and a move or wheel turn is never answered with an
    error; wheel turns are summed while Chrome has two outstanding. Discrete input the display
    cannot take (its X server stopped reading) gets `capacity`. Keys and buttons a viewer held are
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
  2000 entries). `GET /api/sandboxes/{sid}/files/content?root&path&known?` → `{"path","content":
  "…|null","binary","too_large","size","sha256"}` (UTF-8 text up to 2 MiB); with `known`, the
  `sha256` the caller has (64 lowercase hex digits, else 422), an unchanged file answers 204 with
  no body (the Editor looks at the file in front every 2 s, SPEC-181). `PUT
  /api/sandboxes/{sid}/files/content` `{"root?","path","content","base_sha256?"}` →
  `{"path","sha256","size"}`: atomic; 409 when the file changed since `base_sha256` or, without
  a base, when it exists. `POST /api/sandboxes/{sid}/files` `{"root?","path","kind":"file|dir"}`
  → 201 entry (409 when it exists). `DELETE /api/sandboxes/{sid}/files?root&path` → 204 (files,
  links and directory trees; never the folder itself). 409 before the repository is cloned.
  The editor UI (Monaco) keeps each user's open files and unsaved text in their browser only.
  `PUT /api/sandboxes/{sid}/files/presence` (write) `{"root?","path","sha256?","selection":
  {"anchor","head"}|null,"hello"?,"client"?,"seq"?}` → 204 (SPEC-127): the caller's selection in
  UTF-16 offsets of the file's text as saved at `sha256` (64 lowercase hex digits, empty when
  unknown), or `null` when they left the file; `hello`, `client` and `seq` as for notepad presence. Relayed as
  `file.presence` to the sandbox's writers only, stored nowhere. The path is checked like the
  other file requests (403 for `.git`, 422 for an empty or escaping path, a folder or a path over
  4096 bytes); a selection needs an existing regular file (404; leaving a deleted one is fine);
  422 for unknown fields, a bad `sha256` or `client`, or an offset over 2 Mi; 403/404 without
  write access; 429 past 20 updates a second per person and file (the notepads' limiter) or past
  64 files and notepads a person is busy in at once.
- Dropped files (EDT-006, UI-048). One file per request, the raw bytes as the body (`Content-Type`
  is not interpreted), streamed to disk; at most the upload limit (`YULIUS_UPLOAD_MAX_MB`, 25 MiB by
  default; `GET /api/me` → `instance.upload_max_bytes`): a larger `Content-Length` is 422 before
  the body is read, a body that grows past it is 422 as it arrives. At most 8 uploads run at once
  on the instance (429 `capacity`, retry); a body that sends nothing for 60 s fails (422). The
  file is written to a temporary file next to its destination, synced, then linked or renamed
  into place, so readers see the whole file or none; an unfinished upload leaves nothing.
  `UploadedFile = {"path","name","size","sha256","replaced"}`. Every upload is audited
  (`file.uploaded`, `attachment.uploaded`).
  - `PUT /api/sandboxes/{sid}/files/upload?root&path&overwrite` (write) → 201 `UploadedFile`
    (`path` relative to the root). `root` and `path` as in the editor API (403 for `.git` or a
    path leading outside the folder, also through links; 422 for traversal, absolute paths,
    control characters or a name over 255 bytes). Missing folders on the way are created. An
    existing file is 409 unless `overwrite=true` (the UI asks first), which replaces it keeping
    its permissions; a folder or a symbolic link at that name is always 409.
  - `POST /api/tasks/{tid}/attachments?name` (write; whoever may type into the task's terminal)
    and `POST /api/sandboxes/{sid}/shells/{id}/attachments?name` (the shell's owner; 404 for
    everybody else) → 201 `UploadedFile` with the **absolute** path the agent or shell opens:
    `<data>/sandboxes/<sid>/attachments/tasks/<tid>/<name>` or `…/shells/<id>/<name>`, inside the
    sandbox directory (the same path inside containment) and
    outside every worktree, so never in `git status` or a pull request. `name` is reduced to its
    last component with letters, digits, `.`, `_`, `+` (other characters become `-`; no leading
    dot or dash), so the path needs no quoting; an existing name gets `-2`, `-3`, … before its
    extension. At most 500 per task or shell (409). Removed with the task (`DELETE
    /api/tasks/{tid}`) or when the shell ends.
- Terminals (sandbox shells; UI-015, UI-016): each is private to the user who opened it and to its
  sandbox. `Shell = {"id","name","task_id|null","folder","created_at"}`; `folder` is the absolute
  path it started in (the repository or the task's worktree) as its processes see it: the web
  terminal links printed paths inside it to the editor (UI-018). bash reports its working
  directory at every prompt as OSC 7 (`PROMPT_COMMAND`; a user's own `PROMPT_COMMAND` replaces it).
  - `GET /api/sandboxes/{sid}/shells` (write) → `[Shell]`: the caller's live shells in this
    sandbox, oldest first (never another user's or another sandbox's).
  - `POST /api/sandboxes/{sid}/shells` (write) `{"task_id?","name?"}` (or an empty body) → 201
    `Shell`: a login shell in the sandbox repository or that task's worktree, as the caller's
    sandbox identity (their home; uid 0 with `sudo` when contained). The name defaults to
    "Terminal N", the lowest number free among the caller's shells there. At most 8 live shells
    per user and sandbox (429); 409 before the repository is cloned. In an isolated sandbox
    (SPEC-002 ADR-22) the shell joins the sandbox's namespace holder (its own system layer, where
    `sudo apt-get install` persists, and private network); 409 when the sandbox is over its disk
    quota (the message gives the figures; Yulius first prunes the caches of tasks that do not
    work); a task started then fails with that reason in its terminal ("Could not start: …").
    `POST /api/tasks/{tid}/switch` answers 409 with the same reason, before it ends the running
    session, when the new session could not start.
  - `PATCH /api/sandboxes/{sid}/shells/{id}` (write) `{"name"}` → `Shell`: 1–40 characters, no
    control characters (422). `DELETE /api/sandboxes/{sid}/shells/{id}` (write) → 204: the shell
    leaves the list at once and its process is ended (Ctrl-C, SIGTERM, SIGKILL). Both are 404 for
    another user's shell and for a shell of another sandbox.
  - `WS /api/shells/{id}/terminal[?cols=&rows=]` (only the shell's creator; 404 for everybody
    else) speaks the terminal protocol (`state` has `attempt_seq` and `harness` `null`). A shell
    ends when its process exits (the server sends `exit` and closes), when it is closed, or after
    24 h without viewers; it keeps running across a server restart. Shells never change tasks.
- Cockpit (SPEC-021, ADR-0020): every user's own plain Claude Code session per sandbox, a shell
  of kind `cockpit`; the web UI calls it the Superagent (SPEC-074, formerly the Agent) and shows
  it in the right panel (SPEC-035).
  - `POST /api/sandboxes/{sid}/cockpit` (write) → 200 `Shell` (`name` "Cockpit"): the caller's
    running Cockpit, or a new one started (concurrent calls start one) as `claude [--resume
    <newest conversation>] --model claude-opus-5-5 --effort high` (SPEC-023) in the sandbox
    repository, as the caller's sandbox identity, with the token of the pool's best subscription
    for Opus and `CLAUDE_CONFIG_DIR=~/.claude`, otherwise a
    terminal's environment (Git as the caller); no Yulius hooks, settings or approval bypass. It
    gets the Yulius MCP server (SPEC-037, ADR-0029): `--mcp-config` inline JSON for
    `<internal URL>/mcp` with `Authorization: Bearer ${YULIUS_COCKPIT_TOKEN}` (the `ck_…` bearer
    in the secret environment, valid while that Cockpit is live and its owner may work), and
    `--allowedTools` for the read-only tools. It is served `tasks_list`, `task_get`,
    `task_create`, `task_stop`, `task_archive`, `task_delete`, `automation_create`,
    `automations_list`, `automation_update`, `automation_delete`, `automation_run_now`,
    `automation_template_enable`, `workflows_list`, `sessions_search` and `session_read`, acting
    for its owner (SPEC-066); Claude Code asks the person before every tool that is not read-only.
    A conversation it could not resume (an error exit within 10 s) is skipped next time.
    A usage limit Claude Code records in its conversation after it started is recorded on its
    subscription, and the Cockpit starts again on one with capacity, resuming the conversation
    stepped back over the limit with `CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1` (SPEC-074). The old
    one's viewers get `{"type":"exit","status":"switched"}`, and the next call returns the new
    one. Without capacity it stays, and its viewers get a `notice`.
    409 before the repository is cloned or without a Claude
    subscription, 429 while none has capacity. There is no delete: it is never in `GET …/shells`,
    `PATCH`/`DELETE …/shells/{id}` answer 404 for it, and it does not count against the 8
    terminals. Its terminal is `WS /api/shells/{id}/terminal`; files go to
    `POST …/shells/{id}/attachments`. It ends when Claude Code exits or after 24 h without
    viewers, keeps running across a restart, and the next call resumes its conversation.
- Notes ([SPEC-041](../.ai/specs/SPEC-041-here-we-want-auto-saved-notes-sandbox-wi.md),
  [SPEC-103](../.ai/specs/SPEC-103-add-me-after-team-that-is-for-user-and-n.md)): one notepad per
  sandbox, one for the team and one per person, saved by the web UI as people type.
  - `GET /api/sandboxes/{sid}/notes` (read), `GET /api/notes` (anyone signed in) and
    `GET /api/me/notes` (the caller's own; no route reaches another person's) →
    `Note {"scope":"sandbox|team|me","body","revision","updated_at","updated_by"}`; a notepad never
    written is `{"body":"","revision":0,"updated_at":null,"updated_by":null}`.
  - `PUT /api/sandboxes/{sid}/notes` (write), `PUT /api/notes` (anyone signed in) and
    `PUT /api/me/notes` (the caller's own) `{"body","revision"}` → `Note`: `revision` is the
    revision the edit started from; the text is stored as revision + 1. 409 `conflict` (nothing
    stored) when the stored revision differs: someone saved meanwhile, so the client loads the
    note again. 422 over 256 KiB of UTF-8.
    Every stored edit publishes `note.updated`.
  - `PUT /api/notes/presence` (anyone signed in) and `PUT /api/sandboxes/{sid}/notes/presence`
    (read) `{"revision","selection":{"anchor","head"}|null,"hello"?,"client"?,"seq"?}` → 204
    (SPEC-112): the caller's selection in UTF-16 offsets of the text at `revision`, or `null` when
    they left; `hello` asks the others to answer with theirs; `client` names the browser tab (at
    most 32 letters and digits); `seq` (an unsigned integer, default 0) grows with each update the
    tab sends, so receivers drop one older than the newest they heard from that tab (0: not
    numbered; SPEC-170, ADR-0138). Relayed as `note.presence`, stored nowhere.
    422 for unknown fields, a negative `seq` or an offset over 1 Mi, 404 without read access, 429 past 20 updates a
    second per person and notepad. There is no presence for the personal notepad.

## Administration, analytics, status

| Method | Path | Who | Notes |
| --- | --- | --- | --- |
| GET | `/api/admin/users` | instance admin | → `[AdminUser]` = `{id, github_login, display_name, is_instance_admin, disabled, github_linked, two_factor}` (`two_factor`: `missing`, `pending` or `on`; the superadmin is not listed) |
| POST | `/api/admin/users` | superadmin | POST `{"github_login","display_name?","is_instance_admin?"}` → 201 `AdminUser` |
| PATCH | `/api/admin/users/{id}` | superadmin | `{"disabled?","is_instance_admin?"}` → `AdminUser` |
| GET | `/api/admin/accounts` | instance admin | `[Account]` |
| POST | `/api/admin/accounts` | instance admin | `{"provider":"anthropic","label"}` → 201 `{"account":Account,"login_id"}` (status `pending_login`) then use the login terminal; nothing is created when the login cannot start |
| PATCH | `/api/admin/accounts/{id}` | instance admin | `{"label"}` (1–80 chars) → `Account`; the pool has no priority, order or on/off switch (F 2026-10-10): any other field is `invalid` (422) |
| DELETE | `/api/admin/accounts/{id}` | instance admin | → 204; 409 while a task that is not Done uses it; Done tasks' idle sessions on it are closed first (`closed`, as archiving does; 409 if one is still closing), SPEC-057; cancels its running login |
| POST | `/api/admin/accounts/{id}/relogin` | instance admin | → `{"account":Account,"login_id"}`; replaces a running login of the account |
| WS | `/api/admin/accounts/login/{login_id}/terminal` | instance admin | login terminal (below) |
| POST | `/api/admin/accounts/{id}/usage` | instance admin | → `AccountView`: asks Claude for the account's usage windows (`/api/oauth/usage`) and stores them; at or above 98 % in a window its tasks move to the best other subscription at their next turn boundary (ADR-17). The endpoint needs the OAuth scope `user:profile`, which a `claude setup-token` sign-in does not have: its HTTP 403 is not an error — the answer is the account as it is (its usage comes from its running sessions, `StatusLine` below), an earlier `usage: …` note is cleared and the background round asks again a day later. 502 with one plain sentence (also kept as `last_error`, prefixed `usage: `) when Claude refuses the sign-in (401) or the request fails for now. `AccountView` also carries `tokens_24h` / `tokens_7d` = `{"runs","tasks","input|null","output|null","cache_read|null","cache_write|null","cost_usd|null"}` observed by Yulius (ROT-009; runs started in the window; `cost_usd` is their estimate at Claude API list prices, SPEC-110) The server also refreshes every signed-in account in the background (every 10 min; on HTTP 429 it waits for `Retry-After`, else 30 min) and keeps the last known windows on failure; a limit the CLI reports fills its window at 100 % until the reset (ROT-009). |
| GET | `/api/capacity` | signed in | → `CapacityView`: the subscription pool, burn rate, forecast and recommendation (ROT-013, formula in SPEC-002 ADR-17 §2.4) |
| GET | `/api/admin/github` | superadmin | `{"configured","app_slug","owner_login","install_url|null","installed"}` |
| POST | `/api/admin/github/manifest` | superadmin | `{"owner_login","owner_kind":"org|user"}` → `{"post_url","manifest"}` (browser posts the manifest form to GitHub; `post_url` carries a one-time `state`, 30 min); 409 when an App is configured; 422 for a non-https public URL |
| GET | `/api/admin/github/manifest/callback?code&state` | superadmin | converts the code, stores the sealed App credentials and owner, 303 to the App's installation page; failures 303 to `/admin/github?error=…` |
| GET | `/api/analytics/summary?sandbox&from&to` | read | → `AnalyticsSummary` |
| GET | `/api/analytics/tasks?format=json|csv&sandbox&from&to` | read | → `[TaskRow]` or CSV |
| GET | `/api/status` | signed in | → `Status` |
| GET | `/api/update` | signed in | → `UpdateView` (below): the build and a running update's `phase` for everyone; the offered update, the last check, whether the helper is installed and the current or last run for the superadmin (OPS-006..OPS-009, SPEC-002 ADR-18) |
| POST | `/api/admin/update/check` | superadmin | reads `main` now (a check younger than 30 s is reused) → `UpdateView` |
| POST | `/api/admin/update` | superadmin | `{"commit","confirm":true}` → 202 `UpdateView`: installs exactly that commit, which must be the offered head of `main`; 422 without `confirm` or for a malformed commit, 409 without an installed helper, while an update runs, or for any other commit |
| POST | `/api/admin/update/cancel` | superadmin | while building or pausing tasks: nothing is installed, paused tasks go on → `UpdateView`; 409 otherwise |
| POST | `/api/admin/update/proceed` | superadmin | while pausing tasks: restart without waiting for tasks still in a turn → `UpdateView`; 409 otherwise |
| GET | `/api/admin/jev` | superadmin | → `JevKeyView` (never the key) |
| PUT | `/api/admin/jev` | superadmin | `{"key"}` (8–4096 printable characters, no spaces; 422 otherwise) → `JevKeyView`: sealed with the master key, used by task intake at once (ADM-002) |
| DELETE | `/api/admin/jev` | superadmin | removes the saved key (the `YULIUS_JEV_API_KEY_FILE` key is used again, if any) → `JevKeyView` |
| POST | `/api/admin/jev/adopt` | superadmin | saves the key of `YULIUS_JEV_API_KEY_FILE` in settings → `JevKeyView`; 409 without a usable file |
| POST | `/api/admin/jev/test` | superadmin | one small Jev request with the key in use → `{"ok","result":"ok|refused|unreachable|timeout|invalid|none","message","latency_ms|null"}` |

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
them when present, but **the server does not report them yet** (and the mock does not either, so
it looks like production), so today they are absent and the UI shows the label and provider. A
task's overview names each session's subscription by its label (`TaskDetail.attempts[].account_label`).
`in_use_by` counts the live tasks on the subscription except Done ones by the session limit's
rule (SES-101: goal achieved, or plan complete with the change merged; SPEC-057): a Done task
keeps its idle session but no longer holds the subscription.

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
a figure no run reported is `null`. Wall time comes from attempts (sessions open, idle
included), active time from metrics (the agent working, SPEC-087). `median_active` and `p90_active`
are over tasks whose runs reported active time. `by_status` counts a running task whose goal is
achieved as `done`.

```json
AnalyticsSummary = {
  "range": {"sandbox": "…|null", "from": "2026-10-01|null", "to": "2026-10-07|null"},
  "tasks": {"total": 3, "closed": 1, "by_status": {"closed": 1}, "by_harness": {"claude": 2},
            "by_origin": {"ui": 3}},
  "tasks_per_day": [{"date": "2026-10-01", "count": 1, "active_ms": 600000|null,
                     "median_active_ms": 600000|null, "input": 100|null, "output": 50|null,
                     "switches": 0}],
  "duration_ms": {"median": 1800000, "p90": 2400000, "total_wall": 3600000,
                  "total_active": 600000, "median_active": 600000, "p90_active": 600000},
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
days with tasks, runs or switches (never a day outside the range). A day's `active_ms`, `input` and `output` are of the cohort's runs
started that day, `switches` of its switches made that day, and `median_active_ms` the median agent
work of the tasks created that day (SPEC-110). `harnesses`, `models` and `accounts` are `;`-separated. `format=csv` returns
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
  "accounts": [{"label", "status", "usage", "available", "id?", "in_use_by?"}],
  "burn_per_day": [null, 1.2, …, 2.0], "paused_hours_per_day": [0.0, …, 8.0],
  "tokens": {"last_24h": Spend, "last_7d": Spend, "per_day": [Spend, …],
             "unknown_usage_runs": 0, "unpriced_runs": 0}
}
Spend = {"input": 600000, "output": 600000, "cache_read": 6000000, "cache_write": 600000,
         "cost_usd": 18.6|null}
```

Points are percentage points of one subscription's weekly window: `weekly_left` / `five_hour_left`
sum the headroom below 98 % over subscriptions with known use (`null` when none is known);
`burn_*` is points per hour from the provider readings of the last 24 h / 7 days (`null` without an
hour of readings); `runs_out_at` is when the weekly headroom runs out at `burn_24h`, walking through
each subscription's weekly reset, if within 7 days; `paused_hours_7d` / `active_hours_7d` are
task-hours; `needed` = ⌈max(burn_24h × 168 ÷ 98, subscriptions × (active + paused) ÷ active)⌉ and
`add` = needed − subscriptions (SPEC-002 ADR-17 §2.4). `resumes_at` is set while no subscription is
available. `id` and `in_use_by` are for instance administrators only.

The `*_per_day` lists and `tokens.per_day` are seven rolling 24-hour periods, oldest first, the last
ending now (SPEC-110). A run's tokens count in each period by the share of its lifetime (start to
end, or now) inside it; `last_24h` is the last period and `last_7d` their sum. `cost_usd` estimates
what the tokens would cost at Claude API list prices (`yulius_domain::pricing`, ADR-0089: cache
writes at the 5-minute rate); runs of a model without a price add tokens but no cost
(`unpriced_runs`), and `cost_usd` is `null` when every run that spent tokens there lacks a price
(a period without runs costs `0`). Runs without usage are
counted in `unknown_usage_runs`, never as zero.

```json
UpdateView = {"current":{"commit":"<40 hex>|null","short":"…|null"},
  "phase":"building|pausing|restarting|resuming|null",
  // superadmin only (null for everyone else):
  "helper_installed":true,
  "available":{"commit","short","summary":"A · B · C and 4 more","count":7,
               "commits":[{"sha","short","title","author|null","date|null"}],
               "relation":"behind|diverged|unknown"}|null,
  "check":{"checked_at|null","error|null","via":"app|token|anonymous|null",
           "relation":"up_to_date|behind|ahead|diverged|unknown|null","head|null"},
  "run":{"id","target":{"commit","short"},"from":{"commit|null","short|null"},"summary",
         "state":"building|pausing|restarting|resuming|succeeded|failed|rolled_back|cancelled",
         "step":"fetching|verifying|compiling|built|pausing|stopping|backing_up|installing|starting|checking|rolling_back|resuming|…|null",
         "message|null","progress":{"done","total"}|null,
         "tasks":{"paused","total","waiting":[{"id","number","title","sandbox"}],"deadline|null"}|null,
         "error|null","log_tail":["…"],"requested_by","started_at","state_since","finished_at|null",
         "can_cancel","can_proceed"}|null}
JevKeyView = {"configured","source":"settings|config|null","set_at|null","set_by|null",
              "config_present","config_error|null"}
```

`available` lists the commits from the build to the head of `main` without merges and promotions,
titles without their conventional prefix and pull request number. `run.progress` counts compiled
crates while building and released tasks while resuming; `run.tasks` counts live tasks at a pause
point while pausing. `log_tail` is filled only for a failed or rolled-back run. The internal
listener also serves `GET /internal/version` → `{"commit|null","version"}` for the update
helper's health check.

## Secrets vault

SPEC-001 VLT-001..009, SPEC-002 ADR-20. Values are never logged; responses that carry one have
`Cache-Control: no-store`.

**Team secrets** — every signed-in member. A team secret is a title, a value and notes (SPEC-040).

| Method | Path | Body → Response |
| --- | --- | --- |
| GET | `/api/team-secrets` | → `[TeamSecret]`: `{id, title, has_value, has_notes, created_by, updated_by, updated_at, value_updated_at, rotate_due}` (never a value or notes) |
| POST | `/api/team-secrets` | `{title, value?, notes?}` → 201 `TeamSecret` (any other field: 422) |
| PATCH | `/api/team-secrets/{id}` | the same fields, each optional: absent keeps, `""` clears → `TeamSecret` |
| DELETE | `/api/team-secrets/{id}` | → 204 |
| POST | `/api/team-secrets/{id}/reveal` | `{purpose?: "show"\|"copy", fields?: ["value","notes"]}` → `{value, notes}` (only the asked, stored fields). Audited (`vault.team.reveal` / `vault.team.copy`) |

**Two-factor** — the vault's lock (SPEC-040, ADR-0042), per person.

| Method | Path | Body → Response |
| --- | --- | --- |
| GET | `/api/auth/reauth` | → `{fresh, fresh_until, two_factor, identity_fresh, methods}`: `fresh` = unlocked (only with a confirmed seed); `two_factor` is `missing`, `pending`, `on` or `locked` (10 wrong codes in a row, until identity is confirmed again); `identity_fresh` = signed in or confirmed in the last 10 minutes; `methods` ⊆ `["password", "github"]` (identity confirmation) |
| POST | `/api/auth/reauth` | `{password}` (the superadmin) confirms identity → the status; 403 for a wrong password (audited; the session stays signed in) or an account without a password, 429 after 5 attempts a minute |
| POST | `/api/auth/two-factor` | → 201 `{secret, uri}`: a new base32 seed and its `otpauth://totp/…` link (replaces a pending one). Needs `identity_fresh` (else 403 `reauth_required`); 409 once confirmed; 429 after 5 a minute. Audited (`vault.2fa.setup`) |
| POST | `/api/auth/unlock` | `{code}` → the status: unlocks this browser session for 10 minutes; the first code confirms the setup (`vault.2fa.confirm`, later `vault.unlock`). 403 for a wrong or reused code (`vault.unlock.failed`; the 10th wrong code in a row locks codes, `vault.2fa.locked`, until `POST /api/auth/reauth` or a new GitHub sign-in) and while locked, 422 without two-factor, 429 after 5 attempts a minute. A code checked against a seed replaced meanwhile by a new setup is refused (403, not counted as wrong; SPEC-061) |
| DELETE | `/api/users/{id}/two-factor` | Instance administrators and the superadmin, for a lost authenticator (SPEC-061, ADR-0041) → 204: removes that person's seed and the unlocks of their browser sessions, audited `vault.2fa.reset` (detail: their login) when a seed was removed; 204 also when none was set up. Needs the caller's own unlock (else 403 `reauth_required`); 403 for members, bearer tokens and the caller's own id, 404 for an unknown user. The person sets two-factor up again on their next reveal. The user list does not include the superadmin, so their lost authenticator cannot be reset yet (ADR-0041) |
| GET | `/api/secrets/activity?limit=100` | → `[{at, actor, action, secret_id, detail}]`, newest first (≤ 500); an agent secret's entries only for members who can view its sandbox |

A reveal needs the vault **unlocked** in this browser session. Otherwise it answers `403`
`{"error": {"code": "reauth_required", …}, "reauth": <status>}`; the client sets two-factor up
when `two_factor` is not `on` (confirming identity first with `POST /api/auth/reauth` or a new
GitHub sign-in, `/api/auth/github/start?next=<page>`, when `identity_fresh` is false), sends a
code to `POST /api/auth/unlock`, then retries. Bearer tokens never set up, unlock or reveal (403).

**Agent secrets** — agents use values without seeing them; the people who manage one read it
after an unlock.

| Method | Path | Body → Response |
| --- | --- | --- |
| GET | `/api/sandboxes/{sid}/agent-secrets` | (read) → `[AgentSecret]`: `{id, name, description, scope ("sandbox"\|"instance"), hosts, requested_hosts, requested_by, env_files ("no"\|"asked"\|"allowed"), env_requested_by, injection, header_name, injection_text, placeholder, has_username, has_totp, created_by, updated_at, value_updated_at, last_used_at, rotate_due, can_manage}` — the sandbox's and the instance-wide ones (`has_totp`: only older secrets) |
| POST | `/api/sandboxes/{sid}/agent-secrets` | `{scope?: "sandbox"\|"instance", name, description?, hosts?: [..], injection?: "none" (default)\|"bearer"\|"basic"\|"header", header_name?, username?, value}` → 201. Sandbox scope: write access; instance: instance administrators. `name` is `[A-Z][A-Z0-9_]*` (not a reserved environment name); `hosts` 0–16 of `api.example.com`, `*.example.com`, optionally `:port`; `value` ≥ 8 characters without line breaks; `username` only for (and required by) `basic`. 409 when the name already applies to the sandbox |
| PATCH | `/api/agent-secrets/{id}` | any of `name, description, hosts, injection, header_name, username, value` → `AgentSecret` (`value` replaces it; `""` clears `username`) |
| DELETE | `/api/agent-secrets/{id}` | → 204 |
| POST | `/api/agent-secrets/{id}/request` | `{allow: bool, hosts: [..]}` → `AgentSecret`: answers the hosts an agent asked for with `secret_request_hosts` (SPEC-121): `true` binds them, `false` dismisses them; the request is cleared either way, in one step with the bindings. `hosts` are the requested hosts the person saw, in any order: 409 unless they are exactly what waits (a host asked for meanwhile is never allowed unseen), and 409 when nothing waits or the bindings changed meanwhile; 422 when allowing would bind more than 16 hosts (the request stays). People who manage it (403 for readers, 404 without access). Audited (`vault.agent.allow` / `vault.agent.dismiss`) |
| POST | `/api/agent-secrets/{id}/env` | `{allow: bool, env_files, env_requested_by?}` → `AgentSecret`: whether agents (every task of the sandbox; of every sandbox for an instance-wide secret) may copy the value into git-ignored `.env` files with `secret_copy_to_env` (SPEC-129). `true` answers an agent's request (`env_files` `asked` → `allowed`); `false` dismisses it or revokes an allowance (→ `no`; copies already written stay). `env_files` and `env_requested_by` are what the person saw: 409 unless they still hold, and when there is nothing to answer; 422 for an unknown `env_files`. People who manage it (403 for readers, 404 without access). Audited (`vault.agent.env_allow` / `vault.agent.env_dismiss` / `vault.agent.env_revoke`) |
| POST | `/api/agent-secrets/{id}/reveal` | `{purpose?, fields?: ["value","notes"]}` → `{value, notes}` (`notes` = the description, led by a stored user name and, for older secrets, `Two-factor seed: …`) for people who manage it, after an unlock (403 `reauth_required` otherwise; 403 for readers, 404 without access). Audited (`vault.agent.reveal` / `vault.agent.copy`) |

## UI event stream

`WS /api/events` → JSON: `{"type":"task.updated","task":TaskSummary}`, `{"type":"task.removed",
"task_id"}`, `{"type":"steps.updated","task_id"}`,
`{"type":"sandbox.updated","sandbox":Sandbox}`, `{"type":"browser.action_requested","sandbox_id",
"tab_id","reason","task_id"}`, `{"type":"automation.run","automation_id","state","task_id"}`,
`{"type":"account.updated","account":Account}` (instance administrators only),
`{"type":"update.updated","phase":"building|pausing|restarting|resuming|null"}` (an update was
found or moved; clients refetch `GET /api/update`),
`{"type":"note.updated","scope":"team|sandbox|me","sandbox_id","revision","updated_by"}` (a
notepad was saved; a sandbox note's event reaches only that sandbox's viewers, a personal note's
only its owner), `{"type":"note.presence","scope":"team|sandbox","sandbox_id","user_id","client","name",
"revision","selection":{"anchor","head"}|null,"hello","seq"}` (someone's selection in a shared notepad, to
the people who may read it; SPEC-112), `{"type":"file.presence","sandbox_id","root","path","user_id",
"client","name","sha256","selection":{"anchor","head"}|null,"hello","seq"}` (someone's selection in a file
open in the editor, only to people with write access to that sandbox; SPEC-127),
`{"type":"pulls.updated","sandbox_id"}` (the sandbox's pull requests may have
changed: a delivery stage moved or a Yulius CI run was queued, started or ended; clients watching
it read `GET /api/sandboxes/{sid}/git` again; SPEC-108), `{"type":"resync"}`,
`{"type":"heartbeat"}` (every 20 s, `events_heartbeat_ms`: a page that hears nothing for 50 s, or
for one beat when it is shown again or back online, takes the connection for dead and reconnects,
which resyncs; SPEC-191).

`TaskSummary.pause` is null or `{"by":"update|person","user":{"login"}|null,"since"|null,"pending"}`
while a live run is paused (SPEC-191, ADR-0154): an update holds it, or a person paused it;
`pending` while the pause has not taken hold yet. A capacity pause is `status: waiting_capacity`.

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
2 s, so a server restart does not release the agent; a hold nobody asks about for 45 s was given
up by the CLI (an interrupt, its own hook timeout) and ends: the task runs again. `401` means the
run is unknown or finished.

`SessionEnd` (body `{"reason": "prompt_input_exit|clear|logout|resume|other", …}`) answers `200`
with no output; it tells Yulius why the session ends before the process exits (above).

`POST /hooks/claude/StatusLine` is Claude Code's `statusLine` command (`yulius hook claude
StatusLine`, configured in every run's settings): the body is Claude Code's status line input, of
which Yulius reads `session_id` and `rate_limits` (`{"five_hour"|"seven_day": {"used_percentage":
0–100, "resets_at": <Unix seconds>}}`, present once a response of the session reported them);
`200` is `text/plain`: the line, "→ <repository> git:(<branch>)" in 256 colours (HRN-005). The
rate limits become the run's subscription's 5-hour and weekly windows (SPEC-002 ADR-17): stored
when they change (at once for a move of a point or more, a reset or crossing 98 %, otherwise at
most every 15 s) and sampled for the burn rate at most every 5 minutes (`usage_samples.source`
`session`).

While an instance update pauses the tasks (SPEC-002 ADR-18) every `Stop` and `UserPromptSubmit`
answers `202` with `Retry-After: 3` until the server that runs after the update releases the
task; other hooks answer as usual.

`PostToolUse` is not forwarded: the hook client redacts the tool result inside the sandbox
(below).

### Agent secrets on the internal listener (SPEC-002 ADR-20)

- **Egress proxy**: a `CONNECT host:port` or an absolute-form `http://` request to the internal
  listener is a proxy request, authenticated with `Proxy-Authorization: Basic
  base64("yulius:" + task token)` (407 otherwise). Hosts bound to an agent secret of the run's
  sandbox are intercepted (credential added, placeholders `yulius-secret-NAME` replaced,
  `X-Yulius-Secret: NAME` chooses between secrets adding the same header, 409 when ambiguous;
  the response redacted); other hosts are tunneled. Local destinations answer 403.
- `POST /internal/scrub/fingerprints` (task token) → `{version, base, salt, names, needles:
  [{secret, len, rolling, digest}]}`: the run's redaction fingerprints (no values).
- `POST /internal/scrub/report` (task token) `{channel: "bash"|"tool", names}` → 204; audited
  once per run, secret and channel (`vault.agent.redacted`).
- MCP: `secrets_list`, `browser_fill_secret {tab_id, secret, field?: value|password|username|totp,
  selector?}` and `secret_request_hosts {secret, hosts, reason?}` (stores a request a person
  allows, audited `vault.agent.request`, flags the task as needing a person; binds nothing),
  and `secret_copy_to_env {secrets, path?: ".env", reason?}` (SPEC-129: writes the values of
  secrets a person allowed into an ignored, untracked dotenv file of the task's worktree and
  their names into `.env.example`, audited `vault.agent.env`; a secret not yet allowed is asked
  for instead, audited `vault.agent.env_request`, and nothing is written; a key block stored with
  literal `\n` is written as its file in single quotes, SPEC-186); every MCP result is
  redacted. The Superagent (`ck_` token, SPEC-186) is served `secrets_list`,
  `secret_request_hosts` and `secret_copy_to_env` too (its copies go to the sandbox repository,
  its asks read `<login>'s Superagent`, its actor is `cockpit:<login>`), not `browser_fill_secret`;
  the egress proxy accepts its bearer as `yulius:ck_…`.
- MCP `secret_verify {secret, sha256}` (SPEC-178, the Superagent only; `sha256` may be
  `sha256sum`'s line) →
  `{secret, match: exact|trailing_newline|escaped_newlines|different, lines?, meaning}` (`lines`
  on a match only): whether the
  candidate whose SHA-256 is given is the secret's value, without a final line break, or with its
  line breaks as the literal `\n` the vault holds; never the value or a hash Yulius computed from
  it. Production secrets are refused; audited once per vault generation, sandbox, caller, secret
  and candidate (`vault.agent.verify`). Task agents are not served `secret_verify`.
