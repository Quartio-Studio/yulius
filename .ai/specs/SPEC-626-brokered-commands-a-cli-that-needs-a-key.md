---
yulius-task: c6b374d3-f5b1-4772-885e-a6a0365b1fb4
status: in-progress
---

# SPEC-626. Brokered commands: a CLI that needs a key runs outside the sandbox

## Goal

A task's agent runs a command-line tool that needs a private key or a credential file (a deploy
tool, a cloud SDK with a service-account JSON, a signing tool) without the key entering its
sandbox. A project's administrator defines the command once. The agent calls it by name with its
own arguments. Yulius runs it on the trusted side, in a fresh home, with the key in a temporary
file for that run only. It returns the exit code, stdout and stderr, with every stored secret
blanked out. A person approves each run, or only runs that change something, or none, as the
definition says. No answer within the stated time is a no. The owner approved the feature on
2026-11-22 after a review of iTermX's broker (task #610).

## Context

- **Secrets today.** An agent secret ([SPEC-002](SPEC-002-yulius-architecture.md) ADR-20,
  [architecture: agent secret hosts](../../docs/architecture.md#agent-secret-hosts)) reaches an
  agent in two ways. The egress proxy adds it to HTTP requests for its bound hosts, and the agent
  never sees it. Or it is a copy in `.env` (`secret_copy_to_env`), a restored agent state, or a
  file the agent wrote, and then the agent can read it. A CLI that reads a key file only works
  the second way.
- **iTermX's broker** (reference only, `broker.go` `run`). A command has a binary, a key, the
  hosts that may use it, `approve` (`always`, `send`, `never`), fixed arguments with `{keyfile}`,
  allowed subcommands, refused flags and env. Its checks: any argument with `/` or a leading `~`
  is refused, a refused flag is refused in its `--flag`, `--flag=` and `-fVALUE` forms, and the
  first non-flag argument must be an allowed subcommand. It runs in a fresh temporary home with
  the key written to a file there. The output has every stored key blanked out, and an approval
  waits 10 minutes.
- **Where a trusted run can happen on a contained instance.** Every sandbox has a production
  holder `<sid>.prod`
  ([ADR-0208](../../docs/decisions/0208-production-cd-runs-use-a-production-holder-with-its-own-docker-engine.md)).
  It has its own user, network and mount namespaces, its own `slirp4netns`, and no host loopback.
  Runs there use the host's read-only `/usr`, `/etc` and `/opt`, with empty `/var`, `/root` and
  `/home`, and files under `<data>/cd/<sid>/runs`, which the data directory's tmpfs hides from
  agent runs. No agent run joins it. Production CD runs use it
  (`App::cd_containment`, `production_containment`). They go through the detached process host
  and `ci_watch`, which bounds time, output and disk.
- **What may hold a key.** A production secret (`cd_prod`) reaches no run
  ([ADR-0324](../../docs/decisions/0324-a-production-secret-reaches-no-run-and-has-no-hosts.md)).
  A personal secret is resolved only for its owner's runs
  ([ADR-0186](../../docs/decisions/0186-agent-secrets-are-project-global-or-personal.md)).
  `redact_for_sandbox` blanks every agent secret a project can see, including each line of a
  multi-line key.
- **Asking a person.** `secret_request_hosts` flags the task (`NeedsAttention` with a reason that
  links the page), and the sidebar's Needs you group and the person's Telegram notices follow
  from that flag. Telegram has no buttons today: a reply to a notice is typed into the task
  (`telegram_reply`). The Superagent's cross-sandbox requests wait up to 240 s for an answer and
  then answer `waiting`, because MCP clients on Node's HTTP client want headers within 300 s
  ([ADR-0227](../../docs/decisions/0227-the-superagent-starts-tasks-in-other-sandboxes-only-on-its-persons-approval.md)).
- **Related work.** Task #611 adds method and path rules to the egress proxy. It touches the
  proxy, which this feature does not use.

## Requirements

**Definitions**

- **BRK-001** A project's command is a row of that project (sandbox). Its fields:
  - `name`: 1 to 40 of `a-z`, `0-9` and `-`, starting with a letter or digit, unique in the
    project. There are no global commands.
  - `description`.
  - `binary`: an absolute, normalised path (no `.`, `..` or empty component), at most 512 bytes.
  - `secret`: the NAME of an agent secret.
  - `tasks`: task numbers that may run it; empty means every task of the project.
  - `approve`: `always`, `send` or `never`.
  - `send_flags`: the flags that mark a run that changes something.
  - `approval_minutes`: 1 to 60, default 10.
  - `timeout_seconds`: 1 to 1800, default 600.
  - `fixed`: arguments placed before the agent's.
  - `subcommands`: the allowed first non-flag arguments; empty means any.
  - `refuse`: flags Yulius sets itself and refuses from the agent.
  - `slashes`: whether arguments like `owner/repo` pass.
  - `env`: variable name to value.

  Each list is bounded (64 entries, 4096 bytes each). Env names match `[A-Za-z_][A-Za-z0-9_]*`
  and may not be `PATH`, `HOME`, `LD_*` or `YULIUS_*`. Placeholders are `{keyfile}` and
  `{home}` in `fixed` and `env`, and `{secret}` (the value itself) in `env` only, since
  arguments show in a process list. Any other `{…}` is refused when the definition is saved.
- **BRK-002** A person who administers the project (`Action::Administer`) creates, changes and
  removes its commands from a browser session or an API token with the project's scope. Each
  change is audited `command.define`, `command.update` or `command.remove`, with the definition
  but no value. An open page reloads on `commands.updated`. People who may view the project
  read its commands.
- **BRK-003** A command's secret is resolved like a run that holds nobody: the project's secret
  of that NAME, else a global one switched on for the project. A personal secret is never used,
  since a definition is the administrators' and a personal secret only its owner's. A production
  secret (`cd_prod`) is refused at definition and at run, so a run never gets one (ADR-0324). A
  missing secret does not block a definition; the run is refused "not set up".

**Agents' tools**

- **BRK-004** `commands_list` serves a task's agent and the Superagent. It lists the project's
  commands: name, description, approval mode and minutes, subcommands, refused flags, slashes,
  and whether a task may run each, with fixed arguments and env shown as written (placeholders
  unexpanded). No value is shown.
- **BRK-005** `command_run {name, args, changes?}` serves task agents only. Another project's
  command name is "no command NAME in this project", the same as an unknown one. A task outside
  `tasks` is refused. Every refusal is audited `command.refused` with its reason and the
  redacted arguments.
- **BRK-006** Argument checks, in order, each refusing with its reason:
  - at most 64 arguments of at most 4096 bytes each, and no control character;
  - an argument containing `/`, or starting with `~`, is a path. With `slashes`, only these are
    paths: an argument starting with `/` or `~`, or containing `..`, `=/`, `=~`, `@/` or `@~`;
  - a refused flag in its `--flag`, `--flag=…`, or (for a two-character `-f`) `-f…` form;
  - with `subcommands`, the first argument that does not start with `-` must be one of them.
    A flag's value read as a subcommand is refused, not let through.
- **BRK-007** `yulius run NAME [ARGS…]` in the sandbox calls `command_run`, and `command_result`
  while the run waits or runs. It prints stdout to stdout and stderr to stderr, and exits with
  the command's code. It exits 125 for a refusal, a rejection or an expired approval, with the
  reason on stderr, and 2 for a usage error. It goes through the task token as `yulius tool`
  does.

**Running**

- **BRK-008** Where it runs. On a contained server the run joins the project's production holder
  on the host's read-only system: no agent run joins that holder, its network is its own, and
  its files are under `<data>/cd/<sid>/runs/cmd-<run>`, which agent runs cannot see. It gets no
  Docker directory. Another project's run joins that project's own holder: the namespaces
  separate them as they separate two sandboxes. On a server without sandbox isolation it runs
  in the previous containment for production runs (bubblewrap over its own directory). An
  uncontained development server runs it as its child. The binary is found in the host's
  system, never in the sandbox's layer, so an agent cannot replace it.
- **BRK-009** Each run gets a fresh, empty home (`work/home`, the working directory), a temp
  directory and the key written to `work/key/<random>/key` (mode 0600). The key is not in the
  working directory, so an argument without `/` cannot name it. The environment is only `HOME`,
  `PATH` (the host's system paths), `LANG`, `TMPDIR`, `TERM=dumb` and the definition's `env`.
  Nothing of the server's or the agent's environment is passed. `{secret}` reaches only that
  env. The run's files, the key with them, are removed when it ends, whatever the outcome. A
  server that starts removes those of a run it did not see end, and records the run `lost`.
- **BRK-010** Bounds. The run is killed at its `timeout_seconds`, or once its files pass 16 MiB.
  Its stdout and stderr go to files and come back to the agent as the first 96 KiB of each,
  with "[… N bytes more]". Stdin is empty.
- **BRK-011** What comes back is `{state, exit_code, stdout, stderr, reason?}`. stdout, stderr
  and the stored command line are redacted with `redact_for_sandbox`, so every agent secret the
  project can see shows as `[secret:NAME]`, the command's own key and each of its lines too. A
  redaction that cannot load withholds the text. Only the redacted output is stored, in the
  run's row, and only as long as the 200 newest runs of the project are kept.
- **BRK-012** Every run is audited `command.run`: task, command, the redacted command line,
  outcome and exit code, duration, and who approved it. No value is in any audit entry.

**Approval**

- **BRK-013** `never` runs at once. `always` asks for every run. `send` asks when the agent says
  `changes: true` or an argument is one of `send_flags` (or `flag=…`). The flag is what the CLI
  acts on, so an agent that says nothing still meets the approval.
- **BRK-014** A run that needs approval is stored `waiting` with its deadline (now plus
  `approval_minutes`). The task is flagged Needs you with the full command line (binary, fixed
  arguments with placeholders unexpanded, then the agent's arguments), the minutes, and how to
  answer. The flag reaches the sidebar's Needs you group and the person's Telegram notices as
  every flag does. `command_run` waits up to 240 s for the answer and the run, then answers
  `waiting` or `running` with the run's id. `command_result {id}` waits again, up to 240 s. Only
  the task that started a run reads it.
- **BRK-015** People who manage the project's secrets (`Action::Configure`) answer, from a browser
  session (an API token is refused), with Approve or Reject on the Secrets page's Commands
  section, or by replying `approve` or `reject` to the task's Needs you notice in their linked
  Telegram chat. The Telegram reply acts only when the task has one waiting run and the person
  may answer it; otherwise it goes to the task as a reply does today. An answer is one
  conditional write from `waiting` that holds only before the deadline. Two answers never both
  act, and an answer after the deadline is refused. Each answer is audited `command.approve` or
  `command.reject`, and the flag ends with the last waiting run.
- **BRK-016** No answer by the deadline is a no. The run is `expired` and never runs, also across
  a restart: the condition is in the write, and the background tick records expired runs and
  ends their flag.

## Plan

- [ ] Write and validate the spec and ADR-0459
- [ ] Store command definitions and runs (migration 0089)
- [ ] Administrators define, change and remove a project's commands, audited
- [ ] Agents and the Superagent list a project's commands with commands_list
- [ ] Independent review of the definitions slice, findings fixed test-first
- [ ] Refuse paths, refused flags and other subcommands in a run's arguments
- [ ] Run a command in the production holder with a temporary key file and a fresh home
- [ ] Bound a run's time and output and blank every stored secret from it
- [ ] command_run and command_result for task agents, audited, and the yulius run CLI
- [ ] Independent review of the run slice, findings fixed test-first
- [ ] Ask a person to approve a run in Needs you with the full command line
- [ ] Approve or reject in the browser or by a Telegram reply; no answer in time is a no
- [ ] Independent review of the approval slice, findings fixed test-first
- [ ] Commands section on the Secrets page with approvals, end-to-end tested
- [ ] Product, architecture, API, operations and security docs and the changes log
- [ ] Independent security-minded review of the page and docs slice, findings fixed

## Acceptance criteria

- **AC-1** An administrator creates, changes and removes a command. A writer is refused. A
  production secret, an unknown placeholder, a relative binary or an env name like `LD_PRELOAD`
  is refused. Each change is audited without a value.
- **AC-2** `commands_list` shows a project's commands to its agents and its Superagent, and none
  of another project's.
- **AC-3** A run whose binary prints its key file and its env `{secret}` returns
  `[secret:NAME]` in their place. The value is not in the run's row, the audit or the agent's
  answer. No file under the sandbox's directory holds it during or after the run, and the run's
  key file is gone after it ends.
- **AC-4** `/etc/passwd`, `~/.ssh/id_rsa`, `a/b` (without `slashes`), `--key-file x`,
  `--key-file=x`, `-kx` (refused `-k`) and a subcommand outside the list are refused, audited,
  and run nothing.
- **AC-5** A task of another project, or a task outside `tasks`, is refused with nothing run.
- **AC-6** An `always` run waits, flags the task with the full command line, and runs only after
  an Approve. A Reject runs nothing. With no answer it ends `expired` at its deadline and runs
  nothing, and a later Approve is refused. `send` asks only with `changes: true` or a send flag.
- **AC-7** A run past its time limit is killed and says so. Output past 96 KiB per stream comes
  back cut and says how much was left out.
- **AC-8** The Secrets page's Commands section lists, adds, edits and removes commands for
  administrators, shows waiting runs with Approve and Reject to those who may answer, and
  reloads on `commands.updated`.

## Test plan

- Domain unit tests: definition validation (BRK-001), argument checks (BRK-006), approval need
  (BRK-013), placeholder expansion and the command line shown.
- Integration tests in `crates/yulius/tests/brokered_commands.rs` with the real detached process
  host:
  - a stand-in CLI (a shell script) prints its key file, its env and its home;
  - the sandbox directory is scanned for the key during and after the run;
  - refusals, other projects, time and output bounds, approval with a short deadline (the
    configuration shortens the wait as `cross_request_wait_ms` does), rejection, expiry, a late
    answer and the Telegram reply.
- A contained-run test with a recording process host checks that the plan joins the production
  holder on the host system without Docker, and that the environment holds nothing else.
- e2e: `e2e/tests/commands.e2e.ts` against the mock for the section, its forms and the
  approvals.
- Mutation checks by hand on the path check, the deadline condition and the redaction call.

## Decisions

- **Run in the project's production holder, not the server's own namespaces and not a new
  holder.** It already exists per project and keeps agents out: no agent joins it, the data
  directory hides its files, and it runs on the host's read-only system, so neither the binary
  nor its libraries come from a layer agents write. A third holder per project would repeat
  ADR-0208's machinery for no extra separation from agents. Production CD runs share its
  network, which the run's own pid and mount namespaces and its bounded, admin-defined command
  keep apart. Recorded in [ADR-0459](../../docs/decisions/0459-brokered-commands-run-in-the-production-holder-with-a-temporary-key-file.md).
- **No `{url:SERVICE}` through the egress proxy.** The production holder has no route to the
  server's loopback, where the proxy listens. A key carried in a URL or header already works for
  agents through the proxy's placeholders. A fixed service URL goes literally into `fixed` or
  `env`.
- **Project and global secrets only, never personal or production.** See BRK-003.
- **Blocking with a bounded wait.** The call waits up to 240 s (ADR-0227's bound for MCP
  clients). The run itself goes on whatever happens to the call, and `command_result` and
  `yulius run` read it later.
- **Telegram by reply, not buttons.** The Telegram adapter reads only messages, and a reply to a
  notice is the channel's existing way of acting on a task. Inline buttons would need
  `callback_query` polling for one feature.
- **Approvers manage the project's secrets.** An approval lets a key be used, so it belongs to
  the people who already manage that key (Configure), from a browser session or their linked
  Telegram. No fresh two-factor code is asked for each run: the run does not widen where the
  key goes, the definition does.
- **Four pull requests**, each leaving the project working: definitions, running, approval, and
  then the page and docs.
