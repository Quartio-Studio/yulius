---
yulius-task: f9003039-e5b7-49f3-b6af-fa45d54d5498
status: in-progress
---

# SPEC-121: an agent secret is a name, a value and a description; agents find its hosts

Status: in-progress
Date: 2026-10-14
Owner: Yulius task #107
Scope authorization: the person's request (task #107) with a screenshot of the "Add agent secret"
dialog: "Too many options. It should be: key name (like VIDOC_API_KEY), value and description.
Agent will find by itself the type, the adres etc".

## Goal

Adding an agent secret asks for three things: its NAME, its value and a description. The person
no longer chooses how it is added to requests (the type) or the hosts it goes to (the address).
Agents find both: they send the secret through its placeholder in whatever header or query the
service wants, and they work out the service's API host from the description and its
documentation and ask for it. Because the host binding is the boundary that stops a secret from
leaving for an attacker's server, an agent **proposes** hosts and a person who manages the secret
**allows** them with one click on the Secrets page; nothing is sent anywhere before that.

## Context

- `crates/yulius-server/web/assets/pages/secrets.js`, `agentForm()`: Name, Value, Notes, Scope,
  Hosts, "Added to requests as" (+ Header name, User name). Saving refuses an empty Hosts field.
- `crates/yulius-app/src/vault/agents.rs`: `meta()` requires at least one host
  (`parse_hosts`) and defaults the injection to `bearer`.
- SPEC-002 ADR-20: the proxy replaces `yulius-secret-NAME` placeholders in header values (Basic
  credentials included), the path and the query, but only for a host NAME is bound to; an
  injection merely adds a header when the request lacks one. So the injection is a convenience:
  an agent that writes `Authorization: Bearer $NAME` (or `-u user:$NAME`, `X-Api-Key: $NAME`,
  `?key=$NAME`) needs none.
- Host bindings are the security boundary (§2.6 threats 1–2): a secret is used only on its
  hosts, so an agent misled by injected content cannot send it elsewhere. Letting an agent bind
  hosts by itself would remove that boundary.
- `browser_request_user_action` (browser.rs) flags the task `needs_attention` when an agent needs
  a person; the same pattern serves a host request.
- `secrets_list` (runtime.rs) tells agents names, descriptions, hosts and how to use them.
- e2e `secrets.e2e.ts` asserts the six labels of the add dialog; the mock API (`mock.js`) serves
  agent secrets.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| ASH-001 | The "Add agent secret" dialog shows Name, Value and Description only. An instance administrator also finds Scope under a closed "More options". No hosts, no injection, no header or user name. | e2e |
| ASH-002 | The API creates an agent secret without hosts and without an injection (`none`) when they are absent; an unbound secret is sent to and typed into nowhere (proxy and `browser_fill_secret`), and is still redacted everywhere. | integration |
| ASH-003 | MCP tool `secret_request_hosts {secret, hosts, reason?}`: an agent proposes hosts (validated like bindings, at most 16 with the bound ones) for a secret of its project; hosts already bound are reported as bound; new ones are stored as the secret's request with the task number, audited (`vault.agent.request`), and the task is flagged as needing a person. The agent never binds anything by itself. | integration |
| ASH-004 | People who manage the secret allow (`POST /api/agent-secrets/{id}/request {allow: true}`) — the requested hosts join its bindings — or dismiss it (`allow: false`); audited (`vault.agent.allow` / `vault.agent.dismiss`); others get 403/404. | integration |
| ASH-005 | The Secrets page shows an unbound secret as "Not sent anywhere yet", and a request as "Task #N asks to send it to …" with Allow and Dismiss buttons for its managers. The Edit dialog keeps Hosts (people may still bind or unbind directly) and keeps an existing secret's injection controls only when it has one. | e2e |
| ASH-006 | `secrets_list` shows each secret's requested hosts and tells agents to find the host from the description and the service's docs, request it, and send the placeholder in the header the service wants. | integration |
| ASH-007 | SPEC-001 (VLT-004/006/011), SPEC-002, the product summary, the architecture document, an ADR and `docs/changes.md` describe it. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Agent secrets can be created without hosts or an injection, and an unbound one goes nowhere
- [ ] Agents request hosts with secret_request_hosts and people allow or dismiss them
- [ ] The add dialog asks for a name, a value and a description, and the list shows and answers requests
- [ ] Specs, product, architecture, decision record and changes describe agent-found hosts
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-ASH-1: the add dialog's visible field labels are exactly Name, Value, Description for a
  member; an instance administrator sees "More options" closed, with Scope inside.
- AC-ASH-2: `POST /api/sandboxes/{sid}/agent-secrets {name, value, description}` answers 201 with
  `hosts: []` and `injection: "none"`; the proxy policy binds the secret to no host;
  `browser_fill_secret` refuses it with a message pointing to `secret_request_hosts`.
- AC-ASH-3: `secret_request_hosts` with `api.vidoc.example` stores the request (visible in the
  list API as `requested_hosts`, `requested_by: "#N"`), writes `vault.agent.request`, flags the
  running task `needs_attention`; the secret's `hosts` stay empty. An invalid host, an unknown
  secret and more than 16 hosts are refused.
- AC-ASH-4: a manager's allow binds the requested hosts and clears the request; dismiss clears it
  and binds nothing; a read-only member gets 403.
- AC-ASH-5: in the mock UI, a secret with a request shows the sentence and Allow; clicking Allow
  shows the host as bound; the page has no axe violations.

## Test plan

- Unit: `parse_hosts` accepts an empty list; the request merge (dedup, already-bound, limit).
- Integration (`crates/yulius/tests/secrets.rs`): create without hosts/injection; proxy policy
  and browser fill with an unbound secret; MCP `secret_request_hosts` (success, unknown secret,
  invalid host, limit, flag); allow/dismiss by a manager and refusal for a reader; `secrets_list`
  shows the request.
- e2e (`secrets.e2e.ts`, mock UI): the three-field dialog, the admin's "More options", an
  unbound secret's text, a request answered with Allow, axe.

## Decisions

- **Agents propose, people allow (ADR-0098).** Alternatives: (a) the agent binds hosts itself —
  rejected: a prompt-injected agent could bind a secret to its attacker's host and the proxy
  would deliver it, defeating "use, never see"; (b) trust on first use (the first binding by an
  agent sticks) — rejected for the same reason at the most likely moment of misuse, a fresh
  secret; (c) guess hosts on the server from the name — not knowable (`VIDOC_API_KEY` says
  nothing reliable about `api.vidoc.…`); (d) keep the fields but collapse them — the person still
  has to know the address, which the request says they should not.
- **No injection for new secrets.** Placeholders already work in any header, Basic credential,
  path or query; the agent writes them where the service expects. Existing secrets keep their
  injection; the edit dialog shows it only for them.
- **"Description" instead of "Notes"** for agent secrets: the API field is `description`, and
  the person's request says "description"; the hint tells them to say what service it is for.
- **Storage:** migration 0025 adds `requested_hosts` (JSON list) and `requested_by` to
  `vault_secrets`; a request replaces nothing already bound and merges with an open request.
- **The request flags the task `needs_attention`** like `browser_request_user_action`; the
  persistent signal is the request on the Secrets page.
