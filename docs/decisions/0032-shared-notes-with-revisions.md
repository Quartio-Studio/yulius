# 0032. Shared notes are stored on the server and saved by revision

## Status

Accepted. Specified in [SPEC-041](../../.ai/specs/SPEC-041-here-we-want-auto-saved-notes-sandbox-wi.md).

## Context

People asked for auto-saved notes, "sandbox-wise and team-wise": one notepad per sandbox and one
for the whole team, in the tools rail beside Browser, Editor and Shell. Notes are shared, so a
teammate must see what another wrote. Two people may type at the same time. Yulius serves a team
of 2 to 5 people from one SQLite database ([0004](0004-sqlite-wal-store.md)).

## Decision

Each notepad is one row of a `notes` table, keyed `team` or `sandbox:<sid>`, holding the whole
text (at most 256 KiB) and a revision number. The web UI saves the whole text 600 ms after the last
keystroke and when the text box loses focus, the panel hides or the page is hidden. Each save names
the revision it started from, and the store writes it only if that revision is still current
(compare-and-set), so a stale save is refused with 409 and stores nothing. The UI then loads the
current note. If it has the same text it is adopted; otherwise the person picks Keep mine or Use
theirs. Every stored edit publishes `note.updated`, and an idle notepad (nothing unsaved) loads the
new text. Sandbox notes follow GitHub access: read access reads them, write access edits them. Team
notes are shared by everyone signed in.

## Alternatives

- Real-time co-editing (a CRDT or operational transforms over a socket): characters merge live,
  but it needs a library, a new protocol and cursor state for a handful of people who rarely type
  in the same notepad at once.
- Last write wins without revisions: simpler, but a teammate's text could vanish silently.
- Browser storage, as for the composer's drafts ([0021](0021-composer-drafts-in-the-browser.md)):
  not shared, so not team notes.
- Notes as files in the repository: versioned, but agents and commits would pick them up, and the
  team notepad belongs to no repository.

## Consequences

One extra small table and four routes. History is a counter, not versions: an overwritten text is
recoverable only from the person's own screen at the moment of the conflict. Two people typing at
once get the conflict choice instead of a merge. A note's size is bounded and there is one row per
notepad (one for the team and one per sandbox ever written; a removed sandbox's notes stay, like its
other history). Unsaved text that a hidden page could not send is kept in that browser tab's
sessionStorage and offered on the next load.
