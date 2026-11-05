---
yulius-task: 855e4f28-7025-4aa9-83d1-6eeb6c612caa
status: in-progress
---

# SPEC-420: the original prompt's images show an animated loading tile, never the browser's broken image

Status: in progress
Date: 2026-11-05
Owner: Yulius task #402
Scope authorization: task request: "photos dont load instantly when i click 'see prompt' - fix or
make sure loading is properly, like loading icons etc, not default html icon", added before the
start: "animated icon".

## Goal

While an image of the "See the original prompt" dialog (SPEC-043, SPEC-180) is being read, its
thumbnail is a shimmering tile with an animated loading icon, and an `Image N` placeholder clicked
meanwhile turns its icon into a spinner, so the person sees the picture is on its way. The
browser's broken-image icon and the file name drawn as alt text never show, also for a file the
browser cannot draw.

## Context

Reproduced in the mock with `hold=/api/tasks/tsk_studio_12/prompt/attachments/0` and in the
person's screenshot (task #94's prompt): each list tile is `<img alt="pasted-….png">` with no
`src` until the whole file has been read as a blob (`openPromptDialog` in `actions.js`). An `img`
without a source paints the broken-image glyph and its alt text, so for the read's whole length
(a full-size screenshot, `Cache-Control: no-store`, read under the bounded read slots of SPEC-080)
the tile shows the file name wrapped over three lines and a broken icon in its corner.

Root cause: the tile has no loading state; the `img` is in the DOM before it has anything to draw.
The same path also leaves a file the browser cannot decode as a broken image (its `error` event is
not handled), and an `Image N` placeholder clicked during the read gives no feedback until the
preview opens. The e2e tests only looked at the picture after the read (`toBeVisible`), never at
the tile while it is read.

What else the cause could break: the full-size preview opens only after the read, from a decoded
object URL, so it is not affected; the composer's tray draws local files or stored thumbnails and
already shows `Uploading…` with a spinner (`pi-spinner`).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PIL-001 | While an image is read, its list tile is busy (`aria-busy="true"`), shows a shimmering background and a turning ring (an animation, never static), keeps its number, and holds no `img`, so neither alt text nor a broken-image icon can paint. | e2e `original-prompt.e2e.ts` |
| PIL-002 | The picture appears once the browser has decoded it (`HTMLImageElement.decode()` on the read's object URL), and the tile is no longer busy. | e2e |
| PIL-003 | A file the browser cannot decode is shown as a file that "Couldn’t load it", as a failed read is, never as a broken image; its placeholder says "Couldn’t open …: it is not a picture the browser can show.", and its object URL is revoked at once. | e2e (a `.jpg` the mock serves as text) |
| PIL-004 | An `Image N` placeholder clicked while its image is read shows a turning ring in place of its icon and is busy until the preview opens or the read fails. | e2e |

## Plan

- [x] Spec with the root cause of the alt-text thumbnails
- [x] A failing e2e test: a held image shows an animated loading tile, never alt text
- [x] Thumbnails and Image N placeholders show an animated icon while their image loads
- [x] Docs, ADR and changes record the loading tile
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green with dev merged

## Acceptance criteria

- AC-1: with the first image's read held, the dialog's list tile for it is `aria-busy="true"`,
  shows its number and a ring whose computed `animation-name` is `spin`, and holds no `img`; after
  the release the `img` is visible and the tile is not busy.
- AC-2: an attached `.jpg` whose bytes are not an image reads "Couldn’t load it" in the list and
  shows no `img`.
- AC-3: clicking `Image 1` while its read is held makes the placeholder busy with a turning ring;
  after the release the preview opens and the placeholder shows its image icon again.

## Test plan

- e2e `original-prompt.e2e.ts`: a new test for AC-1 and AC-3 with `hold=` on the image's read, and
  one for AC-2 with a `.jpg` the mock serves as text; both fail on dev (an `img` with alt text is
  in the tile, the broken `.jpg` keeps its `img`).
- The existing dialog tests keep passing (thumbnails visible after the read, one read per file,
  retries after a failed read).

## Decisions

- An animated icon inside a skeleton tile, not the skeleton alone: SPEC-092 says loading content is
  a skeleton without a spinner, but the person asked for an animated loading icon here, and a
  picture tile has no text whose shape a skeleton could show. The tile keeps the skeleton's shimmer
  and adds a ring, with no "Loading…" text (ADR-0265).
- The `img` joins the tile only once the picture is decoded, not before with `visibility: hidden`:
  an `img` left in the DOM is still exposed by name to assistive technology and to the tests' `img`
  role while it is not shown.
- No in-page cache of read images across openings and no server thumbnails: both would make
  reopening faster but change memory bounds (SPEC-080) or the server; the loading state is what
  the request needs. Reads stay one per file per opening.
