# 0433. Store each new-task draft under its own key

## Status

Accepted. Extends [ADR-0021](0021-composer-drafts-in-the-browser.md) for [SPEC-594](../../.ai/specs/SPEC-594-i-should-be-able-to-create-many-drafts-l.md).

## Context

A person can have several unfinished tasks in the same sandbox. Some hold uploaded files, and an upload can finish after the person moves to another draft.

## Decision

Keep the original browser storage key for the first draft. Give each additional draft a random suffix under the same user and sandbox. Store the selected key in the tab's session storage. The composer, its pending writes and each upload hold the identity of the draft they edit. Successful start and confirmed discard broadcast that key to other tabs, including when an upload has begun on an empty draft. Clearing text is an ordinary edit. A retired editor moves to a surviving draft or a fresh identity.

## Alternatives

A single collection value would require rewriting siblings on every save and reconciling independent writes across tabs. A server-side draft collection would introduce synchronization across devices and a new persistence contract.

## Consequences

A pre-existing draft opens in place. Users can work on distinct drafts in different tabs. Starting or discarding a task removes that draft's entry. Existing attachment retention, storage quota fallback and sign-out cleanup apply to every draft. Concurrent edits to the same draft retain the last-write behavior of ADR-0021.
