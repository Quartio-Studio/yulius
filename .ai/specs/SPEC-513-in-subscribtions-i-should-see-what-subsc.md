---
yulius-task: 1750d0c6-ed74-4d6d-8f30-e85034e76227
status: in-progress
---

# SPEC-513: Each subscription card names its agent

Status: in-progress
Date: 2026-11-19
Owner: Yulius task #495
Scope authorization: task #495, the person's request of 2026-11-19 ("in subscriptions, I should
see what subscription this is (gemini, codex, claude etc)"). Builds on SPEC-503 (Codex
subscriptions) and SPEC-505 (Antigravity subscriptions), which made the pool hold three agents.

## Goal

Every subscription card says which agent's subscription it is (Claude, Codex or Antigravity) at a
glance, wherever it is shown: the Subscriptions page, the Status panel and the Dashboard.

## Context

Since SPEC-503 and SPEC-505 the Subscriptions page lists Claude, Codex and Antigravity
subscriptions side by side, each agent in its own pool. A card (`accountUsage` in
`web/assets/layout.js`) shows an avatar with the name's initials, the name, the plan when the
provider reports it, and under the name either the email (administrators) or the provider's name
(everyone else). The Subscriptions page and the administrator's Status panel and Dashboard show
the email, so nothing on those cards says which agent a subscription belongs to: a subscription
named "work" may be Claude's, Codex's or Antigravity's, and only the card's buttons ("Paste a new
token", "Import auth.json") hint at it.

The API already carries the provider on every card's data: `GET /api/admin/accounts`
(`AccountView.provider`), `GET /api/status` and `GET /api/capacity` (`CapacityAccount.provider`).
The web mock's capacity view leaves it out, so the Dashboard's cards in the mock read every
subscription as Claude's.

## Requirements

- SUBK-001: Every subscription card shows a badge with its agent's short name (Claude, Codex,
  Antigravity, from `agents.js`) in the line under the subscription's name, before the email for
  administrators and alone for everyone else (where it replaces the plain provider name).
- SUBK-002: The badge's title says whose subscription it is and from which vendor (for example
  "Antigravity subscription · Google Gemini"), and a screen reader reads the agent with the name.
- SUBK-003: The badge never replaces the name, the plan badge or the email, and takes no room from
  the name: a folded row at phone width keeps the name's 64 px (SPEC-172 GLN-004); the email
  shortens with an ellipsis instead.
- SUBK-004: The web mock's `GET /api/capacity` carries each subscription's `provider`, as the
  server's `CapacityAccount` does.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Agent badge on every subscription card, with its e2e test
- [x] Product document and changes entry say what the card shows
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- On the Subscriptions page with a Claude, a Codex and an Antigravity subscription, each card's
  badge reads "Claude", "Codex" and "Antigravity" respectively, with the vendor in its title.
- On the Dashboard's Subscriptions card, a Codex subscription's folded row shows "Codex".
- A Codex card still contains no "Claude" (the existing `codex.e2e.ts` check), and axe finds no
  violations on the page.

## Test plan

- e2e (`e2e/tests/subscription-agent.e2e.ts`): the Subscriptions page with Codex and Antigravity
  subscriptions connected through the mock shows each card's agent badge and its title; the
  Dashboard's folded rows show the same; axe passes. The test fails before the change (no badge).
- The existing subscription e2e files (`subscriptions`, `subscription-names`,
  `subscription-touch`, `codex`, `antigravity`) still pass: the name and email selectors are
  unchanged.

## Decisions

- A badge under the name, before the email, rather than beside the name or a logo: beside the
  name it took the room a folded row at phone width keeps for the name (the SPEC-172 GLN-004 test
  failed at 54 of 64 px), and vendor logos are trademarks the repository does not ship. The email
  stays its own element (`.account-sub`), editable in place as before (SPEC-463). The badge uses the agent's short name the rest of the UI uses (agents.js), so a
  person reads the same "Antigravity" as in the Add dialog and the composer; the vendor, which is
  what the person asked about for Gemini, is in the title.
- A neutral badge (surface colour, muted text), not one colour per agent: the plan badge keeps
  the accent colour, and no new palette tokens are needed.
- No ADR: a display choice on one card, recorded here.
