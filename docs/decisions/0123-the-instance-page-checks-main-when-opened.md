# 0123. The instance page checks main when opened

## Status

Accepted. Specified in [SPEC-149](../../.ai/specs/SPEC-149-it-should-autocheck-automatically-fix.md);
task #143. Amends how SPEC-001 OPS-006 checks `main` (every hour and on "Check now"); OPS-006's
"nothing updates by itself" is unchanged.

## Context

Settings → Instance showed the server's last check of `main`, up to an hour old ("Checked 35 min
ago"), and checked again only when the superadmin clicked "Check now". A person who opens the page
right after a merge to `main` wants to know whether it is on offer, and was told "up to date".

## Decision

- Opening the instance page as superadmin sends the same check as "Check now"
  (`POST /api/admin/update/check`). The server reuses a check younger than 30 s, so reopening the
  page costs no new GitHub compare within 30 s.
- That automatic check is quiet when its request fails: the page keeps the last result and raises
  no toast, as nobody asked. A GitHub read that fails is still said, as the view's error banner.
  "Check now" keeps saying a failed request.
- The hourly server check stays, for the notification and the banner when nobody looks.

## Alternatives

- **Check more often on the server (every few minutes)**: spends GitHub calls whether anyone looks
  or not, and the page still shows a time minutes old.
- **Check only when the cached result is older than some age**: a second threshold beside the
  server's 30 s reuse, for no saving the reuse does not already give.
- **Keep checking while the page stays open**: the hourly check and the `update.updated` event
  already move an open page when the offer changes.

## Consequences

- One GitHub compare per instance page visit, reused for 30 s. Checks sent at the same moment (two
  tabs, or during the hourly check) may each compare once, as "Check now" in two tabs already
  could (#502).
- With `YULIUS_UPDATE_CHECK_MINUTES=0` the page still checks when opened.
