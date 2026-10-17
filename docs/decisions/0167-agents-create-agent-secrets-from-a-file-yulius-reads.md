# 0167. Agents create agent secrets from a file that Yulius reads

## Status

Accepted. Specified in [SPEC-211](../../.ai/specs/SPEC-211-yulius-should-be-able-to-create-secrets.md);
task #198. Extends [ADR-0144](0144-the-superagent-reads-the-agent-vault-and-verifies-values-by-hash.md)
and [ADR-0150](0150-the-superagent-uses-agent-secrets-as-a-task-does.md) (the agent vault tools)
and amends SPEC-037 MGT-006 again (one more Superagent tool).

## Context

A person asked their Superagent to put an SSH private key that sat in a file on the VM
(`~/.ssh/quartio-v2-operator`) into the agent vault. It could not: only people create agent
secrets (the Secrets page, `yulius secrets import`), and an agent that printed the file to copy it
would put the key in its transcript and on screen. So it asked the person to SSH in, `cat` the file
in their own terminal and paste it into the Secrets page, then checked the copy with
`secret_verify`. The person asked "can you not add the key by yourself?" and asked for Yulius to
"create secrets but without printing them".

Agents already use secrets without seeing them: the proxy adds them, `secret_copy_to_env` writes
them, `secret_verify` compares hashes. In each case Yulius, not the model, touches the value.
Yulius runs outside the agent's containment with more rights than the agent (it reads its own
vault key), so anything it reads on an agent's behalf must stay where the agent could read anyway.

## Decision

- **`secret_create_from_file {name, file, description?, delete_file?}`**, served to task agents
  and the Superagent. Yulius reads the file and stores its text as an agent secret of the
  caller's sandbox. The value never goes through the tool call, the result or the transcript.
- **Only the work folder and the home.** `file` is relative to the folder the caller works in (a
  task's worktree, the Superagent's sandbox repository) or `~/…` in its home (a task's: the sandbox
  home; the Superagent's: its person's home, shared by their projects). Absolute paths, `..` and
  `.git` are refused. The walk starts at a folder the agent cannot replace (the canonical sandbox
  folder; for the Superagent's home, its person's folder `users/<id>`), never at the home or
  repository path, which the agent can swap for a link (a task's worktree is first resolved to a
  real folder inside the sandbox folder, which the agent reads anyway). From there the folders on
  the way, the home or repository included, are opened one at a time without following links, as
  `secret_copy_to_env` opens its target (`open_folder`), and the folder reached must lie inside its
  root. The file is opened without following a link and without blocking, and must be a regular
  file of at most 16 KiB in UTF-8.
- **The file's text, as the page stores it.** One final line break is dropped. A multi-line text
  is stored only if it is a key block, with literal `\n` (ADR-0156) through
  `dotenv::agent_value_from_file`. The CLI import now uses the same function. Then
  `check_agent_value` applies.
- **Creating grants nothing.** The secret applies to this sandbox only, with no hosts, no `.env`
  allowance and not for production. Every use still needs a person (`secret_request_hosts`,
  `secret_copy_to_env`). A name that already applies (sandbox or instance-wide) is refused:
  nothing is replaced, and rotation stays on the Secrets page. `VIDOC_API_KEY` is refused too: a
  project secret of that name replaces the instance's Vidoc key for the whole sandbox
  (ADR-0157), which only a person decides.
- **The person's right, the agent's name.** The person the caller acts for (a task's owner, the
  Superagent's person) must be able to manage the sandbox's agent secrets (write access); they
  are recorded as its creator. The audit entry `vault.agent.create` names the agent as actor
  (`task:<n>`, `cockpit:<login>`), and its detail says "from a file, by #n" or "by `<login>`'s
  Superagent".
- **Deleting only on request.** With `delete_file: true`, Yulius removes the file through the
  folder handle it read it from, once the secret is stored. A failed removal keeps the secret and
  says so.
- **Not among the tools run without asking.** The tool is not in `COCKPIT_READ_ONLY_TOOLS`, so
  on an uncontained server Claude Code asks the person before each call. On a contained server
  the Superagent bypasses approvals (ADR-0165), as tasks do; there the safeguards are the ones
  above (a person still allows every use). Its instructions and `secrets_list` say to use it
  instead of asking a person to paste a key the agent can reach. Running Superagents get it
  through the launch digest (ADR-0150).

## Alternatives

- **A `value` argument.** The value would be in the tool call, the transcript and the screen,
  which is what the person wanted to avoid. Rejected.
- **Any path the agent names.** Yulius would read its own files (the vault key, other sandboxes)
  into a secret the agent then asks to use. Rejected; an agent copies a file from elsewhere into its
  folder first, without printing it.
- **Superagent only.** A task that generates a deploy key is in the same situation, and creating
  reveals and grants nothing. Rejected.
- **Generate the value in Yulius** (`generate: bytes`). That covers new random tokens but not keys
  a tool makes (`ssh-keygen`) or keys already on disk. A file covers both: `openssl rand -hex 32 >
  file` writes a token without printing it. Not now.
- **Replace an existing secret** (rotation). A silent replacement can break every agent using the
  key. Rejected, as for the CLI import (ADR-0163).
- **Delete the file by default.** A person's `~/.ssh` key is also used by SSH itself. Rejected.

## Consequences

- An agent with a key in a file stores it in one call. The person only allows the uses, on the
  Secrets page, as for any secret.
- An agent can fill its sandbox's agent vault up to its limit (200) with secrets nobody can use
  until a person allows them. The audit names the task that added each one.
- A secret holding something the agent could read anyway reveals nothing new. Yulius never reads
  outside the caller's own folder or home, and never follows a link to get there.
- On a contained run the home Yulius reads is the one the run sees: the sandbox home for tasks,
  the person's home for the Superagent.
