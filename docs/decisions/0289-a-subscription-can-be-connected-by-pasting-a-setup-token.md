# 0289. A subscription can be connected by pasting a setup-token made elsewhere

## Status

Accepted. Specified in [SPEC-450](../../.ai/specs/SPEC-450-paste-a-claude-token-made-elsewhere.md).
Extends SPEC-001 ROT-001/ROT-002, SPEC-002 ADR-05 and
[ADR-0184](0184-a-captured-token-ends-the-sign-in-and-a-closed-dialog-cancels-it.md): the login
terminal stays the default way in.

## Context

A Claude subscription is connected by running `claude setup-token` in a server-owned terminal
that an instance administrator watches in the browser: they open Claude's link and approve with
the subscription's Claude account, and Yulius captures, masks and seals the token it prints. That
needs the subscription's owner and the administrator at the same browser. `claude setup-token`
runs on any computer and prints the same long-lived token (`sk-ant-oat01-…`, about a year), so
the owner can make it on their laptop; Yulius had no way to take it.

## Decision

- **Paste is a second way in, beside the terminal.** `POST /api/admin/accounts` takes an optional
  `token`: with it the subscription is created signed in (`ok`, no `login_id`, no process);
  `PUT /api/admin/accounts/{id}/token` replaces the token of an existing one (and cancels its
  running sign-in). The Subscriptions page's Add dialog offers "Sign in here" or "Paste token", and
  every card "Paste a new token".
- **Only the shape `setup-token` prints is taken**: `sk-ant-oat01-` and 20 or more of
  `[A-Za-z0-9_-]`, at most 512 characters, surrounding whitespace trimmed; 422 otherwise.
- **One profile request checks it.** Claude's 401 refuses the paste (422, nothing stored); 403 is
  the normal answer for a `setup-token` and accepts it; a temporary failure accepts it and leaves
  the email to the background round.
- **Stored exactly as a captured token**: sealed with the master key, status `ok`, errors and email
  cleared, the pool told, the usage read. The token is never echoed, logged, published or audited;
  the audit says `account.created` with `via=token`, or `account.token_replaced`.
- **The UI warns** that a token is a credential that should not be sent through chats in clear
  text.

## Alternatives

- **Only the terminal** (status quo): the owner of the subscription has to be at the
  administrator's browser or share the approval link.
- **A token field on `/relogin`**: one endpoint answering a `login_id` or not depending on its
  body.
- **Checking with a real inference request**: spends the subscription, is slower and needs a new
  adapter; the profile request already runs after every sign-in.
- **No check**: a mistyped or revoked token would make an `ok` subscription that fails its first
  task.

## Consequences

The subscription's owner connects it without the server's terminal, and a colleague can repair a
subscription whose sign-in expired by pasting a new token. A pasted token passes through the
browser and the request once; the page masks the field, and the server zeroes its working copy
after sealing (request buffers of the HTTP stack are not zeroed). A token Claude cannot answer
for right now is accepted and may turn out not to work at its first task, which then marks the
subscription `relogin` as for any sign-in (ROT-010).
