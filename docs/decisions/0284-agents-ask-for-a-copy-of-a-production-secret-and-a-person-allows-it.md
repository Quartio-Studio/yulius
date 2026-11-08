# 0284. Agents ask for a copy of a production secret and a person allows it

## Status

Accepted. Specified in [SPEC-440](../../.ai/specs/SPEC-440-agent-should-be-able-to-request-copying.md);
task #422. Amends [ADR-0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md) (production
secrets stay exclusive; a separate copy can now be asked for) and extends
[ADR-0098](0098-agents-propose-secret-hosts-people-allow-them.md) (agents ask, people allow) and
[ADR-0167](0167-agents-create-agent-secrets-from-a-file-yulius-reads.md) (agent-made secrets).

## Context

A production secret (`cd_prod`) is never used by agents: `secret_request_hosts`,
`secret_copy_to_env` and `browser_fill_secret` refuse it with "add a separate secret for agents".
A task that needed a dev CI token which existed only as a production secret raised `needs_you`:
"Add agent secret N3_DEV_CI_TOKEN_AGENT (same value as N3_DEV_CI_TOKEN), bound to host
165-245-200-246.sslip.io". The person then had to reveal the production value with their
two-factor code, add a secret, paste it and bind the host. The person asked that the agent be able
to request that copy itself.

## Decision

- **`secret_request_copy {secret, name, hosts, reason?}`**, for task agents and the Superagent,
  stores a request (table `vault_copy_requests`, one per production secret and project) for a new
  project secret `name` of the production secret's value, bound to `hosts`. It stores no value and
  binds nothing. Only production secrets are copied: for any other secret the tool points to
  `secret_request_hosts` and `secret_copy_to_env`. A name that already applies to the project and
  `VIDOC_API_KEY` are refused. The same copy asked again adds the asker; another name or other
  hosts replace the request. The task is flagged as needing a person until it is answered, as for
  a host request.
- **A person allows or dismisses it** on the production secret's card
  (`POST /api/sandboxes/{sid}/agent-secrets/{id}/copy {allow, name, hosts}`, answered only as seen;
  a request that changes while shown holds Allow copy off for a moment, as a host request does).
  Either answer needs manage rights on the production secret and write access to the project.
  Allowing also needs the secret still to be a production secret, and a fresh two-factor unlock, as a reveal does, and API tokens cannot allow. Then the copy is
  created from the stored value in the transaction that removes the request. It keeps the source's
  injection, user name and two-factor seed, is bound to the hosts asked for, is not set up by agents (`auto_allow`
  off), has no `.env` allowance, and its description says whose copy it is. Audited
  `vault.agent.copy_request`, `vault.agent.copy_allow` / `vault.agent.copy_dismiss` and
  `vault.agent.create`.
- **The production secret is unchanged.** It keeps no hosts and no `.env` allowance; the copy is
  an ordinary agent secret that does not follow the source's rotations. Revoking its production
  allowance removes its copy requests in the same transaction, and the asking tasks work on.

## Alternatives

- **Agents copy it themselves** (as `secret_create_from_file`): would make a production value
  agent-usable with no person deciding, undoing ADR-0109's exclusivity.
- **Let agents use production secrets for dev hosts**: the same, with no separate secret to revoke.
- **A copy that follows rotations of its source**: a link between secrets that nothing else has;
  rotating the copy stays a person's, as for any agent secret.
- **A column on `vault_secrets`**: one request per secret, while a global production secret can be
  asked for by several projects.
- **Allow without two-factor**: cheaper, but it makes a production value usable outside production
  with less proof than reading it.

## Consequences

- Agents stop raising `needs_you` for a dev copy of a production token; the person answers on the
  Secrets page in one step and never handles the value.
- One more MCP tool for tasks and the Superagent (the Superagent's tool list grows to 35).
- The copy and the production secret drift apart when either is rotated; the copy's description
  names its source so a person knows to rotate both.
- Deleting the production secret or the project removes its requests (foreign keys cascade).
