---
yulius-task: 197d7905-2c61-41a5-b567-f8ce85d7f442
status: in-progress
---

# SPEC-410: teammates try a playground without GitHub sign-in

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #392
Scope authorization: issue [#412](https://github.com/Quartio-Studio/yulius/issues/412), decided by
the product owner on 2026-11-03 ("#412 - ok"): teammates can try a playground without signing in
with GitHub. The task says: amend SPEC-001 ACC-005 for playgrounds only in an ADR naming that
decision; keep it narrow (guests reach only the playground they were let into, with no access to
other projects, secrets or instance settings); decide how a guest gets in and how the maker
revokes it; cover it with Rust and e2e tests.

## Goal

A person who made a playground (SPEC-099) lets teammates try it without GitHub. They create an
expiring guest link and send it; a teammate opens it, types a name and works in that playground
(tasks, terminals, editor, browser) until the link expires or the maker revokes it. A guest
reaches nothing else: no other sandbox, no secret, no instance setting, no account page.

## Context

- SPEC-001 ACC-005 (a founder requirement): "Every user authenticates with GitHub; there is no user
  password." The only password account is the superadmin's (`auth.rs`, ACC-001). Users are
  admitted by GitHub login (ACC-004).
- A playground (PRJ-012, SPEC-099 PLY-005) is its maker's and the superadmin's alone:
  `access.rs` answers Admin to them and None to everyone else, with no sharing.
- Moving a playground to GitHub (SPEC-176) turns its access into GitHub's (ACC-007).
- `AccessLevel::Write` allows `Action::Work` and `Action::Configure` (automations, workflows,
  sandbox MCP servers), and lets the holder manage the project's agent secrets
  (`vault/agents.rs`) and variables (`variables.rs`). A guest therefore cannot simply be a writer.
- Many places label a user without a GitHub login as "superadmin" (audit actors, task owners),
  which would misattribute a guest's actions.
- Open WebSockets re-check `still_signed_in` and access periodically (ACC-008), so revoking a
  session or access closes them.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| GST-001 | An admin of a playground (its maker, the superadmin) creates a guest link from a browser session or an `operate` token: `POST /api/sandboxes/{sid}/guest-invites` `{days}` with `days` 1, 7 (default) or 30. The answer (201) carries the link `<public_url>/join?code=<code>` once; only the code's SHA-256 is stored. A sandbox that is not a playground is a 409; a non-admin a 403 (a guest included); at most 5 live links per playground (a 6th is a 409). Audited `playground.invite.create`. | `playground_guests::a_maker_creates_an_expiring_link_shown_once`, `only_a_playgrounds_admin_creates_links` |
| GST-002 | `GET /api/guest-invites/{code}` (no session) answers the playground's name, its maker's name and the link's expiry; an unknown, expired or revoked code, or a playground removed or moved to GitHub, is one answer, 410 "this guest link has ended" (so no code is told apart from another). `POST /api/guest-invites/{code}/join` `{name}` (no session) creates a guest: an account with no GitHub link and no password, named `name` (trimmed, 1–60 characters, no control characters), and a browser session that ends when the link expires (or earlier, at the session lifetime). At most 20 guests join one link (the 21st is a 409). Both routes count against a per-client limit of 10 a minute (ACC-013's client key). Audited `playground.guest.join`. | `playground_guests::a_guest_joins_with_a_name_and_gets_a_session`, `a_dead_link_lets_nobody_in`, `a_link_lets_in_at_most_twenty_guests`, `joining_is_rate_limited_per_client` |
| GST-003 | A guest has write access to the playground of their link while it lives (not revoked, not expired, the sandbox still a playground and not removed) and no access to any other sandbox. In it they work (tasks, terminals, editor, browser, files, Git page, merging its tasks) but never configure or administer it: `Action::Configure` and `Action::Administer` answer 403, and their tasks' agents get no configuring tool (`automation_*` need Configure, as on the remote MCP). Inside the playground a guest shares its workspace with its maker: its files and worktrees, terminals, browser (with what is signed in there) and the instance's Claude account its tasks run on (every run carries that account's token, as for any task, so a guest's task can copy it and it outlives the link; the product owner left this to the task on 2026-11-03 and it ships stated in the Invite dialog, ADR-0260). | `playground_guests::a_guest_works_only_in_their_playground`, `a_guest_never_configures_the_playground`, `a_guests_task_gets_no_configuring_tool` |
| GST-004 | A guest reaches no Yulius secret and no setting: the agent-secret, variable and vault routes, API tokens, CLI device approval, making sandboxes or playgrounds, imports, onboarding, Claude accounts, instance administration and every other route outside their playground answer 403 "a guest reaches only the playground they were let into" (an allowlist of exact routes and of the prefixes of the playground's work: tasks, sessions, shells and the listed pages of the sandbox, whose routes each decide by access). Since a guest shares the sandbox, once a guest has joined a playground none of its runs (the guest's, its maker's, a CI job's, whose grant a guest's merge could change) gets an agent secret, and no saved agent state is restored into or saved from it, also after the link ends and after a move to GitHub, and the instance Vidoc key is not added either; the Invite dialog says so. Any account but the superadmin without a GitHub login is fenced so. | `playground_guests::a_guest_reaches_no_secret_or_setting`, `a_playground_with_guests_gives_no_run_an_agent_secret`, `a_playground_with_guests_gets_no_instance_vidoc_key`, `a_playground_with_guests_keeps_saved_agent_states_out` |
| GST-005 | The playground's admins list its live links (`GET /api/sandboxes/{sid}/guest-invites`: expiry, who made it, and each guest's name and join time; never the code) and revoke one (`DELETE /api/sandboxes/{sid}/guest-invites/{id}`). Revoking ends at once every guest it let in: their sessions are revoked, their requests answer 401, their open sockets close at the next re-check (ACC-008), and their live and waiting tasks are stopped and their terminals closed. An expired link does the same: its sessions end at its expiry. A background sweep (its own one-minute loop, at most 20 links a pass, least recently swept first) stops what the guests of every revoked or expired link, and of the live links of a playground moved to GitHub or removed (which it revokes), still run, and keeps doing so until a pass succeeded a minute or more after the link died, so a task created as it died, a failed stop or a restart is caught. A join that races a revoke leaves no session. Audited `playground.invite.revoke`. | `playground_guests::revoking_a_link_ends_its_guests_at_once`, `an_expired_link_ends_its_guests`, `revoking_or_expiry_stops_the_guests_tasks`, `a_join_racing_a_revoke_leaves_no_session`, `a_guests_event_stream_carries_only_their_playground_and_ends_with_the_link`, `moving_or_removing_the_playground_ends_guest_access`; store `playground_guest_links_are_bounded_and_end_once`, `the_guest_link_sweep_takes_the_least_recently_swept_first` |
| GST-006 | A guest is labeled "<name> (guest)" wherever a person is named (audit actor, task owner); `/api/me` names them so (`login`) and says `guest` with the playground's slug and the end of the access, and never calls them superadmin. | `playground_guests::a_guest_is_named_as_a_guest` |
| GST-007 | Web UI. A playground admin's sandbox menu has "Invite guests…": a dialog to create a link (1, 7 or 30 days), copy it, and see and revoke live links with their guests. `/join?code=…` shows who invited whom to which playground and a name field with "Join as guest"; a dead link says so; a browser already signed in is told to sign out first. A guest lands in the playground, sees a "Guest" badge with when the access ends, and no Add sandbox, Settings, secrets or administration. | `e2e/tests/playground-guests.e2e.ts` |

## Plan

- [x] Spec and ADR amending ACC-005 for playground guests
- [x] Guest links: stored, created, listed and revoked by a playground admin
- [x] Joining: a guest account and session from a live link
- [x] Guest access: only the invited playground, fenced routes, no secrets
- [x] Revocation and expiry end a guest's sessions and sockets
- [x] Web UI: Invite guests dialog for the playground admin
- [x] Web UI: join page and guest mode
- [x] Docs: SPEC-001, SPEC-099, architecture, product and changes
- [x] Review fixes: no secrets or states in a guest playground, no configuring tools, revoking stops guests' work
- [x] Review rounds 2 and 3 answered: no token surprise, sweep retries, Vidoc key withheld
- [ ] Independent review approves the head
- [ ] Gates green on the reviewed head

## Acceptance criteria

- A maker creates a link; a teammate without GitHub opens it, joins with a name and creates and
  works on a task in that playground.
- That guest sees no other sandbox (list and direct requests), cannot configure the playground,
  and every user- or instance-scoped route outside the allowlist answers 403.
- Revoking the link or its expiry ends the guest's session at once (401) and their access.
- Moving the playground to GitHub or removing it ends guest access.
- Audit entries name the guest as a guest, never as the superadmin.

## Test plan

- Rust integration tests `crates/yulius/tests/playground_guests.rs` (one binary, ADR-0028) through
  the HTTP router with the real store: link creation and bounds, join and its bounds and limiter,
  access across sandboxes and actions, the fence over representative routes of every area, a
  guest-owned task's resolved agent secrets, revocation and expiry (the harness clock), move to
  GitHub and removal, labels.
- Store tests for the new rows (`crates/yulius-store/tests/store.rs`).
- e2e `e2e/tests/playground-guests.e2e.ts` against the mock: the maker's dialog (create, copy,
  revoke), the join page (live, dead, signed-in) and guest mode.

## Decisions

- **The link is the credential** (ADR-0260): a 256-bit random code in an expiring, revocable
  link, no password. A password per guest would be a second password system to recover and
  rate-limit; an email invite needs mail Yulius does not send.
- **A guest is an account of its own**, one per join, tied to its link (`playground_guests`), not
  a flag on `UserRecord` (32 literal sites, parallel tasks). It never links to GitHub; a teammate
  who later signs in with GitHub is admitted as anyone is (ACC-004), and the maker may hand over
  a guest's tasks (SPEC-070).
- **Write without Configure**, enforced in `App::require` for guests, plus an allowlist fence in
  the server for everything not under the guest's playground.
- **The session ends with the link**; revocation revokes the sessions and stops the guests' tasks
  and terminals, and a background sweep does the same for expired links and for the links of a playground moved to GitHub or removed.
- **A guest shares the playground's workspace** (review of 2026-11-03): separate containment per
  person would be a new isolation boundary. So the rule is on what the playground holds: once a
  guest joined, its runs get no agent secret or saved state. What stays shared is stated in the
  Invite dialog and ADR-0260: its files, terminals, browser and the instance's Claude account;
  every run's environment carries that account's token (true of every task before this change),
  so a guest's task could read it, and it outlives the link. Asked about it, the product owner
  left the choice to the task ("decide what's better", 2026-11-03): ship with it stated rather
  than block guests' tasks; proxying that token is a follow-up for every run.
- **Bounds**: 5 live links per playground, 20 guests per link, 10 joins or peeks per client a
  minute.

## Evidence

Pending.
