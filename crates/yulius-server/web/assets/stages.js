// Stages (SPEC-137): the measured parts of a task, a CI run or a CD run, drawn the Dashboard's way
// (one series colour, recessive grid, a table one click away, hover for the exact figures).
//
// - `waterfall(timeline)`: one subject's spans on a shared time axis (ticks from its start, "now"
//   while it runs, a legend of the bars), nested by their parent, a row each: what it was, its bar,
//   how long it took. A span's children fold under it (a failed one opens). Waits are drawn
//   hatched, failures in the danger colour with a mark and a word, a span still running reaches
//   "now". A CI run's jobs come under "Jobs", each with its wait for a job slot on its own row
//   (SPEC-165).
// - `trendChart(trend)`: a stage's daily p50 (the series line) and p90 (dashed), both labelled at
//   their end and in a legend, with a crosshair and a table.

import { h, svgEl, setChildren, icon } from './dom.js';
import { bytes, duration, int } from './format.js';

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

/** CPU time: tenths of a second below 10 s ("0.4 s"), then as a duration. */
export function cpuTime(ms) {
  if (!Number.isFinite(ms)) return '—';
  return ms < 10_000 ? `${(ms / 1000).toFixed(1)} s` : duration(ms);
}

/** Memory or disk from MiB ("640 MiB", "2.0 GiB"). */
export function mebibytes(mb) {
  return Number.isFinite(mb) ? bytes(mb * 1048576) : '—';
}

/**
 * What a CI job or a CD run used (SPEC-154), as its process host measured it: "CPU 31 min · peak
 * 2.0 GiB · wrote 640 MiB". Empty when nothing was measured: unknown is never shown as zero.
 */
export function useText(detail) {
  const d = detail || {};
  return [
    Number.isFinite(d.cpu_ms) ? `CPU ${cpuTime(d.cpu_ms)}` : null,
    Number.isFinite(d.memory_peak_mb) ? `peak ${mebibytes(d.memory_peak_mb)}` : null,
    // Rounded to the nearest MiB: 0 is under half a MiB, not nothing.
    Number.isFinite(d.disk_written_mb) ? `wrote ${d.disk_written_mb === 0 ? 'under 1 MiB' : mebibytes(d.disk_written_mb)}` : null,
  ].filter(Boolean).join(' · ');
}

/** Figures worth a line in a tooltip or a table cell: tests, tokens, resource use, exit code, reason. */
export function figureText(span) {
  const d = span.detail || {};
  const parts = [];
  const tests = ['tests_passed', 'tests_failed', 'tests_flaky', 'tests_skipped'].filter((k) => Number.isFinite(d[k]));
  if (tests.length) parts.push(tests.map((k) => `${int(d[k])} ${k.slice(6)}`).join(', '));
  if (Number.isFinite(d.output_tokens) || Number.isFinite(d.input_tokens)) parts.push(`${int((d.input_tokens || 0) + (d.output_tokens || 0))} tokens`);
  if (Number.isFinite(d.active_ms) && d.active_ms > 0) parts.push(`agent work ${duration(d.active_ms)}`);
  if (Number.isFinite(d.cache_hits)) parts.push(`${int(d.cache_hits)} cache hits`);
  const used = useText(d);
  if (used) parts.push(used);
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

// A CI job's mark beside its name (SPEC-165 STR-003): its state as an icon with its word.
const JOB_MARKS = { running: 'refresh', passed: 'check', skipped: 'forward', failed: 'x', error: 'warn', cancelled: 'x' };
// A job that checked nothing (ADR-0133) ran, but is not drawn as one that passed.
const SKIPPED = 'skipped';

const time = (iso) => Date.parse(iso);
const jobId = (key, prefix) => (key.startsWith(prefix) && key.includes('@') ? key.slice(prefix.length, key.lastIndexOf('@')) : null);

/**
 * Each CI job's wait for a job slot, by its job span's key (SPEC-165): `ci::exec` records the
 * wait as `wait:<job id>@…` beside the job's `job:<job id>@…`, so they pair by the job id; a job
 * requeued waited more than once and takes its last wait before it started.
 */
function slotWaits(spans) {
  const waits = new Map();
  for (const span of spans) {
    const id = span.kind === 'wait' && span.name === 'job slot' ? jobId(span.key, 'wait:') : null;
    if (id) waits.set(id, [...(waits.get(id) || []), span]);
  }
  const paired = new Map();
  for (const span of spans) {
    const id = span.kind === 'job' ? jobId(span.key, 'job:') : null;
    const before = (id && waits.get(id) || []).filter((w) => time(w.started_at) <= time(span.started_at));
    if (before.length) paired.set(span.key, before.reduce((a, b) => (time(b.started_at) > time(a.started_at) ? b : a)));
  }
  return paired;
}

/**
 * The rows of a list of spans: groups (a CI run's own spans under "Run", its jobs under "Jobs" in
 * the order they began waiting), each a tree by `parent_key` with a job's slot wait on its row.
 */
function grouped(spans, heading) {
  const waits = slotWaits(spans);
  const waitKeys = new Set([...waits.values()].map((w) => w.key));
  const shown = spans.filter((s) => !waitKeys.has(s.key));
  const keys = new Set(shown.map((s) => s.key));
  const children = new Map();
  const roots = [];
  for (const span of shown) {
    if (span.parent_key && keys.has(span.parent_key)) children.set(span.parent_key, [...(children.get(span.parent_key) || []), span]);
    else roots.push(span);
  }
  const began = (span) => time((waits.get(span.key) || span).started_at);
  const jobs = roots.filter((s) => s.kind === 'job' && s.subject_kind === 'ci_run').sort((a, b) => began(a) - began(b));
  const tree = { children, waits };
  if (jobs.length === 0) return [{ heading, roots, tree }];
  const isJob = new Set(jobs);
  return [
    ...(heading ? [{ heading, roots: [], tree }] : []),
    { heading: 'Run', roots: roots.filter((s) => !isJob.has(s)), tree },
    { heading: 'Jobs', roots: jobs, tree, jobs: true },
  ];
}

// Axis steps: the smallest that leaves at most six intervals (SPEC-165 STR-005).
const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const TICK_STEPS = [1, 2, 5, 10, 15, 30].map((n) => n * SECOND)
  .concat([1, 2, 5, 10, 15, 30].map((n) => n * MINUTE), [1, 2, 3, 6, 12, 24, 48, 168].map((n) => n * HOUR));
const tickStep = (total) => TICK_STEPS.find((step) => total / step <= 6) || total / 6;

/** A tick's time from the start, exact at every step: "30 s", "1 min 30 s", "2 h", "1 h 30 min". */
function tickLabel(ms) {
  if (ms === 0) return '0';
  const s = Math.round(ms / SECOND);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return s % 60 ? `${m} min ${s % 60} s` : `${m} min`;
  const hours = Math.floor(m / 60);
  if (hours < 48) return m % 60 ? `${hours} h ${m % 60} min` : `${hours} h`;
  return hours % 24 ? `${Math.floor(hours / 24)} d ${hours % 24} h` : `${Math.floor(hours / 24)} d`;
}

/**
 * One subject's spans as a waterfall (and a table). `related` spans (a task's CI runs) follow
 * under their own heading. Empty when nothing was measured. `view` keeps what a person chose
 * (the spans they opened or closed, the table) across redraws of a running subject.
 */
export function waterfall(timeline, { now = Date.now(), title = 'Stages', view = { open: new Map(), table: false } } = {}) {
  const spans = (timeline && timeline.spans) || [];
  const related = (timeline && timeline.related) || [];
  const all = [...spans, ...related];
  if (all.length === 0) return h('p', { class: 'muted small st-empty' }, 'No stages measured yet.');
  const start = Math.min(...all.map((s) => time(s.started_at)));
  const end = Math.max(...all.map((s) => (s.ended_at ? time(s.ended_at) : now)), start + 1);
  const total = end - start;
  const running = all.some((s) => !s.ended_at);
  const step = tickStep(total);
  const at = (ms) => ((ms - start) / total) * 100;
  const tip = h('div', { class: 'chart-tip st-tip', hidden: true });
  const groups = [...grouped(spans, null), ...(related.length ? grouped(related, 'CI runs of its pull request') : [])];
  const anyWait = groups.some((g) => g.tree.waits.size > 0);
  // The bar looks in use, folded or not, for the legend.
  const used = {
    wait: anyWait || all.some((s) => WAIT_KINDS.has(s.kind)),
    ran: all.some((s) => s.ended_at && !WAIT_KINDS.has(s.kind) && !FAILED.has(s.outcome) && s.outcome !== SKIPPED),
    running: all.some((s) => !s.ended_at && !WAIT_KINDS.has(s.kind)),
    skipped: all.some((s) => s.outcome === SKIPPED && !WAIT_KINDS.has(s.kind)),
    failed: all.some((s) => FAILED.has(s.outcome)),
  };

  const bar = (from, to, classes) => h('span', {
    class: ['st-bar', ...classes],
    style: { left: `${at(from)}%`, width: `${Math.max(((to - from) / total) * 100, 0.4)}%` },
  });
  // What one span says: its words, how long it took (and waited), its outcome and figures.
  const describe = (span, inJobs, wait) => {
    const from = time(span.started_at);
    const to = span.ended_at ? time(span.ended_at) : now;
    const label = inJobs && span.kind === 'job' ? span.name : stageLabel(span);
    const took = span.ended_at ? duration(span.duration_ms) : `${duration(to - from)} so far`;
    const waited = wait ? duration(time(wait.ended_at || span.started_at) - time(wait.started_at)) : null;
    const figures = figureText(span);
    const said = `${label}: ${waited ? `waited ${waited} for a job slot, ran ${took}` : took}, ${outcomeWord(span)}${figures ? ` (${figures})` : ''}`;
    return { from, to, label, took, waited, figures, said, failed: FAILED.has(span.outcome) };
  };

  const rowOf = (span, depth, { inJobs, wait, kids, open, toggle }) => {
    const d = describe(span, inJobs, wait);
    const isWait = WAIT_KINDS.has(span.kind);
    const bars = [
      wait ? bar(time(wait.started_at), d.from, ['st-bar-wait', 'st-bar-lead']) : null,
      bar(d.from, d.to, [isWait && 'st-bar-wait', d.failed && 'st-bar-failed', !isWait && span.outcome === SKIPPED && 'st-bar-skipped', !span.ended_at && 'st-bar-running', span.source === 'backfilled' && 'st-bar-backfilled', wait && 'st-bar-tail']),
    ];
    const show = (event) => {
      setChildren(tip, h('span', null, `${d.label}${span.source === 'backfilled' ? ' · backfilled' : ''}`), h('strong', null, d.took),
        d.waited ? h('span', null, `waited ${d.waited} for a job slot`) : null, d.figures ? h('span', null, d.figures) : null);
      tip.hidden = false;
      const box = tip.parentElement.getBoundingClientRect();
      tip.style.left = `${Math.max(80, Math.min(box.width - 80, event.clientX - box.left))}px`;
      tip.style.top = `${event.clientY - box.top}px`;
    };
    const word = outcomeWord(span);
    const mark = span.kind === 'job' && JOB_MARKS[word] ? icon(JOB_MARKS[word], { size: 12, label: word })
      : d.failed ? icon('x', { size: 12, label: 'failed' }) : null;
    const name = [
      kids ? icon(open ? 'down' : 'right', { size: 12 }) : h('span', { class: 'st-fold-space', 'aria-hidden': 'true' }),
      mark ? h('span', { class: ['st-mark', `st-mark-${d.failed ? 'failed' : word}`] }, mark) : null,
      h('span', { class: 'st-name-text' }, d.label),
      kids ? h('span', { class: 'st-count' }, `${kids} ${kids === 1 ? 'stage' : 'stages'}`) : null,
    ];
    return h('li', { class: ['st-row', span.kind === 'job' && inJobs && 'st-job'], title: d.said, onPointerMove: show, onPointerLeave: () => { tip.hidden = true; } },
      kids
        ? h('button', { type: 'button', class: 'st-name st-fold', 'aria-expanded': String(open), style: { paddingLeft: `${depth * 14}px` }, onClick: toggle }, name)
        : h('span', { class: 'st-name', style: { paddingLeft: `${depth * 14}px` } }, name),
      h('span', { class: 'st-track', 'aria-hidden': 'true' }, bars),
      h('span', { class: ['st-took', d.failed && 'st-took-failed'] }, h('span', null, d.took), d.waited ? h('span', { class: 'st-waited' }, `waited ${d.waited}`) : null));
  };

  const list = h('ol', { class: 'st-rows', role: 'list', 'aria-label': `${title}: ${duration(total)} from the first start to the last end` });
  // A span opens by itself when it or anything under it failed, so a failure is never folded away.
  const failedWithin = (group, span) => FAILED.has(span.outcome) || (group.tree.children.get(span.key) || []).some((c) => failedWithin(group, c));
  const draw = () => {
    const rows = [];
    const walk = (group, span, depth) => {
      const kids = (group.tree.children.get(span.key) || []).length;
      const open = kids > 0 && (view.open.has(span.key) ? view.open.get(span.key) : failedWithin(group, span));
      rows.push(rowOf(span, depth, {
        inJobs: Boolean(group.jobs), wait: group.tree.waits.get(span.key), kids, open,
        toggle: () => { view.open.set(span.key, !open); draw(); [...list.children].find((r) => r.dataset.key === span.key)?.querySelector('.st-fold')?.focus(); },
      }));
      rows[rows.length - 1].dataset.key = span.key;
      if (open) for (const child of group.tree.children.get(span.key)) walk(group, child, depth + 1);
    };
    for (const group of groups) {
      if (group.heading) rows.push(h('li', { class: 'st-group' }, group.heading));
      group.roots.forEach((span) => walk(group, span, 0));
    }
    setChildren(list, rows);
  };
  draw();

  // The table: every span, folded or not, in the same groups.
  const tableRows = [];
  const tableRow = (group, span, depth) => {
    const wait = group.tree.waits.get(span.key);
    const d = describe(span, Boolean(group.jobs), wait);
    tableRows.push(h('tr', null,
      h('td', null, h('span', { style: { paddingLeft: `${depth * 14}px` } }, d.label)),
      h('td', null, new Date(d.from).toLocaleTimeString()),
      anyWait ? h('td', { class: 'num st-waited' }, d.waited || '') : null,
      h('td', { class: 'num' }, d.took),
      h('td', null, outcomeWord(span)),
      h('td', null, [d.figures, span.source === 'backfilled' ? 'backfilled' : ''].filter(Boolean).join(' · '))));
    for (const child of group.tree.children.get(span.key) || []) tableRow(group, child, depth + 1);
  };
  const columns = ['Stage', 'Started', anyWait ? 'Waited' : null, 'Took', 'Outcome', 'Figures'].filter(Boolean);
  for (const group of groups) {
    if (group.heading) tableRows.push(h('tr', null, h('th', { colspan: columns.length, scope: 'rowgroup' }, group.heading)));
    group.roots.forEach((span) => tableRow(group, span, 0));
  }
  const table = h('table', { class: 'table st-table', hidden: !view.table },
    h('thead', null, h('tr', null, columns.map((c) => h('th', { class: c === 'Took' || c === 'Waited' ? 'num' : null }, c)))),
    h('tbody', null, tableRows));

  // The axis: time from the subject's start, a faint line at each tick, "now" while it runs.
  const ticks = [];
  // The last tick keeps clear of the end, and of "now" while it runs.
  const last = running ? 84 : 92;
  for (let t = 0; at(start + t) < last; t += step) ticks.push(h('span', { class: 'st-tick', style: { left: `${at(start + t)}%` } }, tickLabel(t)));
  if (running) ticks.push(h('span', { class: 'st-tick st-tick-now', style: { left: '100%' } }, 'now'));
  const axis = h('div', { class: 'st-axis', 'aria-hidden': 'true' }, h('span'), h('span', { class: 'st-axis-ticks' }, ticks), h('span'));
  const legend = h('ul', { class: 'st-legend st-bars-legend', 'aria-label': 'What the bars mean' }, [
    used.wait && ['Waiting', 'st-bar-wait'], used.ran && ['Ran', null], used.running && ['Still running', 'st-bar-running'],
    used.skipped && ['Skipped', 'st-bar-skipped'], used.failed && ['Failed', 'st-bar-failed'],
  ].filter(Boolean).map(([text, cls]) => h('li', null, h('span', { class: ['st-swatch', cls], 'aria-hidden': 'true' }), text)));

  const plot = h('div', { class: 'st-plot', hidden: view.table, style: { '--st-step': `${(step / total) * 100}%` } }, legend, axis, list, tip);
  const toggle = h('button', { type: 'button', class: 'btn btn-sm db-table-toggle', 'aria-pressed': String(view.table), onClick: () => {
    view.table = !view.table;
    toggle.setAttribute('aria-pressed', String(view.table));
    table.hidden = !view.table;
    plot.hidden = view.table;
  } }, icon('tasks', { size: 14 }), 'Table');
  return h('section', { class: 'st-waterfall', 'aria-label': title },
    h('div', { class: 'st-head' }, h('h3', { class: 'st-title' }, title), h('span', { class: 'muted small' }, `${duration(total)} in all`), h('span', { class: 'spacer' }), toggle),
    timeline.truncated ? h('p', { class: 'muted small' }, 'Only the newest 2,000 stages are shown.') : null,
    plot, table);
}

const drawn = new WeakMap(); // host → the timeline it shows, so a poll that changed nothing keeps it
const views = new WeakMap(); // host → what a person opened, closed and toggled there

/**
 * Loads and draws one subject's waterfall into `host` (`subject`: `ci_run:<id>`, …). A timeline
 * that has not changed since the last load is left as it is (its table toggle, its hover); one
 * that has is redrawn with the spans opened or closed and the table as the person left them.
 */
export async function loadWaterfall(ctx, sandbox, subject, host, options = {}) {
  try {
    const timeline = await ctx.api.get(`/api/sandboxes/${encodeURIComponent(sandbox.id)}/stages/timeline?subject=${encodeURIComponent(subject)}`);
    const text = JSON.stringify(timeline);
    const running = [...(timeline.spans || []), ...(timeline.related || [])].some((s) => !s.ended_at);
    if (!running && drawn.get(host) === text && host.firstChild) return;
    drawn.set(host, text);
    const key = `${sandbox.id} ${subject}`;
    if (views.get(host)?.key !== key) views.set(host, { key, open: new Map(), table: false });
    // The control that had the keyboard keeps it through the redraw: a span's fold or the Table.
    const focused = host.contains(document.activeElement) ? document.activeElement : null;
    const row = focused?.closest('.st-row')?.dataset.key;
    const wasToggle = Boolean(focused?.closest('.db-table-toggle'));
    setChildren(host, waterfall(timeline, { ...options, view: views.get(host) }));
    const again = row ? [...host.querySelectorAll('.st-row')].find((r) => r.dataset.key === row)?.querySelector('.st-fold')
      : wasToggle ? host.querySelector('.db-table-toggle') : null;
    again?.focus();
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
