---
yulius-task: d9335b0e-efd8-4e66-8baa-b8097fa80616
status: done
---

# SPEC-101: the session's spider moves, its web stays in view, and a cockroach runs by

Status: implemented
Date: 2026-10-14
Owner: Yulius task #86
Scope authorization: user request (task #86): "spider should be animated and the web should be
sticky (should stick to the top while scrolling). And sometimes pixelart cockroach should run
through the screen in this view". The attached screenshot shows a session's well (SPEC-075) with
the pixel-art web and its spider in the top-right corner, under the session's header.

## Goal

The read-only session view (SPEC-075) feels a little more alive and a little more abandoned: the
spider climbs its thread and drops back now and then, the web stays in the top-right corner of
the view while the transcript scrolls, and every so often a pixel-art cockroach scurries across
the session. None of it moves for people who asked for reduced motion, and none of it gets in
the way of reading, selecting or using the keyboard.

## Context

- The web is `spiderWeb()` in `crates/yulius-server/web/assets/pages/sessions.js`: an inline SVG
  (`svg.ss-web`, 28×34 pixels drawn as `rect` runs, `shape-rendering: crispEdges`) floated right
  at the top of the well `.ss-tui`, `aria-hidden`, `pointer-events: none`, threads in
  `--session-web` and the spider in `--session-dim`. The first lines wrap beside it (SPEC-075
  STU-005, e2e "a session ends with a locked prompt box and has a spider web in its corner").
- The page scrolls in `.ss-page` (`.page { overflow: auto }`); `.ss-tui` is `position: relative`,
  `display: flow-root` and fills the content area edge to edge (SPEC-086). Entries are
  `position: relative` with `content-visibility: auto`.
- Motion tokens and the global reduced-motion rule are in `tokens.css` and `styles.css`
  (`@media (prefers-reduced-motion: reduce)` collapses every animation to one instant
  iteration); components that loop also set `animation: none` there.
- `mountSessionView` returns `destroy()`, called when the page leaves; timers must stop there.
- The e2e harness (`@e2e-dev/web`) has no media emulation; `addInitScript` runs before the
  page's scripts, and pure exports of `pages/sessions.js` are tested in the page (`metaLines`).

## Requirements

SPEC-109 (task #94) supersedes SPD-001's stuck colour: the web keeps `--session-web` and
`--session-dim` when stuck, and `--session-web-stuck`, its observer and the mark are gone
(ADR-0088). The mark also lifted the web 16 px above the well's corner, which SPEC-109 fixes.

| ID | Requirement | Evidence |
| --- | --- | --- |
| SPD-001 | The web sticks: while the transcript scrolls, the web stays at the top-right corner of the visible part of the well (`position: sticky; top: 0` on the float), and it never leaves the well. Text keeps wrapping beside it at the top, paints over it further down (everything after it is positioned), and it never catches the pointer. While it is stuck under the text, its threads and spider fade to `--session-web-stuck`, on which every text colour of the well is at least 4.5:1 in both themes (superseded by SPEC-109: it keeps its colours). | e2e, repository check |
| SPD-002 | The spider is animated: on a loop of about 8 s it hangs still, climbs 6 pixels up its thread in pixel steps (the thread shortening with it), hangs, and drops back, its legs switching between two frames while it moves. It rests where SPEC-075 drew it. Under reduced motion it does not move. | e2e (animations sought to a time, the reduced-motion rule) |
| SPD-003 | Now and then a cockroach runs through: a pixel-art cockroach (inline SVG, `aria-hidden`, `pointer-events: none`, legs in two frames) crosses the visible part of the well from one side to the other in about 2–4 s, left to right or right to left, at a random height, and is removed when it is out. The first comes 20–60 s after the session opens, then one every 45–150 s. None runs under reduced motion or while the page is hidden; none is left or scheduled after the view is left. The well clips it, so it never adds a horizontal scroll bar. | e2e (the scheduler and runner as pure exports in the page, and the mounted view) |
| SPD-004 | SPEC-001 UI-049, an ADR, `docs/changes.md`, the web README and `e2e/README.md` describe it. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The corner's web sticks to the top of the view while the transcript scrolls
- [x] The spider climbs its thread and wiggles its legs, still under reduced motion
- [x] Now and then a pixel-art cockroach runs across the session, never under reduced motion
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-SPD-1: after scrolling the mock's long session down, the web's top is within 2 px of the
  scroll area's top and its right within 2 px of the well's right; at the top it still sits in
  the well's corner with the first lines beside it.
- AC-SPD-2: the spider and its thread have running animations; sought to the climb's top, the
  spider is 6 pixels (18 CSS px at 3 px per pixel) higher and the thread 6 pixels shorter, and
  the second leg frame shows mid-climb; at rest it is where SPEC-075 drew it. The stylesheet stops
  both under `prefers-reduced-motion: reduce`.
- AC-SPD-3: `cockroachDelay` gives 20–60 s first and 45–150 s after; `runCockroach` adds one
  `aria-hidden`, `pointer-events: none` SVG inside the visible part of the well that crosses it
  and is gone when its run ends, and adds none under reduced motion or a hidden page;
  `scheduleCockroaches` runs one per timer and stops for good on `stop()`; leaving the session
  view stops it. No WCAG violations.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e `e2e/tests/sessions.e2e.ts` (mock mode), each red before its slice: the sticky web after
  scrolling; the spider's animations sought with the Web Animations API; the cockroach's pure
  exports with an injected random source and timer, and the mounted view with reduced motion
  stubbed through `addInitScript` (`matchMedia`).
- No server change.

## Decisions

- The web sticks as it is, a float with `position: sticky`: the float keeps the first lines
  beside it at the top, and the sticky offset keeps it in the corner of the visible part of the
  well without a second copy or a scroll listener. Further down, the text (positioned entries,
  later in the document) paints over the dim threads, so reading is not hindered. See ADR-0082.
- The spider's motion is CSS keyframes on SVG groups in pixel `steps()`, not SMIL or a script:
  it stays crisp, costs no timer, and the reduced-motion rule stops it like every other loop.
  The thread shortens with `scaleY` on its own rect (`transform-box: fill-box`), so each step
  is a whole pixel.
- The cockroach is a script on a random timer with the Web Animations API, because its path
  depends on the well's visible size and a random height and direction. It is checked against
  `prefers-reduced-motion` and `document.hidden` each time, and the timer stops with the view.
- Stuck under the text, the web fades (review round 1): text crossing its threads fell to 2.9:1
  (`--session-dim`, dark). An IntersectionObserver on a mark at the well's top sets
  `.ss-web-stuck` and the web takes `--session-web-stuck`, which the repository check holds to AA
  against the well's text colours. CSS `scroll-state(stuck)` queries were not used because they
  cannot style inside the SVG reliably and are Chromium-only.
- Review round 2 (fresh session): APPROVE at d2931614 with minors, fixed test-first. The mark sat
  below the well's padding, so the web stuck about 16 px before it faded; it now sits at the
  well's top. The observer reads its latest entry. Right-to-left runs are tested.
- The well clips horizontally (`overflow-x: clip`), not `hidden`: `clip` makes no scroll
  container, so the web's sticky position still follows the page's scroll.

## Evidence

- Red first, `e2e/tests/sessions.e2e.ts` (mock mode). Each test failed before its change, for the
  stated reason:
  - The sticky test: the web scrolled away, its top 47 px above the view.
  - The spider test: there was no `.ss-web-silk` (no animations).
  - The three cockroach tests, run against the previous assets: `cockroachDelay is not a function`,
    and no timer was set in the session view.
  - Review round 1: the faded-colour test (still `--session-web`) and the lowest run (421 > 420).
  - Review round 2: the web was stuck but not yet faded 3 px past the well's top.
- `scripts/dev ci` at 2d288bf1 (origin/dev dd0f8bb4 merged): exit 0. nextest ran 1520 tests:
  1520 passed, 16 skipped. Earlier, at 79ccf930: exit 0, 1520 passed. The first run had failed on
  the repository check: animation durations must be `--duration*` tokens, fixed with
  `--duration-spider` and `--duration-scurry`.
- `python3 scripts/check_repository.py`: passes, including the new `--session-web-stuck` pairs. At
  first it caught the light theme's `--session-ok` at 4.11:1 on the stuck colour, which was
  lightened to `#e6e2da`.
- The full e2e suite at 2d288bf1: 292 passed, 1 skipped (42 files). `sessions.e2e.ts`: 20 of 20,
  axe clean.
- Looked at by hand in Chromium (Playwright screenshots of the mock): the spider at rest, mid-climb
  (second leg frame) and at the top; the web stuck and faded after scrolling; the cockroach in
  the dark and light themes and at 390 px.
- Independent review in fresh sessions (`om-code-review`):
  - Round 1 at cb80760f: approve pending the gate, with 2 minors and 3 nits. All were fixed
    test-first in 158afc69.
  - Round 2 at d2931614: APPROVE, with 2 minors and 2 nits, fixed in 2d288bf1.
- Not covered: a real server with real sessions; the view and its assets are the same, only the
  mock backend serves them in tests. The harness cannot emulate `prefers-reduced-motion`, so
  reduced motion is tested through the stylesheet rule and a stubbed `matchMedia`. A reviewer
  also checked it with Playwright's real `reducedMotion: 'reduce'`.
