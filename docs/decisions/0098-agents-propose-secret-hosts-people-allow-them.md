# 0098. Agents propose an agent secret's hosts; people allow them

## Status

Accepted. Specified in [SPEC-121](../../.ai/specs/SPEC-121-too-many-options-it-should-be-key-name-l.md).

## Context

Adding an agent secret asked for six things: name, value, notes, scope, hosts and how it is added
to requests. The person asked: "Too many options. It should be: key name (like VIDOC_API_KEY),
value and description. Agent will find by itself the type, the adres etc". The request style is
only a convenience: the proxy already replaces a `yulius-secret-NAME` placeholder in any header,
Basic credential, path or query (SPEC-002 ADR-20). The hosts are not a convenience. They are the
boundary that keeps a secret from leaving for a server an agent was tricked into calling.

## Decision

A new agent secret is a name, a value and a description; it starts with no hosts and no
injection, so the proxy and `browser_fill_secret` use it nowhere. The agent finds the service's
host from the description and the service's documentation and proposes it with the MCP tool
`secret_request_hosts`. The tool stores a **request** on the secret (migration 0025:
`requested_hosts`, `requested_by`), audits it (`vault.agent.request`) and flags the task as
needing a person. It never binds anything. On the Secrets page, a person who manages the secret
sees "Task #N asks to send it to …" and presses Allow (the hosts join the bindings,
`vault.agent.allow`) or Dismiss (`vault.agent.dismiss`). Requests are compare-and-set, so an
answer never drops a newer request unseen. People can still edit the hosts directly. Instance
administrators pick the scope under "More options". Older secrets keep their injection, and the
edit dialog shows it only for them.

## Alternatives

- The agent binds hosts itself: a prompt-injected agent could bind a secret to its attacker's
  host, and the proxy would deliver it. That defeats "use, never see".
- Trust on first use (an agent's first binding sticks): the same hole, at the most likely moment
  of misuse, a fresh secret.
- Guess the hosts on the server from the name: `VIDOC_API_KEY` says nothing reliable about the
  API's address.
- Keep the fields under "More options": the person would still have to know the address, which
  the request says they should not need to.

## Consequences

- One click per new secret (or new host), made by a person who knows what the secret is for;
  the agent does the research.
- A secret is unusable between its creation and the Allow. The agent goes on with other work,
  and `secrets_list` tells it which hosts are bound and which still wait.
- The `POST /api/sandboxes/{sid}/agent-secrets` API's default injection changes from `bearer`
  to `none`, and `hosts` becomes optional. API callers that relied on the bearer default must
  send `injection: "bearer"`.
