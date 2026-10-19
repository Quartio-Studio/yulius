// CI tab of the CI/CD page (SPEC-001 DLV-006/007, SPEC-002 ADR-21, SPEC-151): Yulius is the
// projects' only CI. The jobs in force and where they come from, the runs of pull request heads and
// of the integration and production branches with each job's log, Run again and Cancel, and
// repository hygiene (stale branches and worktrees). A run opens in place of the list at
// `?run=<id>`, the link of the `yulius/ci` commit status (SPEC-238: a view, not a dialog).

import { enc } from '../api.js';
import { h, badge, itemCard, emptyState, setChildren, placeholder, toast, errorToast, confirmDialog, pollPaused, icon, bar, drawShape } from '../dom.js';
import { relLong, dateTime, duration, plural } from '../format.js';
import { allows } from '../status.js';
import { href } from '../router.js';
import { loader } from './common.js';
import { BRAND } from '../brand.js';
import { loadWaterfall } from '../stages.js';
import { pipelineHref } from './pipeline.js';
import { runView, stageNow } from '../runview.js';

const STATE = {
  queued: { label: 'Queued', tone: '', icon: 'clock' },
  running: { label: 'Running', tone: 'warn', icon: 'refresh' },
  passed: { label: 'Passed', tone: 'ok', icon: 'check' },
  failed: { label: 'Failed', tone: 'danger', icon: 'x' },
  error: { label: 'Error', tone: 'danger', icon: 'warn' },
  cancelled: { label: 'Cancelled', tone: '', icon: 'x' },
  // SPEC-162: the job exited with $YULIUS_CI_SKIP_CODE: it checked nothing and blocks nothing.
  skipped: { label: 'Skipped', tone: 'warn', icon: 'forward' },
};
export const ACTIVE = new Set(['queued', 'running']);
const SOURCE = {
  yulius_toml: 'yulius.toml on the integration branch',
  profile: 'the sandbox’s delivery profile',
  pull_request: 'the pull request’s own yulius.toml (bootstrap)',
};
const POLL_ACTIVE_MS = 3000;
const POLL_IDLE_MS = 20000;

/** A CI run's or job's state in words, tone and icon. */
export const stateOf = (state) => STATE[state] || { label: state || 'Unknown', tone: '', icon: 'clock' };
const stateBadge = (state) => badge(stateOf(state).label, stateOf(state).tone);
const shortSha = (sha) => String(sha || '').slice(0, 7);

/** A run's jobs in a line: "3 jobs · 1 passed · 2 running". */
export function jobsLine(jobs) {
  const list = Array.isArray(jobs) ? jobs : [];
  const count = (state) => list.filter((j) => j.state === state).length;
  const parts = [plural(list.length, 'job')];
  for (const state of ['passed', 'skipped', 'failed', 'error', 'running', 'queued', 'cancelled']) {
    const n = count(state);
    if (n && !(state === 'passed' && n === list.length)) parts.push(`${n} ${stateOf(state).label.toLowerCase()}`);
  }
  return parts.join(' · ');
}

/** What the CI/CD page's sub-heading says on the CI tab. */
export const CI_SUB = `tests run on this ${BRAND.name} machine`;

/**
 * Mount the CI tab into `host`, with its header action in `actions`: the runs and the hygiene, or
 * the run `?run=` names in their place. Returns its teardown, which stops its refreshes.
 */
export function mountCiSection(ctx, sandbox, route, host, actions) {
  const write = allows(sandbox.access, 'write');
  const base = `/api/sandboxes/${enc(sandbox.id)}`;
  const syncButton = h('button', { type: 'button', class: 'btn', disabled: !write, title: write ? 'Look for new commits now' : 'Requires write access', onClick: () => sync() }, icon('refresh', { size: 16 }), 'Check for commits');
  setChildren(actions, syncButton);
  const page = `/s/${encodeURIComponent(sandbox.slug)}/ci`;
  const shownRun = route && route.query && route.query.get('run');
  const runsHost = h('div');
  const hygieneHost = h('div');
  let timer = null;
  let poll = null;
  let destroyed = false;
  if (shownRun) {
    const view = showRun(shownRun);
    return () => { destroyed = true; clearTimeout(poll); view.dispose(); };
  }
  host.append(runsHost, hygieneHost);

  const reload = loader(runsHost, 'the CI runs', () => ctx.api.get(`${base}/ci`), (overview) => {
    schedule(overview);
    return renderOverview(overview);
  }, { shape: loadingLayout });

  /** The runs while they load (SPEC-120): the configuration card, the Runs heading, run cards. */
  function loadingLayout() {
    return h('div', { class: 'stack' },
      h('div', { class: 'card card-pad stack-sm' }, h('div', { class: 'card-title' }, bar('xl')), bar(70), bar(50)),
      h('div', { class: 'ci-heading' }, bar('md')),
      drawShape('items'));
  }

  let last = { runs: [] };
  /** Refreshes quietly: every few seconds while a run is active, else now and then. */
  function schedule(overview) {
    last = overview;
    clearTimeout(timer);
    if (destroyed) return;
    const active = (overview.runs || []).some((r) => ACTIVE.has(r.state));
    timer = setTimeout(async () => {
      timer = null;
      if (!pollPaused()) await reload({ quiet: true });
      // A failed quiet refresh renders nothing, so it does not reschedule by itself.
      if (timer === null) schedule(last);
    }, active ? POLL_ACTIVE_MS : POLL_IDLE_MS);
  }

  function renderOverview(overview) {
    const parts = [];
    if (overview.status_access) parts.push(h('div', { class: 'banner banner-info', role: 'status' }, icon('shield', { size: 16 }), h('span', { class: 'banner-text' }, overview.status_access)));
    const workflows = overview.actions_workflows || [];
    if (workflows.length) {
      parts.push(h('div', { class: 'banner banner-warn', role: 'status' }, icon('warn', { size: 16 }),
        h('span', { class: 'banner-text' }, `GitHub Actions workflows are still on the integration branch (${workflows.join(', ')}). ${BRAND.name} is the only CI: onboarding moves their checks into [[ci.jobs]] in yulius.toml and deletes them.`)));
    }
    if (overview.gate_problem) parts.push(h('div', { class: 'banner banner-warn', role: 'status' }, icon('warn', { size: 16 }), h('span', { class: 'banner-text' }, `CI is not running: ${overview.gate_problem}`)));
    parts.push(configCard(overview.gate));
    const runs = overview.runs || [];
    if (runs.length === 0) {
      parts.push(emptyState({ iconName: 'check', title: 'No runs yet', text: overview.gate
        ? 'Every pull request into the integration branch and every new commit on the integration and production branches is tested here, once per commit.'
        : 'Once CI is configured, every pull request and every new commit on the integration and production branches is tested here.' }));
    } else {
      parts.push(h('h2', { class: 'ci-heading' }, 'Runs'));
      parts.push(h('div', { class: 'item-list' }, runs.map(runCard)));
    }
    return h('div', { class: 'stack' }, parts);
  }

  function configCard(gate) {
    if (!gate) {
      return h('section', { class: 'card card-pad stack-sm', 'aria-label': 'CI configuration' },
        h('h2', { class: 'card-title' }, 'No CI configured'),
        h('p', { class: 'muted' }, 'Add jobs to yulius.toml on the integration branch. They run in parallel; a matrix runs a job once per value, passed as $YULIUS_CI_MATRIX.'),
        h('pre', { class: 'code-view ci-config' }, '[[ci.jobs]]\nname = "test"\ncommand = "cargo test"\nmatrix = "1/2 2/2"   # optional'));
    }
    return h('section', { class: 'card card-pad stack-sm', 'aria-label': 'CI configuration' },
      h('h2', { class: 'card-title' }, `${plural(gate.jobs.length, 'job')} from ${SOURCE[gate.source] || gate.source}`),
      h('ul', { class: 'ci-jobs' }, gate.jobs.map((job) => h('li', null, h('span', { class: 'ci-job-name' }, job.name), h('code', { class: 'ci-job-command' }, job.command)))));
  }

  function runCard(run) {
    const st = stateOf(run.state);
    const meta = [
      h('span', { title: run.started_at ? `Queued ${dateTime(run.queued_at)} · started ${dateTime(run.started_at)}` : `Queued ${dateTime(run.queued_at)}` }, `${run.started_at ? 'Started' : 'Queued'} ${relLong(run.started_at || run.queued_at)}`),
      run.duration_ms !== null && run.duration_ms !== undefined ? ` · took ${duration(run.duration_ms)}` : null,
      run.requested_by && run.requested_by !== 'yulius' ? ` · by ${run.requested_by}` : null,
      run.status && run.status !== 'published' && run.status !== 'pending' ? h('span', { class: 'muted' }, ` · GitHub status ${run.status}`) : null,
    ];
    const controls = [];
    const which = `${run.label} at ${shortSha(run.sha)}`;
    if (write && ACTIVE.has(run.state)) controls.push(h('button', { type: 'button', class: 'btn btn-sm', 'aria-label': `Cancel: ${which}`, onClick: () => cancel(run) }, 'Cancel'));
    if (write && !ACTIVE.has(run.state)) controls.push(h('button', { type: 'button', class: 'btn btn-sm', 'aria-label': `Run again: ${which}`, onClick: () => rerun(run) }, 'Run again'));
    return itemCard({
      iconEl: h('span', { class: ['ci-state', `ci-${run.state}`] }, icon(st.icon, { size: 18, label: st.label })),
      title: [run.label, ' ', h('code', { class: 'muted' }, shortSha(run.sha))],
      badges: [stateBadge(run.state)],
      description: [jobsLine(run.jobs), run.reason ? ` · ${run.reason}` : null],
      extra: h('div', { class: 'item-meta' }, meta),
      controls,
      onOpen: () => ctx.navigate(`${page}?run=${encodeURIComponent(run.id)}`),
      openLabel: `Open the run of ${run.label} at ${shortSha(run.sha)}`,
    });
  }

  async function sync() {
    syncButton.disabled = true;
    try {
      await ctx.api.post(`${base}/ci/sync`);
      reload({ quiet: true });
      loadHygiene();
    } catch (error) {
      errorToast(error, 'Couldn’t check for commits');
    } finally {
      syncButton.disabled = !write;
    }
  }

  async function cancel(run) {
    if (!(await confirmDialog({ title: `Cancel the run of ${run.label}?`, message: 'Its jobs are stopped. Run it again later from this page.', confirmLabel: 'Cancel run', danger: true }))) return;
    try {
      await ctx.api.post(`${base}/ci/runs/${enc(run.id)}/cancel`);
      toast(`Run of ${run.label} cancelled.`, { kind: 'ok' });
      reload({ quiet: true });
    } catch (error) {
      errorToast(error, 'Couldn’t cancel the run');
    }
  }

  async function rerun(run) {
    try {
      await ctx.api.post(`${base}/ci/runs/${enc(run.id)}/rerun`);
      toast(`${run.label} queued again.`, { kind: 'ok' });
      reload({ quiet: true });
    } catch (error) {
      errorToast(error, 'Couldn’t run it again');
    }
  }

  /** A run while it loads (SPEC-120, #449): its head line, the job tabs and the output box. */
  function runLoading() {
    return h('div', { class: 'stack' },
      h('div', { class: 'run-summary' }, h('div', { class: 'row-wrap run-head' }, bar('sm'), bar('lg'), bar('sm'), bar('md'))),
      h('div', { class: 'ci-tabs' }, [0, 1, 2].map(() => h('span', { class: 'ci-tab' }, bar('md')))),
      h('div', { class: 'code-view run-output' }, bar(60), bar(40), bar(52)));
  }

  /** The run's jobs and logs in place of the list, refreshed while it runs (SPEC-238). */
  function showRun(id) {
    let selected = 0;
    const view = runView({ name: 'CI run', list: page });
    const tabs = h('div', { class: 'ci-tabs', role: 'tablist', 'aria-label': 'Jobs' });
    const head = view.head;
    const output = h('pre', { class: 'code-view run-output', 'aria-live': 'polite', tabindex: '0' }, '');
    // Its stages (SPEC-137): queue, preparation, jobs and what their output names, as a waterfall.
    const stages = h('div', { class: 'run-stages' });
    const pipeline = h('a', { class: 'btn btn-sm', href: href(pipelineHref(sandbox, `ci_run:${id}`)), dataset: { route: pipelineHref(sandbox, `ci_run:${id}`) } }, icon('clock', { size: 14 }), 'In Pipeline');
    view.loading(placeholder('Loading the CI run…', 'loading', runLoading));
    host.append(view.el);
    let first = true;
    const load = async () => {
      try {
        const run = await ctx.api.get(`${base}/ci/runs/${enc(id)}`);
        if (destroyed) return;
        if (first) {
          setChildren(view.body, stages, tabs, output);
          view.ready();
          const failing = (run.jobs || []).findIndex((j) => j.state === 'failed' || j.state === 'error');
          selected = failing >= 0 ? failing : 0;
          first = false;
        }
        render(run);
        head.append(h('span', { class: 'spacer' }), pipeline);
        const timeline = await loadWaterfall(ctx, sandbox, `ci_run:${id}`, stages, { title: 'Stages of this run' });
        if (destroyed) return;
        // The summary (SPEC-238 CRV-003): how long, the stages running now and the jobs.
        view.summarize({
          running: ACTIVE.has(run.state) && Boolean(run.started_at), startedAt: run.started_at, took: run.duration_ms,
          since: `${run.started_at ? 'started' : 'queued'} ${relLong(run.started_at || run.queued_at)}`,
          now: ACTIVE.has(run.state) ? stageNow(timeline) : null, line: jobsLine(run.jobs),
        });
        if (ACTIVE.has(run.state)) poll = setTimeout(load, POLL_ACTIVE_MS);
      } catch (error) {
        if (first && !destroyed) view.loading(placeholder(`Couldn’t load the run: ${error.message}`, 'error'));
      }
    };
    const render = (run) => {
      setChildren(head, stateBadge(run.state), h('strong', null, run.label), h('code', { class: 'muted' }, shortSha(run.sha)),
        run.reason ? h('span', { class: 'muted small' }, run.reason) : null);
      const jobs = run.jobs || [];
      setChildren(tabs, jobs.map((job, i) => {
        const tab = h('button', { type: 'button', class: ['ci-tab', i === selected && 'active'], role: 'tab', 'aria-selected': String(i === selected), onClick: () => { selected = i; render(run); } },
          h('span', { class: ['ci-state', `ci-${job.state}`] }, icon(stateOf(job.state).icon, { size: 14, label: stateOf(job.state).label })), job.name);
        return tab;
      }));
      const job = jobs[selected];
      if (!job) { output.textContent = ''; return; }
      const atBottom = output.scrollTop + output.clientHeight >= output.scrollHeight - 8;
      const exit = job.exit_code === null || job.exit_code === undefined ? '' : `\n[exit code ${job.exit_code}]`;
      output.textContent = (job.output || (ACTIVE.has(job.state) ? 'Waiting for output…' : 'No output.')) + exit;
      if (atBottom || ACTIVE.has(job.state)) output.scrollTop = output.scrollHeight;
    };
    load();
    return view;
  }

  // ---- hygiene

  const loadHygiene = loader(hygieneHost, 'the repository hygiene', () => ctx.api.get(`${base}/hygiene`), renderHygiene, { shape: hygieneLoading });

  /** The hygiene card while it loads (SPEC-120, #449): its head line with the buttons' room, its list. */
  function hygieneLoading() {
    return h('div', { class: 'card card-pad stack-sm' },
      h('div', { class: 'row-wrap' }, h('div', { class: 'card-title' }, bar('xl')), h('span', { class: 'spacer' }), h('span', { class: 'small' }, bar('md')),
        h('span', { class: 'skeleton-button skeleton-button-sm' }), h('span', { class: 'skeleton-button skeleton-button-sm' })),
      h('ul', { class: 'ci-hygiene' }, [40, 55, 30].map((width) => h('li', null, bar('lg'), bar(width)))),
      h('p', { class: 'muted small' }, bar(60)));
  }

  function renderHygiene(report) {
    const items = report.items || [];
    const safe = items.filter((i) => i.safe);
    const kinds = { remote_branch: 'Branch on GitHub', local_branch: 'Local branch', worktree: 'Worktree' };
    const scanned = report.scanned_at ? h('span', { title: dateTime(report.scanned_at) }, `Checked ${relLong(report.scanned_at)}`) : 'Not checked yet';
    const scan = h('button', { type: 'button', class: 'btn btn-sm', disabled: !write, title: write ? null : 'Requires write access', onClick: async () => {
      scan.disabled = true;
      try { setChildren(hygieneHost, renderHygiene(await ctx.api.post(`${base}/hygiene/scan`))); } catch (error) { errorToast(error, 'Couldn’t check the repository'); scan.disabled = !write; }
    } }, 'Check now');
    const clean = h('button', { type: 'button', class: 'btn btn-sm btn-primary', disabled: !write || safe.length === 0, title: write ? null : 'Requires write access', onClick: () => cleanup(safe) }, 'Clean up');
    return h('section', { class: 'card card-pad stack-sm', 'aria-label': 'Repository hygiene' },
      h('div', { class: 'row-wrap' }, h('h2', { class: 'card-title' }, 'Repository hygiene'), h('span', { class: 'spacer' }), h('span', { class: 'muted small' }, scanned), scan, clean),
      items.length === 0
        ? h('p', { class: 'muted' }, 'No stale branches or worktrees.')
        : h('ul', { class: 'ci-hygiene' }, items.map((item) => h('li', null,
          badge(kinds[item.kind] || item.kind, item.safe ? '' : 'warn'), h('code', null, item.name), h('span', { class: 'muted' }, item.safe ? item.reason : `${item.reason} (kept: unmerged work)`)))),
      (report.notes || []).length ? h('p', { class: 'muted small' }, report.notes.join(' ')) : null,
      h('p', { class: 'muted small' }, 'Agents clean up with repo_cleanup; merged pull requests lose their branch right after the merge.'));
  }

  async function cleanup(safe) {
    if (!(await confirmDialog({ title: `Remove ${plural(safe.length, 'stale item')}?`, message: 'Branches whose pull request was merged or closed, branches already merged, unused local branches and worktrees of finished tasks. GitHub keeps the commits of every pull request.', confirmLabel: 'Clean up' }))) return;
    try {
      const result = await ctx.api.post(`${base}/hygiene/cleanup`, { items: [], force: false });
      const removed = (result.removed || []).length;
      const skipped = (result.skipped || []).length;
      toast(`${plural(removed, 'item')} removed.${skipped ? ` ${plural(skipped, 'item')} kept.` : ''}`, { kind: 'ok' });
      setChildren(hygieneHost, renderHygiene(result.report));
    } catch (error) {
      errorToast(error, 'Couldn’t clean up');
    }
  }

  return () => { destroyed = true; clearTimeout(timer); };
}
