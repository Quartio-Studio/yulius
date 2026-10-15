// Pipeline (SPEC-137, ADR-0112): how long every stage of this project's tasks, CI runs and CD runs
// takes, beside CI (tests) and CD (deployment). The delivery figures of dev and production first
// (deployment frequency, lead time, change failure rate, time to restore), then the chosen
// stage's daily p50 and p90, then every stage's figures, which choose the trend. Filters (range,
// what, environment, job) and the chosen stage are kept in the address; `?open=<kind>:<id>` shows
// one run's, task's or deployment's waterfall on top. CSV/JSON export like the Dashboard's.

import { h, setChildren, icon, iconButton, openMenu, bar } from '../dom.js';
import { duration, int } from '../format.js';
import { replaceQuietly } from '../router.js';
import { pageFrame, loader } from './common.js';
import { trendChart, stageTitle, loadWaterfall, subjectLabel } from '../stages.js';

const RANGES = [[7, '7 days'], [30, '30 days'], [90, '90 days']];
const DAY = 86400000;
// Every stage of the range is read: refreshed by itself every 5 minutes (Refresh reads now).
const LIVE_MS = 5 * 60_000;
const SHORT_DATE = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
const isoDay = (date) => date.toISOString().slice(0, 10);
const ENV_NAMES = { dev: 'Dev', prod: 'Production' };
const OPEN_KINDS = { ci_run: 'CI run', cd_run: 'Deployment', task: 'Task' };

/** Mount the Pipeline page. */
export function mountPipelinePage(ctx, sandbox, route) {
  const query = route ? route.query : new URLSearchParams();
  const asked = Number(query.get('days'));
  const state = {
    days: RANGES.some(([value]) => value === asked) ? asked : 30,
    subject: query.get('subject') || '',
    environment: query.get('environment') || '',
    job: query.get('job') || '',
    trend: query.get('trend') || '',
    open: query.get('open') || '',
  };
  const base = `/s/${encodeURIComponent(sandbox.slug)}/pipeline`;
  const api = `/api/sandboxes/${encodeURIComponent(sandbox.id)}`;
  let options = { jobs: [], environments: [] };

  function remember() {
    const params = new URLSearchParams();
    if (state.days !== 30) params.set('days', String(state.days));
    for (const key of ['subject', 'environment', 'job', 'trend', 'open']) if (state[key]) params.set(key, state[key]);
    const text = params.toString();
    replaceQuietly(text ? `${base}?${text}` : base);
  }

  function period() {
    const to = new Date();
    const from = new Date(to.getTime() - (state.days - 1) * DAY);
    return { from: isoDay(from), to: isoDay(to) };
  }

  function params(extra = {}) {
    const p = period();
    const out = new URLSearchParams({ from: p.from, to: p.to });
    for (const key of ['subject', 'environment', 'job', 'trend']) if (state[key]) out.set(key, state[key]);
    for (const [k, v] of Object.entries(extra)) out.set(k, v);
    return out.toString();
  }

  function download(format) {
    const a = h('a', { href: `${api}/stages/spans?${params({ format })}`, download: '' });
    document.body.append(a);
    a.click();
    a.remove();
  }

  const rangeLabel = h('span', { class: 'db-range' });
  const exportButton = h('button', { type: 'button', class: 'btn btn-sm', 'aria-haspopup': 'menu', onClick: () => openMenu(exportButton, [
    { label: 'Stages as CSV', description: 'One row per measured stage, for spreadsheets', onSelect: () => download('csv') },
    { label: 'Stages as JSON', description: 'The same rows as JSON', onSelect: () => download('json') },
  ], { align: 'end' }) }, icon('download', { size: 14 }), 'Export');
  const range = h('div', { class: 'seg db-seg', role: 'radiogroup', 'aria-label': 'Date range' }, RANGES.map(([value, label]) => h('label', null,
    h('input', { type: 'radio', name: 'pipeline-range', value: String(value), checked: value === state.days, onChange: () => { state.days = value; remember(); reload(); } }), label)));
  const frame = pageFrame({ title: 'Pipeline', sub: 'how long every stage of tasks, CI and CD takes', actions: [rangeLabel, range, exportButton, iconButton('refresh', 'Refresh', () => reload())] });
  const openHost = h('div');
  const filters = h('div', { class: 'pl-filters', role: 'group', 'aria-label': 'Filters' });
  const host = h('div', { class: 'db' });
  frame.body.append(openHost, filters, host);

  function select(label, key, choices) {
    return h('label', { class: 'pl-filter' }, h('span', { class: 'pl-filter-label' }, label),
      h('select', { class: 'select', 'aria-label': label, onChange: (event) => { state[key] = event.target.value; state.trend = ''; remember(); reload(); } },
        choices.map(([value, text]) => h('option', { value, selected: value === state[key] }, text))));
  }

  function renderFilters() {
    const jobs = [...new Set([...options.jobs, state.job].filter(Boolean))];
    const envs = [...new Set([...options.environments, state.environment].filter(Boolean))];
    setChildren(filters,
      select('What', 'subject', [['', 'Tasks, CI and CD'], ['task', 'Tasks'], ['ci_run', 'CI runs'], ['cd_run', 'CD runs']]),
      select('Environment', 'environment', [['', 'Every environment'], ...envs.map((e) => [e, ENV_NAMES[e] || e])]),
      select('Job', 'job', [['', 'Every job'], ...jobs.map((j) => [j, j])]));
  }

  function showOpen() {
    if (!state.open) { setChildren(openHost); return; }
    const [kind, ...rest] = state.open.split(':');
    const id = rest.join(':');
    const body = h('div', null, h('div', { class: 'skeleton-field' }));
    const close = iconButton('x', 'Close the timeline', () => { state.open = ''; remember(); showOpen(); });
    setChildren(openHost, h('section', { class: 'card db-card', 'aria-label': 'Timeline' },
      h('header', { class: 'db-card-head' }, h('div', { class: 'db-card-titles' },
        h('h2', { class: 'card-title' }, `${OPEN_KINDS[kind] || 'Subject'} timeline`),
        h('span', { class: 'db-card-sub' }, h('code', null, id.slice(0, 12)))), close),
      body));
    loadWaterfall(ctx, sandbox, state.open, body, { title: `${OPEN_KINDS[kind] || 'Subject'} stages` });
  }

  const reload = loader(host, 'the pipeline', async () => {
    const p = period();
    rangeLabel.textContent = `${SHORT_DATE.format(new Date(`${p.from}T00:00:00Z`))} – ${SHORT_DATE.format(new Date(`${p.to}T00:00:00Z`))}`;
    return ctx.api.get(`${api}/stages/summary?${params()}`);
  }, (summary) => {
    options = summary.options || options;
    renderFilters();
    return render(summary);
  }, { shape: loadingLayout });

  function chooseTrend(row) {
    state.trend = `${row.subject_kind}:${row.kind}:${row.name}`;
    remember();
    reload({ quiet: true });
  }

  function render(summary) {
    const trend = summary.trend || { days: [] };
    const trendRow = (summary.stages || []).find((r) => r.subject_kind === trend.subject_kind && r.kind === trend.kind && r.name === trend.name);
    const label = stageTitle(trendRow || trend);
    const chart = trendChart(trend, { label });
    return [
      summary.truncated ? h('div', { class: 'banner banner-warn small' }, icon('warn', { size: 14 }), h('span', { class: 'banner-text' }, 'This range has more stages than are read at once; the figures describe the newest.')) : null,
      deliveryCard(summary.delivery || []),
      h('section', { class: 'card db-card', 'aria-label': `Trend: ${label}` },
        h('header', { class: 'db-card-head' },
          h('div', { class: 'db-card-titles' }, h('h2', { class: 'card-title' }, label),
            h('span', { class: 'db-card-sub' }, trendRow ? `${int(trendRow.count)} ended · p50 ${duration(trendRow.p50_ms)} · p90 ${duration(trendRow.p90_ms)} · UTC days` : 'UTC days · choose a stage below')),
          h('div', { class: 'row-wrap' }, chart.legend, chart.toggle)),
        chart.el),
      stagesCard(summary),
    ];
  }

  function stagesCard(summary) {
    const rows = summary.stages || [];
    const reused = summary.reused_runs || 0;
    const sub = [`${int(rows.length)} stages`, reused ? `${int(reused)} reused CI ${reused === 1 ? 'result' : 'results'} not counted` : null].filter(Boolean).join(' · ');
    if (rows.length === 0) {
      return h('section', { class: 'card db-card', 'aria-label': 'Stages' }, h('h2', { class: 'card-title' }, 'Stages'),
        h('p', { class: 'db-empty' }, 'Nothing measured in this range yet. Stages are recorded as tasks, CI runs and deployments happen.'));
    }
    const current = summary.trend || {};
    return h('section', { class: 'card db-card', 'aria-label': 'Stages' },
      h('header', { class: 'db-card-head' }, h('div', { class: 'db-card-titles' }, h('h2', { class: 'card-title' }, 'Stages'), h('span', { class: 'db-card-sub' }, sub))),
      h('div', { class: 'table-wrap' }, h('table', { class: 'table pl-table' },
        h('thead', null, h('tr', null,
          h('th', null, 'Stage'), h('th', { class: 'hide-phone' }, 'Job or environment'), h('th', { class: 'num' }, 'Ended'), h('th', { class: 'num' }, 'Failed'),
          h('th', { class: 'num' }, 'p50'), h('th', { class: 'num' }, 'p90'), h('th', { class: 'num hide-phone' }, 'Longest'), h('th', { class: 'num hide-phone' }, 'Total'))),
        h('tbody', null, rows.map((row) => {
          const chosen = row.subject_kind === current.subject_kind && row.kind === current.kind && row.name === current.name;
          return h('tr', { class: chosen ? 'pl-chosen' : null },
            h('td', null, h('button', { type: 'button', class: 'link-btn', 'aria-pressed': String(chosen), title: 'Show its trend', onClick: () => chooseTrend(row) }, stageTitle(row)),
              row.backfilled ? h('span', { class: 'muted small', title: 'Some of these were derived from earlier records, before stages were measured' }, ` · ${int(row.backfilled)} backfilled`) : null,
              row.running ? h('span', { class: 'muted small' }, ` · ${int(row.running)} running`) : null),
            h('td', { class: 'hide-phone' }, row.job || (row.environment ? ENV_NAMES[row.environment] || row.environment : '')),
            h('td', { class: 'num' }, int(row.count)),
            h('td', { class: ['num', row.failed && 'pl-failed'] }, row.failed ? int(row.failed) : '—'),
            h('td', { class: 'num' }, duration(row.p50_ms)),
            h('td', { class: 'num' }, duration(row.p90_ms)),
            h('td', { class: 'num hide-phone' }, duration(row.max_ms)),
            h('td', { class: 'num hide-phone' }, duration(row.total_ms)));
        })))),
      h('p', { class: 'muted small' }, `${subjectLabel('task')} stages follow each task’s states, steps, runs, pauses and delivery; CI and CD stages are their queue, jobs, deploys and the stages their output names (::yulius-stage).`));
  }

  function deliveryCard(envs) {
    const tiles = (env) => {
      const failure = Number.isFinite(env.change_failure_rate) ? `${Math.round(env.change_failure_rate * 100)}%` : '—';
      return [
        tile('Deployments', Number.isFinite(env.per_week) ? `${env.per_week.toFixed(1)} a week` : '—', `${int(env.passed)} passed of ${int(env.deployments)}`),
        tile('Lead time', duration(env.lead_time_p50_ms), env.lead_time_count ? `median from merge to deployed · p90 ${duration(env.lead_time_p90_ms)}` : 'no merge deployed yet'),
        tile('Change failure rate', failure, env.deployments ? `${int(env.deployments - env.passed)} of ${int(env.deployments)} failed` : 'no deployments'),
        tile('Time to restore', duration(env.time_to_restore_p50_ms), env.restores ? `median of ${int(env.restores)}` : 'no failure restored'),
      ];
    };
    return h('section', { class: 'card db-card', 'aria-label': 'Delivery' },
      h('header', { class: 'db-card-head' }, h('div', { class: 'db-card-titles' }, h('h2', { class: 'card-title' }, 'Delivery'),
        h('span', { class: 'db-card-sub' }, 'deployments, lead time from merge, change failure rate and time to restore'))),
      envs.map((env) => h('div', { class: 'pl-env', role: 'group', 'aria-label': ENV_NAMES[env.environment] || env.environment },
        h('h3', { class: 'pl-env-title' }, ENV_NAMES[env.environment] || env.environment),
        h('div', { class: 'db-kpis pl-kpis' }, tiles(env)))));
  }

  function tile(label, value, sub) {
    return h('div', { class: 'db-kpi' }, h('span', { class: 'db-kpi-label' }, label), h('div', { class: 'db-kpi-row' }, h('span', { class: 'db-kpi-value' }, value)), h('span', { class: 'db-kpi-sub' }, sub));
  }

  /** The page while it loads (SPEC-120): the delivery tiles, the trend and the table. */
  function loadingLayout() {
    const titles = () => h('div', { class: 'db-card-titles' }, bar(40), bar(60));
    return h('div', { class: 'stack' },
      h('div', { class: 'card db-card' }, titles(), h('div', { class: 'db-kpis pl-kpis' }, Array.from({ length: 4 }, () => h('div', { class: 'db-kpi' }, h('div', { class: 'db-kpi-label' }, bar(60)), h('div', { class: 'db-kpi-value' }, bar(50)))))),
      h('div', { class: 'card db-card' }, titles(), h('div', { class: 'skeleton-chart' })),
      h('div', { class: 'card db-card' }, titles(), h('div', { class: 'skeleton-field' })));
  }

  renderFilters();
  showOpen();
  const live = setInterval(() => {
    if (document.visibilityState === 'visible') reload({ quiet: true });
  }, LIVE_MS);
  return { el: frame.el, key: `pipeline:${sandbox.id}`, destroy: () => clearInterval(live) };
}

/** The Pipeline page's address for one subject's waterfall. */
export function pipelineHref(sandbox, subject) {
  return `/s/${encodeURIComponent(sandbox.slug)}/pipeline?open=${encodeURIComponent(subject)}`;
}
