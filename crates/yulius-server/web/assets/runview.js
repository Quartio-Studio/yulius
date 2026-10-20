// A CI run's or a deploy's view inside the CI/CD page (SPEC-238, ADR-0180), in place of the tab's
// list rather than in a dialog over it: a region named for it, "All runs" back to the list, and
// its live summary, which stays on top while the page scrolls (state, what, how long, ticking
// while it runs, and the stages running now). The run is the page's address, so Back, Forward and
// the sidebar's remembered place (SPEC-113) come back to it.

import { h, icon, setChildren } from './dom.js';
import { href } from './router.js';
import { duration } from './format.js';
import { stageLabel } from './stages.js';

const WAITS = new Set(['queue', 'wait', 'pause']);
const STAGES_NAMED = 2;

/**
 * Focus `el` once it is in the page, when the keyboard has nowhere to be: on the page itself or
 * on an element that went or is hidden (the card or link that opened a view, which a remount
 * removed). Focus a person put elsewhere (the sidebar, a terminal) stays where it is.
 */
export function focusIfLost(el) {
  requestAnimationFrame(() => {
    const active = document.activeElement;
    // `checkVisibility` where the browser has it, else a hidden element has no boxes.
    const isHidden = (node) => (node.checkVisibility ? !node.checkVisibility() : node.getClientRects().length === 0);
    const lost = !active || active === document.body || !active.isConnected || isHidden(active);
    if (lost && el && el.isConnected) el.focus();
  });
}

/** How long something has been running: "12 s so far", "1 min 5 s so far". */
export function soFar(seconds) {
  const n = Math.max(0, Math.floor(seconds));
  return `${n < 60 ? `${n} s` : `${Math.floor(n / 60)} min ${n % 60} s`} so far`;
}

/**
 * The stages running now in a timeline (stages.js), in words: each unfinished span with no
 * unfinished span under it, waits left out, the run itself only when nothing under it runs. A
 * job's stage reads "engine › build". `null` when nothing runs.
 */
export function stageNow(timeline) {
  const open = ((timeline && timeline.spans) || []).filter((s) => !s.ended_at && !WAITS.has(s.kind));
  const parents = new Set(open.map((s) => s.parent_key).filter(Boolean));
  let leaves = open.filter((s) => !parents.has(s.key));
  if (leaves.some((s) => s.kind !== 'run')) leaves = leaves.filter((s) => s.kind !== 'run');
  if (leaves.length === 0) return null;
  leaves.sort((a, b) => Date.parse(a.started_at) - Date.parse(b.started_at));
  const words = leaves.map((s) => (s.kind === 'job' ? s.name : s.job ? `${s.job} › ${stageLabel(s)}` : stageLabel(s)));
  const named = words.slice(0, STAGES_NAMED).join(', ');
  return words.length > STAGES_NAMED ? `${named} and ${words.length - STAGES_NAMED} more` : named;
}

/**
 * The view's frame: `name` names the region ("CI run", "Deploy"); "All runs" goes to `list` (a
 * page address), or calls `onBack` instead while the view has no address of its own (a deploy
 * still starting, SPEC-189) until `linkBack()`. Returns the region `el`, its `head` line and its
 * `body`, shown by `ready()`; until then `loading(node)` stands in for both (the run's loading
 * shape). `summarize()` fills the summary's facts; `dispose()` stops its clock.
 */
export function runView({ name, list, onBack }) {
  let backTo = onBack || null;
  // Until `linkBack()`, a click calls `onBack` (the router leaves a prevented click alone).
  const back = h('a', { class: 'btn btn-sm run-back', href: href(list), onClick: (event) => { if (backTo) { event.preventDefault(); backTo(); } } },
    icon('back', { size: 14 }), 'All runs');
  if (!backTo) back.dataset.route = list;
  const head = h('div', { class: 'row-wrap run-head' });
  const body = h('div', { class: 'stack' });
  const time = h('span', { class: 'run-time' });
  const facts = h('div', { class: 'run-facts muted small' });
  const summary = h('div', { class: 'run-summary' }, head, facts);
  let clock = null;
  const stopClock = () => { clearInterval(clock); clock = null; };
  const content = h('div', { class: 'stack run-content' });
  // Focusable from script, so opening a run takes the keyboard there (it is announced by name).
  const el = h('section', { class: 'run-view stack', 'aria-label': name, tabindex: '-1' }, back, content);
  return {
    el, head, body, back,
    loading: (node) => setChildren(content, node),
    ready: () => { if (!summary.isConnected) setChildren(content, summary, body); },
    /** The view has its own address now: "All runs" is a link to the list. */
    linkBack: () => { backTo = null; back.dataset.route = list; },
    /**
     * The summary's facts: how long (`startedAt` while `running`, ticking each second; `took` ms
     * once ended, with `since`, "started 2 hours ago"; else `since` alone), the stages running now
     * and a line of its own.
     */
    summarize({ running, startedAt, took, since, now, line }) {
      stopClock();
      const start = startedAt ? Date.parse(startedAt) : NaN;
      const ticking = running && Number.isFinite(start);
      const ended = !running && Number.isFinite(took);
      if (ticking) {
        const tick = () => { time.textContent = soFar((Date.now() - start) / 1000); };
        tick();
        clock = setInterval(tick, 1000);
      } else {
        time.textContent = ended ? `took ${duration(took)}` : since || '';
      }
      setChildren(facts, time.textContent ? time : null,
        ended && since ? h('span', null, since) : null,
        running && now ? h('span', { class: 'run-now' }, `Now: ${now}`) : null,
        line ? h('span', null, line) : null);
    },
    dispose: stopClock,
  };
}
