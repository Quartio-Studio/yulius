---
yulius-task: 1750d0c6-ed74-4d6d-8f30-e85034e76227
status: done
---

# SPEC-513: Each subscription card names its agent

Status: done
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
  the name: a folded row at phone width keeps the name's 64 px (SPEC-172 GLN-004). The badge stays
  whole; the email shortens with an ellipsis and goes to its own line only when not even its start
  (4 em) fits beside the badge. On a folded row, where the email is read-only, its title is the
  whole address, so a shortened one can be read; on the Subscriptions page the open field shows
  it. Open, the email field and its refusal take the whole line under the badge.
- SUBK-004: The web mock's `GET /api/capacity` carries each subscription's `provider`, as the
  server's `CapacityAccount` does.
- SUBK-005: Nothing in the line under the name clips: the email's pencil and field keep their
  whole focus ring.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Agent badge on every subscription card, with its e2e test
- [x] Product document and changes entry say what the card shows
- [x] Independent review in a fresh session, findings fixed test-first

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
- The same file: the Status panel shows each agent for an administrator and for a member (whose
  line is the badge alone); the email's pencil and field have no clipping ancestor within their
  4 px focus ring; at 360 px an Antigravity subscription at its limit with an old reading (the
  widest line) keeps its badge whole, the name's room and at least 44 px (4 em) of its email, in the
  Dashboard and the Status panel; axe passes on each. The ring and phone tests fail on the round-1
  CSS (`overflow: hidden` on the line, no wrap). At 360 px on a card at its limit, the open email
  field and its refusal stay inside the column under the name (review round 2; fails without the
  open field's own line).
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
- An outlined badge (`--border-strong` edge, muted text, no fill), not one colour per agent and
  not a filled pill: a fill of `--surface-2` or `--surface-3` matches a folded row's own fill or
  its hover fill and disappears there (review round 1); the plan badge keeps the accent colour,
  and no new palette tokens are needed.
- The line wraps rather than clips: `overflow: hidden` on it cut the email pencil's focus ring
  (review round 1), and a badge that shrinks would no longer say whose subscription it is. The
  email takes the rest of the line with an ellipsis and a 4 em basis, so it moves under the badge
  only in the narrowest rows.
- The badge names the agent, not the vendor, in text; the vendor (Google Gemini for Antigravity)
  is in its title. Every page names the agent (the Add dialog, the composer, a task's notices),
  and Gemini is what the composer's model menu shows for Antigravity.
- No ADR: a display choice on one card, recorded here.

## Evidence

- Test first: `e2e/tests/subscription-agent.e2e.ts` failed on dev's UI ("expected null to equal
  ["Claude","Claude Code subscription · Anthropic"]", no badge), then passed with the badge.
- Round 1 (request changes, Fable 5.2): the line's `overflow: hidden` cut the email pencil's focus
  ring (major); the email nearly vanished at 360 px, the filled badge was invisible on folded rows,
  and tests missed order, the Status panel and members (minors); alignment (nit). Answered in
  ee8cb1752 and 85bec4b56; the ring and phone tests failed with round 1's `styles.css` put back
  ("expected ["account-subline"] to equal []", ".db-card Antigravity: expected 0 to be greater
  than or equal to 24").
- Round 2 (approve on 85bec4b56, Fable 5.2): one minor (the open email field 70 px wide beside the
  badge on a card with a state at 360 px, its refusal past the column) and two nits (the phone test
  asserted 24 px against the 4 em of SUBK-003; SUBK-003's title sentence too broad, SUBK-005 out of
  order). Fixed in 2ca737d29 within the approval's 20 product lines (2 CSS lines): the new test
  failed first ("expected 243 to be less than or equal to 217.5"), then passed.
- `npx e2e run` of subscription-agent (6 tests), subscriptions, subscription-names,
  subscription-touch, phone and responsive: 111 passed; earlier on the round-1 fix, those plus
  codex, antigravity, text-fits and loading-shapes: 169 passed.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^ui_contract::/)'`: 11 passed.
- `scripts/dev quick`: passed (it caught a 5 px padding off the 4 px scale, fixed in 85bec4b56).
- Screenshots at 1280 and 390 px of the Subscriptions page and the Dashboard (mock, dark theme):
  the badge reads beside the email; kept locally under the ignored `target/shots/`.
- Yulius CI on e4c6fc183 passed (quality, both test shards, e2e). The `security` job was skipped
  (exit 78: Vidoc not run on this instance, as on every recent run), which is unrun, not passed.
- Yulius CI on 85bec4b56 failed in `migrate_helper::tests::a_stop_of_the_helper_ends_the_command_first`
  (NotFound), the flake open as #1375, outside this change (no Rust touched). Its cause: the helper
  says Running once the command started, before the test script set its TERM trap, so a stop
  under load killed bash first. A 0.3 s delay before the trap reproduced it every time; with the
  script's own mark awaited (8ce2552b1, tests only, in both trap tests) it passed with the delay,
  and `--stress-count 20` of all 18 `migrate_helper::` tests passed 20 of 20.
- Yulius CI on 2d2fc89ef failed in `restart::a_wait_ends_at_its_deadline_while_yulius_is_down`
  ("the task token was refused"), also outside this change: the test freed the port it picked to
  stand for a stopped Yulius, and another test's server took it. Serving Yulius on that port in
  the test reproduced the message exactly; the test now holds the port with a bound,
  non-listening socket (refused connections, no other bind: both a std and a tokio listener were
  refused in a probe), tests only. `--stress-count 10` of `restart::a_wait*` and
  `restart::a_refused*`: 10 of 10.
- Not tested: a real server with real Codex and Antigravity subscriptions (the server already sends
  `provider` on all three endpoints, checked in `CapacityAccount`, `AccountView` and the status
  view; the UI is proved against the mock, whose capacity view now matches the server).
