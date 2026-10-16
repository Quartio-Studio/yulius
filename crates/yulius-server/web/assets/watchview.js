// A task someone else owns, as its watchers see it (SPEC-179): the owner's terminal is drawn for
// the owner's window, so a watcher reads the task's Claude Code session printed as the Sessions view
// prints it (SPEC-075), in the live terminal's colours, as the session writes. Reads
// GET /api/tasks/{tid}/transcript: the tail first, then from `end` about every 1.5 s while the task
// is live and the page is shown, answered without a read while the transcript's size is unchanged.

import { enc } from './api.js';
import { h, icon, setChildren, loadError } from './dom.js';
import { href } from './router.js';
import { isLive } from './status.js';
import { transcriptEntries } from './pages/sessions.js';

/** How often a watcher asks for new lines while the task is live (WAT-003). */
export const WATCH_POLL_MS = 1500;
/** Entries a watcher starts with: the tail of the session. */
const TAIL = 200;
/** How close to the bottom (px) still counts as reading the latest lines. */
const FOLLOW_SLACK = 32;

/**
 * Mount the live transcript of a task owned by someone else. `task()` is the task as the view knows
 * it now, `owner()` its owner's login, `emptyMessage(task)` what a session without lines yet means
 * ({title, text, busy} or null) and `sessionPath(docId)` the Sessions page of the whole session.
 * Returns {el, start(), update(), destroy()}; `el.dataset.reading` says whether it reads on
 * (`live`), waits for the page to be shown (`paused`) or has read an ended task (`ended`).
 * `timer` stands in for setTimeout and clearTimeout in tests.
 */
export function createWatchView(ctx, { taskId, task, owner, emptyMessage, sessionPath, timer: clock = { set: (fn, ms) => setTimeout(fn, ms), clear: (id) => clearTimeout(id) } }) {
  const transcript = transcriptEntries();
  const earlier = h('p', { class: 'watch-earlier ss-dim small', hidden: true });
  const empty = h('div', { class: 'watch-empty', role: 'status', hidden: true });
  const working = h('div', { class: 'watch-working ss-line', hidden: true }, h('span', { class: 'ss-mark ss-mark-think', 'aria-hidden': 'true' }, '✻'), h('span', { class: 'ss-line-body watch-working-text' }, 'Working…'));
  const lockedText = h('span', { class: 'ss-line-body' });
  // Where the TUI's prompt box was: who types, and that this person watches (not a control).
  const locked = h('div', { class: 'ss-locked' }, h('span', { class: 'ss-mark ss-mark-prompt', 'aria-hidden': 'true' }, '>'), lockedText, icon('lock', { size: 14 }));
  const tui = h('div', { class: 'ss-tui ss-live' }, earlier, transcript.el, empty, working, locked);
  const scroller = h('div', { class: 'watch-scroll', tabindex: 0, role: 'region', 'aria-label': 'Live transcript' }, tui);
  // Jumping down hides the button: the keyboard stays in the transcript, not on the page.
  const jump = h('button', { type: 'button', class: 'btn btn-sm watch-jump', hidden: true, onClick: () => { toBottom(); scroller.focus({ preventScroll: true }); } }, icon('down', { size: 14 }), 'New messages');
  const el = h('div', { class: 'watch', dataset: { reading: 'live' } }, scroller, jump);

  let alive = true;
  let started = false;
  let timer = null;
  let reading = null; // the AbortController of the read in flight
  let failed = false;
  let again = false; // the task changed during a read: read once more after it
  let seen = { session: null, end: 0, bytes: 0, loaded: false };

  const atBottom = () => scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight <= FOLLOW_SLACK;
  // Following the latest lines is a state (SPEC-185): only the reader scrolling up away from the
  // bottom ends it, and while it lasts a height that changes (the Working line, the card above, a
  // block laid out late, the window) keeps the view at the bottom. Measured at each read instead,
  // such a change left the view above the bottom for good, behind "New messages" (#575).
  let following = true;
  let lastTop = 0;
  function toBottom() {
    scroller.scrollTop = scroller.scrollHeight;
    lastTop = scroller.scrollTop;
    following = true;
    jump.hidden = true;
  }
  scroller.addEventListener('scroll', () => {
    const top = scroller.scrollTop;
    if (atBottom()) {
      following = true;
      jump.hidden = true;
    } else if (top < lastTop) following = false;
    lastTop = top;
  }, { passive: true });
  const sizes = typeof ResizeObserver === 'function' ? new ResizeObserver(() => { if (following) toBottom(); }) : null;

  function render() {
    const current = task();
    const who = owner();
    const live = isLive(current.status);
    lockedText.textContent = live ? `@${who} is at the keyboard · you are watching` : `@${who} owns this task · you are watching`;
    working.hidden = !(current.status === 'running' && current.activity === 'working');
    if (failed) return; // the box says why the first read failed, with Try again
    const blank = seen.loaded && !transcript.el.children.length;
    const message = blank ? (emptyMessage(current) || { title: 'Nothing written yet', text: live ? 'The session’s messages appear here as it writes them.' : 'This session has no messages to show.' }) : null;
    empty.hidden = !message;
    setChildren(empty, message ? [h('p', { class: 'term-empty-title' }, message.title), message.text ? h('p', { class: 'term-empty-text' }, message.text) : null] : null);
  }

  function state() {
    if (!isLive(task().status)) return 'ended';
    return document.hidden ? 'paused' : 'live';
  }

  function schedule() {
    clock.clear(timer);
    timer = null;
    el.dataset.reading = state();
    if (!alive || !started || el.dataset.reading !== 'live') return;
    timer = clock.set(read, WATCH_POLL_MS);
  }

  function apply(page) {
    const follow = !seen.loaded || following;
    const entries = page.entries || [];
    if (!seen.loaded) {
      const path = page.doc_id && sessionPath(page.doc_id);
      earlier.hidden = !page.earlier;
      setChildren(earlier, page.earlier ? ['Earlier messages are in the whole session', path ? [': ', h('a', { href: href(path), dataset: { route: path } }, 'open it in Sessions')] : null, '.'] : null);
    }
    const added = transcript.append(entries);
    seen = { session: page.session_id || null, end: page.end || 0, bytes: page.bytes || 0, loaded: true };
    render();
    if (follow) toBottom();
    else if (added) jump.hidden = false; // a result landing in a call above is not new below
  }

  async function read() {
    clock.clear(timer);
    timer = null;
    if (!alive || reading) return;
    // The tail first, and again for a session that had none; then on from where the last read ended.
    // `seen` lets the server answer an unchanged transcript without reading it; after a full page
    // the rest is read whatever the size (null).
    const query = seen.loaded && seen.session
      ? `from=${seen.end}&session=${encodeURIComponent(seen.session)}${seen.bytes === null ? '' : `&seen=${seen.bytes}`}`
      : `limit=${TAIL}`;
    reading = new AbortController();
    let page = null;
    try {
      page = await ctx.api.get(`/api/tasks/${enc(taskId)}/transcript?${query}`, { signal: reading.signal });
    } catch (error) {
      if (!alive) return; // left: the read was cancelled
      // The first read shows why and a way to try again; a later one tries again on its own.
      if (!seen.loaded) {
        failed = true;
        setChildren(empty, loadError(`Couldn’t load the session: ${error.message}`, () => { failed = false; empty.hidden = true; read(); }));
        empty.hidden = false;
        return;
      }
    } finally {
      reading = null;
    }
    if (!alive) return;
    if (page) {
      if (failed) {
        failed = false;
        empty.hidden = true;
      }
      // Another session (a continue started one), or this one rewritten shorter: from its tail.
      if (seen.loaded && seen.session && (page.session_id !== seen.session || page.end < seen.end)) {
        transcript.clear();
        seen = { session: null, end: 0, bytes: 0, loaded: false };
        read();
        return;
      }
      apply(page);
      if (page.next !== null && page.next !== undefined) {
        seen.end = page.next;
        seen.bytes = null;
        read();
        return;
      }
    }
    if (again && !failed) {
      again = false;
      read();
      return;
    }
    if (!failed) schedule();
  }

  const visibility = () => { if (!document.hidden && el.dataset.reading === 'paused') read(); else schedule(); };
  document.addEventListener('visibilitychange', visibility);

  render();
  return {
    el,
    start() {
      if (started) return;
      started = true;
      if (sizes) {
        sizes.observe(scroller);
        sizes.observe(tui);
      }
      read();
    },
    /** The task changed: say so, and read once more when it ended or started again. */
    update() {
      const before = el.dataset.reading;
      render();
      if (!started) return;
      const now = state();
      // Same state: a planned read stays as planned (events must not put it off).
      if (before === now || now === 'paused') {
        if (!reading && !timer && !failed) schedule();
      } else if (reading) again = true;
      else read();
    },
    focus() {
      scroller.focus({ preventScroll: true });
    },
    destroy() {
      alive = false;
      clock.clear(timer);
      if (reading) reading.abort();
      if (sizes) sizes.disconnect();
      transcript.destroy();
      document.removeEventListener('visibilitychange', visibility);
    },
  };
}
