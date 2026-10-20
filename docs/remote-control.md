# Drive Yulius from your laptop

You can run a Yulius instance from your own computer, without opening its web UI: ask your local
Claude Code (or any agent) "import my repo", "make a task", "restart #12", or script it with the
`yulius` CLI. Both use the instance's remote MCP with a personal API token
([SPEC-247](../.ai/specs/SPEC-247-drive-yulius-fully-from-a-local-agent-cl.md),
[ADR-0188](decisions/0188-remote-control-uses-scoped-personal-api-tokens.md)).

You need the instance's URL (here `https://yulius.example.com`) and one visit to its web UI to
make the token. Everything after that happens on your laptop.

## 1. Make a token

Open **Settings → Connect your local agent** in the web UI. Name the token after where it will
live ("laptop Claude Code") and pick a scope:

- **Operate**: everything you may do yourself: import and onboard repositories, tasks, CI, file
  changes, automations, secret bindings.
- **Read only**: lists, tasks, transcripts, CI and files. It changes nothing, not even through
  the REST API.

The token is shown once, with the two commands that use it. Yulius stores only its fingerprint.
The list shows when each token was last used; **Revoke** ends a token at once. A token never
gives more than your own access: on each project, your GitHub permission decides what you may do,
as it does in the web UI.

## 2. Connect your agent

Claude Code:

```sh
claude mcp add --transport http yulius https://yulius.example.com/mcp \
  --header "Authorization: Bearer yul_…"
```

Any agent that speaks MCP over streamable HTTP can use the same URL and header. Then ask in plain
words. The server tells the agent how its tools fit together.

The CLI, for shells, scripts and agents without MCP:

```sh
yulius remote login https://yulius.example.com   # paste the token when it asks
yulius remote status                             # who the token acts for, and its scope
```

`yulius remote login` checks the token before it stores it, in
`~/.config/yulius/remote-credentials.json` (readable only by you). This file is separate from
the token of `yulius login`, which uploads checkouts with `yulius onboard`. Inside a GitHub checkout,
`--project` defaults to its `origin`. Pass `--project owner/name` anywhere else.

## 3. Onboard a new repository, laptop only

What to ask your agent, with the tool it calls and the CLI command that does the same:

| Step | Ask your agent | Tool | CLI |
| --- | --- | --- | --- |
| See what you can add | "Which repositories can I add to Yulius?" | `repositories_list` | `yulius project repositories` |
| Import it | "Import acme/shop" | `project_import` | `yulius project import acme/shop` |
| See what onboarding does | "What will onboarding do?" | `project_get` | `yulius project get` |
| Start onboarding | "Start its onboarding" | `onboarding_start` | `yulius project onboard` |
| Follow it | "How is the onboarding task doing?" | `tasks_list`, `task_get`, `session_read` | `yulius task list`, `yulius task get 1`, `yulius task transcript 1` |

The onboarding task adapts the repository to Yulius: docs, gates and Yulius CI, in its own pull
request, which it delivers on green. New tasks wait until it is done. An admin of the project can
skip it with `onboarding_skip`.

### Set up or change CI yourself

Yulius CI is configured by `[[ci.jobs]]` in the repository's `yulius.toml`. To change it, or any
other file, without a task:

1. Read the file: "show me yulius.toml" (`project_file_read`; `yulius project cat yulius.toml`).
   What it reads has the project's agent secrets masked as `[secret:NAME]`, as every tool
   result does; do not write such a masked copy back.
2. Change it: "add a job that runs `cargo test`" (`project_change`). Yulius commits your whole
   new file as you, on a new branch `yulius/change-…`, and opens a pull request into the
   integration branch. From a checkout's root, `yulius project change -m "ci: run cargo test"
   yulius.toml` sends your local copy.
3. Follow the CI on it: "how is CI on that pull request?" (`ci_runs`, `ci_run`; `yulius ci runs`,
   `yulius ci run <id>`).
4. Merge it once it passed: "merge it" (`change_merge`; `yulius project merge <number>`). A run
   that is pending or failed is refused, with its state.

Changes never touch a running task's files or the sandbox's checkout
([ADR-0189](decisions/0189-remote-file-changes-are-commits-on-a-change-branch-with-a-pull-request.md)).
For anything that needs judgment ("make the tests pass on macOS"), create a task instead.

### Secrets

Values never pass through an agent. Add them on the Secrets page, or from a dotenv file with
`yulius secrets import --to agent --sandbox acme/shop .env`. Your agent manages names and
bindings: `secrets_list` (`yulius secrets list`), `secret_update` (bound hosts, description)
and `secret_hosts_answer` (allow or dismiss the hosts a task asked for).

## 4. Day-to-day

| Ask your agent | Tool | CLI |
| --- | --- | --- |
| "Make a task: add a /health endpoint" | `task_create` | `yulius task create "Add a /health endpoint"` |
| "What are my tasks doing?" | `tasks_list` | `yulius task list --scope active` |
| "Tell #12 it may use the staging database" | `task_message` | `yulius task message 12 "…"` |
| "Restart #12" | `task_restart` (Continue; stop it first if it runs) | `yulius task restart 12` |
| "Stop #12" / "archive it" / "delete it" | `task_stop`, `task_archive`, `task_delete` | `yulius task stop 12`, `archive`, `delete` |
| "Pause #12" / "resume it" / "start #13 now" | `task_pause`, `task_unpause`, `task_start_now` | `yulius remote call task_pause '{"number": 12}'` |
| "What's #12's pull request and checks?" | `task_delivery` | `yulius task delivery 12` |
| "Why did CI fail?" | `ci_runs`, `ci_run`, `ci_rerun` | `yulius ci runs`, `yulius ci run <id>`, `yulius ci rerun <id>` |
| "Every night, update the docs" | `automation_create`, `automations_list`, … | `yulius remote call automation_create '{…}'` |
| "Deploy dev" | `cd_status`, `cd_deploy` (dev only) | `yulius remote call cd_deploy '{"environment": "dev"}'` |

`yulius remote tools` lists every tool your token may call. `yulius remote call <tool> '<JSON>'`
calls any of them. `--json` prints a tool's record instead of its text. The CLI exits 0 when the
tool did its work, 1 when the tool refused (its message says why), and 2 when the request reached
no tool (no token, a revoked token, the network).

## Limits and safety

- 120 requests a minute per token; tools create at most 20 tasks an hour per project.
- Production deploys are a person's action on the project's CD page
  ([ADR-0110](decisions/0110-only-a-person-deploys-production.md)).
- Every change made through a token is in the audit log as `remote:<your login>`, with the tool
  and the token, without its arguments.
- A lost laptop: revoke its token on Settings. Disabling a person refuses all their tokens.
