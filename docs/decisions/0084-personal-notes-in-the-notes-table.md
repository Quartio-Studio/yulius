# 0084. Personal notes live in the notes table and their events reach only their owner

## Status

Accepted. Specified in [SPEC-103](../../.ai/specs/SPEC-103-add-me-after-team-that-is-for-user-and-n.md).
Extends [0032](0032-shared-notes-with-revisions.md).

## Context

People asked for a third notepad in the rail's Notes, "Me", after Team: "for user and not visible
for others but sandbox-agnostic". Notes are stored on the server by revision (0032), and the event
stream (`WS /api/events`) could only scope an event to a sandbox's viewers, to administrators or to
everyone signed in; nothing could reach one person.

## Decision

A personal notepad is the row `user:<user id>` of the existing `notes` table, read and saved
through `GET/PUT /api/me/notes`, which always resolve to the caller: no route names another
person's notepad, administrators included. It keeps the notes contract (revision, 409, size
limit) and the web UI's notepad controller. A stored edit is published with
`EventBus::publish_to_user`, which sets the new `Envelope.user`; the stream drops an envelope whose
`user` is someone else, so the owner's other tabs and devices reload an idle notepad.

## Alternatives

- Browser storage, like the composer's drafts (0021): private for free, but per browser, so not the
  same text on another device; "sandbox-agnostic" for a person means everywhere they work.
- A `/api/users/{id}/notes` route with an owner check: one more authorisation to get right, for no
  use, since nobody else may ever read it.
- No event for personal notes: the person's other tab would learn of an edit only through a 409
  conflict instead of a live reload.
- Keyed by GitHub login: the superadmin may have none, and a login can be renamed.

## Consequences

No migration and one more row per person who writes personal notes (users are disabled, never
deleted, so a row stays with its owner). "Not visible to others" holds for the API and the event
stream; the instance operator, who holds the SQLite file, can read it like every other note.
Events can now be addressed to one person, which later per-person features can reuse.
