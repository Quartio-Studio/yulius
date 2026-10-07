// Dashboard (ANL-001..003): analytics summary tiles, simple inline SVG/CSS charts and
// CSV/JSON export. Unknown values render as "—", never as zero.

import { h, svgEl, setChildren, icon, iconButton } from '../dom.js';
import { compact, duration, int } from '../format.js';
import { harnessInfo, statusInfo } from '../status.js';
import { pageFrame, loader } from './common.js';

const RANGES = [[7, 'Last 7 days'], [30, 'Last 30 days'], [90, 'Last 90 days']];

function isoDay(date) {
  return date.toISOString().slice(0, 10);
}

/** Mount the Dashboard page. */
export function mountDashboardPage(ctx, sandbox) {
  let days = 30;
  const exportCsv = h('a', { class: 'btn btn-sm', download: '' }, icon('download', { size: 13 }), 'CSV');
  const exportJson = h('a', { class: 'btn btn-sm', download: '' }, icon('download', { size: 13 }), 'JSON');
  const frame = pageFrame({ title: 'Dashboard', sub: sandbox.name, actions: [exportCsv, exportJson, iconButton('refresh', 'Refresh', () => reload())] });
  const rangeInputs = RANGES.map(([value, label]) => {
    const input = h('input', { type: 'radio', name: 'dash-range', value: String(value), checked: value === days });
    input.addEventListener('change', () => { days = value; updateLinks(); reload(); });
    return h('label', null, input, label);
  });
  const host = h('div', { class: 'stack' });
  frame.body.append(h('div', { class: 'toolbar' }, h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Date range' }, rangeInputs)), host);

  // Inclusive UTC days: "last 7 days" is today and the 6 days before.
  function range() {
    const to = new Date();
    const from = new Date(to.getTime() - (days - 1) * 86400000);
    return { from: isoDay(from), to: isoDay(to) };
  }

  function query(extra = {}) {
    const { from, to } = range();
    return new URLSearchParams({ sandbox: sandbox.id, from, to, ...extra }).toString();
  }

  function updateLinks() {
    exportCsv.href = `/api/analytics/tasks?${query({ format: 'csv' })}`;
    exportJson.href = `/api/analytics/tasks?${query({ format: 'json' })}`;
    exportCsv.title = 'Per-task rows as CSV (for spreadsheet analysis)';
    exportJson.title = 'Per-task rows as JSON';
  }
  updateLinks();

  const reload = loader(host, () => ctx.api.get(`/api/analytics/summary?${query()}`), (data) => renderSummary(data || {}));
  return { el: frame.el, key: `dashboard:${sandbox.id}` };
}

/** Known input + output tokens of a `Tokens` group; null when neither was reported. */
function sumTokens(row) {
  const parts = [row.input, row.output].filter((v) => Number.isFinite(v));
  return parts.length ? parts.reduce((a, b) => a + b, 0) : null;
}

function stat(label, value, sub) {
  return h('div', { class: 'stat' }, h('span', { class: 'stat-label' }, label), h('span', { class: 'stat-value' }, value), sub ? h('span', { class: 'stat-sub' }, sub) : null);
}

function card(title, ...content) {
  return h('section', { class: 'card' }, h('div', { class: 'card-head' }, h('h2', { class: 'card-title' }, title)), h('div', { class: 'card-body' }, content));
}

/** Render `AnalyticsSummary` (docs/api.md "Administration, analytics, status"). */
export function renderSummary(data) {
  const tasks = data.tasks || {};
  const tokens = data.tokens || {};
  const durationStats = data.duration_ms || {};
  const switches = data.switches || {};
  const feedback = data.feedback || {};
  const perDay = Array.isArray(data.tasks_per_day) ? data.tasks_per_day : [];
  const fbTotal = (feedback.correct || 0) + (feedback.incorrect || 0);
  const totalTokens = sumTokens(tokens);

  const byStatus = Object.entries(tasks.by_status || {}).map(([status, count]) => ({ label: statusInfo({ status, activity: null }).label, value: count }));
  const byModel = (Array.isArray(data.tokens_by_model) ? data.tokens_by_model : []).map((r) => ({ label: r.model, value: sumTokens(r) }));
  const byHarness = (Array.isArray(data.tokens_by_harness) ? data.tokens_by_harness : []).map((r) => ({ label: harnessInfo(r.harness).name, value: sumTokens(r) }));
  const byAccount = (Array.isArray(data.tokens_by_account) ? data.tokens_by_account : []).map((r) => ({ label: r.account_label || (r.account_id ? 'deleted account' : 'no account'), value: sumTokens(r) }));
  const byOrigin = Object.entries(tasks.by_origin || {}).map(([origin, count]) => ({ label: origin, value: count }));
  const unknownRuns = Number.isFinite(tokens.unknown_usage_runs) ? tokens.unknown_usage_runs : 0;

  return [
    data.truncated ? h('div', { class: 'banner banner-warn small' }, icon('warn', { size: 14 }), h('span', { class: 'banner-text' }, 'This range has more than 50 000 tasks; the figures describe the newest 50 000.')) : null,
    h('div', { class: 'stat-grid' },
      stat('Tasks', int(tasks.total), [Number.isFinite(tasks.closed) ? `${tasks.closed} closed` : null, ...Object.entries(tasks.by_harness || {}).map(([k, v]) => `${harnessInfo(k).name} ${v}`)].filter(Boolean).join(' · ') || null),
      stat('Median duration', duration(durationStats.median ?? null), durationStats.p90 !== undefined ? `p90 ${duration(durationStats.p90)}` : null),
      stat('Active agent time', duration(durationStats.total_active ?? null), durationStats.total_wall !== undefined ? `wall ${duration(durationStats.total_wall)}` : null),
      stat('Tokens', compact(totalTokens), `in ${compact(tokens.input)} · out ${compact(tokens.output)}${Number.isFinite(tokens.cache_read) ? ` · cache read ${compact(tokens.cache_read)}` : ''}${unknownRuns ? ` · ${unknownRuns} run${unknownRuns === 1 ? '' : 's'} without usage` : ''}`),
      stat('Switches', int(switches.total), Number.isFinite(switches.provider) ? `${switches.account ?? 0} account · ${switches.provider} provider` : null),
      stat('Feedback correct', fbTotal ? `${Math.round(((feedback.correct || 0) / fbTotal) * 100)}%` : '—', fbTotal ? `${feedback.correct || 0} of ${fbTotal} rated` : 'no feedback yet')),
    card('Tasks per day', columnChart(perDay.map((d) => ({ label: d.date, value: d.count })), { unit: 'tasks' })),
    h('div', { class: 'grid-2' },
      card('Tokens by model', hbars(byModel, { format: compact, empty: 'No token data (unknown usage is not counted as zero).' })),
      card('Tokens by harness', hbars(byHarness, { format: compact })),
      card('Tokens by account', hbars(byAccount, { format: compact })),
      card('Tasks by status', hbars(byStatus, { format: int })),
      card('Tasks by origin', hbars(byOrigin, { format: int })),
      card('Feedback', feedbackRatio(feedback))),
  ];
}

/** Horizontal bars, one series (single hue), value at the bar tip. */
function hbars(rows, { format = int, empty = 'No data for this range.' } = {}) {
  const known = rows.filter((r) => Number.isFinite(r.value));
  if (known.length === 0) return h('div', { class: 'muted small' }, empty);
  const max = Math.max(...known.map((r) => r.value), 1);
  const sorted = [...known].sort((a, b) => b.value - a.value).slice(0, 8);
  return h('div', { class: 'hbars' }, sorted.map((r) => h('div', { class: 'hbar', title: `${r.label}: ${format(r.value)}` },
    h('span', { class: 'hbar-label' }, r.label),
    h('span', { class: 'hbar-track' }, h('span', { class: 'hbar-fill', style: { width: `${Math.max(0.5, (r.value / max) * 100)}%` } })),
    h('span', { class: 'hbar-value' }, format(r.value)))),
  known.length > 8 ? h('div', { class: 'muted xs' }, `+ ${known.length - 8} more in the export`) : null);
}

/** Correct vs incorrect: status colors always paired with glyph + label. */
function feedbackRatio(feedback) {
  const ok = feedback.correct || 0;
  const bad = feedback.incorrect || 0;
  const total = ok + bad;
  if (!total) return h('div', { class: 'muted small' }, 'No feedback yet. Use “Feedback…” in a task’s action menu.');
  return h('div', { class: 'stack-sm' },
    h('div', { class: 'ratio', role: 'img', 'aria-label': `${ok} correct, ${bad} incorrect` },
      ok ? h('span', { class: 'ratio-ok', style: { width: `${(ok / total) * 100}%` } }) : null,
      bad ? h('span', { class: 'ratio-bad', style: { width: `${(bad / total) * 100}%` } }) : null),
    h('div', { class: 'row-wrap small' },
      h('span', null, h('span', { style: { color: 'var(--ok)' } }, '✓ '), `${ok} correct`),
      h('span', null, h('span', { style: { color: 'var(--danger)' } }, '✕ '), `${bad} incorrect`)));
}

function niceMax(value) {
  if (value <= 5) return 5;
  const pow = 10 ** Math.floor(Math.log10(value));
  for (const step of [1, 2, 2.5, 5, 10]) if (step * pow >= value) return step * pow;
  return 10 * pow;
}

/** Column chart (single series): ≤ 24 px columns, 4 px rounded caps, hover tooltips, data table. */
function columnChart(rows, { unit = '' } = {}) {
  const wrap = h('div', { class: 'chart' });
  const tip = h('div', { class: 'chart-tip', hidden: true });
  const table = h('details', { class: 'small' }, h('summary', { class: 'muted', style: { cursor: 'pointer', marginTop: '8px' } }, 'Show data table'),
    h('table', { class: 'table', style: { marginTop: '6px' } }, h('thead', null, h('tr', null, h('th', null, 'Date'), h('th', { class: 'num' }, unit || 'Value'))),
      h('tbody', null, rows.map((r) => h('tr', null, h('td', null, r.label), h('td', { class: 'num' }, int(r.value)))))));
  if (rows.length === 0) return h('div', { class: 'muted small' }, 'No tasks in this range.');
  const height = 180;
  const pad = { top: 12, right: 8, bottom: 22, left: 34 };

  function draw() {
    const width = Math.max(260, wrap.clientWidth || 600);
    const plotW = width - pad.left - pad.right;
    const plotH = height - pad.top - pad.bottom;
    const values = rows.map((r) => (Number.isFinite(r.value) ? r.value : 0));
    const max = niceMax(Math.max(...values, 1));
    const band = plotW / rows.length;
    const barW = Math.max(2, Math.min(24, band - 2));
    const y = (v) => pad.top + plotH - (v / max) * plotH;
    const ticks = [0, max / 2, max];
    const svg = svgEl('svg', { viewBox: `0 0 ${width} ${height}`, width, height, role: 'img', 'aria-label': `Column chart of ${unit} per day` },
      ticks.map((t) => [
        svgEl('line', { class: t === 0 ? 'chart-baseline' : 'chart-grid', x1: pad.left, x2: width - pad.right, y1: y(t), y2: y(t) }),
        svgEl('text', { class: 'chart-axis', x: pad.left - 6, y: y(t) + 3, 'text-anchor': 'end' }, int(t)),
      ]),
      rows.map((r, i) => {
        const v = values[i];
        const x = pad.left + i * band + (band - barW) / 2;
        const top = y(v);
        const barH = Math.max(0, pad.top + plotH - top);
        const radius = Math.min(4, barW / 2, barH);
        const d = barH <= 0 ? '' : `M${x},${top + barH}V${top + radius}Q${x},${top} ${x + radius},${top}H${x + barW - radius}Q${x + barW},${top} ${x + barW},${top + radius}V${top + barH}Z`;
        const bar = svgEl('path', { class: 'chart-bar', d });
        const hit = svgEl('rect', { class: 'chart-hit', x: pad.left + i * band, y: pad.top, width: band, height: plotH });
        hit.addEventListener('pointerenter', () => {
          bar.classList.add('hover');
          setChildren(tip, h('strong', null, `${int(r.value)} ${unit}`), h('span', null, r.label));
          tip.hidden = false;
          tip.style.left = `${x + barW / 2}px`;
          tip.style.top = `${Math.max(0, top - 6)}px`;
        });
        hit.addEventListener('pointerleave', () => { bar.classList.remove('hover'); tip.hidden = true; });
        return [hit, bar];
      }),
      rows.map((r, i) => {
        const every = Math.ceil(rows.length / Math.max(2, Math.floor(plotW / 70)));
        if (i % every !== 0 && i !== rows.length - 1) return null;
        return svgEl('text', { class: 'chart-axis', x: pad.left + i * band + band / 2, y: height - 6, 'text-anchor': 'middle' }, String(r.label).slice(5));
      }));
    setChildren(wrap, svg, tip);
  }

  const observer = new ResizeObserver(() => draw());
  requestAnimationFrame(() => { draw(); observer.observe(wrap); });
  return h('div', null, wrap, table);
}
