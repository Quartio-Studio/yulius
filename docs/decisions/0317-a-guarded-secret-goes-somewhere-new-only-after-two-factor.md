# 0317. A secret allowed use by use goes somewhere new only after two-factor; redaction that cannot load withholds

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 5); task #465. Applies [0042](0042-personal-two-factor-guards-the-vault.md) (two-factor
guards the vault) to the ways a value leaves other than the screen, as
[0284](0284-agents-ask-for-a-copy-of-a-production-secret-and-a-person-allows-it.md) did for
copies; narrows [0188](0188-remote-control-uses-scoped-personal-api-tokens.md) and
[0098](0098-agents-propose-secret-hosts-people-allow-them.md) (who allows a host).

## Context

The audit of task #465 found three places where the vault's stated rules held on one path and
not on another:

- **Reading a value needs a two-factor code; sending it anywhere did not.** ADR-0042 put a
  personal code before every reveal, because "whoever held a session cookie read every
  secret". An agent secret is also read by sending it to a server of one's choice, or by
  letting agents copy it into a `.env` file. Binding a new host (`PATCH`, the answer to an
  agent's host request, the remote MCP's `secret_update` and `secret_hosts_answer`) and
  allowing `.env` copies needed write access only, by a session or by an API token. A stolen
  session cookie or a leaked `operate` token therefore read every agent secret of its person's
  projects in two requests, while the reveal next to it asked for a code.
- **Redaction that cannot load the secrets.** "Never shown unredacted" (SPEC-014 VLT-007) held
  for a terminal's first screen and for text redacted in one piece, which are withheld when
  the store or a decryption fails. The live terminal stream passed its bytes through and
  remembered the failed load as current until the next change of a secret; the egress proxy
  redacted with an empty set, which also switched off its refusal of compressed answers.
- **An edit undid a rotation made beside it.** `vault_update` compared the row's metadata with
  what the edit had read, not the sealed value. An edit of a description read the old value,
  another person saved a new one, and the edit sealed the old one again: the rotation was
  gone, reported as saved to both.

Not every agent secret is guarded from the people and agents of its project. One whose creator
left "Let agents set it up" on (`auto_allow`, the default, SPEC-216) is bound to hosts by any
agent of the project, without a person. A code before a person binds a host of such a secret
would guard nothing.

## Decision

- **A secret allowed use by use goes somewhere new only as a reveal.** For an agent secret
  with `auto_allow` off (production secrets and copies of them among them), four changes need
  a browser session with a two-factor code entered in the last 10 minutes
  (`App::fresh_to_widen`): binding a host it is not bound to, allowing `.env` copies, allowing
  it for production deploys (they run the repository's command with the value) and switching
  `auto_allow` on. Where no code is asked because agents set the secret up, the write holds
  only while `auto_allow` still is what was read (`vault_answer_request`,
  `vault_set_env_files`). Without the code the answer is 403 `reauth_required` with the
  person's status, as for a reveal, and nothing changes; the page then asks for the code and
  repeats the request. An API token is refused (403 `forbidden`) with where the person does
  it. What sends the value nowhere new needs neither: a description, a name, fewer hosts, a new
  value, a dismissal, a revocation.
- **A secret agents set up is unchanged.** Its hosts are bound by agents and by its people's
  tokens as before (ADR-0194's unattended onboarding imports such secrets).
- **Redaction that cannot load withholds.** The terminal stream shows one notice, drops its
  output and asks again after a second or when a secret changes, until the secrets can be
  read (`TerminalScrub::needs_load`, `loaded`); the failure is logged once, not per chunk. The proxy reads the values to redact before it sends a request;
  when it cannot, it answers 503 and sends nothing (`EgressPolicy::egress_redactor` returns
  `None`).
- **An edit is refused when the value changed under it.** `vault_update` also compares the
  sealed value the edit read; the edit answers 409 (an agent secret's names the value, a team
  secret's says someone saved it meanwhile).

## Alternatives

- **The code for every agent secret.** Simpler to say, and it would ask people for a code
  where any agent of the project binds the same host without them. A rule that guards nothing
  teaches people to type codes without reading. Rejected.
- **Tokens keep answering host requests.** The remote MCP's `secret_hosts_answer` was made for
  it. But a host request exists only for a secret a person chose to allow use by use, the
  hosts are whatever an agent proposed, and a token is a bearer credential on a laptop. The
  same reasoning made ADR-0284 ask for the code before a copy. A token still lists, describes
  and dismisses, and binds hosts of secrets agents set up. Rejected.
- **The code before dropping a secret's production status.** After it the secret is still
  allowed use by use with no host, so every way on from there asks for the code. Not needed.
- **A new error code for "needs your code".** The page already unlocks and retries on
  `reauth_required` (`withReauth`); one code keeps one dialog. Rejected.
- **The terminal keeps redacting with the previous secrets** when the new ones cannot be read.
  A secret added a moment ago is exactly what the next output may print. Rejected.
- **The proxy answers 502 after the upstream answered.** The request, with the secret in it,
  would already have had its effect and the agent could not tell. Refusing first sends nothing.
- **A version column for edits.** The sealed value is already unique per save (a fresh nonce):
  comparing it needs no migration.

## Consequences

- A person who allows a host, `.env` copies or production deploys for a secret they allow use
  by use, or lets agents set it up, is asked for their two-factor code unless they entered one in the last 10
  minutes; someone without two-factor sets it up there.
- An agent or script with an API token cannot do those four things for such a secret; it is
  told to use the Secrets page. `secret_hosts_answer {allow: true}` therefore works only for a
  request left on a secret that agents set up since.
- While the secrets cannot be read, a task's terminal shows "[Yulius: the screen is withheld
  until agent secrets can be read]" once and no output, and requests through the proxy get 503.
  A store that answers again ends it by itself. A secret that no longer opens (its key is
  gone) does not: the terminals of its sandbox stay withheld until someone replaces or deletes
  that secret (the server's log names the error), and no run starts there meanwhile (a start
  reads the secrets).
- A withheld screen is not what the program drew, so Yulius reads nothing off it and types
  nothing into it: a run waiting for its first message or its goal keeps waiting
  (`wait_input_ready`), and after the time a trust dialog gets (5 minutes) the task is flagged
  with what to do. Before, a wait that timed out typed blind, which a withheld folder-trust
  dialog would have taken as its answer.
- Two people editing one secret: the second save is refused and reloads, whatever each changed.
- Not covered here: switching a global secret on for a project stays its administrators'
  switch, and a personal one its owner's (the project's agents then use it as the secret
  already allows, which whoever switches could do in a project it is on for); and "may this production secret
  reach this run" still has one older exception (finding V6 of SPEC-483, a later slice).
