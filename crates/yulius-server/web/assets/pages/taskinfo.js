// Tasks → a task's overview (SPEC-001 UI-032): everything about a task before you open its session —
// state and what it needs, times, usage, author and subscription, its plan, sessions, changes, pull
// request and delivery — with "Review changes" and "Go to task" at the top. Reads GET
// /api/tasks/{tid} (TaskDetail) and the sandbox's Git overview (through the store).

import { enc } from '../api.js';
import { h, icon, setChildren, placeholder, statusMark, prChip, loadError, bar } from '../dom.js';
import { href } from '../router.js';
import { relLong, dateTime, duration, compact, int, plural, accountRef } from '../format.js';
import { taskTime } from '../tasktime.js';
import { taskInfo, activityLine, isLive } from '../status.js';
import { openDeliveryDialog, stageLabel } from '../actions.js';
import { initials } from '../layout.js';
import { modelLabel } from '../models.js';
import { loadWaterfall } from '../stages.js';
import { pipelineHref } from './pipeline.js';

const ORIGINS = { ui: 'Started here', automation: 'Automation', import: 'Imported', onboarding: 'Onboarding', agent: 'Another task' };
const STEP_MARK = { done: '✓', in_progress: '▸', pending: '·', skipped: '–', blocked: '✕' };
const STEP_LABEL = { done: 'Done', in_progress: 'In progress', pending: 'To do', blocked: 'Blocked', skipped: 'Skipped' };
const ATTEMPT_STATE = { live: 'Running', exited: 'Ended', failed: 'Failed', lost: 'Lost' };
// Why a session ended, in words (raw codes such as `spawn_failed` are not shown).
const EXIT_REASON = { exited: 'exited normally', cancelled: 'stopped by a person', switched: 'moved to another subscription', spawn_failed: 'couldn’t start', lost: 'process lost', switch_timeout: 'move timed out' };
const LAUNCH_KIND = { resume: 'resumed', continue: 'continued' };
const exitReason = (reason) => EXIT_REASON[reason] || (String(reason).startsWith('exited') ? 'exited with an error' : null);

/** A fact tile: label, value, an optional line under it. */
function tile(label, value, sub, extra) {
  return h('div', { class: 'stat ti-stat' }, h('span', { class: 'stat-label' }, label), h('span', { class: 'stat-value ti-value' }, value), sub ? h('span', { class: 'stat-sub' }, sub) : null, extra || null);
}

/**
 * The overview while it loads (SPEC-120, #449): the six figures (Plan with its bar; a figure's
 * line under it mostly takes two lines in the six-column grid), the Plan and Details cards, Stages
 * and Sessions.
 */
function loadingLayout() {
  const fact = (i) => h('div', { class: 'stat ti-stat' }, h('span', { class: 'stat-label' }, bar('md')), h('span', { class: 'stat-value ti-value' }, bar('lg')),
    h('span', { class: 'stat-sub' }, bar(80), bar(45)), i === 3 ? h('span', { class: 'ti-progress' }) : null);
  const card = (...lines) => h('div', { class: 'card ti-card' }, h('div', { class: 'card-title' }, bar('md')), lines);
  return h('div', { class: 'ti-body' },
    h('div', { class: 'stat-grid ti-facts' }, Array.from({ length: 6 }, (_, i) => fact(i))),
    h('div', { class: 'ti-columns' }, card(bar(70), bar(55), bar(62)), card(bar(60), bar(50), bar(64), bar(40))),
    card(h('div', { class: 'skeleton-field' })),
    card(bar(80), bar(70)));
}

/** Mount the overview of one task. */
export function mountTaskInfoView(ctx, sandbox, task) {
  const base = `/s/${encodeURIComponent(sandbox.slug)}`;
  const taskPath = `${base}/t/${task.number}`;
  const reviewPath = `${base}/git/t/${task.number}`;
  let current = task;
  let detail = null;
  let detailError = null;
  const reads = new AbortController();

  const meta = h('div', { class: 'gt-meta' });
  // Its stages (SPEC-137): states, steps, runs, pauses, delivery and its pull request's CI runs.
  const stagesHost = h('div', null, h('div', { class: 'skeleton-field' }));
  const stagesPath = pipelineHref(sandbox, `task:${task.id}`);
  const stagesCard = h('section', { class: 'card ti-card', 'aria-labelledby': 'ti-stages' },
    h('div', { class: 'row-wrap' }, h('h2', { class: 'card-title', id: 'ti-stages' }, icon('clock', { size: 14 }), 'Stages'), h('span', { class: 'spacer' }),
      h('a', { class: 'btn btn-sm', href: href(stagesPath), dataset: { route: stagesPath } }, 'In Pipeline')),
    stagesHost);
  const body = h('div', { class: 'ti-body' }, placeholder('Loading the task…', 'loading', loadingLayout));
  const titleEl = h('h1', { class: 'page-title gt-title' });
  const head = h('header', { class: 'gt-head' },
    h('a', { class: 'gt-back', href: href(`${base}/tasks`), dataset: { route: `${base}/tasks`, restore: 'page' } }, icon('back', { size: 14 }), 'Tasks'),
    h('div', { class: 'gt-title-row' }, titleEl,
      task.branch ? h('a', { class: 'btn', href: href(reviewPath), dataset: { route: reviewPath } }, icon('branch', { size: 14 }), 'Review changes') : null,
      h('a', { class: 'btn btn-primary gt-go', href: href(taskPath), dataset: { route: taskPath } }, 'Go to task', icon('forward', { size: 14 }))),
    meta);

  function renderHead() {
    const info = taskInfo(current, ctx.store.state.actionRequests, ctx.store.pullsOf(current.sandbox_id).get(current.id));
    setChildren(titleEl, h('span', { class: 'muted' }, `#${current.number} `), current.title || `Task #${current.number}`);
    const pr = ctx.store.pullsOf(sandbox.id).get(current.id);
    // The activity line starts with the state's label: show only what it adds.
    const line = activityLine(current, info) || '';
    const extra = line.startsWith(info.label) ? line.slice(info.label.length).replace(/^\s*·\s*/, '') : line;
    setChildren(meta,
      h('span', { class: 'gt-state' }, statusMark(info, 14), info.label),
      extra ? h('span', { class: info.group === 'needs' ? 'ti-what ti-needs' : 'ti-what' }, extra) : null,
      pr ? prChip(pr) : null);
  }

  function steps() {
    const list = detail && Array.isArray(detail.steps_list) ? [...detail.steps_list].sort((a, b) => a.ordinal - b.ordinal) : [];
    if (!list.length) return h('p', { class: 'muted small' }, 'The agent has not written a plan yet.');
    return h('ol', { class: 'ti-steps' }, list.map((step) => h('li', { class: ['ti-step', `st-${step.status}`] },
      h('span', { class: 'step-mark', role: 'img', 'aria-label': STEP_LABEL[step.status] || step.status }, STEP_MARK[step.status] || '·'),
      h('span', { class: 'ti-step-text' }, h('span', null, step.title), step.note ? h('span', { class: 'muted small' }, step.note) : null),
      h('span', { class: 'muted xs', title: dateTime(step.updated_at) }, [step.updated_by, step.updated_at ? relLong(step.updated_at) : null].filter(Boolean).join(' · ')))));
  }

  function sessions() {
    const list = detail && Array.isArray(detail.attempts) ? detail.attempts : [];
    if (!list.length) return h('p', { class: 'muted small' }, 'No session has started yet.');
    // A narrow page stacks each session's cells under its number (each cell names its column), so
    // the overview never scrolls sideways at 390 px (#147).
    // Explicit roles keep the table a table for screen readers when a narrow page stacks it.
    const cell = (label, content, attrs = {}) => h('td', { ...attrs, role: 'cell', dataset: { label } }, h('span', null, content));
    return h('table', { class: 'table ti-sessions', role: 'table', 'aria-label': 'Sessions' },
      h('thead', { role: 'rowgroup' }, h('tr', { role: 'row' }, ['#', 'Subscription', 'Started', 'Ended', 'Open for', 'State'].map((label) => h('th', { scope: 'col', role: 'columnheader' }, label)))),
      h('tbody', { role: 'rowgroup' }, list.map((a) => h('tr', { role: 'row' },
        cell('#', String(a.seq)),
        cell('Subscription', [accountRef(a.account_label, a.account_email, { unknown: ctx.isInstanceAdmin() }) || '—', a.launch_kind && a.launch_kind !== 'new' ? h('span', { class: 'muted xs' }, ` · ${LAUNCH_KIND[a.launch_kind] || a.launch_kind}`) : null]),
        cell('Started', a.started_at ? relLong(a.started_at) : '—', { title: dateTime(a.started_at) }),
        cell('Ended', a.ended_at ? relLong(a.ended_at) : 'running', { title: a.ended_at ? dateTime(a.ended_at) : null }),
        cell('Open for', a.started_at ? duration(Math.max(0, (a.ended_at ? Date.parse(a.ended_at) : Date.now()) - Date.parse(a.started_at))) : '—'),
        cell('State', [ATTEMPT_STATE[a.state] || a.state || '—', a.exit_reason && exitReason(a.exit_reason) ? h('span', { class: 'muted xs' }, ` · ${exitReason(a.exit_reason)}`) : a.exit_code !== null && a.exit_code !== undefined ? h('span', { class: 'muted xs' }, ` · exit ${a.exit_code}`) : null])))));
  }

  function render() {
    renderHead();
    if (detailError) { setChildren(body, loadError(`Couldn’t load the task: ${detailError.message}`, () => { detailError = null; load(); })); return; }
    if (!detail) return;
    const m = detail.metrics || {};
    const git = ctx.store.gitRowOf(sandbox.id, current.id);
    const pr = ctx.store.pullsOf(sandbox.id).get(current.id);
    const stepsCount = current.steps || { done: 0, total: 0 };
    const tokens = [m.input_tokens, m.output_tokens].every((v) => v === null || v === undefined) ? null : (m.input_tokens || 0) + (m.output_tokens || 0);
    const login = current.created_by && current.created_by.login;
    const ownerLogin = (current.owner && current.owner.login) || login;
    const delivery = detail.delivery || null;

    // Each time says what it measures (SPEC-087 TIM-003, MET-005): when the task was made, how
    // long it has been in its state, and the agent's work apart from the time its sessions stayed
    // open (idle included).
    const state = taskTime(current, taskInfo(current, ctx.store.state.actionRequests, pr));
    const facts = h('div', { class: 'stat-grid ti-facts' },
      tile('Created', relLong(current.created_at), dateTime(current.created_at)),
      tile('Now', state.text, state.since === null ? null : `since ${dateTime(new Date(state.since).toISOString())}`),
      tile('Agent work', m.active_ms ? duration(m.active_ms) : '—', `${m.turns ? `${plural(m.turns, 'turn')} · ` : ''}sessions open ${duration(m.wall_ms)}${isLive(current.status) ? ' so far' : ''}`),
      tile('Plan', stepsCount.total ? `${stepsCount.done}/${stepsCount.total}` : '—', stepsCount.current || (stepsCount.total && stepsCount.done === stepsCount.total ? 'All steps done' : null),
        stepsCount.total ? h('span', { class: 'ti-progress', role: 'progressbar', 'aria-valuemin': 0, 'aria-valuemax': stepsCount.total, 'aria-valuenow': stepsCount.done, 'aria-label': 'Plan steps done' }, h('span', { class: 'ti-progress-fill', style: { width: `${Math.round((stepsCount.done / stepsCount.total) * 100)}%` } })) : null),
      tile('Changes', git && git.files_changed ? h('span', { class: 'gt-stat' }, h('span', { class: 'gt-add' }, `+${git.insertions ?? 0}`), ' ', h('span', { class: 'gt-del' }, `−${git.deletions ?? 0}`)) : '—',
        git && git.files_changed ? h('a', { href: href(reviewPath), dataset: { route: reviewPath } }, `${plural(git.files_changed, 'file')} · review`) : 'No branch changes yet'),
      tile('Tokens', tokens === null ? 'unknown' : compact(tokens), tokens === null ? null : `in ${compact(m.input_tokens)} · out ${compact(m.output_tokens)} · cache ${compact((m.cache_read_tokens || 0) + (m.cache_write_tokens || 0))}`));

    const rows = [
      ['Author', login ? h('span', { class: 'ti-person' }, h('span', { class: 'avatar ti-avatar', 'aria-hidden': 'true' }, initials(login)), login) : '—'],
      // Who the task acts on GitHub as, and the only person who changes it (SPEC-070).
      ['Owner', ownerLogin ? h('span', { class: 'ti-person' }, h('span', { class: 'avatar ti-avatar', 'aria-hidden': 'true' }, initials(ownerLogin)), ownerLogin) : '—'],
      ['Origin', ORIGINS[current.origin] || current.origin || '—'],
      ['Mode', current.mode === 'prompt' ? 'Prompt' : 'Goal (works until it is met)'],
      ['Subscription', accountRef(current.account_label, current.account_email, { unknown: ctx.isInstanceAdmin() }) || '—'],
      ['Models', m.models && m.models.length ? m.models.map((id) => modelLabel(id)).join(', ') : '—'],
      ['Turns', m.turns === null || m.turns === undefined ? '—' : int(m.turns)],
      ['Subscription switches', String(m.switches ?? (detail.switches || []).length ?? 0)],
      ['Branch', current.branch ? h('code', null, current.branch) : 'not created yet'],
      // The delivery record knows the pull request before the Git overview lists it.
      ['Pull request', pr || (delivery && delivery.pr) ? prChip(pr || delivery.pr) : 'none yet'],
      ['Delivery', delivery ? h('span', null, stageLabel(delivery.stage), ' ', h('button', { type: 'button', class: 'link-btn small', onClick: () => openDeliveryDialog(ctx, current) }, 'Details…')) : '—'],
      ['Spec', detail.spec_path ? h('code', null, detail.spec_path) : '—'],
      ['Worktree', detail.worktree ? h('code', null, detail.worktree) : '—'],
    ];
    const switches = Array.isArray(detail.switches) ? detail.switches : [];

    setChildren(body,
      facts,
      h('div', { class: 'ti-columns' },
        h('section', { class: 'card ti-card', 'aria-labelledby': 'ti-plan' }, h('h2', { class: 'card-title', id: 'ti-plan' }, icon('tasks', { size: 14 }), 'Plan'), steps()),
        h('section', { class: 'card ti-card', 'aria-labelledby': 'ti-details' }, h('h2', { class: 'card-title', id: 'ti-details' }, icon('target', { size: 14 }), 'Details'),
          h('dl', { class: 'kv ti-kv' }, rows.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)])))),
      stagesCard,
      h('section', { class: 'card ti-card', 'aria-labelledby': 'ti-sessions' }, h('h2', { class: 'card-title', id: 'ti-sessions' }, icon('terminal', { size: 14 }), 'Sessions'), sessions()),
      switches.length ? h('section', { class: 'card ti-card', 'aria-labelledby': 'ti-switches' }, h('h2', { class: 'card-title', id: 'ti-switches' }, icon('refresh', { size: 14 }), 'Subscription switches'),
        h('ul', { class: 'list-plain small' }, switches.map((sw) => h('li', null, `${relLong(sw.at)}: ${sw.from} → ${sw.to}`, sw.reason ? h('span', { class: 'muted' }, ` (${sw.reason})`) : null)))) : null);
  }

  let alive = true;
  async function load() {
    try {
      const fresh = await ctx.api.get(`/api/tasks/${enc(task.id)}`, { signal: reads.signal });
      if (!alive) return;
      detail = fresh;
      current = { ...current, ...fresh };
      loadWaterfall(ctx, sandbox, `task:${task.id}`, stagesHost, { title: 'Stages of this task' });
    } catch (error) {
      if (!alive || error.code === 'aborted') return;
      detailError = error;
    }
    render();
  }

  const unwatchPulls = ctx.store.watchPulls(sandbox.id);
  const offs = [
    ctx.store.on('pulls', (sid) => { if (sid === sandbox.id) render(); }),
    // The task moves on while you read: its state, steps and times follow.
    ctx.store.on('task', (t) => {
      if (t.id !== task.id) return;
      // Summaries never carry the subscription's email (SPEC-244): a new subscription drops the
      // old one's until the detail is read again.
      const moved = t.account_label !== current.account_label;
      current = { ...current, ...t };
      if (moved) { delete current.account_email; load(); }
      render();
    }),
    ctx.store.on('steps', (taskId) => { if (taskId === task.id) load(); }),
  ];
  const el = h('div', { class: 'page ti-page' }, h('div', { class: 'page-inner ti-inner' }, head, body));
  renderHead();
  load();

  return {
    el,
    destroy() { alive = false; reads.abort(); unwatchPulls(); offs.forEach((off) => off()); },
  };
}
