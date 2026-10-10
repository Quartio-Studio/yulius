// CI page (SPEC-001 DLV-006/007, SPEC-002 ADR-18): Yulius is the projects' only CI. The jobs
// in force and where they come from, the runs of pull request heads and of the integration and
// production branches with each job's log, Run again and Cancel, and repository hygiene (stale
// branches and worktrees). A run opens from `?run=<id>`, the link of the `yulius/ci` commit status.

import { enc } from '../api.js';
import { h, badge, itemCard, emptyState, setChildren, placeholder, openModal, toast, errorToast, confirmDialog, pollPaused, icon } from '../dom.js';
import { relLong, dateTime, duration, plural } from '../format.js';
import { allows } from '../status.js';
import { replaceQuietly } from '../router.js';
import { pageFrame, loader } from './common.js';
import { BRAND } from '../brand.js';

const STATE = {
  queued: { label: 'Queued', tone: '', icon: 'clock' },
  running: { label: 'Running', tone: 'warn', icon: 'refresh' },
  passed: { label: 'Passed', tone: 'ok', icon: 'check' },
  failed: { label: 'Failed', tone: 'danger', icon: 'x' },
  error: { label: 'Error', tone: 'danger', icon: 'warn' },
  cancelled: { label: 'Cancelled', tone: '', icon: 'x' },
};
const ACTIVE = new Set(['queued', 'running']);
const SOURCE = {
  yulius_toml: 'yulius.toml on the integration branch',
  profile: 'the sandbox’s delivery profile',
  pull_request: 'the pull request’s own yulius.toml (bootstrap)',
};
const POLL_ACTIVE_MS = 3000;
const POLL_IDLE_MS = 20000;

const stateOf = (state) => STATE[state] || { label: state || 'Unknown', tone: '', icon: 'clock' };
const stateBadge = (state) => badge(stateOf(state).label, stateOf(state).tone);
const shortSha = (sha) => String(sha || '').slice(0, 7);

function jobsLine(jobs) {
  const list = Array.isArray(jobs) ? jobs : [];
  const count = (state) => list.filter((j) => j.state === state).length;
  const parts = [plural(list.length, 'job')];
  for (const state of ['passed', 'failed', 'error', 'running', 'queued', 'cancelled']) {
    const n = count(state);
    if (n && !(state === 'passed' && n === list.length)) parts.push(`${n} ${stateOf(state).label.toLowerCase()}`);
  }
  return parts.join(' · ');
}

/** Mount the CI page. */
export function mountCiPage(ctx, sandbox, route) {
  const write = allows(sandbox.access, 'write');
  const base = `/api/sandboxes/${enc(sandbox.id)}`;
  const syncButton = h('button', { type: 'button', class: 'btn', disabled: !write, title: write ? 'Look for new commits now' : 'Requires write access', onClick: () => sync() }, icon('refresh', { size: 16 }), 'Check for commits');
  const frame = pageFrame({ title: 'CI', sub: `runs on this ${BRAND.name} machine`, actions: [syncButton] });
  const runsHost = h('div');
  const hygieneHost = h('div');
  frame.body.append(runsHost, hygieneHost);
  let timer = null;
  let destroyed = false;
  let openRun = null;

  const reload = loader(runsHost, () => ctx.api.get(`${base}/ci`), (overview) => {
    schedule(overview);
    return renderOverview(overview);
  });

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
      h('span', { title: dateTime(run.queued_at) }, `${run.state === 'queued' ? 'Queued' : 'Started'} ${relLong(run.started_at || run.queued_at)}`),
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
      onOpen: () => showRun(run.id),
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

  /** The run's jobs and logs in a dialog, refreshed while it runs. */
  function showRun(id) {
    if (openRun) openRun.close();
    let selected = 0;
    let poll = null;
    const tabs = h('div', { class: 'ci-tabs', role: 'tablist', 'aria-label': 'Jobs' });
    const head = h('div', { class: 'row-wrap' });
    const output = h('pre', { class: 'code-view run-output', 'aria-live': 'polite', tabindex: '0' }, '');
    const body = h('div', { class: 'stack' }, head, tabs, output);
    const modal = openModal({
      title: 'CI run', wide: true, body: placeholder('Loading…', 'loading'),
      actions: [{ label: 'Close' }],
      onClose: () => { clearTimeout(poll); openRun = null; replaceQuietly(`/s/${encodeURIComponent(sandbox.slug)}/ci`); },
    });
    openRun = modal;
    replaceQuietly(`/s/${encodeURIComponent(sandbox.slug)}/ci?run=${encodeURIComponent(id)}`);
    let first = true;
    const load = async () => {
      try {
        const run = await ctx.api.get(`${base}/ci/runs/${enc(id)}`);
        if (first) {
          setChildren(modal.body, body);
          const failing = (run.jobs || []).findIndex((j) => j.state === 'failed' || j.state === 'error');
          selected = failing >= 0 ? failing : 0;
          first = false;
        }
        render(run);
        if (ACTIVE.has(run.state) && modal.dialog.isConnected) poll = setTimeout(load, POLL_ACTIVE_MS);
      } catch (error) {
        if (first) setChildren(modal.body, placeholder(`Couldn’t load the run: ${error.message}`, 'error'));
      }
    };
    const render = (run) => {
      const st = stateOf(run.state);
      setChildren(head, stateBadge(run.state), h('strong', null, run.label), h('code', { class: 'muted' }, shortSha(run.sha)),
        h('span', { class: 'muted small' }, [run.duration_ms ? `took ${duration(run.duration_ms)}` : `${st.label.toLowerCase()} ${relLong(run.started_at || run.queued_at)}`, run.reason ? ` · ${run.reason}` : ''].join('')));
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
  }

  // ---- hygiene

  const loadHygiene = loader(hygieneHost, () => ctx.api.get(`${base}/hygiene`), renderHygiene);

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

  const deepRun = route && route.query && route.query.get('run');
  if (deepRun) showRun(deepRun);

  return {
    el: frame.el,
    key: `ci:${sandbox.id}`,
    destroy: () => { destroyed = true; clearTimeout(timer); if (openRun) openRun.close(); },
  };
}
