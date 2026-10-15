// Stages (SPEC-137): the measured parts of a task, a CI run or a CD run, drawn the Dashboard's way
// (one series colour, recessive grid, a table one click away, hover for the exact figures).
//
// - `waterfall(timeline)`: one subject's spans on a shared time axis, nested by their parent, a
//   row each: what it was, its bar, how long it took. Waits are drawn hatched, failures in the
//   danger colour with a mark and a word, a span still running reaches "now".
// - `trendChart(trend)`: a stage's daily p50 (the series line) and p90 (dashed), both labelled at
//   their end and in a legend, with a crosshair and a table.

import { h, svgEl, setChildren, icon } from './dom.js';
import { duration, int } from './format.js';

const STATE_NAMES = {
  queued: 'Queued', chained: 'Waiting for another task', prepared: 'Prepared, waiting to start',
  starting: 'Starting', working: 'Working', idle: 'Idle, waiting for input', needs_you: 'Needs you',
  done: 'Done, session open', paused: 'Paused for capacity', switching: 'Switching subscription',
  stopping: 'Stopping',
};
const DELIVERY_NAMES = {
  pr_open: 'Pull request open', merged_integration: 'Merged, before promotion', env_done: 'Environment done',
  promoted: 'Promoted',
};
const KIND_NAMES = {
  state: 'State', step: 'Step', run: 'Run', pause: 'Pause', delivery: 'Delivery', wait: 'Wait',
  update: 'Update', queue: 'Queue', prepare: 'Preparation', job: 'Job', stage: 'Stage',
  promote: 'Promotion', deploy: 'Deploy', verify: 'Verify',
};
const SUBJECT_NAMES = { task: 'Task', ci_run: 'CI', cd_run: 'CD' };
const WAIT_KINDS = new Set(['queue', 'wait', 'pause']);
const FAILED = new Set(['failed', 'error']);
const PAUSE_NAMES = { limit: 'Usage limit', turn: 'Capacity at a turn', prompt: 'Capacity before a prompt', start: 'Capacity before the start' };
const WAIT_NAMES = { needs_you: 'Asked a person', merge_armed: 'Armed merge, waiting for checks', 'job slot': 'Waiting for a job slot', confirmation: 'Confirmation' };

/** The words for a span (or a stage row): "Queued", "Pull request open", "Job quality". */
export function stageLabel(span) {
  const { kind, name } = span;
  if (kind === 'state') return STATE_NAMES[name] || name;
  if (kind === 'delivery') return DELIVERY_NAMES[name] || name;
  if (kind === 'pause') return PAUSE_NAMES[name] || `Pause: ${name}`;
  if (kind === 'wait') return WAIT_NAMES[name] || name;
  if (kind === 'run') return span.subject_kind === 'task' ? `Run${span.detail && span.detail.seq ? ` ${span.detail.seq}` : ''}` : span.subject_kind === 'cd_run' ? 'Deployment' : 'Run';
  if (kind === 'queue') return name === 'queue' ? 'Queue' : name.replace(/^queue · /, 'Queue: ');
  if (kind === 'job') return `Job ${name}`;
  if (kind === 'step' && name === 'step') return 'Steps';
  if (kind === 'prepare') return name === 'checkout' ? 'Checkout' : name === 'commit' ? 'Fetch the commit' : 'Checks';
  if (kind === 'promote') return 'Promotion';
  if (kind === 'deploy' || kind === 'verify') return KIND_NAMES[kind];
  if (kind === 'update') return `Update ${name}`;
  return name;
}

/** "CI · Queue", "Task · Step": a stage row's subject and words. */
export function stageTitle(row) {
  return `${SUBJECT_NAMES[row.subject_kind] || row.subject_kind} · ${stageLabel(row)}`;
}

/** The sort of stage, in words. */
export function kindLabel(kind) {
  return KIND_NAMES[kind] || kind;
}

/** The subject kind, in words. */
export function subjectLabel(kind) {
  return SUBJECT_NAMES[kind] || kind;
}

/** Figures worth a line in a tooltip or a table cell: tests, tokens, exit code, reason. */
export function figureText(span) {
  const d = span.detail || {};
  const parts = [];
  const tests = ['tests_passed', 'tests_failed', 'tests_flaky', 'tests_skipped'].filter((k) => Number.isFinite(d[k]));
  if (tests.length) parts.push(tests.map((k) => `${int(d[k])} ${k.slice(6)}`).join(', '));
  if (Number.isFinite(d.output_tokens) || Number.isFinite(d.input_tokens)) parts.push(`${int((d.input_tokens || 0) + (d.output_tokens || 0))} tokens`);
  if (Number.isFinite(d.active_ms) && d.active_ms > 0) parts.push(`agent work ${duration(d.active_ms)}`);
  if (Number.isFinite(d.cache_hits)) parts.push(`${int(d.cache_hits)} cache hits`);
  if (d.reused === true) parts.push('reused result, not run');
  if (d.exit_code !== undefined && d.exit_code !== null && d.exit_code !== 0) parts.push(`exit ${d.exit_code}`);
  if (typeof d.reason === 'string' && d.reason) parts.push(d.reason);
  if (typeof d.requested_by === 'string' && span.kind === 'run') parts.push(`by ${d.requested_by}`);
  if (typeof d.sha === 'string' && span.kind === 'run') parts.push(d.sha.slice(0, 7));
  if (typeof d.action === 'string') parts.push(d.action);
  if (d.measured_by === 'page') parts.push('measured by the page');
  return parts.join(' · ');
}

function outcomeWord(span) {
  if (!span.ended_at) return 'running';
  if (FAILED.has(span.outcome)) return span.outcome;
  return span.outcome || 'ended';
}

/** Spans in tree order with their depth: a child right after its parent. */
function ordered(spans) {
  const keys = new Set(spans.map((s) => s.key));
  const children = new Map();
  const roots = [];
  for (const span of spans) {
    if (span.parent_key && keys.has(span.parent_key)) {
      if (!children.has(span.parent_key)) children.set(span.parent_key, []);
      children.get(span.parent_key).push(span);
    } else {
      roots.push(span);
    }
  }
  const out = [];
  const walk = (span, depth) => {
    out.push({ span, depth });
    for (const child of children.get(span.key) || []) walk(child, depth + 1);
  };
  roots.forEach((span) => walk(span, 0));
  return out;
}

const time = (iso) => Date.parse(iso);

/**
 * One subject's spans as a waterfall (and a table). `related` spans (a task's CI runs) follow
 * under their own heading. Empty when nothing was measured.
 */
export function waterfall(timeline, { now = Date.now(), title = 'Stages' } = {}) {
  const spans = (timeline && timeline.spans) || [];
  const related = (timeline && timeline.related) || [];
  const all = [...spans, ...related];
  if (all.length === 0) return h('p', { class: 'muted small st-empty' }, 'No stages measured yet.');
  const start = Math.min(...all.map((s) => time(s.started_at)));
  const end = Math.max(...all.map((s) => (s.ended_at ? time(s.ended_at) : now)), start + 1);
  const total = end - start;
  const tip = h('div', { class: 'chart-tip st-tip', hidden: true });
  const rows = [];
  const tableRows = [];
  const add = (span, depth) => {
    const from = time(span.started_at);
    const to = span.ended_at ? time(span.ended_at) : now;
    const left = ((from - start) / total) * 100;
    const width = Math.max(((to - from) / total) * 100, 0.4);
    const failed = FAILED.has(span.outcome);
    const label = stageLabel(span);
    const took = span.ended_at ? duration(span.duration_ms) : `${duration(to - from)} so far`;
    const figures = figureText(span);
    const said = `${label}: ${took}, ${outcomeWord(span)}${figures ? ` (${figures})` : ''}`;
    const barEl = h('span', {
      class: ['st-bar', WAIT_KINDS.has(span.kind) && 'st-bar-wait', failed && 'st-bar-failed', !span.ended_at && 'st-bar-running', span.source === 'backfilled' && 'st-bar-backfilled'],
      style: { left: `${left}%`, width: `${width}%` },
    });
    const show = (event) => {
      setChildren(tip, h('span', null, `${label}${span.source === 'backfilled' ? ' · backfilled' : ''}`), h('strong', null, took), figures ? h('span', null, figures) : null);
      tip.hidden = false;
      const box = tip.parentElement.getBoundingClientRect();
      tip.style.left = `${Math.max(80, Math.min(box.width - 80, event.clientX - box.left))}px`;
      tip.style.top = `${event.clientY - box.top}px`;
    };
    const row = h('li', { class: 'st-row', title: said, onPointerMove: show, onPointerLeave: () => { tip.hidden = true; } },
      h('span', { class: 'st-name', style: { paddingLeft: `${depth * 14}px` } },
        failed ? icon('x', { size: 12, label: 'failed' }) : null,
        h('span', { class: 'st-name-text' }, label)),
      h('span', { class: 'st-track', 'aria-hidden': 'true' }, barEl),
      h('span', { class: ['st-took', failed && 'st-took-failed'] }, took));
    rows.push(row);
    tableRows.push(h('tr', null,
      h('td', null, h('span', { style: { paddingLeft: `${depth * 14}px` } }, label)),
      h('td', null, new Date(from).toLocaleTimeString()),
      h('td', { class: 'num' }, took),
      h('td', null, outcomeWord(span)),
      h('td', null, [figures, span.source === 'backfilled' ? 'backfilled' : ''].filter(Boolean).join(' · '))));
  };
  ordered(spans).forEach(({ span, depth }) => add(span, depth));
  if (related.length) {
    rows.push(h('li', { class: 'st-group' }, 'CI runs of its pull request'));
    tableRows.push(h('tr', null, h('th', { colspan: 5, scope: 'rowgroup' }, 'CI runs of its pull request')));
    ordered(related).forEach(({ span, depth }) => add(span, depth));
  }
  const list = h('ol', { class: 'st-rows', role: 'list', 'aria-label': `${title}: ${duration(total)} from the first start to the last end` }, rows);
  const table = h('table', { class: 'table st-table', hidden: true },
    h('thead', null, h('tr', null, ['Stage', 'Started', 'Took', 'Outcome', 'Figures'].map((c, i) => h('th', { class: i === 2 ? 'num' : null }, c)))),
    h('tbody', null, tableRows));
  const toggle = h('button', { type: 'button', class: 'btn btn-sm db-table-toggle', 'aria-pressed': 'false', onClick: () => {
    const on = toggle.getAttribute('aria-pressed') !== 'true';
    toggle.setAttribute('aria-pressed', String(on));
    table.hidden = !on;
    plot.hidden = on;
  } }, icon('tasks', { size: 14 }), 'Table');
  const plot = h('div', { class: 'st-plot' }, list, tip);
  return h('section', { class: 'st-waterfall', 'aria-label': title },
    h('div', { class: 'st-head' }, h('h3', { class: 'st-title' }, title), h('span', { class: 'muted small' }, `${duration(total)} in all`), h('span', { class: 'spacer' }), toggle),
    plot, table);
}

/** Loads and draws one subject's waterfall into `host` (`subject`: `ci_run:<id>`, …). */
export async function loadWaterfall(ctx, sandbox, subject, host, options = {}) {
  try {
    const timeline = await ctx.api.get(`/api/sandboxes/${encodeURIComponent(sandbox.id)}/stages/timeline?subject=${encodeURIComponent(subject)}`);
    setChildren(host, waterfall(timeline, options));
  } catch (error) {
    setChildren(host, h('p', { class: 'muted small' }, `Couldn’t load the stages: ${error.message || error}`));
  }
}

function niceMax(value) {
  if (value <= 0) return 1000;
  const pow = 10 ** Math.floor(Math.log10(value));
  for (const step of [1, 2, 2.5, 5, 10]) if (step * pow >= value) return step * pow;
  return 10 * pow;
}

const SHORT_DATE = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
const LONG_DATE = new Intl.DateTimeFormat(undefined, { weekday: 'short', month: 'short', day: 'numeric', timeZone: 'UTC' });
const day = (date) => new Date(`${date}T00:00:00Z`);

/** A stage's daily p50 and p90 over the range (days without spans are gaps), with a table. */
export function trendChart(trend, { label = 'Stage' } = {}) {
  const days = (trend && trend.days) || [];
  const known = days.filter((d) => Number.isFinite(d.p50_ms));
  const wrap = h('div', { class: 'chart st-chart' });
  const table = h('table', { class: 'table db-table', hidden: true },
    h('thead', null, h('tr', null, h('th', null, 'Day'), h('th', { class: 'num' }, 'Ended'), h('th', { class: 'num' }, 'p50'), h('th', { class: 'num' }, 'p90'))),
    h('tbody', null, days.map((d) => h('tr', null, h('td', null, LONG_DATE.format(day(d.date))), h('td', { class: 'num' }, int(d.count)), h('td', { class: 'num' }, duration(d.p50_ms)), h('td', { class: 'num' }, duration(d.p90_ms))))));
  const toggle = h('button', { type: 'button', class: 'btn btn-sm db-table-toggle', 'aria-pressed': 'false', onClick: () => {
    const on = toggle.getAttribute('aria-pressed') !== 'true';
    toggle.setAttribute('aria-pressed', String(on));
    table.hidden = !on;
    wrap.hidden = on;
  } }, icon('tasks', { size: 14 }), 'Table');
  const legend = h('ul', { class: 'st-legend', 'aria-label': 'Legend' },
    h('li', null, h('span', { class: 'st-key st-key-p50', 'aria-hidden': 'true' }), 'p50 (median)'),
    h('li', null, h('span', { class: 'st-key st-key-p90', 'aria-hidden': 'true' }), 'p90 (9 in 10 within)'));
  if (known.length === 0) {
    return { el: h('p', { class: 'db-empty' }, `No ${label.toLowerCase()} ended in this range.`), toggle: null, legend: null };
  }
  const tip = h('div', { class: 'chart-tip db-tip', hidden: true });
  const height = 220;
  const pad = { top: 16, right: 44, bottom: 26, left: 56 };
  function draw() {
    const width = Math.max(280, wrap.clientWidth || 600);
    const plotW = width - pad.left - pad.right;
    const plotH = height - pad.top - pad.bottom;
    const max = niceMax(Math.max(...known.map((d) => d.p90_ms ?? d.p50_ms), 1));
    const step = days.length > 1 ? plotW / (days.length - 1) : plotW;
    const x = (i) => pad.left + (days.length > 1 ? i * step : plotW / 2);
    const y = (v) => pad.top + plotH - (v / max) * plotH;
    const bottom = pad.top + plotH;
    const path = (key) => {
      let d = '';
      let open = false;
      days.forEach((row, i) => {
        if (!Number.isFinite(row[key])) { open = false; return; }
        d += `${open ? 'L' : 'M'}${x(i).toFixed(1)},${y(row[key]).toFixed(1)}`;
        open = true;
      });
      return d;
    };
    const dots = (key, cls) => days.map((row, i) => (Number.isFinite(row[key]) ? svgEl('circle', { class: cls, cx: x(i), cy: y(row[key]), r: 4 }) : null));
    const lastIndex = days.length - 1 - [...days].reverse().findIndex((r) => Number.isFinite(r.p50_ms));
    const last = days[lastIndex];
    const labelsTooClose = Number.isFinite(last.p90_ms) && Math.abs(y(last.p90_ms) - y(last.p50_ms)) < 14;
    const every = Math.max(1, Math.ceil(days.length / Math.max(2, Math.floor(plotW / 70))));
    const cross = svgEl('line', { class: 'st-cross', x1: 0, x2: 0, y1: pad.top, y2: bottom, visibility: 'hidden' });
    const svg = svgEl('svg', { viewBox: `0 0 ${width} ${height}`, width, height, role: 'img', 'aria-label': `${label}, p50 and p90 by day; the table has every day` },
      [max / 2, max].map((t) => [
        svgEl('line', { class: 'chart-grid db-grid', x1: pad.left, x2: width - pad.right, y1: y(t), y2: y(t) }),
        svgEl('text', { class: 'chart-axis', x: pad.left - 8, y: y(t) + 4, 'text-anchor': 'end' }, duration(t)),
      ]),
      svgEl('line', { class: 'chart-baseline', x1: pad.left, x2: width - pad.right, y1: bottom, y2: bottom }),
      days.map((row, i) => (i % every === 0 ? svgEl('text', { class: 'chart-axis', x: x(i), y: height - 8, 'text-anchor': 'middle' }, SHORT_DATE.format(day(row.date))) : null)),
      cross,
      svgEl('path', { class: 'st-line-p90', d: path('p90_ms') }),
      svgEl('path', { class: 'st-line-p50', d: path('p50_ms') }),
      dots('p90_ms', 'st-dot-p90'),
      dots('p50_ms', 'st-dot-p50'),
      svgEl('text', { class: 'chart-axis st-end-label', x: x(lastIndex) + 8, y: y(last.p50_ms) + (labelsTooClose ? 10 : 4) }, 'p50'),
      Number.isFinite(last.p90_ms) ? svgEl('text', { class: 'chart-axis st-end-label', x: x(lastIndex) + 8, y: y(last.p90_ms) - (labelsTooClose ? 6 : -4) }, 'p90') : null);
    const hit = svgEl('rect', { class: 'chart-hit', x: pad.left - step / 2, y: pad.top, width: plotW + step, height: plotH });
    hit.addEventListener('pointermove', (event) => {
      const box = svg.getBoundingClientRect();
      const px = (event.clientX - box.left) * (width / box.width);
      const i = Math.max(0, Math.min(days.length - 1, Math.round((px - pad.left) / (step || 1))));
      const row = days[i];
      cross.setAttribute('x1', String(x(i)));
      cross.setAttribute('x2', String(x(i)));
      cross.setAttribute('visibility', 'visible');
      setChildren(tip, h('span', { class: 'db-tip-date' }, LONG_DATE.format(day(row.date))),
        h('strong', null, Number.isFinite(row.p50_ms) ? `p50 ${duration(row.p50_ms)} · p90 ${duration(row.p90_ms)}` : 'nothing ended'),
        h('span', null, `${int(row.count)} ended`));
      tip.hidden = false;
      tip.style.left = `${Math.max(80, Math.min(box.width - 80, (x(i) / width) * box.width))}px`;
      tip.style.top = `${((Number.isFinite(row.p90_ms) ? y(row.p90_ms) : bottom - 8) / height) * box.height}px`;
    });
    hit.addEventListener('pointerleave', () => { cross.setAttribute('visibility', 'hidden'); tip.hidden = true; });
    svg.append(hit);
    setChildren(wrap, svg, tip);
  }
  const observer = new ResizeObserver(() => draw());
  requestAnimationFrame(() => { draw(); observer.observe(wrap); });
  return { el: h('div', null, wrap, table), toggle, legend };
}
