# 0180. CI and CD runs are views of the CI/CD page, under a live Now summary

## Status

Accepted. Specified in [SPEC-238](../../.ai/specs/SPEC-238-we-need-some-other-gui-for-ci-so-we-can.md);
task #224. Amends the run dialogs of SPEC-151 and SPEC-189 (CDS-001 to CDS-004 keep their
meaning, in a view instead of a dialog).

## Context

- A CI run and a deploy opened in a modal dialog over the CI/CD page. Its address named the run
  only while it was open; closing it wrote the plain address back. So once closed, the run was
  gone from Back, from the sidebar's remembered place (SPEC-113) and from sight: the person had to
  find its card again, on the CD tab under both environment cards.
- The page said nothing live about a run unless its dialog was open: a run card reads
  "Deploying · Started 1 min ago", and that line does not change.
- The person asked for "some other GUI for CI so we can go back easily after this is closed and
  we see some dynamic summary all the time".

## Decision

- **A run is a view of its tab, at its own address.** `?run=<id>` (CI) and
  `?section=cd&run=<id>` (CD), the addresses the commit status, toasts and `/cd?run=` already
  use, show the run in place of the tab's list, as a region named "CI run" or "Deploy" with an
  "All runs" link (`runview.js`). A card, a toast's Open and the Now summary open a run by
  navigating (a history entry), and app.js remounts the page for the new address. Back, Forward
  and the sidebar's CI/CD link come back to the run. The one exception is a deploy still starting
  (SPEC-189): it has no id yet, so its view is drawn in place. Once the server answers, the view
  becomes the run and its address is written quietly. "All runs" pressed while it starts shows the
  list again, and the toast with Open follows as before.
- **The view's summary is live and sticky.** It shows the run's state, what it is, the time so far
  (ticking every second, from the run's start), or "took …" once the run ended, and the stages
  running now, read from the timeline the waterfall already loads (`stageNow`: open spans with no
  open child, waits left out, a job's stage as "engine › build"). A CI run also shows its jobs
  line. The summary stays at the top of the page while the stages and the log scroll.
- **The Now summary sits above the tabs, on both** (`pages/cinow.js`, `sectionedPage`'s `before`).
  It lists each CI run and deploy that is queued or running, with its state, its jobs and the time
  so far, and links to each one. When nothing runs, one line names the latest CI run and the latest
  deploy and how each ended. It reads `/ci` and `/cd` every 3 s while something runs and every 20 s
  otherwise. It skips a read while the page is hidden and stops when the page goes. A failed read
  of one overview keeps the last one read. Opening a run or "All runs" mounts the page again, so
  the summary first draws the overviews it read last (kept in memory per sandbox). On the first
  visit it holds a line of its own height. Either way the tabs do not move when the read comes.
- **The keyboard follows the view.** When the control that opened a run went, opening the run
  focuses its region, which is announced by name. "All runs" and Back focus that run's card in the
  list, and "All runs" on a starting deploy focuses the CD tab.

## Alternatives

- **Keep the dialog and add a "reopen" button.** Rejected: it fixes "go back" for one run but
  still hides the page and still drops the run from Back and from the sidebar's place.
- **A new route, `/s/<slug>/ci/runs/<id>`.** Rejected: every link to a run already uses `?run=`.
  A new route would change them all for the same view.
- **A master-detail split (the list beside the run).** Rejected: the log needs the width, and on a
  phone the split would stack back into the same in-place view.
- **The Now summary in the app's top bar, on every page.** Deferred: it would read both overviews
  every 3 s from every page of every sandbox, for a view the person did not open. This decision
  keeps it on the page where runs are watched. A global indicator can follow if people ask for it.

## Consequences

- No run opens a dialog: e2e suites look for the "CI run" or "Deploy" region instead.
  Confirmations (Deploy dev?, Deploy production? with its typed slug, Cancel, Clean up) stay
  dialogs.
- The CI/CD page reads `/ci` and `/cd` twice while a tab polls the same overview: once for the tab
  and once for the Now summary. The tab and the summary are separate components with separate
  lifetimes, and the cost is one small GET every 3 s while something runs.
- `loadWaterfall` returns the timeline it drew, or `null` if the read failed.
