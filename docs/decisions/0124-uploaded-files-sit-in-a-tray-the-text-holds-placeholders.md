# 0124. Uploaded files sit in a tray; the text holds placeholders

## Status

Accepted. Specified in
[SPEC-150](../../.ai/specs/SPEC-150-fix-the-ux-i-d-expect-uploading-images-a.md). Refines
[0101](0101-attached-files-keep-their-place-in-the-prompt.md) (the composer holds files inside its
text); the prompt format, the server and what the agent gets are unchanged.

## Context

With 0101 an image pasted into the new-task prompt became a 64 px high thumbnail inside the
line, so one line of words grew to the image's height. The only way to see the image large was a
double-click on it or the Preview. Deleting the thumbnail deleted the upload. The person asked
to upload images first and then position placeholders, and to keep zooming the image.

## Decision

- **A tray above the text holds every uploaded file**, in upload order: an image as a numbered
  thumbnail that opens large, another file as a tile with its name. × takes the file out of the
  tray and its placeholders out of the text, one undo step of the prompt.
- **The text holds a placeholder one line high**: `Image N` (N the image's number in the tray,
  like Claude Code's `[Image #N]`) or a file's name. It is still the 0101 atom: it moves by drag
  or by cut and paste, and a paste or drop still puts it at the caret.
- **A tray tile positions its file**: dragged into the text, the file's placeholder lands at the
  drop point (moved there when the text holds it, inserted otherwise); a file not in the text has
  a button putting it at the caret left in the prompt.
- **A file without a placeholder stays attached** and is sent after the text, as an `@path` line,
  which is how every older prompt reads; the draft's `attached` is the tray.
- **The image dialog fits the image to the window**, and a toggle ("Actual size", or a click on
  the image) shows it at its own size, scrollable.

## Alternatives

- **Thumbnails inside the text, only smaller.** It keeps one gesture, but a picture a line high is
  unreadable, and seeing it large still needs a hidden double-click.
- **Upload to the tray only and leave placing to the person.** It matches the request's word order
  literally, but the caret's place at a paste is usually the place meant (0101's reason), and an
  unplaced file now loses nothing: it goes after the text.
- **A zoom slider or pinch handling in the dialog.** It is more than "I still want to zoom" needs;
  fit and actual size cover screenshots, and phones pinch the page as before.

## Consequences

- Deleting a placeholder no longer deletes the upload (0101 / SPEC-124 INL-005 said it did); only
  the tray's × does.
- An older draft's files show in the tray instead of being appended to its text.
- Late uploads (ending after the person left the composer) join the tray, not the text.
- The tray hides while the Preview shows: the Preview draws each placed file in the formatted text.
