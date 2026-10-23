# 0211. GitHub App creation opens a new tab, and setup ends with the first subscription

## Status

Accepted. Specified in [SPEC-283](../../.ai/specs/SPEC-283-frontend-onboarding-flow-and-github-inte.md);
task #269. Extends [SPEC-246](../../.ai/specs/SPEC-246-onboarding-requires-a-connected-claude-a.md) (Connect Claude first) and the setup wizard of
ACC-001..003.

## Context

Creating the instance's GitHub App posts a manifest form to GitHub. The form was submitted in the
Yulius tab, so GitHub replaced Yulius, the callback continued to the App's installation page, and
the person had no way back. After setup, the wizard ended on Home without saying that a Claude
subscription is needed before anything runs, or that GitHub is optional.

## Decision

- The click on Create GitHub App on GitHub opens a blank tab synchronously (inside the user
  gesture, so pop-up blockers allow it), sets its `opener` to `null` (GitHub's pages get no handle
  on Yulius), then writes the manifest form into that tab and submits it there once the server
  answered. A blocked tab posts nothing and says to allow pop-ups.
- The Yulius tab shows a waiting card and reads `GET /api/admin/github` when it becomes visible
  and every 5 s while visible, for at most 15 minutes and only while the page is open; when the
  App exists it shows the connected card with the next steps and updates `me`.
- The setup wizard numbers its steps and ends with "Add your first Claude subscription": Add
  subscription finishes setup on Subscriptions with the sign-in open; Skip for now on Home.
  Home's empty view lists the getting-started steps with their state.

## Alternatives

- Same tab with a `setup_url` back to Yulius: GitHub only uses it after the installation, not
  when the person stops at the App form, and a changed manifest affects existing Apps' setup.
- A sized pop-up window: blocked more often, awkward on phones, and still needs the waiting card.
- Embedding the subscription sign-in terminal in the wizard: duplicates the Subscriptions dialog
  for one screen; ending setup there reuses it.

## Consequences

- No server change: the manifest, callback and installation redirect stay as they are; the new
  tab ends on GitHub's installation page, and the person returns to the Yulius tab.
- The waiting poll is bounded (5 s, 15 minutes, visible tab only, cancelled with the page).
- Tests stub `window.open` in the mock to see the new tab without reaching GitHub.
