# 0324. A production secret reaches no run and has no hosts

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 12, finding V6); task #465. Finishes what SPEC-134 began (a secret allowed for
production deploys is exclusive) and SPEC-308 and SPEC-405 worked around.

## Context

An agent secret allowed for production deploys (`cd_prod`) is for those deploys only: agents
never get it. Since SPEC-134 such a secret cannot be bound to a host, and every change of a
row keeps the two apart. A row that was made a production secret before that kept its hosts,
and "may this production secret reach this run" was then answered four ways:

- the egress proxy sent it to those hosts for an agent's run (`EgressClient::may_use`: `true`
  for an agent), and refused it for a Superagent and for a CI job;
- typing it into a page, copying it into an `.env` file and a `${secret:NAME}` reference
  refused it for everyone;
- the list an agent reads of its secrets said "agents never use it, not through hosts";
- the Edit dialog showed such hosts with the sentence "agents' requests to them still get
  it" and one choice, to remove them.

The store's insert of a whole row (a new team or agent secret) did not check the pair at all.

## Decision

- **One rule: a production secret applies to no client of the egress proxy.**
  `EgressClient::may_use` answers `false` for it, whoever asks; of the other secrets, a CI job
  has those it was granted and an agent run and a Superagent have all of their sandbox's.
- **No row says otherwise.** Migration 0055 removes the hosts of every production secret that
  kept some and writes one audit line per secret (`vault.agent.hosts_cleared`, actor `yulius`,
  with the hosts it had). The store refuses to insert a production secret with hosts; the
  changes of an existing row already refused it.
- **What described the old state is removed:** the Edit dialog's "Remove these hosts" for a
  production secret, the web mock's and the test helper's old rows.

## Alternatives

- **Keep the hosts and only change the rule.** The hosts would say something the server
  never does, and a production allowance revoked later would bring them back to life unseen.
- **Leave the rule to the rows** (no row has hosts, so the proxy needs no rule). The proxy is
  where the value leaves the machine: it keeps its own answer, and a test holds it.
- **Ask each project's people before removing.** The hosts did nothing a person could want
  kept: every text said agents never get such a secret, and the person who wants an agent to
  send a value to a host adds a secret for agents, as the dialog says.

## Consequences

- An agent's request to a host that a production secret was still bound to goes out with the
  placeholder, as to any host no secret is bound to. A project that relied on that gets a
  failing request and finds the removal in its audit log.
- Schema version 55. The migration changes rows only where the pair exists.
- A production secret's Edit dialog always reads "None. This is a production secret: …".
