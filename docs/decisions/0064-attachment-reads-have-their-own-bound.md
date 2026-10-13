# 0064. A prompt's attached files are read under their own bound, streamed, and confirmed on Linux

## Status

Accepted. Specified in
[SPEC-080](../../.ai/specs/SPEC-080-fix-303-original-prompt-thumbnails-alway.md). Amends
[ADR-0033](0033-original-prompt-on-demand.md) (#303).

## Context

ADR-0033 served a prompt's attached file under a slot of the uploads' bound: 8 at once, failing
at once when full, the file read whole into memory and the slot released before the bytes were
sent. The "Original prompt" dialog loads every thumbnail at the same time, so a prompt with more
than 8 images, or one opened while uploads ran, got 429 for some and showed them as broken for
good. The slot bounded neither memory nor the responses in flight. ADR-0033 also served a file
whose `/proc/self/fd` link could not be read, which on Linux silently skipped the check against a
folder swapped for a link between the checks and the open.

## Decision

- Reads of attached files have their own bound, 16 at once. A read waits up to 30 s for a slot
  before it answers 429 `capacity`; uploads keep their own bound of 8.
- The file is streamed in chunks of at most 64 KiB, exactly the length checked when it was opened
  (`Content-Length`), while its read slot is held. A response left unread for 60 s (the uploads'
  idle bound) gives its slot back and breaks off.
- On Linux an opened file whose `/proc/self/fd` link cannot be read is refused (404). Elsewhere
  there is no such link, and serving it stays as ADR-0033 records.
- The opening and the checks on disk run off the async runtime.

## Alternatives

- The dialog loads images one after another and retries on `capacity`: slower for every prompt
  with images, and it still competes with uploads; a single tab cannot fix another tab's load.
- Keeping the shared bound with a wait: uploads and reads would still starve each other, and a
  long upload would hold up thumbnails.
- Reading the whole file under the slot, then sending: memory per response up to the upload
  limit (25 MiB), 8 or 16 times over.
- Releasing the slot after the open and streaming without it: responses in flight, and their open
  files, would be unbounded.

## Consequences

- Thumbnails load together and wait briefly when many dialogs open at once; a 429 now means 16
  reads held their slots for 30 s.
- All attached-file responses in flight hold at most 16 × 64 KiB of file data.
- A client that stops reading holds a slot for at most 60 s.
- A Linux host without `/proc` mounted serves no attached file.
