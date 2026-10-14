// Dashboard (ANL-001..003, SPEC-001 UI-040): the analytics summary laid out like a modern product
// dashboard. One strip of key numbers, each with its change against the previous period of the
// same length (a second summary request) and a line of context; the daily chart as the hero, with
// the average, weekends and a readable hover; the outcomes as one split bar with a legend; the
// breakdowns as bar lists with shares. Unknown values render as "—", never as zero. CSV/JSON
// export of per-task rows stays one click away. The subscription pool (ROT-013) — capacity left,
// burn rate, forecast and whether to buy more — has its own card for the whole instance.

import { h, svgEl, setChildren, icon, iconButton, openMenu, bar } from '../dom.js';
import { compact, duration, int } from '../format.js';
import { statusInfo } from '../status.js';
import { pageFrame, loader } from './common.js';
import { modelLabel } from '../models.js';
import { accountUsage, capacityPanel } from '../layout.js';
import { sparkline } from '../charts.js';
import { replaceQuietly } from '../router.js';

const RANGES = [[7, '7 days'], [30, '30 days'], [90, '90 days']];
const DAY = 86400000;
// The API's days are UTC dates: format them in UTC so a date never shifts by the viewer's zone.
const SHORT_DATE = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
const LONG_DATE = new Intl.DateTimeFormat(undefined, { weekday: 'short', month: 'short', day: 'numeric', timeZone: 'UTC' });
const WEEKDAY = new Intl.DateTimeFormat(undefined, { weekday: 'short', timeZone: 'UTC' });
const ORIGIN_NAMES = { ui: 'Started here', automation: 'Automation', import: 'Imported', onboarding: 'Onboarding', agent: 'Another task' };

/** How often an open dashboard refreshes itself. */
const LIVE_MS = 30_000;
const isoDay = (date) => date.toISOString().slice(0, 10);
const parseDay = (day) => new Date(`${day}T00:00:00Z`);

/** Mount the Dashboard page. */
export function mountDashboardPage(ctx, sandbox, route) {
  // The period is kept in the address, so it comes back with the page (SPEC-113 KST-007).
  const asked = Number(route && route.query.get('days'));
  let days = RANGES.some(([value]) => value === asked) ? asked : 30;
  const base = `/s/${encodeURIComponent(sandbox.slug)}/dashboard`;
  const rangeLabel = h('span', { class: 'db-range' });
  const exportButton = h('button', { type: 'button', class: 'btn btn-sm', 'aria-haspopup': 'menu', onClick: () => openMenu(exportButton, [
    { label: 'Tasks as CSV', description: 'One row per task, for spreadsheets', onSelect: () => download('csv') },
    { label: 'Tasks as JSON', description: 'The same rows as JSON', onSelect: () => download('json') },
  ], { align: 'end' }) }, icon('download', { size: 14 }), 'Export');
  const range = h('div', { class: 'seg db-seg', role: 'radiogroup', 'aria-label': 'Date range' }, RANGES.map(([value, label]) => h('label', null,
    h('input', { type: 'radio', name: 'dash-range', value: String(value), checked: value === days, onChange: () => { days = value; replaceQuietly(days === 30 ? base : `${base}?days=${days}`); reload(); } }), label)));
  // The subtitle says what the page is for (#147); the dates of the chosen range sit with the range.
  const frame = pageFrame({ title: 'Dashboard', sub: 'what the tasks of this sandbox did, and what it took', actions: [rangeLabel, range, exportButton, iconButton('refresh', 'Refresh', () => reload())] });
  const host = h('div', { class: 'db' });
  frame.body.append(host);

  // Inclusive UTC days: "last 7 days" is today and the 6 days before; the previous period is the
  // same number of days just before it.
  function period(offset = 0) {
    const to = new Date(Date.now() - offset * days * DAY);
    const from = new Date(to.getTime() - (days - 1) * DAY);
    return { from: isoDay(from), to: isoDay(to) };
  }
  const query = (p, extra = {}) => new URLSearchParams({ sandbox: sandbox.id, from: p.from, to: p.to, ...extra }).toString();

  function download(format) {
    const a = h('a', { href: `/api/analytics/tasks?${query(period(), { format })}`, download: '' });
    document.body.append(a);
    a.click();
    a.remove();
  }

  const reload = loader(host, 'the dashboard', async () => {
    const current = period();
    rangeLabel.textContent = `${SHORT_DATE.format(parseDay(current.from))} – ${SHORT_DATE.format(parseDay(current.to))}`;
    // The previous period only adds the changes; the page still works without it.
    const [now, before, capacity] = await Promise.all([
      ctx.api.get(`/api/analytics/summary?${query(current)}`),
      ctx.api.get(`/api/analytics/summary?${query(period(1))}`).catch(() => null),
      ctx.api.get('/api/capacity').catch(() => null),
    ]);
    return { now: now || {}, before, days, capacity };
  }, ({ now, before, days: n, capacity }) => renderSummary(now, before, n, capacity), { shape: loadingLayout });
  // Live while open (#31): tokens and runs of tasks in progress change; refresh in place while
  // the tab is visible.
  const live = setInterval(() => {
    if (document.visibilityState === 'visible') reload({ quiet: true });
  }, LIVE_MS);
  return { el: frame.el, key: `dashboard:${sandbox.id}`, destroy: () => clearInterval(live) };
}

/** Known input + output tokens of a `Tokens` group; null when neither was reported. */
function sumTokens(row) {
  const parts = [row.input, row.output].filter((v) => Number.isFinite(v));
  return parts.length ? parts.reduce((a, b) => a + b, 0) : null;
}

/** The key numbers of one summary, so two periods compare the same way. */
function figures(data) {
  if (!data) return null;
  const tasks = data.tasks || {};
  const d = data.duration_ms || {};
  const fb = data.feedback || {};
  const rated = (fb.correct || 0) + (fb.incorrect || 0);
  return {
    tasks: Number.isFinite(tasks.total) ? tasks.total : null,
    work: Number.isFinite(d.median_active) ? d.median_active : null,
    active: Number.isFinite(d.total_active) ? d.total_active : null,
    tokens: sumTokens(data.tokens || {}),
    switches: data.switches && Number.isFinite(data.switches.total) ? data.switches.total : null,
    correct: rated ? (fb.correct || 0) / rated : null,
  };
}

/**
 * The change against the previous period: "▲ 12%" (or points for a share). `better` says which
 * direction is good ('up', 'down') or null when neither is (then it stays neutral).
 */
function delta(now, before, { better = 'up', points = false, days } = {}) {
  if (!Number.isFinite(now) || !Number.isFinite(before)) return null;
  const diff = points ? (now - before) * 100 : before === 0 ? null : ((now - before) / before) * 100;
  if (diff === null) return now > 0 ? h('span', { class: 'db-delta' }, 'new') : null;
  const rounded = Math.round(diff);
  const text = rounded === 0 ? '0%' : `${Math.abs(rounded)}${points ? ' pt' : '%'}`;
  const tone = rounded === 0 || !better ? 'flat' : (rounded > 0) === (better === 'up') ? 'good' : 'bad';
  const arrow = rounded > 0 ? '▲' : rounded < 0 ? '▼' : '';
  const said = rounded === 0 ? 'no change' : `${rounded > 0 ? 'up' : 'down'} ${text}`;
  return h('span', { class: ['db-delta', `db-delta-${tone}`], title: `vs the previous ${days} days` },
    arrow ? h('span', { 'aria-hidden': 'true' }, arrow) : null, h('span', { 'aria-hidden': 'true' }, text),
    h('span', { class: 'sr-only' }, `${said} vs the previous ${days} days`));
}

function kpi({ label, value, change, sub, extra, note, hint }) {
  return h('div', { class: 'db-kpi' },
    h('span', { class: 'db-kpi-label', title: hint || null }, label),
    h('div', { class: 'db-kpi-row' }, h('span', { class: 'db-kpi-value' }, value), change),
    extra || null,
    sub ? h('span', { class: 'db-kpi-sub' }, sub) : null,
    note ? h('span', { class: 'db-kpi-note' }, note) : null);
}

function card(title, sub, body, { tools, className } = {}) {
  return h('section', { class: ['card', 'db-card', className] },
    h('header', { class: 'db-card-head' }, h('div', { class: 'db-card-titles' }, h('h2', { class: 'card-title' }, title), sub ? h('span', { class: 'db-card-sub' }, sub) : null), tools || null),
    body);
}

/**
 * The subscription pool for the whole instance (`CapacityView`, ROT-013): the recommendation and
 * its numbers, then each subscription's windows with use and reset times.
 */
function subscriptions(capacity) {
  if (!capacity) return null;
  const accounts = Array.isArray(capacity.accounts) ? capacity.accounts : [];
  return card('Subscriptions', 'whole instance · Claude subscriptions in one pool', h('div', { class: 'stack' },
    capacityPanel(capacity),
    accounts.length ? h('div', { class: 'account-list' }, accounts.map((a, i) => accountUsage({ ...a, id: a.id || `pool-${i}`, provider: 'anthropic' }, { foldable: true, open: false }))) : null));
}

/**
 * The page while its figures load (SPEC-120): the six key figures in their strip, the
 * Subscriptions card (shown whenever the capacity is read) and chart cards. Its own stack: the
 * page's `.db` host already holds the container.
 */
function loadingLayout() {
  const titles = () => h('div', { class: 'db-card-titles' }, bar(40), bar(60));
  const chartCard = () => h('div', { class: 'card db-card' }, titles(), h('div', { class: 'skeleton-chart' }));
  return h('div', { class: 'stack' },
    // Tokens (the fourth figure) has a note line under its sub line: what came from the cache.
    h('div', { class: 'card db-kpis' }, Array.from({ length: 6 }, (_, i) => h('div', { class: 'db-kpi' },
      h('div', { class: 'db-kpi-label' }, bar(60)), h('div', { class: 'db-kpi-value' }, bar(50)), h('div', { class: 'skeleton-spark' }),
      h('div', { class: 'db-kpi-sub' }, bar(70)), i === 3 ? h('div', { class: 'db-kpi-note' }, bar(50)) : null))),
    h('div', { class: 'card db-card' }, titles(), h('div', { class: 'skeleton-field' })),
    h('div', { class: 'db-row' }, chartCard(), chartCard()));
}

/** Render `AnalyticsSummary` (docs/api.md "Administration, analytics, status"). */
export function renderSummary(data, previous = null, days = 30, capacity = null) {
  const tasks = data.tasks || {};
  const tokens = data.tokens || {};
  const d = data.duration_ms || {};
  const feedback = data.feedback || {};
  const perDay = Array.isArray(data.tasks_per_day) ? data.tasks_per_day : [];
  const now = figures(data);
  const before = figures(previous);
  const was = (key) => (before ? before[key] : null);
  const rated = (feedback.correct || 0) + (feedback.incorrect || 0);
  // A chart of the range's days beside each figure whose days mean something (SPEC-110 TUS-009):
  // sums read a day without runs as 0, the median leaves a day without tasks as a gap.
  const trend = (name, pick, format) => sparkline(perDay.map(pick), { label: `${name}, last ${days} days`, format });
  const orZero = (v) => (Number.isFinite(v) ? v : 0);
  const unknownRuns = Number.isFinite(tokens.unknown_usage_runs) ? tokens.unknown_usage_runs : 0;

  // `done` counts running tasks whose goal is achieved: finished, though their session is open.
  const byStatus = Object.entries(tasks.by_status || {}).map(([status, count]) => ({ status, info: status === 'done' ? statusInfo({ status: 'running', activity: 'done' }) : statusInfo({ status, activity: null }), value: count }));
  const byModel = (Array.isArray(data.tokens_by_model) ? data.tokens_by_model : []).map((r) => ({ label: r.model === 'unknown' ? 'Unknown model' : modelLabel(r.model), value: sumTokens(r) }));
  const byAccount = (Array.isArray(data.tokens_by_account) ? data.tokens_by_account : []).map((r) => ({ label: r.account_label || (r.account_id ? 'Removed subscription' : 'No subscription'), value: sumTokens(r) }));
  const byOrigin = Object.entries(tasks.by_origin || {}).map(([origin, count]) => ({ label: ORIGIN_NAMES[origin] || origin, value: count }));

  const feedbackBar = rated ? h('div', { class: 'db-ratio', 'aria-hidden': 'true' },
    h('span', { class: 'db-ratio-ok', style: { width: `${((feedback.correct || 0) / rated) * 100}%` } }),
    h('span', { class: 'db-ratio-bad', style: { width: `${((feedback.incorrect || 0) / rated) * 100}%` } })) : null;
  const tokenNotes = [Number.isFinite(tokens.cache_read) ? `${compact(tokens.cache_read)} read from cache` : null, unknownRuns ? `${unknownRuns} run${unknownRuns === 1 ? '' : 's'} reported no usage` : null].filter(Boolean).join(' · ');

  return [
    data.truncated ? h('div', { class: 'banner banner-warn small' }, icon('warn', { size: 14 }), h('span', { class: 'banner-text' }, `This range has more than ${int(50000)} tasks; the figures describe the newest ${int(50000)}.`)) : null,
    h('section', { class: 'card db-kpis', 'aria-label': 'Key figures' },
      kpi({ label: 'Tasks', value: int(tasks.total), change: delta(now.tasks, was('tasks'), { days }), sub: Number.isFinite(tasks.closed) ? `${int(tasks.closed)} closed` : null, extra: trend('Tasks', (x) => x.count || 0, int) }),
      // Agent work is the time Claude Code spent in its turns (waiting for its own background agents
      // included), never how long a session stayed open while idle (SPEC-087 MET-005).
      kpi({ label: 'Agent work per task', value: duration(d.median_active ?? null), change: delta(now.work, was('work'), { better: null, days }), extra: trend('Agent work per task', (x) => (Number.isFinite(x.median_active_ms) ? x.median_active_ms : null), duration), sub: Number.isFinite(d.p90_active) ? `median · 9 in 10 within ${duration(d.p90_active)}` : 'median', hint: 'The median of each task’s agent working time: the time Claude Code spent in its turns' }),
      kpi({ label: 'Agent working time', value: duration(d.total_active ?? null), change: delta(now.active, was('active'), { better: null, days }), extra: trend('Agent working time', (x) => orZero(x.active_ms), duration), sub: d.total_wall !== undefined ? `sessions open ${duration(d.total_wall)}, idle included` : null, hint: 'All tasks’ agent working time; sessions stay open while they wait for a person' }),
      kpi({ label: 'Tokens', value: compact(now.tokens), change: delta(now.tokens, was('tokens'), { better: null, days }), extra: trend('Tokens', (x) => orZero(x.input) + orZero(x.output), compact), sub: `${compact(tokens.input)} in · ${compact(tokens.output)} out, sub-agents included`, hint: tokenNotes || null,
        note: tokenNotes || null }),
      kpi({ label: 'Subscription switches', value: int(switches(data)), change: delta(now.switches, was('switches'), { better: 'down', days }), extra: trend('Subscription switches', (x) => x.switches || 0, int), sub: 'made by the pool, between subscriptions' }),
      kpi({ label: 'Feedback correct', value: rated ? `${Math.round(now.correct * 100)}%` : '—', change: delta(now.correct, was('correct'), { points: true, days }), extra: feedbackBar, sub: rated ? `${feedback.correct || 0} of ${rated} rated` : 'no feedback yet' })),
    subscriptions(capacity),
    h('div', { class: 'db-row' },
      tasksPerDay(perDay, tasks.total),
      outcomes(byStatus)),
    h('div', { class: 'db-row db-row-3' },
      card('Tokens by model', 'input + output', barList(byModel, { format: compact, empty: 'No token data. Unknown usage is not counted as zero.' })),
      card('Tokens by subscription', 'input + output', barList(byAccount, { format: compact })),
      card('Where tasks start', null, barList(byOrigin, { format: int }))),
  ];
}

const switches = (data) => (data.switches && Number.isFinite(data.switches.total) ? data.switches.total : null);

/** Bar list: the bar sits behind the label, the value and its share on the right. */
function barList(rows, { format = int, empty = 'No data for this range.', limit = 6 } = {}) {
  const known = rows.filter((r) => Number.isFinite(r.value));
  if (known.length === 0) return h('p', { class: 'db-empty' }, empty);
  const total = known.reduce((a, r) => a + r.value, 0) || 1;
  const max = Math.max(...known.map((r) => r.value), 1);
  const sorted = [...known].sort((a, b) => b.value - a.value);
  return h('div', null,
    h('ul', { class: 'db-bars' }, sorted.slice(0, limit).map((r) => h('li', { class: 'db-bar' },
      h('span', { class: 'db-bar-fill', style: { width: `${Math.max(1, (r.value / max) * 100)}%` }, 'aria-hidden': 'true' }),
      h('span', { class: 'db-bar-label' }, r.label),
      h('span', { class: 'db-bar-value' }, format(r.value), h('span', { class: 'db-bar-share' }, `${Math.round((r.value / total) * 100)}%`))))),
    sorted.length > limit ? h('p', { class: 'db-more' }, `+ ${sorted.length - limit} more in the export`) : null);
}

/** Tone class of a task status for the outcome bar: done, stopped, problem, failed, live. */
function outcomeTone(info) {
  if (info.tone === 'closed' || info.tone === 'done') return 'done';
  if (info.tone === 'failed') return 'failed';
  if (info.tone === 'stopped-abnormal') return 'problem';
  if (info.group === 'ended') return 'stopped';
  return 'live';
}

/** Outcomes: one split bar (glyph and label beside every colour) and a legend with shares. */
function outcomes(rows) {
  const known = rows.filter((r) => Number.isFinite(r.value) && r.value > 0).sort((a, b) => b.value - a.value);
  const total = known.reduce((a, r) => a + r.value, 0);
  if (!total) return card('Outcomes', null, h('p', { class: 'db-empty' }, 'No tasks in this range.'));
  const done = known.filter((r) => outcomeTone(r.info) === 'done').reduce((a, r) => a + r.value, 0);
  return card('Outcomes', `${Math.round((done / total) * 100)}% done or closed`, h('div', { class: 'db-outcomes' },
    h('div', { class: 'db-split', 'aria-hidden': 'true' }, known.map((r) => h('span', { class: ['db-split-part', `db-t-${outcomeTone(r.info)}`], style: { width: `${(r.value / total) * 100}%` }, title: `${r.info.label}: ${int(r.value)}` }))),
    h('ul', { class: 'db-legend' }, known.map((r) => h('li', null,
      h('span', { class: ['db-dot', `db-t-${outcomeTone(r.info)}`], 'aria-hidden': 'true' }),
      h('span', { class: 'db-legend-label' }, r.info.label),
      h('span', { class: 'db-legend-value' }, int(r.value)),
      h('span', { class: 'db-legend-share' }, `${Math.round((r.value / total) * 100)}%`))))), { className: 'db-side' });
}

function niceMax(value) {
  if (value <= 4) return 4;
  const pow = 10 ** Math.floor(Math.log10(value));
  for (const step of [1, 2, 2.5, 5, 10]) if (step * pow >= value) return step * pow;
  return 10 * pow;
}

/** The hero chart: columns per day, weekends shaded, the average dashed, today marked as partial. */
function tasksPerDay(rows, total) {
  const values = rows.map((r) => (Number.isFinite(r.count) ? r.count : 0));
  const avg = values.length ? values.reduce((a, b) => a + b, 0) / values.length : 0;
  const tableToggle = h('button', { type: 'button', class: 'btn btn-sm db-table-toggle', 'aria-pressed': 'false', onClick: () => {
    const on = tableToggle.getAttribute('aria-pressed') !== 'true';
    tableToggle.setAttribute('aria-pressed', String(on));
    table.hidden = !on;
    wrap.hidden = on;
  } }, icon('tasks', { size: 14 }), 'Table');
  const wrap = h('div', { class: 'chart db-chart' });
  const table = h('table', { class: 'table db-table', hidden: true }, h('thead', null, h('tr', null, h('th', null, 'Day'), h('th', { class: 'num' }, 'Tasks'))),
    h('tbody', null, rows.map((r) => h('tr', null, h('td', null, LONG_DATE.format(parseDay(r.date))), h('td', { class: 'num' }, int(r.count))))));
  // Days are UTC dates, as the range's are: said, so a task made after midnight local time is not
  // looked for on the wrong day.
  const sub = rows.length ? `${int(total ?? values.reduce((a, b) => a + b, 0))} tasks · ${avg.toFixed(1)} a day on average · UTC days` : null;
  if (!rows.length) return card('Tasks per day', null, h('p', { class: 'db-empty' }, 'No tasks in this range.'), { className: 'db-main' });

  const tip = h('div', { class: 'chart-tip db-tip', hidden: true });
  const height = 220;
  const pad = { top: 16, right: 44, bottom: 26, left: 28 };
  const today = isoDay(new Date());

  function draw() {
    const width = Math.max(280, wrap.clientWidth || 600);
    const plotW = width - pad.left - pad.right;
    const plotH = height - pad.top - pad.bottom;
    const max = niceMax(Math.max(...values, 1));
    const band = plotW / rows.length;
    const barW = Math.max(2, Math.min(28, band * 0.62));
    const y = (v) => pad.top + plotH - (v / max) * plotH;
    const bottom = pad.top + plotH;
    // X labels: every day when there is room for weekday names, else Mondays (then every other).
    const daily = band >= 34;
    let mondayEvery = 1;
    while (!daily && band * 7 * mondayEvery < 56) mondayEvery += 1;
    let mondaySeen = 0;
    const hoverBand = svgEl('rect', { class: 'db-hover-band', x: 0, y: pad.top, width: band, height: plotH, rx: 4, visibility: 'hidden' });
    const svg = svgEl('svg', { viewBox: `0 0 ${width} ${height}`, width, height, role: 'img', 'aria-label': `Tasks per day: ${sub}. The table has every day.` },
      // weekends
      rows.map((r, i) => {
        const wd = parseDay(r.date).getUTCDay();
        return wd === 0 || wd === 6 ? svgEl('rect', { class: 'db-weekend', x: pad.left + i * band, y: pad.top, width: band, height: plotH }) : null;
      }),
      [max / 2, max].map((t) => [
        svgEl('line', { class: 'chart-grid db-grid', x1: pad.left, x2: width - pad.right, y1: y(t), y2: y(t) }),
        svgEl('text', { class: 'chart-axis', x: pad.left - 8, y: y(t) + 4, 'text-anchor': 'end' }, int(t)),
      ]),
      svgEl('line', { class: 'chart-baseline', x1: pad.left, x2: width - pad.right, y1: bottom, y2: bottom }),
      hoverBand,
      rows.map((r, i) => {
        const v = values[i];
        const x = pad.left + i * band + (band - barW) / 2;
        const top = y(v);
        const barH = Math.max(0, bottom - top);
        const radius = Math.min(4, barW / 2, barH);
        const d = barH <= 0 ? '' : `M${x},${bottom}V${top + radius}Q${x},${top} ${x + radius},${top}H${x + barW - radius}Q${x + barW},${top} ${x + barW},${top + radius}V${bottom}Z`;
        return svgEl('path', { class: ['chart-bar', 'db-bar-col', r.date === today && 'db-partial'].filter(Boolean).join(' '), d });
      }),
      avg > 0 ? [
        svgEl('line', { class: 'db-avg', x1: pad.left, x2: width - pad.right, y1: y(avg), y2: y(avg) }),
        svgEl('text', { class: 'chart-axis db-avg-label', x: width - pad.right + 6, y: y(avg) + 4 }, `avg ${avg.toFixed(1)}`),
      ] : null,
      rows.map((r, i) => {
        const date = parseDay(r.date);
        const cx = pad.left + i * band + band / 2;
        if (daily) return svgEl('text', { class: 'chart-axis', x: cx, y: height - 8, 'text-anchor': 'middle' }, rows.length <= 10 ? WEEKDAY.format(date) : String(date.getUTCDate()));
        if (date.getUTCDay() !== 1) return null;
        mondaySeen += 1;
        if ((mondaySeen - 1) % mondayEvery !== 0) return null;
        return svgEl('text', { class: 'chart-axis', x: cx, y: height - 8, 'text-anchor': 'middle' }, SHORT_DATE.format(date));
      }));
    // One hover target over the plot: the nearest day shows its band and a tooltip.
    const hit = svgEl('rect', { class: 'chart-hit', x: pad.left, y: pad.top, width: plotW, height: plotH });
    const show = (event) => {
      const box = svg.getBoundingClientRect();
      const i = Math.max(0, Math.min(rows.length - 1, Math.floor(((event.clientX - box.left) * (width / box.width) - pad.left) / band)));
      const r = rows[i];
      hoverBand.setAttribute('x', String(pad.left + i * band));
      hoverBand.setAttribute('visibility', 'visible');
      setChildren(tip, h('span', { class: 'db-tip-date' }, `${LONG_DATE.format(parseDay(r.date))}${r.date === today ? ' · so far' : ''}`), h('strong', null, `${int(values[i])} ${values[i] === 1 ? 'task' : 'tasks'}`));
      tip.hidden = false;
      const left = ((pad.left + i * band + band / 2) / width) * box.width;
      tip.style.left = `${Math.max(56, Math.min(box.width - 56, left))}px`;
      tip.style.top = `${(Math.min(y(values[i]), bottom - 8) / height) * box.height}px`;
    };
    hit.addEventListener('pointermove', show);
    hit.addEventListener('pointerleave', () => { hoverBand.setAttribute('visibility', 'hidden'); tip.hidden = true; });
    svg.append(hit);
    setChildren(wrap, svg, tip);
  }

  const observer = new ResizeObserver(() => draw());
  requestAnimationFrame(() => { draw(); observer.observe(wrap); });
  return card('Tasks per day', sub, h('div', null, wrap, table), { tools: tableToggle, className: 'db-main' });
}
