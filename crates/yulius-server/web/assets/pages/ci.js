// CI tab of the CI/CD page (SPEC-001 DLV-006/007, SPEC-002 ADR-21, SPEC-151): Yulius is the
// projects' only CI. The jobs in force and where they come from, the runs of pull request heads and
// of the integration and production branches with each job's log, Cancel and Run again, and
// repository hygiene (stale branches and worktrees). A run opens in place of the list at
// `?run=<id>`, the link of the `yulius/ci` commit status (SPEC-238: a view, not a dialog); Run
// again lives there, not on every card (SPEC-258).

import { enc } from '../api.js';
import { h, badge, itemCard, emptyState, setChildren, placeholder, toast, errorToast, confirmDialog, pollPaused, icon, bar, drawShape } from '../dom.js';
import { relLong, dateTime, duration, plural } from '../format.js';
import { allows } from '../status.js';
import { href } from '../router.js';
import { loader } from './common.js';
import { BRAND } from '../brand.js';
import { loadWaterfall } from '../stages.js';
import { pipelineHref } from './pipeline.js';
import { runView, stageNow, soFar, focusIfLost } from '../runview.js';

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
/** What a run's card's open button is called. */
const openLabel = (run) => `Open the run of ${run.label} at ${shortSha(run.sha)}`;

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

// The run whose view was left last, per sandbox: the list it goes back to puts the keyboard on its
// card (SPEC-238 review round 1).
const leftRun = new Map();

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
    focusIfLost(view.el);
    return () => { destroyed = true; clearTimeout(poll); view.dispose(); leftRun.set(sandbox.id, shownRun); };
  }
  host.append(runsHost, hygieneHost);

  const reload = loader(runsHost, 'the CI runs', () => ctx.api.get(`${base}/ci`), (overview) => {
    schedule(overview);
    const drawn = renderOverview(overview);
    // Back from a run's view: the keyboard goes to that run's card, once.
    const left = (overview.runs || []).find((r) => r.id === leftRun.get(sandbox.id));
    leftRun.delete(sandbox.id);
    if (left) focusIfLost(drawn.querySelector(`[aria-label="${CSS.escape(openLabel(left))}"]`));
    return drawn;
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
    // SPEC-258: only Cancel, which is urgent; Run again is in the run's view.
    const controls = write && ACTIVE.has(run.state) ? [cancelButton(run, () => reload({ quiet: true }))] : [];
    return itemCard({
      iconEl: h('span', { class: ['ci-state', `ci-${run.state}`] }, icon(st.icon, { size: 18, label: st.label })),
      title: [run.label, ' ', h('code', { class: 'muted' }, shortSha(run.sha))],
      badges: [stateBadge(run.state)],
      description: [jobsLine(run.jobs), run.reason ? ` · ${run.reason}` : null],
      extra: h('div', { class: 'item-meta' }, meta),
      controls,
      onOpen: () => ctx.navigate(`${page}?run=${encodeURIComponent(run.id)}`),
      openLabel: openLabel(run),
    });
  }

  async function sync() {
    syncButton.disabled = true;
    try {
      await ctx.api.post(`${base}/ci/sync`);
      // A run's view has no list to refresh (review round 1): it says the look happened.
      if (shownRun) { toast('Looked for new commits.', { kind: 'ok' }); return; }
      reload({ quiet: true });
      loadHygiene();
    } catch (error) {
      errorToast(error, 'Couldn’t check for commits');
    } finally {
      syncButton.disabled = !write;
    }
  }

  function which(run) { return `${run.label} at ${shortSha(run.sha)}`; }

  /** Cancel for an active run (its card and its view); `then` redraws what shows it. */
  function cancelButton(run, then) {
    return h('button', { type: 'button', class: 'btn btn-sm', 'aria-label': `Cancel: ${which(run)}`, onClick: () => cancel(run, then) }, 'Cancel');
  }

  async function cancel(run, then) {
    if (!(await confirmDialog({ title: `Cancel the run of ${run.label}?`, message: 'Its jobs are stopped. Run it again later from its view.', confirmLabel: 'Cancel run', danger: true }))) return;
    try {
      await ctx.api.post(`${base}/ci/runs/${enc(run.id)}/cancel`);
      toast(`Run of ${run.label} cancelled.`, { kind: 'ok' });
      then();
    } catch (error) {
      errorToast(error, 'Couldn’t cancel the run');
    }
  }

  /** Tests the run's commit again and opens the new run (SPEC-258 RRA-002); the button is off
   * while the request is out, so a second click sends nothing. */
  async function rerun(run, button) {
    button.disabled = true;
    try {
      const fresh = await ctx.api.post(`${base}/ci/runs/${enc(run.id)}/rerun`);
      toast(`${run.label} queued again.`, { kind: 'ok' });
      if (!destroyed) ctx.navigate(`${page}?run=${encodeURIComponent(fresh.id)}`);
    } catch (error) {
      button.disabled = false;
      errorToast(error, 'Couldn’t run it again');
    }
  }

  /**
   * A run while it loads (SPEC-120, #449): its head line, the job tabs and the output box. The head
   * is a run's without a reason (state, name, commit) and has the loaded head's buttons (Run again
   * or Cancel for writers, then In Pipeline), so on a phone it wraps as the loaded one does
   * (SPEC-259).
   */
  function runLoading() {
    const buttons = (write ? [0, 1] : [0]).map(() => h('span', { class: 'skeleton-button skeleton-button-sm' }));
    return h('div', { class: 'stack' },
      h('div', { class: 'run-summary' }, h('div', { class: 'row-wrap run-head' }, bar('sm'), bar('lg'), bar('sm'), h('span', { class: 'spacer' }), ...buttons),
        h('div', { class: 'run-facts muted small' }, bar('lg'))),
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
    let lastNow = null;
    host.append(view.el);
    let first = true;
    // Each load supersedes the one before (a Cancel reloads at once): an older one draws nothing.
    let generation = 0;
    const load = async () => {
      const mine = ++generation;
      try {
        const run = await ctx.api.get(`${base}/ci/runs/${enc(id)}`);
        if (destroyed || mine !== generation) return;
        if (first) {
          setChildren(view.body, stages, tabs, output);
          view.ready();
          const failing = (run.jobs || []).findIndex((j) => j.state === 'failed' || j.state === 'error');
          selected = failing >= 0 ? failing : 0;
          first = false;
        }
        drawHead(run);
        render(run);
        // The summary (SPEC-238 CRV-003): how long, the stages running now and the jobs; drawn
        // before the stages load, so it keeps its height, and again with what runs now.
        const summarize = (now) => view.summarize({
          running: ACTIVE.has(run.state) && Boolean(run.started_at), startedAt: run.started_at, took: run.duration_ms ?? NaN,
          since: `${run.started_at ? 'started' : 'queued'} ${relLong(run.started_at || run.queued_at)}`,
          now: ACTIVE.has(run.state) ? now : null, line: jobsLine(run.jobs),
        });
        summarize(lastNow);
        const timeline = await loadWaterfall(ctx, sandbox, `ci_run:${id}`, stages, { title: 'Stages of this run' });
        if (destroyed || mine !== generation) return;
        lastNow = stageNow(timeline);
        summarize(lastNow);
        if (ACTIVE.has(run.state)) poll = setTimeout(load, POLL_ACTIVE_MS);
      } catch (error) {
        if (first && !destroyed) view.loading(placeholder(`Couldn’t load the run: ${error.message}`, 'error'));
      }
    };
    // The head: what the run is, then its controls and In Pipeline. A job's tab leaves it alone,
    // and a control is replaced only when the run's state calls for another one, so the keyboard
    // stays on it across refreshes and goes to its successor (review round 1 of SPEC-258).
    const spacer = h('span', { class: 'spacer' });
    let lead = [];
    let controls = [];
    let controlsFor = null;
    const drawHead = (run) => {
      for (const node of lead) node.remove();
      lead = [stateBadge(run.state), h('strong', null, run.label), h('code', { class: 'muted' }, shortSha(run.sha)),
        run.reason ? h('span', { class: 'muted small' }, run.reason) : null].filter(Boolean);
      if (!spacer.isConnected) head.append(spacer, pipeline);
      spacer.before(...lead);
      const wanted = !write ? 'none' : ACTIVE.has(run.state) ? 'cancel' : 'again';
      if (wanted === controlsFor) return;
      const focused = controls.some((node) => node.contains(document.activeElement));
      for (const node of controls) node.remove();
      controls = runControls(run);
      controlsFor = wanted;
      pipeline.before(...controls);
      if (focused && controls.length) controls[0].focus();
    };
    const render = (run) => {
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
    // SPEC-258 RRA-002/003: Cancel while it runs, Run again once it ended, for writers only.
    const runControls = (run) => {
      if (!write) return [];
      if (ACTIVE.has(run.state)) return [cancelButton(run, () => { if (destroyed) return; clearTimeout(poll); load(); })];
      const again = h('button', { type: 'button', class: 'btn btn-sm', 'aria-label': `Run again: ${which(run)}`, onClick: () => rerun(run, again) }, icon('refresh', { size: 14 }), 'Run again');
      return [again];
    };
    load();
    return view;
  }

  // ---- hygiene
  // SPEC-269: Yulius removes the safe items itself every 6 hours; Clean up does it now. The card
  // is drawn once per load and filled again by its actions, so its status line stays the same live
  // region (read when it changes): what is running with the seconds so far, then the result.

  let hygieneCard = null; // the card shown now: { fill(report), report }
  let hygieneBusy = null; // { kind: 'scan' | 'clean', count, since }
  let hygieneProblem = null; // what the last failed action said
  let hygieneTick = null;

  const loadHygiene = loader(hygieneHost, 'the repository hygiene', () => ctx.api.get(`${base}/hygiene`), renderHygiene, { shape: hygieneLoading });

  /** The hygiene card while it loads (SPEC-120, #449): its head line with the buttons' room, what it is for, its list. */
  function hygieneLoading() {
    return h('div', { class: 'card card-pad stack-sm' },
      h('div', { class: 'row-wrap' }, h('div', { class: 'card-title' }, bar('xl')), h('span', { class: 'spacer' }), h('span', { class: 'small' }, bar('md')),
        h('span', { class: 'skeleton-button skeleton-button-sm' }), h('span', { class: 'skeleton-button skeleton-button-sm' })),
      h('div', null, h('p', { class: 'muted small' }, bar(80))),
      h('ul', { class: 'ci-hygiene' }, [40, 55, 30].map((width) => h('li', null, bar('lg'), bar(width)))));
  }

  const HYGIENE_KINDS = { remote_branch: 'Branch on GitHub', local_branch: 'Local branch', worktree: 'Worktree' };
  const kindOf = (kind) => HYGIENE_KINDS[kind] || kind;

  function renderHygiene(report) {
    const scanned = h('span', { class: 'muted small' });
    const scan = h('button', { type: 'button', class: 'btn btn-sm', title: write ? null : 'Requires write access', onClick: () => scanNow() });
    const clean = h('button', { type: 'button', class: 'btn btn-sm btn-primary', title: write ? null : 'Requires write access', onClick: () => cleanup() });
    const intro = h('p', { class: 'muted small' });
    const status = h('div', { class: 'ci-hygiene-status', role: 'status' });
    // Parts drawn into the card's stack: an empty one takes no room (display: contents).
    const result = h('div', { class: 'ci-hygiene-part' });
    const list = h('div', { class: 'ci-hygiene-part' });
    const notes = h('div', { class: 'ci-hygiene-part' });
    const card = h('section', { class: 'card card-pad stack-sm', 'aria-label': 'Repository hygiene' },
      h('div', { class: 'row-wrap' }, h('h2', { class: 'card-title' }, 'Repository hygiene'), h('span', { class: 'spacer' }), scanned, scan, clean),
      h('div', null, intro, status), result, list, notes);
    const shown = { report, fill };

    function fill(next) {
      if (next) shown.report = next;
      const now = shown.report;
      const items = now.items || [];
      const safe = items.filter((i) => i.safe);
      setChildren(scanned, now.scanned_at ? h('span', { title: dateTime(now.scanned_at) }, `Checked ${relLong(now.scanned_at)}`) : 'Not checked yet');
      const nextRound = now.next_cleanup_at ? ` (next ${relLong(now.next_cleanup_at)})` : '';
      intro.textContent = `Branches and worktrees no task needs any more. ${BRAND.name} removes the safe ones by itself${nextRound}; Clean up removes them now.`;
      const busy = hygieneBusy;
      card.setAttribute('aria-busy', busy ? 'true' : 'false');
      scan.textContent = busy && busy.kind === 'scan' ? 'Checking…' : 'Check now';
      clean.textContent = busy && busy.kind === 'clean' ? 'Cleaning up…' : 'Clean up';
      scan.disabled = Boolean(busy) || !write;
      clean.disabled = Boolean(busy) || !write || safe.length === 0;
      const last = now.last_cleanup;
      if (busy) {
        const doing = busy.kind === 'clean' ? `Removing ${plural(busy.count, 'item')}…` : 'Checking the repository…';
        status.textContent = `${doing} ${soFar((performance.now() - busy.since) / 1000)}`;
      } else if (hygieneProblem) {
        status.textContent = hygieneProblem;
      } else if (last) {
        const by = last.automatic ? BRAND.name : last.by;
        const removed = (last.removed || []).length;
        const kept = (last.skipped || []).length;
        const what = removed || kept ? `${removed} removed${kept ? `, ${kept} kept` : ''}` : 'nothing to remove';
        status.textContent = `Cleaned up ${relLong(last.at)} by ${by}: ${what}.`;
      } else {
        status.textContent = '';
      }
      status.classList.toggle('ci-hygiene-problem', !busy && Boolean(hygieneProblem));
      const rows = busy || !last ? [] : [
        ...(last.removed || []).map((t) => h('li', null, badge('Removed', 'ok'), h('span', { class: 'muted small' }, kindOf(t.kind)), h('code', null, t.name))),
        ...(last.skipped || []).map((t) => h('li', null, badge('Kept', 'warn'), h('span', { class: 'muted small' }, kindOf(t.kind)), h('code', null, t.name), h('span', { class: 'muted' }, t.reason))),
      ];
      setChildren(result, rows.length ? h('ul', { class: 'ci-hygiene ci-hygiene-result', 'aria-label': 'Last cleanup' }, rows) : null);
      setChildren(list, items.length === 0
        ? h('p', { class: 'muted' }, 'No stale branches or worktrees.')
        : [rows.length ? h('p', { class: 'muted small' }, 'Still listed:') : null, h('ul', { class: 'ci-hygiene', 'aria-label': 'Stale branches and worktrees' }, items.map((item) => h('li', null,
          badge(kindOf(item.kind), item.safe ? '' : 'warn'), h('code', null, item.name),
          item.safe
            ? h('span', { class: 'muted' }, item.reason)
            : [badge('Kept', 'warn'), h('span', { class: 'muted' }, `${item.reason}: unmerged work, which ${BRAND.name} never deletes by itself`)])))]);
      setChildren(notes, (now.notes || []).length ? h('p', { class: 'muted small' }, now.notes.join(' ')) : null);
    }

    fill();
    hygieneCard = shown;
    return card;
  }

  /** Runs one hygiene action with the card busy, then shows its report or says why it failed. */
  async function hygieneAction(kind, count, request, failure) {
    hygieneBusy = { kind, count, since: performance.now() };
    hygieneProblem = null;
    if (hygieneCard) hygieneCard.fill();
    clearInterval(hygieneTick);
    hygieneTick = setInterval(() => { if (hygieneCard) hygieneCard.fill(); }, 1000);
    let report = null;
    try {
      report = await request();
    } catch (error) {
      hygieneProblem = `${failure}: ${error && error.message ? error.message : String(error)}`;
    } finally {
      clearInterval(hygieneTick);
      hygieneBusy = null;
    }
    if (destroyed || !hygieneCard) return;
    hygieneCard.fill(report);
  }

  function scanNow() {
    return hygieneAction('scan', 0, () => ctx.api.post(`${base}/hygiene/scan`), 'Couldn’t check the repository');
  }

  async function cleanup() {
    const safe = ((hygieneCard && hygieneCard.report.items) || []).filter((i) => i.safe);
    if (!(await confirmDialog({ title: `Remove ${plural(safe.length, 'stale item')}?`, message: `Branches whose pull request was merged or closed, branches already merged, unused local branches and worktrees of finished tasks. GitHub keeps the commits of every pull request. ${BRAND.name} would remove them at its next round; this removes them now.`, confirmLabel: 'Clean up' }))) return;
    await hygieneAction('clean', safe.length, async () => (await ctx.api.post(`${base}/hygiene/cleanup`, { items: [], force: false })).report, 'Couldn’t clean up');
  }

  return () => { destroyed = true; clearTimeout(timer); clearInterval(hygieneTick); };
}
