# 0336. A secret's value is typed by its person on the Superagent's panel

## Status

Accepted. Specified in
[SPEC-487](../../.ai/specs/SPEC-487-the-superagent-can-do-everything-its-per.md) (SEC-001 to
SEC-004), task #469, slice 10a; the person's decision of 2026-11-13 ("no agent sees a secret's
value; the person types it masked beside the chat"). Keeps
[0144](0144-the-superagent-reads-the-agent-vault-and-verifies-values-by-hash.md) (the Superagent
never sees a value) and [0156](0156-a-pasted-multi-line-secret-is-stored-with-escaped-line-breaks.md)
(a multi-line key is stored with literal `\n`); builds on
[0333](0333-the-superagents-requests-to-its-person-last-until-answered.md).

## Context

A Superagent setting up a deploy needed 14 values; it could not take them, so the person typed
each on the Secrets page by hand, or pasted it into the chat, where the agent and its transcript
would hold it (wall 10 of task #469). Once a multi-line key pasted into a single-line field lost
its line breaks without a word.

## Decision

- **`secret_request_value {name, description?, scope?}`** is a request on the Superagent's own
  panel, lasting as its others are: the card names the secret and what it is for, and holds a
  masked multi-line field (a password field where the browser cannot mask a text box), its scope
  (this project's, or the person's own) and, when it is so, that it replaces the value of the
  person's secret of that name in that scope. Refused before anyone is asked: a name the vault
  takes from no agent (not capitals, digits and underscores, reserved, the Vidoc key), a scope
  other than `sandbox` (default; the person needs Configure in the project) or `personal`, a
  description over 500 characters, unknown arguments.
- **The value goes from the page to the vault.** The answer carries it once; it is checked as
  the Secrets page checks a pasted value (`agent_value_from_file`: a final line break dropped, a
  key's line breaks as literal `\n`, a cut or flattened key refused) before the request is
  taken, so a value the vault refuses for its own rules leaves the request waiting, typed (the
  vault's later refusals, such as a capacity or a race on the name, are the answer of both the
  click and the call). Approved, it is stored with the answering person's browser session as
  theirs: the new value of their secret of that name in the scope asked, or a new agent secret
  there; never another scope's secret, never a global one. Like every agent secret it starts
  with agents allowed to propose its hosts (`auto_allow`), as `secret_create_from_file`'s do. The
  request, its file, its outcome and the audit hold the name only; the value is wiped once
  stored, as far as the server holds it in one place (the request body and the vault's own
  copies are not: best effort, as on the Secrets page), and the page clears the field once the
  server took it.
- **The Superagent learns the name and whether it was created or updated**, and uses it as
  `$NAME` or its placeholder, as every agent secret.

## Alternatives

- **The value pasted into the chat.** The agent, its transcript and every tool output would hold
  it: what ADR-0144 rules out. Rejected.
- **A link to the Secrets page.** What was done before: the person leaves the conversation for a
  form that asks again what the Superagent already knows (the name, what it is for).
- **A one-time link per value**: another page and another way in, for what the panel already
  carries with the person's session.

## Consequences

- "Here are the 14 keys" becomes 14 cards on the panel, each typed once.
- A value is stored only by a person's click in a browser; no token, agent or Superagent stores
  one this way.
