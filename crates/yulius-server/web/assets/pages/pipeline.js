// Pipeline (SPEC-137, ADR-0112): how long every stage of this project's tasks, CI runs and CD runs
// takes, the Dashboard's Pipeline (stages) tab (SPEC-161, ADR-0132). The delivery figures of dev and production first
// (deployment frequency, lead time, change failure rate, time to restore), then the chosen
// stage's daily p50 and p90, then every stage's figures, which choose the trend. Filters (range,
// what, environment, job) and the chosen stage are kept in the address after `?section=pipeline`
// (`/s/<slug>/pipeline` is its earlier address); `open=<kind>:<id>` shows
// one run's, task's or deployment's waterfall on top. CSV/JSON export like the Dashboard's.

import { h, setChildren, icon, iconButton, openMenu, bar } from '../dom.js';
import { duration, int } from '../format.js';
import { replaceQuietly } from '../router.js';
import { loader } from './common.js';
import { BRAND } from '../brand.js';
import { modelLabel } from '../models.js';
import { trendChart, stageTitle, loadWaterfall, subjectLabel, cpuTime, mebibytes } from '../stages.js';

const RANGES = [[7, '7 days'], [30, '30 days'], [90, '90 days']];
const DAY = 86400000;
// Every stage of the range is read: refreshed by itself every 5 minutes (Refresh reads now).
const LIVE_MS = 5 * 60_000;
const SHORT_DATE = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
const isoDay = (date) => date.toISOString().slice(0, 10);
const ENV_NAMES = { dev: 'Dev', prod: 'Production' };
const OPEN_KINDS = { ci_run: 'CI run', cd_run: 'Deployment', task: 'Task' };

export const PIPELINE_SUB = 'how long every stage of tasks, CI and CD takes';

/**
 * Mount the Pipeline tab into `section`, with its range, export and refresh in `actions`; returns
 * its teardown, which stops its refreshes. Its addresses are the Dashboard's `?section=pipeline`.
 */
export function mountPipelineSection(ctx, sandbox, route, section, actions) {
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
  const base = pipelinePath(sandbox);
  const api = `/api/sandboxes/${encodeURIComponent(sandbox.id)}`;
  let options = { jobs: [], environments: [] };

  function remember() {
    const params = new URLSearchParams();
    if (state.days !== 30) params.set('days', String(state.days));
    for (const key of ['subject', 'environment', 'job', 'trend', 'open']) if (state[key]) params.set(key, state[key]);
    const text = params.toString();
    replaceQuietly(text ? `${base}&${text}` : base);
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
  setChildren(actions, rangeLabel, range, exportButton, iconButton('refresh', 'Refresh', () => reload()));
  const openHost = h('div');
  const filters = h('div', { class: 'pl-filters', role: 'group', 'aria-label': 'Filters' });
  const host = h('div', { class: 'db' });
  section.append(openHost, filters, host);

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
    // The review's numbers (SPEC-493 REV-006) and the merge queue's (MQC-005) beside the
    // stages; the stages show without them.
    const range = new URLSearchParams({ sandbox: sandbox.id, from: p.from, to: p.to });
    const [summary, reviews, queue, docker, load] = await Promise.all([
      ctx.api.get(`${api}/stages/summary?${params()}`),
      ctx.api.get(`/api/analytics/reviews?${range}`).catch(() => null),
      ctx.api.get(`/api/analytics/merge-queue?${range}`).catch(() => null),
      // What the sandbox's Docker front needed (SPEC-494): restarts, give-ways, Needs you.
      ctx.api.get(`/api/analytics/docker?${range}`).catch(() => null),
      // The host's load from CI per hour (SPEC-497): jobs' waits, CPUs busy, held, measured.
      ctx.api.get(`/api/analytics/ci-load?${new URLSearchParams({ from: p.from, to: p.to })}`).catch(() => null),
    ]);
    const weeks = Math.max(1, (Date.parse(`${p.to}T00:00:00Z`) - Date.parse(`${p.from}T00:00:00Z`)) / 86400000 + 1) / 7;
    return { ...summary, reviews, queue, load, docker: docker && { ...docker, rangeWeeks: weeks } };
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
      summary.reviews ? reviewCard(summary.reviews) : null,
      summary.queue ? queueCard(summary.queue) : null,
      summary.docker ? dockerCard(summary.docker) : null,
      summary.load && summary.load.hours && summary.load.hours.length ? loadCard(summary.load) : null,
      h('section', { class: 'card db-card', 'aria-label': `Trend: ${label}` },
        h('header', { class: 'db-card-head' },
          h('div', { class: 'db-card-titles' }, h('h2', { class: 'card-title' }, label),
            h('span', { class: 'db-card-sub' }, trendRow ? `${int(trendRow.count)} ended · p50 ${duration(trendRow.p50_ms)} · p90 ${duration(trendRow.p90_ms)} · UTC days` : 'UTC days · choose a stage below')),
          h('div', { class: 'row-wrap' }, chart.legend, chart.toggle)),
        chart.el),
      stagesCard(summary),
    ];
  }

  /** "31 min · 52 min": a figure's p50 and p90, a dash when no span of the row measured it. */
  function pair(p50, p90, format) {
    return Number.isFinite(p50) ? `${format(p50)} · ${format(p90)}` : '—';
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
          h('th', { class: 'num' }, 'p50'), h('th', { class: 'num' }, 'p90'), h('th', { class: 'num hide-phone' }, 'Longest'), h('th', { class: 'num hide-phone' }, 'Total'),
          h('th', { class: 'num hide-phone', title: 'CPU time its processes used (CI jobs and deployments)' }, 'CPU time p50 · p90'),
          h('th', { class: 'num hide-phone', title: 'The largest peak memory of one of its processes (CI jobs and deployments)' }, 'Peak memory p50 · p90'))),
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
            h('td', { class: 'num hide-phone' }, duration(row.total_ms)),
            h('td', { class: 'num hide-phone pl-cpu' }, pair(row.cpu_p50_ms, row.cpu_p90_ms, cpuTime)),
            h('td', { class: 'num hide-phone pl-memory' }, pair(row.memory_peak_p50_mb, row.memory_peak_p90_mb, mebibytes)));
        })))),
      h('p', { class: 'muted small' }, `${subjectLabel('task')} stages follow each task’s states, steps, runs, pauses and delivery; CI and CD stages are their queue, jobs, deploys and the stages their output names (::yulius-stage). A job’s and a deployment’s CPU time and peak memory are what ${BRAND.name} measured its processes using; the peak is the largest single process’s.`));
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

  /** The independent review of the pull requests merged in the range (SPEC-493 REV-006). */
  function reviewCard(r) {
    const prs = r.pull_requests || 0;
    const share = (n) => (prs ? `${Math.round((n / prs) * 100)}%` : '—');
    const ratio = (n, of) => (of ? `${Math.round(((n || 0) / of) * 100)}%` : '—');
    const models = (r.rounds_by_model || []).map((m) => `${modelLabel(m.model)} ${int(m.rounds)}`).join(' · ');
    return h('section', { class: 'card db-card', 'aria-label': 'Review' },
      h('header', { class: 'db-card-head' }, h('div', { class: 'db-card-titles' }, h('h2', { class: 'card-title' }, 'Review'),
        h('span', { class: 'db-card-sub' }, `independent review of the ${int(prs)} pull ${prs === 1 ? 'request' : 'requests'} merged in this range`))),
      h('div', { class: 'db-kpis pl-kpis' },
        tile('First review asked for changes', share(r.first_rounds_requesting_changes || 0), `${int(r.first_rounds_requesting_changes || 0)} of ${int(prs)}`),
        tile('Rounds per pull request', Number.isFinite(r.median_rounds) ? `${r.median_rounds}` : '—', 'median'),
        tile('First review cut off', share(r.first_rounds_without_report || 0), 'ended with no report (a usage limit)'),
        tile('Rounds by model', models || '—', r.no_fable_rounds ? `${int(r.no_fable_rounds)} on Opus for want of Fable` : 'the model that answered'),
        tile('Re-reviews that asked again', ratio(r.re_reviews_asking_again, r.re_reviews_after_changes), `${int(r.re_reviews_asking_again || 0)} of ${int(r.re_reviews_after_changes || 0)} rounds after a request for changes`),
        tile('Re-approvals', int((r.rounds_after_approval || 0) - (r.rounds_after_approval_asking || 0)), `rounds after an approval; ${int(r.rounds_after_approval_asking || 0)} of ${int(r.rounds_after_approval || 0)} asked for changes`)));
  }

  /** The merge queue's candidates in the range (SPEC-493 MQC-005). */
  function queueCard(q) {
    const why = { member_left: 'a member left', member_failed: "a member's own run failed", bisected: 'bisected', base_moved: 'base moved', errored: 'errored', too_long: 'too long', gate_red: 'gate red' };
    const dissolved = Object.entries(q.dissolved_by || {}).map(([k, n]) => `${why[k] || k} ${int(n)}`).join(' · ');
    const minutes = Number.isFinite(q.median_arm_to_merge_minutes) ? `${Math.round(q.median_arm_to_merge_minutes)} min` : '—';
    return h('section', { class: 'card db-card', 'aria-label': 'Merge queue' },
      h('header', { class: 'db-card-head' }, h('div', { class: 'db-card-titles' }, h('h2', { class: 'card-title' }, 'Merge queue'),
        h('span', { class: 'db-card-sub' }, 'merge candidates: armed pull requests tested together on top of the integration branch'))),
      h('div', { class: 'db-kpis pl-kpis' },
        tile('Candidates formed', int(q.formed || 0), 'in this range'),
        tile('Merged', int(q.merged || 0), 'every member merged on one run'),
        tile('Dissolved', int(q.dissolved || 0), dissolved || 'none'),
        tile('Arming to merge', minutes, `median of ${int(q.merges || 0)} armed ${q.merges === 1 ? 'merge' : 'merges'}`),
        ...ciMinutesTiles(q.ci_minutes)),
      dissolvedList(q.dissolved_recent || [], why));
  }

  /** The last candidates dissolved, newest first, each with why and its whole reason (SPEC-497
   * MQW-004); nothing when none was. */
  function dissolvedList(recent, why) {
    if (!recent.length) return null;
    return h('details', { class: 'pl-dissolved' },
      h('summary', {}, `Why the last ${recent.length === 1 ? 'candidate was' : `${int(recent.length)} candidates were`} dissolved`),
      h('ul', { 'aria-label': 'Dissolved candidates' }, ...recent.map((d) =>
        h('li', {}, h('time', { datetime: d.at }, String(d.at || '').slice(0, 16).replace('T', ' ')),
          ` · ${why[d.why] || d.why}: ${d.reason}`))));
  }

  /** CI minutes by how the runs ended (SPEC-497 OFC-003): wasted is failed outside the change or
   * cancelled, against the target of 15 %; nothing when the server sends none. */
  function ciMinutesTiles(m) {
    if (!m) return [];
    const total = (m.passed || 0) + (m.failed || 0) + (m.failed_outside || 0) + (m.cancelled || 0);
    const wasted = Number.isFinite(m.outside_share) ? `${Math.round(m.outside_share * 100)}%` : '—';
    return [
      tile('CI minutes', int(Math.round(total)), `${int(Math.round(m.passed || 0))} passed · ${int(Math.round(m.failed || 0))} failed`),
      tile('Outside the change', wasted,
        `${int(Math.round(m.failed_outside || 0))} min failed for the infrastructure · ${int(Math.round(m.cancelled || 0))} min cancelled · ${int(Math.round(m.rerun || 0))} min run again; target 15%`),
    ];
  }

  /** The host's load from CI in the range (SPEC-497): job-minutes waited to start, the host's
   * CPUs busy, and the busiest hour; the instance's, not the sandbox's. */
  function loadCard(l) {
    const peak = (l.hours || []).reduce((top, hour) => (!top || hour.wait_job_minutes > top.wait_job_minutes ? hour : top), null);
    const busy = Number.isFinite(l.busy_percent) ? `${Math.round(l.busy_percent)}%` : '—';
    const held = (l.hours || []).filter((hr) => Number.isFinite(hr.held_cpus));
    const avg = (xs) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null);
    const heldAvg = avg(held.map((hr) => hr.held_cpus));
    const measuredAvg = avg((l.hours || []).filter((hr) => Number.isFinite(hr.measured_cpus)).map((hr) => hr.measured_cpus));
    return h('section', { class: 'card db-card', 'aria-label': 'CI load' },
      h('header', { class: 'db-card-head' }, h('div', { class: 'db-card-titles' }, h('h2', { class: 'card-title' }, 'CI load'),
        h('span', { class: 'db-card-sub' }, 'the host, all projects: jobs waiting to start and the CPUs busy, per hour (UTC)'))),
      h('div', { class: 'db-kpis pl-kpis' },
        tile('Waiting to start', `${int(l.wait_job_minutes || 0)} job-min`, peak ? `most in one hour: ${int(peak.wait_job_minutes)} (${peak.hour.slice(5, 10)} ${peak.hour.slice(11)}:00)` : 'in this range'),
        tile('Host busy', busy, 'average of the hours read'),
        tile('CPUs held by CI', heldAvg === null ? '—' : `${Math.round(heldAvg)}`, measuredAvg === null ? 'by the running jobs\u2019 budgets' : `${Math.round(measuredAvg)} measured in use`)));
  }

  /** What the sandbox's Docker front needed in the range (SPEC-494): totals and per week. */
  function dockerCard(d) {
    const perWeek = (n) => `${Number((n / d.rangeWeeks).toFixed(1))} a week`;
    return h('section', { class: 'card db-card', 'aria-label': 'Docker' },
      h('header', { class: 'db-card-head' }, h('div', { class: 'db-card-titles' }, h('h2', { class: 'card-title' }, 'Docker'),
        h('span', { class: 'db-card-sub' }, 'the sandbox’s Docker front: restarted by itself, on a person’s ask or after an update; what CI jobs left in it'))),
      h('div', { class: 'db-kpis pl-kpis' },
        tile('Front restarts', int(d.restarts || 0), perWeek(d.restarts || 0)),
        tile('Connections that gave way', int(d.gave_way || 0), perWeek(d.gave_way || 0)),
        tile('Docker Needs you', int(d.needs_you || 0), perWeek(d.needs_you || 0)),
        tile('Left by CI jobs, removed', int((d.ci_containers || 0) + (d.ci_networks || 0)),
          `${int(d.ci_containers || 0)} containers, ${int(d.ci_networks || 0)} networks; ${int(d.ci_swept || 0)} by the sweep`)));
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
  return () => clearInterval(live);
}

/** The Dashboard's Pipeline tab. */
export function pipelinePath(sandbox) {
  return `/s/${encodeURIComponent(sandbox.slug)}/dashboard?section=pipeline`;
}

/** The Pipeline tab's address for one subject's waterfall. */
export function pipelineHref(sandbox, subject) {
  return `${pipelinePath(sandbox)}&open=${encodeURIComponent(subject)}`;
}
