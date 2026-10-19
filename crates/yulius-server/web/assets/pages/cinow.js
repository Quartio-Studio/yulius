// The CI/CD page's "Now" summary (SPEC-238 CRV-004, ADR-0180), above its tabs and so on both:
// each CI run and deploy queued or running, with its jobs and a time that grows by itself, each a
// link to its run; with nothing running, how the latest CI run and the latest deploy ended. It
// reads both overviews every few seconds while something runs, now and then otherwise. A page
// mounted again (a run opened, All runs) draws the last overviews it read at once, so the tabs
// under it do not move.

import { enc } from '../api.js';
import { h, setChildren, pollPaused, badge, bar } from '../dom.js';
import { relLong } from '../format.js';
import { href, currentLocation } from '../router.js';
import { soFar } from '../runview.js';
import { stateOf as ciState, jobsLine, ACTIVE } from './ci.js';
import { stateOf as cdState } from './cd.js';

const POLL_ACTIVE_MS = 3000;
const POLL_IDLE_MS = 20000;
const shortSha = (sha) => String(sha || '').slice(0, 7);
const envTitle = (name) => (name === 'dev' ? 'Dev' : 'Production');
// The overviews last read, per sandbox: `{ ci, cd, at }`, each kept until a read of it succeeds,
// and drawn on a mount only while younger than KEPT_MS (older ones may name runs long ended).
const lastRead = new Map();
const KEPT_MS = 60000;

/** Mount the summary for `sandbox`: `{ el, destroy }`; `destroy()` stops its reads and its clock. */
export function mountNowSummary(ctx, sandbox) {
  const base = `/api/sandboxes/${enc(sandbox.id)}`;
  const page = `/s/${encodeURIComponent(sandbox.slug)}/ci`;
  // Until the first read, what was read last, or a line of its height (the page's own loading
  // status says what loads), so the tabs do not move when it comes.
  const el = h('section', { class: 'ci-now', 'aria-label': 'Now' }, h('div', { class: 'ci-now-row', 'aria-hidden': 'true' }, bar(40)));
  let timer = null;
  let clock = null;
  let destroyed = false;

  // A link named by its text (WCAG 2.5.3); the one of the run in view adds no history entry.
  const link = (path, ...children) => h('a', { href: href(path), dataset: { route: path }, onClick: (event) => { if (currentLocation().full === path) event.preventDefault(); } }, ...children);

  /** One run that is going on: its state, what it is, a line about it and its time so far. */
  function liveRow({ path, kind, st, what, sha, line, startedAt, queuedAt }) {
    const time = startedAt ? h('span', { class: 'run-time', dataset: { since: startedAt } }, '') : h('span', null, `queued ${relLong(queuedAt)}`);
    return h('li', { class: 'ci-now-row' },
      link(path, badge(st.label, st.tone), ' ', h('span', { class: 'ci-now-kind' }, kind), ' ', what, ' at ', h('code', null, shortSha(sha))),
      line ? h('span', { class: 'muted' }, line) : null,
      h('span', { class: 'muted ci-now-time' }, time));
  }

  function render(ci, cd) {
    const ciRuns = (ci && ci.runs) || [];
    const cdRuns = (cd && cd.runs) || [];
    const live = [
      ...ciRuns.filter((r) => ACTIVE.has(r.state)).map((r) => liveRow({
        path: `${page}?run=${encodeURIComponent(r.id)}`, kind: 'CI', st: ciState(r.state), what: r.label, sha: r.sha,
        line: jobsLine(r.jobs), startedAt: r.started_at, queuedAt: r.queued_at,
      })),
      ...cdRuns.filter((r) => r.state === 'running').map((r) => liveRow({
        path: `${page}?section=cd&run=${encodeURIComponent(r.run_id)}`, kind: 'CD', st: cdState(r.state), what: envTitle(r.environment), sha: r.commit,
        line: null, startedAt: r.started_at,
      })),
    ];
    if (live.length) {
      setChildren(el, h('ul', { class: 'ci-now-list' }, live));
    } else {
      // Nothing running: how the latest of each ended, a link to it.
      const lastCi = ciRuns[0];
      const lastCd = cdRuns[0];
      const parts = [];
      if (lastCi) {
        const words = `${ciState(lastCi.state).label.toLowerCase()} ${relLong(lastCi.ended_at || lastCi.queued_at)}`;
        parts.push(link(`${page}?run=${encodeURIComponent(lastCi.id)}`, `Last CI run: ${lastCi.label} at ${shortSha(lastCi.sha)} ${words}`));
      }
      if (lastCd) {
        const words = `${cdState(lastCd.state).label.toLowerCase()} ${relLong(lastCd.ended_at || lastCd.started_at)}`;
        parts.push(link(`${page}?section=cd&run=${encodeURIComponent(lastCd.run_id)}`, `Last deploy: ${envTitle(lastCd.environment)} at ${shortSha(lastCd.commit)} ${words}`));
      }
      setChildren(el, h('p', { class: 'muted small ci-now-idle' }, h('span', null, 'Nothing is running.'), parts));
    }
    tick();
    clearInterval(clock);
    clock = live.length ? setInterval(tick, 1000) : null;
    return live.length > 0;
  }

  function tick() {
    for (const time of el.querySelectorAll('.run-time[data-since]')) time.textContent = soFar((Date.now() - Date.parse(time.dataset.since)) / 1000);
  }

  async function load() {
    timer = null;
    let active = false;
    if (!pollPaused()) {
      const [ci, cd] = await Promise.allSettled([ctx.api.get(`${base}/ci`), ctx.api.get(`${base}/cd`)]);
      if (destroyed) return;
      // A failed read keeps what was read of that overview before (review round 1).
      const kept = lastRead.get(sandbox.id) || {};
      const read = { ci: ci.status === 'fulfilled' ? ci.value : kept.ci, cd: cd.status === 'fulfilled' ? cd.value : kept.cd, at: Date.now() };
      lastRead.set(sandbox.id, read);
      if (read.ci || read.cd) active = render(read.ci, read.cd);
      else active = Boolean(clock);
    } else {
      active = Boolean(clock);
    }
    if (!destroyed) timer = setTimeout(load, active ? POLL_ACTIVE_MS : POLL_IDLE_MS);
  }

  const kept = lastRead.get(sandbox.id);
  if (kept && Date.now() - kept.at < KEPT_MS) render(kept.ci, kept.cd);
  load();
  return { el, destroy: () => { destroyed = true; clearTimeout(timer); clearInterval(clock); } };
}
