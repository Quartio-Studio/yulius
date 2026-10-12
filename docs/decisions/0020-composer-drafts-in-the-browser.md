# 0020. New-task drafts live in the browser

## Status

Accepted. Specified in [SPEC-026](../../.ai/specs/SPEC-026-if-i-type-something-here-and-i-change-ta.md).

## Context

The new-task composer was rebuilt empty whenever a person opened another page and came back, so
a half-written task, its attached files and its choices were lost. Attached files are already
uploaded to the server when they are added (`attachments/drafts/<user>/`, removed after a week);
the browser only holds their paths.

## Decision

The composer's draft (prompt text, mode, workflow, "run after", start rule, and each attachment's
path, name, type, upload time and a scaled-down thumbnail) is kept per user and sandbox by the web
UI module `drafts.js`: in memory while the page lives, and in `localStorage` under
`yulius.draft.<user id>.<sandbox id>` so it survives a reload. It is cleared when the task starts
and removed at sign-out, like the notification history. Attachments older than the server's
draft age are dropped on read; thumbnails are bounded in number and size, and a full storage
falls back to no thumbnails, then to memory only.

## Alternatives

- A server-side draft API (a table and endpoints): drafts would follow a person across devices,
  but it adds schema, permissions and cleanup for unfinished input nobody asked to share.
- `sessionStorage`: dies with the browser tab, so a closed tab or a crash still lost the work.
- IndexedDB for the original image bytes: full-size previews after a reload, but more machinery
  than a preview needs; the server already holds the file.
- Keeping the composer view alive (not destroying it on navigation): only covers in-app
  navigation, not a reload, and keeps a hidden view with its listeners around.

## Consequences

A draft is per browser: another device or browser starts empty. Two tabs on the same sandbox each
keep their own draft and the last write wins in storage. Unfinished prompts stay on the device
until the task starts or the person signs out.
