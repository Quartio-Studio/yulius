---
yulius-task: 4aa3a527-c2da-49fd-bb26-1546636dce4a
status: done
---

# SPEC-463: A subscription's email is typed in place, like its name

Status: done
Date: 2026-11-11
Owner: Yulius task #448
Scope authorization: task #448 (`/goal`, with a screenshot of the Subscriptions page: three
cards, two of them named by an address, each over "email unknown"): "We need to have a
possibility to change address email and the main text. The format should be following", then two
lines: "Agent TEAM" and, under it, the team's address (written here as `agent@example.com`; this
repository is public). "The rest stays the same. Only this small UI fix (those 2 editable lines).
Just add it and don't change anything else." Amends SPEC-244 (SUB-003, SUB-006), SPEC-243
(LGN-003) and SPEC-001 ROT-009 and UI-022.

## Goal

On the Subscriptions page both lines of a subscription's head are edited in place: the name, as
today, and the email under it, which is new. A card then reads "Agent TEAM" over
"agent@example.com". Nothing else on the card or the page changes.

## Context

Confirmed against `dev` at dbec951d.

- **What the person sees.** Every subscription of the production instance reads "email unknown",
  so the address went into the name, and the name had nowhere to go.
- **Why the email is unknown.** Yulius signs a subscription in with `claude setup-token`. The
  email is read only from Claude's profile with that credential (SPEC-244 SUB-003,
  [ADR-0185](../../docs/decisions/0185-a-subscription-email-comes-from-claudes-profile-at-sign-in.md)),
  and such a sign-in may not read the profile. ADR-0185 foresaw this ("every subscription reads
  'email unknown'") and named a follow-up, reading the email from the token exchange through the
  egress proxy. That follow-up was not built.
- **The name is already edited in place** (SPEC-244 SUB-001): `renamableName` in `layout.js`, saved
  by `PATCH /api/admin/accounts/{id}` with `{"label"}` (`App::update_account`, administrators
  only, audited `account.updated`).
- **The email is read-only.** `accounts.email` and `email_checked_at` (schema 38) are written by
  `record_account_email` (after a sign-in, and once by the background usage round) and cleared by
  every new credential (`set_account_credential`): the new sign-in may be another Claude account.
- **Typing the email was rejected before.** ADR-0185: "that is a second name, not the account
  behind the credential, and a typo would be shown as fact". That reason still holds. The owner
  now asks for it and takes that risk: without it the line says nothing.
- **Who reads the email.** Administrators, wherever a subscription is named (SUB-005, UI-022):
  the Subscriptions page, the Status panel, the Dashboard pool, Task details, the analytics. The
  toast after a sign-in also names it: "*name* is connected as *email*" (SPEC-243 LGN-003).
  Every one of them reads `accounts.email`.
- **Parallel work.** Pull request #1144 (connect a subscription by pasting a token) changes the
  same files for another flow and adds two such toasts; its new token clears the email, which
  stays true for an email read from Claude. Task #441 added migration 0051.

## Requirements

| ID | Requirement |
| --- | --- |
| EML-001 | On the Subscriptions page the email line of each subscription is edited in place, as its name is: a pencil button "Change the email of *name*" (or a click on the email, or on "email unknown") turns the line into a field holding the current email, empty when it is unknown. Enter or leaving the field saves it, and Escape cancels, even after an email was typed. The value is trimmed; an unchanged one closes the field without a request. A value that is not an address is refused before anything is sent ("Type an address like name@example.com."), and one the server refuses gets the server's message; either way the field stays open. Focus returns to the field only after Enter, never when the person left it. Once a key ends the edit, the keyboard is back on the pencil; when the list is drawn again (a renamed subscription changes its place, a card's usage is read again), the keyboard stays on the same control of the same subscription. A saved email shows at once on the card and, through `account.updated`, in every open view. An update that arrives while a field is open, or a list read that was on its way when it opened, neither closes it nor saves it; it is drawn once the field's save has settled or the field has closed. The address is stored as it was typed. |
| EML-002 | Enter in an empty field removes the typed email: the subscription then shows what Claude shares for its sign-in, or "email unknown". Leaving an emptied field removes nothing: the field closes and the email stays. |
| EML-003 | `PATCH /api/admin/accounts/{id}` takes `label`, `email`, or both; a request with neither, or with any other field, is `invalid` (422). `email` is a string, trimmed: an address (one `@` with text on both sides, no whitespace or control characters, at most 254 characters), or empty to remove it. Nothing is written when either field is refused. Only instance administrators and the superadmin may call it (403 otherwise, unchanged ACC-009). It is audited as `account.updated` with `email=set` or `email=removed`, never the address. |
| EML-004 | A typed email is the person's, like the name: it stays when the subscription signs in again, and an answer from Claude never replaces it. Removing it lets the background round ask Claude again, once. An email read from Claude is still dropped by a new sign-in (SUB-003 unchanged). |
| EML-005 | The typed email is shown where, and to whom, the email was shown before (SUB-005, UI-022): administrators only, wherever a subscription is named; task summaries never carry it. The name edit, the other card elements, the Status panel and the Dashboard are as before; there the email stays read-only. |
| EML-006 | Yulius never says that Claude confirmed a typed email. `AccountView` carries `email_typed` (`true` when an administrator typed the email), and the toast after a sign-in names the email only when Claude gave it: with a typed one it reads "*name* is connected. Tasks can use it now." (amends LGN-003). |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The store keeps a typed email across sign-ins and Claude's answers
- [x] PATCH /api/admin/accounts/{id} sets or removes the email for administrators
- [x] The Subscriptions page edits the email line in place
- [x] ADR-0300, architecture, API reference and product spec say the email can be typed
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- On the Subscriptions page, renaming a subscription to "Agent TEAM" and typing
  "agent@example.com" as its email leaves a card whose first line reads "Agent TEAM" and whose
  second reads "agent@example.com"; the Status panel shows both, and offers no pencil (e2e).
- A value that is not an address is refused with the field kept open, and nothing is sent or
  stored; Escape keeps the earlier email; Enter in an empty field brings "email unknown" back;
  leaving an emptied field keeps the email (e2e, API test).
- A field that is open keeps its text and its focus when another subscription's change arrives,
  and when a list read that was on its way lands; nothing is sent for it. After a save by leaving
  the field, the list is read again: the initials follow the email, and a change another
  administrator made meanwhile shows (e2e).
- An unchanged, a refused and a cancelled value are never sent; an address Claude gave is not
  marked as typed by opening and closing its field (e2e, counted by the mock).
- An address with a domain outside ASCII is stored and shown as typed (e2e).
- The keyboard stays with the subscription when the list is drawn again: after a name saved with
  Tab it is on that subscription's email pencil although the card moved, after Shift+Tab on the
  neighbouring card's Remove, after an email saved with Tab on that card's next control, and
  after a card's usage is read again still on its pencil, so Tab goes on to the email (e2e).
- With the mouse, typing the name and pressing the email's pencil saves the name and opens the
  email; both are stored (e2e).
- At 360 px a long address is cut short of its pencil, which stays on its line, and nothing
  spills (e2e).
- A change the server refuses keeps the field open with the server's message, for both lines
  (e2e).
- A member is refused the change with 403, and sees no email anywhere (API test).
- A request with a good name and a bad email, or the reverse, writes neither; `{"email": null}`
  alone is 422 (API test).
- A typed email is still there after the subscription signs in again, whether Claude refuses the
  profile or answers another address; an email read from Claude is still dropped by a new sign-in
  (store and integration tests).
- After a typed email is removed, the background round asks Claude for it once more (integration
  test).
- The audit entry of an email change does not hold the address (integration test).
- After a sign-in of a subscription with a typed email, the toast does not say "as *email*"
  (e2e); `AccountView.email_typed` says which emails were typed (integration test).
- The tests that were in the `subscription-names` and `subscriptions` e2e specs before this
  change pass as they were written: the name edit is intact.

## Test plan

- Store (`crates/yulius-store/tests/store.rs`, and the upgrade test in `src/lib.rs`): the
  migration adds `email_typed` and leaves stored emails Claude's; a typed email survives
  `set_account_credential` and `set_account_email`, which still stamps `email_checked_at`;
  removing it clears the email and the check; a rename leaves it alone; an empty change writes
  nothing.
- App and API (`crates/yulius/tests/account_email.rs`), with the fake profile and the fake
  sign-in host: an administrator sets, changes and removes the email; a bad address, an empty
  request, `null`, an unknown field and a good field beside a bad one are 422 and write nothing;
  a member gets 403; the views carry the typed email for administrators only, and `email_typed`
  on `AccountView`; a re-sign-in keeps it; removal makes the background round ask again; the
  audit detail has no address.
- Web: the mock's `PATCH` follows the server's rule. The e2e `subscription-names` spec edits the
  email in place (refusal, Escape, save, removal by Enter, leaving an emptied field, the two-line
  format, focus back on the pencil) with no WCAG violation. It counts the changes the mock was
  sent, so "not sent" can fail. It holds the list's read to open a field while the read is on its
  way, waits until the list says a refresh waits, and saves by leaving. It checks a server
  refusal on both lines and an address outside ASCII. It saves with Tab and Shift+Tab a name
  that moves its card and asks where the keyboard is, holds a card's usage answer until its
  pencil has the keyboard, and measures a long address at 360 px. The `subscriptions` spec signs
  a subscription with a typed email in again and reads the toast.
- Gates: `scripts/dev ci`, the e2e suite, `scripts/dev security`.

## Review

- **Spec review** (a fresh session given only the spec path; verdict: revise). Its findings and
  what they changed:
  - (high) The draft quoted real addresses from the request and the screenshot, in a public
    repository: replaced by `example.com` ones, and the unpushed commits were rebuilt without
    them.
  - (high) The toast after a sign-in ("connected as *email*") was an unlisted reader that would
    present a typed email as Claude's: EML-006 and `AccountView.email_typed`.
  - (medium) Leaving an emptied field would have deleted the only copy of the address: removing
    takes Enter (EML-002).
  - (medium) A redraw while a field is open was not settled: EML-001's last sentences, with
    tests.
  - (medium) The context said ADR-0185 assumed Claude would give the email; it foresaw the
    refusal: corrected.
  - (low) The reworded "email unknown" hint in the Status panel went beyond the request:
    reverted.
- **Round 1** (REQUEST CHANGES on 377e9336; store, API, permissions and privacy held):
  1. (high, pre-existing) The whole e2e run was red on `cd.e2e.ts` "a refusal after its view was
     left…", and the note that pull request #1151 fixes it was wrong: #1151 merged without
     touching it. It is a load-dependent failure of a test this change does not touch (it passes
     alone, and failed before in another pull request's run). Filed as #1171; `dev` merged; the
     whole suite then passed (Evidence).
  2. (medium, own) An update that came while a field was open was never drawn after a save by
     leaving: the field that closes then has no focus to lose, and only a `focusout` read the
     waiting refresh. The page now reads it when the save settles. Test first: the initials kept
     "CS" beside the new email, and another administrator's rename stayed undrawn.
  3. (medium, own) Two "not sent" assertions could not fail, and no test showed a server
     refusal: the tests count the changes the mock was sent and read what it stored, and a
     refusal is tested on both lines.
  4. (low, own) An `email` field returned a domain outside ASCII as punycode: it is a text field
     with an email keyboard. Test first (`jan@bücher.example` came back as
     `jan@xn--bcher-kva.example`).
  5. (low, own) ADR-0300 said the list is never redrawn while a field is open, and the
     architecture said every write is one statement: both reworded. SPEC-243 LGN-003 and SPEC-244
     SUB-003 and SUB-006 now point here.
  6. (low, own) A test waited two animation frames for "the read has landed": it waits until the
     list says a refresh waits (`data-refresh="waiting"`).
- **Round 2** (REQUEST CHANGES on 16a1e1ec; the six answers of round 1 held):
  1. (high, own) A name saved with Tab moved the keyboard to another subscription. The list, read
     again once the save settles (round 1's fix), is sorted by name; the email pencil's name had
     changed with the subscription's, so `keepFocus` fell back to the control's place, where
     another card's control now was. The card's controls say whose they are (`data-focus-key`)
     and are found by that. Test first: the keyboard was on Claude #3's "Sign in again".
  2. (low, own) The spec said done while its review was open: this record, and Evidence for the
     new head.
  3. (low, own) The held-read test used the subscription whose usage the page reads as it loads;
     on another host that answer could mark the list waiting early. It uses a subscription whose
     usage is fresh.
- **The author's own probe after round 2** (keyboard and mouse on both lines, phone width):
  - (own, fixed test first) The redraw of a card's head after its usage answer dropped the
    keyboard from its pencil. An idle subscription's usage stays old, so on a real instance that
    answer follows every list read, and Tab after a saved name went back to the name's pencil.
    The redraw keeps the focus. `dev` has the same for the name's pencil.
  - (own, fixed test first) At 360 px a long address pushed its pencil onto a second line: the
    address is cut short of the pencil.
  - (pre-existing, #1174) A refusal shown as a field is left adds a line under it, so a press on
    its way to a control below it in the same card misses once. Measured on `dev`'s assets: with
    a name emptied, Remove moved 20 px and no dialog opened. The email's pencil is now such a
    control, and the email field refuses the same way. Not changed here: it is how every leave
    of these fields is handled, and nothing is lost by the miss.
- **Round 3** is the review of the head that carries this record; its verdict goes with the
  delivery.

## Evidence

- **Tests that fail without the change** (each was run red before its code was written):
  - Store: `a_typed_account_email_stays_until_its_person_removes_it` failed with Claude's answer
    in place of the typed email (`Some("claude@example.com")` for `Some("agent@example.com")`),
    and `emails_stored_before_one_could_be_typed_stay_claudes` for lack of the migration.
  - API: `an_administrator_types_changes_and_removes_a_subscriptions_email` and
    `a_typed_email_outlives_sign_ins_until_it_is_removed` failed with 422 for `email`; the
    `email_typed` assertions failed with `null` before the view carried it.
  - e2e: the email tests failed with no "Change the email of …" button; the sign-in toast test
    read "… is connected as agent@example.com"; "a field opened while the list is read again…"
    failed with the name field gone; the round-1 tests failed as its findings say.
  - e2e, round 2 and the probe: after a name saved with Tab the keyboard was not on "Change the
    email of Agent TEAM"; after the usage answer the pencil had lost the keyboard; at 360 px the
    pencil was on a second line (that test fails again with the `max-width` taken out).
- **`scripts/dev ci` at 23a11eab** exited 0: repository contracts, 152 script tests, check, fmt,
  clippy (Linux and macOS), nextest 3535 passed and 16 skipped, doctests, deny, shear, doc. That
  head has `dev` eb2e8a4e merged in, with its two conflicts resolved: the store port's imports,
  and the migrations list, where this change's migration goes after `dev`'s two as schema
  version 52. The head after it adds this record only.
- **Whole e2e suite at 23a11eab** (`cd e2e && npx e2e run`): 942 passed, 1 skipped, exit 0, at a
  load average of 50 to 60. Earlier whole runs: 935 passed at 4eeaedfc, where the two
  subscription specs (37 tests) also passed 6 runs of 6 alone; 929 passed at 1bd025b0; 916 passed
  at 377e9336 with the one `cd.e2e.ts` failure of #1171. The logs are in the worktree's ignored
  `target/`.
- **`yulius security-scan --base origin/dev`**: "Security scan disabled" (Vidoc is switched off
  for this project in Yulius). Disabled, not passed: nothing was scanned.
- **Seen in the browser** (the mock, `/admin/accounts`): a card renamed to "Agent TEAM" with
  "agent@example.com" typed under it reads those two lines, each with its pencil; "not an
  address" is refused under the field.
- **#1174 on `dev`'s own assets** (`git archive origin/dev crates/yulius-server/web`, served as
  they are, a Playwright script): a name emptied, then one click on Remove of the same card: the
  refusal shows, Remove is 20 px lower and no dialog opens. The same on this branch.
- **What could not be tested.**
  - No real Claude sign-in runs in this sandbox, so Claude's profile answer is a fake, as in
    SPEC-244. Whether Claude gives the email to a `setup-token` sign-in is known only from the
    production instance, which shows "email unknown".
  - The e2e suite is Chromium only; Firefox and Safari were not tried.
  - Production's data was not touched: its subscriptions get their emails when an administrator
    types them after the update.

## Decisions

- **A person may type the email; it reverses one alternative rejected in ADR-0185.** The owner
  asks for it and takes the risk ADR-0185 named, that a typo is shown as the account's address.
  EML-006 bounds it: Yulius never presents a typed email as confirmed by Claude. Reading the email
  from Claude's token exchange stays the way to know it for certain; it is not part of this
  change. Recorded in
  [ADR-0300](../../docs/decisions/0300-an-administrator-can-type-a-subscriptions-email.md).
- **The typed email lives in `accounts.email`, marked by a new `email_typed` column.** Every view
  already reads `email`, so no reader changes. Rejected:
  - *No marker.* The next sign-in would clear what the person typed, and a late answer from
    Claude would replace it.
  - *A second column shown in preference.* Every reader (views, task details, the analytics'
    SQL) would have to choose between two columns.
- **The store keeps the rule in SQL.** Its three writes (`set_account_email`,
  `set_account_credential`, the person's change) each settle a typed email in one statement, so
  an answer from Claude that arrives while a person types cannot replace it. The cost is one
  profile request per sign-in whose answer is not stored.
- **The marker reaches `AccountView` only.** The sign-in toast is the one place that says where
  an email comes from ("connected as"), and it reads the account view. The Status panel, the
  Dashboard, Task details and the analytics show the email as they did; telling the two apart
  there needs a field on four more views and is not asked for.
- **`PATCH /api/admin/accounts/{id}` is reused**, with `label` now optional. It is already
  limited to administrators, validated and audited. An empty string removes the email; JSON
  `null` means "not given", as for any optional field here.
- **One rule for an address.** The typed email passes `plausible_email`, the check Claude's
  answer passes (SUB-006); the page applies the same rule before sending, so a refused value
  costs no request. Nothing is sent to the address, so no stricter check is needed.
- **Removing needs Enter.** Leaving a field saves what was typed, as for the name, but an emptied
  field that is left removes nothing: the audit trail does not hold the address, so a slip would
  lose the only copy.
- **The address is not written to the audit trail** (`email=set` or `email=removed`): SUB-006
  keeps the email out of logs, and the audit row outlives the subscription.
- **Assumed (reversible): removing an email read from Claude asks Claude again**, so it may come
  back within minutes. The line shows what the administrator typed, else what Claude shares; a
  third state ("show none") is not asked for.
- **The email is edited only on the Subscriptions page**, where the name is. The Status panel and
  the Dashboard stay read-only, hint included.
- **One implementation for both lines.** The focus rules of the name edit took SPEC-244 three
  review rounds; the email line uses the same code, so they cannot drift apart.
- **The page holds its refresh while a field is open, and reads it when the save settles.** Two
  editable lines make "save one, go on to the other" the usual flow, and it hit two races of the
  name edit: a list read on its way redrew the field just opened (its blur saved what was half
  typed), and an update that came during a save by leaving was never drawn, because the field
  that closes then has no focus to lose. The loader takes a `hold` for quiet refreshes only; a
  reload the person starts still redraws. The list says that a refresh waits
  (`data-refresh="waiting"`), which is what the test waits on.
- **The email field is a text field with an email keyboard.** An `email` field returns its domain
  in punycode in Chromium, so the address would not be stored as typed.
- **A card's controls say whose they are.** A rename changes the names of a subscription's
  controls and its place in the list, the two things `keepFocus` went by. The pencils, the
  fields, Sign in again and Remove carry `data-focus-key` (the account and the control), and
  `keepFocus` looks for it first; controls without one are found as before. Rejected: restoring
  focus in the Subscriptions page itself, which would repeat what `keepFocus` is for.
- **A long address is cut short of its pencil** (`max-width` on the text), so the pencil keeps
  its place at phone width; the field shows the whole address.
- **The miss after a refusal on leave stays as it is on `dev`** (#1174): fixing it means doing
  every leave's work after the press has ended, a change to the name edit that was not asked for.
- **Initials are unchanged**: an avatar takes them from the email's name part when there is an
  email, else from the name.
- **Rollback is as for every schema change** (ADR-0264): the update restores the database it
  backed up when the new build fails to start. An older build refuses the newer schema, so
  undoing this change later means reverting its code and keeping the migration; the column is
  then unused.
