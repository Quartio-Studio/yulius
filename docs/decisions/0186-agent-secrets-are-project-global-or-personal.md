# 0186. Agent secrets are a project's, global or personal

## Status

Accepted. Specified in [SPEC-245](../../.ai/specs/SPEC-245-secrets-scopes-project-global-personal.md);
task #231. Amends [0171](0171-agents-set-up-new-agent-secrets-and-secrets-belong-to-a-sandbox.md)
(team-wide agent secrets no longer apply to every sandbox by themselves) and
[0116](0116-ci-jobs-use-named-agent-secrets-through-the-proxy.md) (what a granted name resolves to).

## Context

An early external user found the vault's scopes pointless and, in a fork, gave agent secrets three:
a project's, global (defined once, switched on per project) and personal (defined once by one
person, switched on per project, used only by that person's tasks). Until now an agent secret was
one sandbox's or "team-wide", which every run of every sandbox got, with no way to keep a
credential to some projects or to one person.

Constraints: every place that resolves a run's secrets (environment placeholders, the egress
proxy, CI grants, CD, MCP references, `.env` copies, browser filling, the MCP tools, redaction)
must agree; tasks of one sandbox share its file system and its browser; a task acts for its owner
(SPEC-070); an automation's task is owned by the automation's creator, but anyone who configures
the project can change the automation's prompt.

## Decision

- **Three scopes, one column each.** `sandbox_id` (project), `owner_id` (personal), neither
  (global). `vault_enabled (secret_id, sandbox_id)` switches a global or personal secret on for a
  project. Names are unique within a scope only.
- **Precedence project > personal > global, not an error.** A run resolves each name once: the
  project's secret, else its person's, else the global one, else the instance Vidoc key. The
  project's secrets are what the project's people agreed on, so a task behaves the same whoever
  started it unless the project leaves a name open; a person's own credential is more specific
  than the instance's default. Refusing a clash instead would let one person's private secret
  block a project's writers and tell them it exists. The losers are marked `shadowed_by` on the
  Secrets page, so the shadowing is never silent to the people who manage them.
- **Who manages what.** Project: the project's writers (as before). Global: instance
  administrators create, edit, reveal and switch them per project; which code a shared credential
  reaches is their decision, as binding a host is its manager's. Personal: the owner only, for
  projects they may work in; to everyone else, administrators included, it does not exist.
- **Who gets personal secrets.** A task's run gets its owner's personal secrets switched on for its
  project; the Superagent its person's. A task an automation started gets none, even though its
  owner is the automation's creator: other people may edit the automation's prompt, so they could
  direct an agent holding someone else's credential; the tasks such a task creates are automation
  tasks too. CI jobs and CD runs belong to no person and get none; their names resolve among the
  project's and the switched-on global secrets. For the same reason only its owner messages a task
  holding personal secrets (`task_message`), and a hand-over drops the run's cached proxy
  credential, so the new owner's secrets apply from the next request.
- **A personal value stays in the proxy.** `.env` copies, `${secret:NAME}` MCP references and
  `browser_fill_secret` refuse personal secrets: the first two write the value where every task of
  the sandbox can read it, and the third signs the shared project browser in as the person. The
  proxy credential is per run, so injection is the one channel that reaches the owner's run alone.
  Production deploys never get a personal secret.
- **Redaction covers everyone's; fingerprints only one's own.** Yulius' own redaction of a project
  (transcripts, terminals, MCP results, logs, proxy responses) covers every secret that can reach
  any of its runs. The fingerprint file a run receives covers only its own set, so no run holds
  hashes of another person's value to guess against offline.
- **Upgrade without loss.** Schema 38 turns each team-wide agent secret into a global one switched
  on for every live sandbox; project secrets are unchanged. A global secret added later is on only
  for the project it was added from.

## Alternatives

- **An error on a name clash.** Clear at configuration time between project and global secrets,
  but a personal clash would have to be refused to a project writer who cannot see it, or at run
  time. Rejected for one rule everywhere.
- **Personal over project.** Lets a person replace the project's credential for their own tasks;
  rejected because a project's tasks should not change behaviour with who started them unless the
  project leaves the name open. The person names their secret differently instead.
- **Project writers switch global secrets on.** Would let any writer of any project use every
  shared credential; the previous behaviour (every team-wide secret everywhere) did that and is
  what the feedback found pointless.
- **Automations' tasks get their creator's personal secrets.** Rejected for the prompt-editing
  reason above; an automation needing a credential uses a project or global secret.
- **Copy team-wide secrets into every project (as ADR-0171 did for team secrets).** Copies would let
  each project's writers reveal and rebind a credential an administrator holds; a global secret
  switched on everywhere keeps it the administrator's.
- **Personal secrets in `.env`, MCP and browser too.** Needs per-person file systems and browsers,
  which sandboxes do not have.

## Consequences

- A person's personal secret can be shadowed by a project secret of the same name; they see that on
  their Personal tab and rename theirs.
- Redaction is an oracle for weak values (ADR-0014): another person's agent printing a guess of a
  personal value in the project sees `[secret:NAME]` in Yulius' views. The 8-character minimum
  and the per-run fingerprints keep this the known online oracle.
- A new project gets no global secret until an administrator switches it on.
- Personal secrets cannot sign agents into websites; a per-person browser would be its own change.
- An agent's `secret_create_from_file` cannot see other people's personal secrets, so a project
  secret it adds may shadow one of theirs; the owner's card says so. Refusing would tell the agent
  that the secret exists.
- After a hand-over the run keeps its environment: the placeholders of the previous owner's
  personal secrets stay as literal text (the proxy no longer replaces them), and its fingerprint
  file is the one it started with (the Bash wrapper asks Yulius for current fingerprints first).
