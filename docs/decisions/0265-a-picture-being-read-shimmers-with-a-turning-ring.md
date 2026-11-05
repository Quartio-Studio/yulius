# 0265. A picture being read shimmers with a turning ring, and is drawn only once decoded

## Status

Accepted. Specified in [SPEC-420](../../.ai/specs/SPEC-420-photos-dont-load-instantly-when-i-click.md);
task #402, issue #1065. Narrows [ADR-0073](0073-loading-shows-shimmering-fields.md) and
[ADR-0097](0097-loading-fields-take-the-shape-of-what-loads.md) for image tiles.

## Context

The "See the original prompt" dialog (SPEC-043, SPEC-180) put each image's `<img alt="name">` in
its tile before the file had been read, so for the whole read the browser painted its broken-image
icon and the file name as alt text; a file it could not decode stayed a broken image. The person
asked for a proper loading state with an animated icon. The UI's rule is a skeleton in the shape
of what loads, never a spinner with "Loading…" text, and a spinner only for an action under way.
A picture tile has no text whose shape a skeleton bar could stand for.

## Decision

- A picture being read is a tile of its final size with the skeleton's shimmer and a turning ring
  in its middle, busy (`aria-busy="true"`) and keeping its number; no text says "Loading".
- Its `img` joins the tile only once the picture is decoded (`HTMLImageElement.decode()` on the
  read's object URL). A file that cannot be decoded fails as a read does ("Couldn’t load it" in
  the list, "Couldn’t open …" for a placeholder), never a broken image.
- An `Image N` placeholder clicked while its picture is read swaps its icon for a ring and is busy
  until the preview opens or the read fails: an action under way, as ADR-0073 allows.

## Alternatives

- The skeleton alone: animated, but reads as an empty grey box; the person asked for a loading
  icon.
- An `img` hidden with `visibility: hidden` until `load`: still exposed by name to assistive
  technology and to the tests' `img` role before it shows.
- Faster reads instead (an in-page cache across openings, server-made thumbnails): changes the
  memory bounds of SPEC-080 or the server, and still needs a loading state for the first read.

## Consequences

- Each read now decodes the picture once before it is shown; the preview and the tile use the
  same decoded object URL.
- Other picture tiles that read over the network should follow this pattern; the composer's tray
  draws local files and already shows its uploads with a ring.
