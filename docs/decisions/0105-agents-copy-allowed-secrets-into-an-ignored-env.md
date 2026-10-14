# 0105. Keys live in a git-ignored .env; agents copy a secret there only after a person allows it

## Status

Accepted. Specified in [SPEC-129](../../.ai/specs/SPEC-129-default-place-for-keys-should-be-env-tha.md).
Extends [ADR-0098](0098-agents-propose-secret-hosts-people-allow-them.md) and keeps the rule of
[ADR-0078](0078-repository-mcp-json-feeds-the-mcp-section.md).

## Context

The person asked for one default place for keys: a `.env` file Git ignores, next to a tracked
`.env.example`. When an agent starts using an agent secret in code that runs for real, it should
copy the secret into `.env`. Agents see only `[secret:NAME]`, so they cannot write the value
themselves: they would write the masked text. This prepares Yulius CD (task #113), whose deploy
scripts read keys from `.env`.

Agent secrets are built on "use, never see" and on host bindings. The egress proxy adds a value
only to requests for the hosts a person allowed (ADR-0098). ADR-0078 refused to let an
agent-written file give a value in clear to a command an agent chose. A value in `.env` is
exactly that: any code in the worktree can read it and send it anywhere.

## Decision

- **Default place.** A project's keys go in a `.env` that Git ignores (`.env`, `.env.*`,
  `!.env.example` in `.gitignore`), beside a committed `.env.example` that names each key with no
  value. The briefing, `secrets_list` and the `infra-bootstrap` skill say so. The Yulius repository
  already ignores them this way and keeps its own keys in its vault.
- **Mechanical copy.** The MCP tool `secret_copy_to_env {secrets, path?, reason?}` writes
  `NAME=<value>` lines into a dotenv file of the task's worktree (default `.env`) and adds `NAME=`
  to the `.env.example` beside it. Yulius writes the value; the agent never receives it.
- **A person allows each secret once.** A secret not yet allowed is asked for instead. The
  secret's `env_files` becomes `asked` (migration 0026), the request is audited
  (`vault.agent.env_request`), the task is flagged as needing a person, and nothing is written.
  The Secrets page shows "Task #N asks to copy its value into the project's git-ignored .env" with
  Allow and Dismiss, and Revoke once allowed (`POST /api/agent-secrets/{id}/env`, compare and set).
- **Git is the guard against accidents.** The copy is refused, and nothing is written, unless
  Git ignores the file and does not track it, so a later `git add .` does not commit a value. A
  temporary file is used only under a name Git also ignores, else the file is written in place,
  so a crash leaves nothing committable. This stops mistakes, not a determined agent, which could
  still edit `.gitignore`.
- **No link is followed.** Yulius runs outside the agent's containment, and the agent can rename
  any folder of its sandbox at any moment. The target's folder is opened once, one component at a
  time without following links, its real path must be inside the worktree and out of `.git`, and
  every read and write goes through that handle (`openat`, `renameat`). A path outside the
  worktree, a link, a file that is not a regular file and a template are refused.
- **Checks before asking.** Everything that would refuse the copy (the file, Git, the values'
  quoting) is checked before a person is asked, so an Allow is never wasted.
- **All or nothing, portable lines.** Lines for the names are replaced in place and every other
  line is kept, and so are Windows line endings. Values are written bare, in single quotes, or in
  double quotes. A value that common readers (Node `dotenv`, `python-dotenv`, Docker Compose, `sh`)
  would read differently is refused: one with `\` or `${` (unescaped or expanded even in single
  quotes), or with `'` and `"`, `$` or a backtick. The file is written whole with mode 0600, and each copy is audited once per run, secret
  and file (`vault.agent.env`).

## Alternatives

- **The agent copies freely.** This is what the request says literally. But a prompt-injected run
  could move any secret out of its bound hosts with one call, and the secret's managers would
  never be asked.
- **A person allows each copy.** This adds a click for every task and every file. It adds no
  safety once the value is on disk in the project.
- **A "may be written to files" option in the add dialog.** The person asked for fewer options
  (SPEC-121), and the need shows up later, when code starts to use the key.
- **Yulius keeps `.env` files in sync with rotations.** Yulius would rewrite files in worktrees
  behind the agent's back. Instead the agent copies again after a rotation.

## Consequences

- An Allow covers every task of the sandbox (of every sandbox for an instance-wide secret) until
  it is revoked; the Secrets page says so. Revoking stops new copies, and copies already written
  stay until someone removes them or rotates the value.
- Once allowed, a value on disk is outside the proxy's host binding. Redaction still hides it from
  the agent (the Bash wrapper and the `PostToolUse` hook read files back as `[secret:NAME]`). But
  code in the worktree can use it anywhere, which is why a person decides.
- Task #113 can reuse the same writer for its deploy checkout, and its scripts can rely on `.env`.
- A copy is a snapshot: rotating a value leaves old copies until an agent copies again.
