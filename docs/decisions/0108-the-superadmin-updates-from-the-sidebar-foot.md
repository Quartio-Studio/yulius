# 0108. The superadmin updates from the sidebar foot

## Status

Accepted. Specified in [SPEC-133](../../.ai/specs/SPEC-133-upgrade-button-should-be-somewhere-here.md);
task #121.

## Context

Updating Yulius (SPEC-001 OPS-006..OPS-009, SPEC-002 ADR-18) was announced by a pill in the tab
row, dismissible per commit, and done on the instance page, reached from Settings → Instance.
Once the pill was dismissed, or before an update was offered, the only way to update was through
Settings. The request: "Upgrade button should be somewhere here right in the superadmin, not
hidden deeply in settings", with a screenshot of the home screen whose sidebar foot shows the
account button "Superadmin".

## Decision

- The superadmin has a button beside the account button in the sidebar foot, on every screen.
  When main has moved on it reads **Update** in the accent tone, and one click opens the
  instance page's confirmation (`confirmAndStartUpdate` in `updates.js`, shared with "Update
  now…"). While an update runs it reads **Updating** with a spinner; otherwise it is a quiet icon
  named "Version and updates". Both open the instance page. Without the update helper the click
  opens the instance page, which explains why.
- The pill stays the announcement and can be dismissed; the button cannot.
- The account menu's Administration section lists "Version and updates" for the superadmin.
- The action keeps its name, "update", although the request said "upgrade".

## Alternatives

- **A button in the home screen's header** (next to New playground). Only on the home screen, and
  next to actions on sandboxes, not on the instance.
- **The pill made undismissable.** It lives only while an update is on offer, competes with the
  tab row's width, and the person asked for a button, not a louder announcement.
- **The button leads to the instance page instead of the confirmation.** One more click and a
  page change for the common case; the confirmation already says what happens, and the commit
  list stays one click away.
- **"Upgrade" as the label.** A second word for the action that the pill, the page, the dialog,
  the notifications and the API call "update" would read as a second feature.

## Consequences

- The sidebar foot holds two controls for the superadmin: the account name ellipsizes first, down
  to the 200 px sidebar.
- Starting an update no longer requires seeing the commit list; the dialog shows their summary.
- Instance administrators and members see no change: updates stay the superadmin's.
