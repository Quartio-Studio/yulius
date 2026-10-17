# 0171. Agents set up new agent secrets; every secret belongs to a sandbox unless team-wide

## Status

Accepted. Specified in [SPEC-216](../../.ai/specs/SPEC-216-agent-s-secrets-for-default-should-allow.md);
task #203. Amends [0098](0098-agents-propose-secret-hosts-people-allow-them.md) ("an agent never
binds a host itself") and [0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md)
("production runs get only those a person allowed for production deploys") for secrets whose
creator lets agents set them up; leaves both as they are for the others.

## Context

The person asked that an agent secret, by default, let the agent bind the host it works out and
let production deploys have it, without a person confirming either, with an option to turn that
off while adding the secret. They also asked that team and agent secrets be split into a
sandbox's own and team-wide ones, a sandbox's own being the default, and that the secrets that
exist be assigned that way.

Until now a host an agent proposed waited for a person (SPEC-121), and production deploys got a
secret only after a person's production-only allowance, which excluded any agent use (ADR-0109).
Team secrets were all instance-wide; agent secrets were a sandbox's or every sandbox's.

## Decision

- **`auto_allow`, on for new agent secrets.** One option, "Let agents set it up", in the Add
  dialog (and Edit): `secret_request_hosts` binds the hosts at once (audited
  `vault.agent.auto_allow`), and production CD runs get the value (`allowed_for(prod)`). The API
  field defaults to `true`; secrets that existed keep it off.
- **The production-only allowance stays exclusive.** `cd_prod` and `auto_allow` exclude each
  other in the application and in the store's compare-and-set. A person who wants a credential for
  production deploys only turns the option off and allows production as before.
- **A scope for every secret.** `sandbox_id` (already a column) now scopes team secrets too: a
  sandbox's team secret is for the people who can view that sandbox (404 otherwise, its activity
  hidden), a team-wide one for every member. New ones default to the open sandbox; team-wide is
  under More options (agent secrets: instance administrators only, as before).
- **Existing secrets are copied into each sandbox once.** Schema version 35 lists the secrets with
  no sandbox (`vault_scope_pending`). At the next start, before serving, each is re-sealed under a
  new id per sandbox and replaced by those copies in one transaction (audited `vault.*.assign` by
  `yulius`). With no sandbox it stays team-wide.

## Alternatives

- **Two options (hosts, production).** More combinations than anyone asked for; the request names
  one option.
- **Turn the new default on for existing secrets.** Their people added them under the promise
  that a person confirms; a value that has left cannot be called back.
- **Drop the exclusivity of `cd_prod`.** Would make every production-only credential reachable by
  agents as soon as one host is allowed; keeping it costs one rule.
- **Assign each existing secret to one sandbox (the first, or where it was used).** Nothing records
  which sandbox a team secret is for, and use is audited per run, not per sandbox; any pick would
  take secrets away from sandboxes that had them.
- **Assign in SQL.** The payload is sealed with the row's id as associated data, so a copy must be
  re-sealed by the application.

## Consequences

- An agent misled by injected content can send a self-set-up secret to a host it chose. The
  person accepts that per secret; the binding is audited with the asking task and shown on the
  card, and the tool descriptions tell agents to ask only for the service's documented hosts.
- A secret that applied to every sandbox now exists once per sandbox: rotating it means each copy.
  People delete the copies they do not need.
- Activity entries of secrets removed by the assignment (and of deleted ones) are shown to
  instance administrators only.
