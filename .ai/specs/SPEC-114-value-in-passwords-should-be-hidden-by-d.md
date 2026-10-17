---
yulius-task: 390df895-a86d-4902-9e83-893ace623a66
status: done
---

# SPEC-114: a shown secret value stays masked until the eye is pressed

Status: done
Date: 2026-10-14
Owner: Yulius task #99
Scope authorization: the person's request (task #99), "Value in passwords should be hidden by
default even after show, and shown after explicitly clicking eye icon. On hover there should be
option to copy + the current copy button".

## Goal

On the Secrets page, the dialog that **Show** opens (team secrets, and agent secrets for the people
who manage them) shows a secret's value masked. The value appears only after the person presses the
eye button beside it, and hides again when they press it once more. Hovering the value (or moving
the keyboard into it) shows a copy button inside the value box. The current **Copy** button stays.

## Context

- `crates/yulius-server/web/assets/pages/secrets.js`, `showRevealed()`: after the two-factor code
  (SPEC-040, VLT-002) it opens a dialog whose Value field is `<code class="secret-value">` with the
  plain value, beside a `Copy` button (`aria-label="Copy value"`), and the notes below. Anyone
  looking at the screen sees the value as soon as the dialog opens, including during a screen share.
- The two-factor setup dialog shows the authenticator key (`setup.secret`) with its own Copy
  button. The person has to read or copy that key into an app once, and it is not a stored
  password, so it is out of scope (see Decisions).
- Existing hover-revealed controls (`.task-quick-archive`, `.tl-actions`) use
  `visibility: hidden` turned `visible` on `:hover, :focus-within`. Hidden that way, a control is
  also out of the tab order until something else in its group has focus.
- `dom.js` has an `eye` icon (composer Preview) but no `eye-off` or `copy` icon.
- e2e `secrets.e2e.ts` asserts that the value is visible in the dialog right after the code
  (`shown.getByText(TEAM_VALUE)`, `AGENT_VALUE`). That changes with this feature.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PWV-001 | The Show dialog's Value opens masked: a fixed run of dots that does not reveal the value's length, and the value is not in the page's DOM until it is revealed. Screen readers hear "Value hidden". | e2e |
| PWV-002 | An eye button in the value box (`aria-label="Show value"`, `aria-pressed`) reveals the value; pressed again it masks the value and removes it from the DOM. Every new Show dialog opens masked again. | e2e |
| PWV-003 | *Superseded by SPEC-207 (ADR-0164): the copy icon is always visible in the box.* Hovering the value box, or focus inside it, shows a copy icon button (`Copy`) in the box; it copies the value whether masked or not, with the same toast and 30-second clipboard clearing as the Copy button. Without hover or focus it takes no space a click could reach. | e2e |
| PWV-004 | *Superseded by SPEC-207 (ADR-0164): the box's copy icon is the only copy.* The current `Copy` button (`Copy value`) stays beside the box and behaves as before. | e2e |
| PWV-005 | The dialog has no WCAG A/AA violations masked or revealed, in both themes; the box fits a 390 px window. | e2e (axe) |
| PWV-006 | SPEC-001 (VLT-010), the product summary, a decision record, `e2e/README.md` and `docs/changes.md` describe it. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A shown secret value opens masked and the eye button reveals and hides it
- [x] Hovering or focusing the value offers a quick copy beside the kept Copy button
- [x] Docs, decision record and changes log describe the masked value
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-PWV-1: after the two-factor code, the team secret's Show dialog contains no `TEAM_VALUE` in
  `document.body.innerHTML`, shows the dots, and has a `Show value` button with
  `aria-pressed="false"`; the same holds for an agent secret (`AGENT_VALUE`).
- AC-PWV-2: clicking `Show value` makes the value visible and `aria-pressed="true"`; clicking again
  removes it from the DOM; closing and opening Show again starts masked.
- AC-PWV-3: the quick copy button is not visible before hover; on hover of the value box it is
  visible, and clicking it while masked puts the value on the clipboard (the stubbed
  `navigator.clipboard.writeText` receives it) and shows "Value copied"; focus on the eye button
  makes it visible too.
- AC-PWV-4: the `Copy value` button still copies the value.
- AC-PWV-5: axe reports no violations in the dialog, masked and revealed.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e `e2e/tests/secrets.e2e.ts` (mock mode): a new test "a shown value stays masked until the eye
  is pressed, and copies on hover" covering AC-PWV-1..5 for a team secret, and the agent-secret
  test changed to assert it opens masked and reveals with the eye. Red first: today the value is in
  the DOM as soon as the dialog opens and there is no `Show value` button.
- The clipboard is stubbed in the page (`navigator.clipboard.writeText` records its argument), so no
  browser permission is needed.
- No server change: the reveal API, its audit (`vault.team.reveal`, `vault.agent.reveal`) and the
  two-factor unlock are unchanged.

## Decisions

- **Mask in the dialog, keep the reveal request.** The value is still fetched (and audited as a
  reveal) when Show is pressed; the eye only toggles what the page draws. Fetching on the eye
  instead would add a second request and a second audit entry for one look, and the Show button
  already names the action. ADR-0093 records it.
- **Fixed-length mask** (12 dots), so the mask does not tell the value's length.
- **The value leaves the DOM when masked**, not just hidden by CSS: a text selection, find-in-page
  or an extension then cannot read it.
- **The eye is a toggle button with a constant name** ("Show value", `aria-pressed`), as ARIA
  recommends for toggles, with its icon switching between eye and eye-off and its tooltip saying
  "Show value" or "Hide value".
- **The quick copy is a real button** named "Copy", visible on `:hover, :focus-within` of the value
  box like the task row's quick Archive; keyboard users reach it after the eye button. The kept
  button keeps its name "Copy value", so the two stay distinct for assistive technology.
- **Notes stay shown** as today: the request is about the value, and notes hold the user name and
  instructions people read alongside it.
- **The two-factor setup key stays shown**: it is not a stored password and the person has to
  read or copy it into an authenticator once.
- **Review round 1 (approved at aba21f22).** The two minor findings were taken: the test now runs
  axe in the light theme too (masked and shown) and presses Tab from the eye to reach the quick
  copy; with `:focus-within` dropped from the CSS that test fails. Nits not taken: the tooltip
  says "Hide value" while pressed with the name constant, as chosen above; the eye is built by
  hand because it redraws its own icon and title; on touch screens the quick copy shows once the
  eye is tapped (focus in the box), and the always-visible Copy button covers a direct copy, so
  no `hover: none` rule is added.

## Evidence

- Red first: with the new tests on the code of `dev`, `npx e2e run tests/secrets.e2e.ts` failed 3
  of 8 (the value was in the page on opening, no `Show value` button); after the masking slice
  only the quick-copy assertion failed; after the quick copy, 8 of 8 passed.
- Mutation: with `.secret-box:focus-within .secret-quick-copy` removed from the CSS, the new test
  fails (`toBeVisible failed`).
- `scripts/dev ci` (repository check, fmt, clippy, 1604 nextest tests, doctests, deny, shear,
  doc): exit 0. `python3 scripts/check_repository.py` at the final head: passed.
- `E2E_TELEMETRY_DISABLED=1 npx e2e run` (full suite): 45 files passed, 1 skipped (`agent.e2e.ts`
  needs a model); 338 tests passed.
- Screenshots of the trace (1440 px masked on hover, 390 px shown) checked by eye: dots, eye, quick
  copy and Copy inside the dialog.
- Independent review in a fresh session: round 1 approved aba21f22 with two minor test gaps,
  fixed in 792841e2; round 2 approved 792841e2 with no findings to fix.
- Not tested here: a real clipboard (the tests stub `navigator.clipboard.writeText`) and real touch
  devices.
- Yulius CI at 5bf67ccf (after clean merges of `dev`): run 8e89f8af failed only on the known flake
  #394 (`terminal.e2e.ts`, Monaco under load; 3 of 3 local runs passed), and its rerun 11f0fcf8
  failed only on `terminals.e2e.ts` reading `window.__yuliusMock` before the mock installed.
  `seedTwoShells` now waits with `mockReady()`, as `support.ts` documents for any test that
  touches the mock right after `app.open`.
