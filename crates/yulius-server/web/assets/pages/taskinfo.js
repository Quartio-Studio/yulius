// Tasks → a task's overview (SPEC-001 UI-032): everything about a task before you open its session —
// state and what it needs, times, usage, author and account, its plan, sessions, changes, pull
// request and delivery — with "Review changes" and "Go to task" at the top. Reads GET
// /api/tasks/{tid} (TaskDetail) and the sandbox's Git overview (through the store).

import { enc } from '../api.js';
import { h, icon, setChildren, placeholder, statusMark, prChip, loadError } from '../dom.js';
import { href } from '../router.js';
import { relLong, dateTime, duration, compact, int, plural } from '../format.js';
import { taskInfo, activityLine, isLive } from '../status.js';
import { openDeliveryDialog, stageLabel } from '../actions.js';
import { initials } from '../layout.js';

const ORIGINS = { ui: 'Started here', automation: 'Automation', import: 'Imported', onboarding: 'Onboarding', agent: 'Another task' };
const STEP_MARK = { done: '✓', in_progress: '▸', pending: '·', skipped: '–', blocked: '✕' };
const STEP_LABEL = { done: 'Done', in_progress: 'In progress', pending: 'To do', blocked: 'Blocked', skipped: 'Skipped' };
const ATTEMPT_STATE = { live: 'Running', exited: 'Ended', failed: 'Failed', lost: 'Lost' };

/** A fact tile: label, value, an optional line under it. */
function tile(label, value, sub, extra) {
  return h('div', { class: 'stat ti-stat' }, h('span', { class: 'stat-label' }, label), h('span', { class: 'stat-value ti-value' }, value), sub ? h('span', { class: 'stat-sub' }, sub) : null, extra || null);
}

/** Mount the overview of one task. */
export function mountTaskInfoView(ctx, sandbox, task) {
  const base = `/s/${encodeURIComponent(sandbox.slug)}`;
  const taskPath = `${base}/t/${task.number}`;
  const reviewPath = `${base}/git/t/${task.number}`;
  let current = task;
  let detail = null;
  let detailError = null;
  let accounts = null; // account label → account (with its sign-in email for administrators)
  const reads = new AbortController();

  /** An account by its sign-in email when the server gives it (administrators), else by its label. */
  const accountName = (label) => {
    const account = accounts && label ? accounts.get(label) : null;
    return account && account.email ? h('span', { title: label }, account.email) : (label || '—');
  };

  const meta = h('div', { class: 'gt-meta' });
  const body = h('div', { class: 'ti-body' }, placeholder('Loading the task…', 'loading'));
  const titleEl = h('h1', { class: 'page-title gt-title' });
  const head = h('header', { class: 'gt-head' },
    h('a', { class: 'gt-back', href: href(`${base}/tasks`), dataset: { route: `${base}/tasks` } }, icon('back', { size: 14 }), 'Tasks'),
    h('div', { class: 'gt-title-row' }, titleEl,
      task.branch ? h('a', { class: 'btn', href: href(reviewPath), dataset: { route: reviewPath } }, icon('branch', { size: 14 }), 'Review changes') : null,
      h('a', { class: 'btn btn-primary gt-go', href: href(taskPath), dataset: { route: taskPath } }, 'Go to task', icon('forward', { size: 14 }))),
    meta);

  function renderHead() {
    const info = taskInfo(current, ctx.store.state.actionRequests, ctx.store.pullsOf(current.sandbox_id).get(current.id));
    setChildren(titleEl, h('span', { class: 'muted' }, `#${current.number} `), current.title || `Task #${current.number}`);
    const pr = ctx.store.pullsOf(sandbox.id).get(current.id);
    setChildren(meta,
      h('span', { class: 'gt-state' }, statusMark(info, 14), info.label),
      h('span', { class: info.group === 'needs' ? 'ti-what ti-needs' : 'ti-what' }, activityLine(current, info)),
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
    // Scrolls sideways on a phone: a named, focusable region so the keyboard can scroll it.
    return h('div', { class: 'table-wrap', role: 'region', 'aria-label': 'Sessions table', tabindex: '0' }, h('table', { class: 'table ti-sessions' },
      h('thead', null, h('tr', null, ['#', 'Account', 'Started', 'Ended', 'Length', 'State'].map((label) => h('th', { scope: 'col' }, label)))),
      h('tbody', null, list.map((a) => h('tr', null,
        h('td', null, String(a.seq)),
        h('td', null, accountName(a.account_label), a.launch_kind && a.launch_kind !== 'new' ? h('span', { class: 'muted xs' }, ` · ${a.launch_kind}`) : null),
        h('td', { title: dateTime(a.started_at) }, a.started_at ? relLong(a.started_at) : '—'),
        h('td', { title: a.ended_at ? dateTime(a.ended_at) : null }, a.ended_at ? relLong(a.ended_at) : 'running'),
        h('td', null, a.started_at ? duration((a.ended_at ? Date.parse(a.ended_at) : Date.now()) - Date.parse(a.started_at)) : '—'),
        h('td', null, ATTEMPT_STATE[a.state] || a.state || '—', a.exit_reason ? h('span', { class: 'muted xs' }, ` · ${a.exit_reason}`) : a.exit_code !== null && a.exit_code !== undefined ? h('span', { class: 'muted xs' }, ` · exit ${a.exit_code}`) : null))))));
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
    const delivery = detail.delivery || null;

    const facts = h('div', { class: 'stat-grid ti-facts' },
      tile('Started', relLong(current.created_at), dateTime(current.created_at)),
      tile(isLive(current.status) ? 'Running for' : 'Ran for', duration(m.wall_ms), m.active_ms ? `${duration(m.active_ms)} of it working` : null),
      tile('Last change', relLong(current.updated_at), dateTime(current.updated_at)),
      tile('Plan', stepsCount.total ? `${stepsCount.done}/${stepsCount.total}` : '—', stepsCount.current || (stepsCount.total && stepsCount.done === stepsCount.total ? 'All steps done' : null),
        stepsCount.total ? h('span', { class: 'ti-progress', role: 'progressbar', 'aria-valuemin': 0, 'aria-valuemax': stepsCount.total, 'aria-valuenow': stepsCount.done, 'aria-label': 'Plan steps done' }, h('span', { class: 'ti-progress-fill', style: { width: `${Math.round((stepsCount.done / stepsCount.total) * 100)}%` } })) : null),
      tile('Changes', git && git.files_changed ? h('span', { class: 'gt-stat' }, h('span', { class: 'gt-add' }, `+${git.insertions ?? 0}`), ' ', h('span', { class: 'gt-del' }, `−${git.deletions ?? 0}`)) : '—',
        git && git.files_changed ? h('a', { href: href(reviewPath), dataset: { route: reviewPath } }, `${plural(git.files_changed, 'file')} · review`) : 'No branch changes yet'),
      tile('Tokens', tokens === null ? 'unknown' : compact(tokens), tokens === null ? null : `in ${compact(m.input_tokens)} · out ${compact(m.output_tokens)} · cache ${compact((m.cache_read_tokens || 0) + (m.cache_write_tokens || 0))}`));

    const rows = [
      ['Author', login ? h('span', { class: 'ti-person' }, h('span', { class: 'avatar ti-avatar', 'aria-hidden': 'true' }, initials(login)), login) : '—'],
      ['Origin', ORIGINS[current.origin] || current.origin || '—'],
      ['Mode', current.mode === 'prompt' ? 'Prompt' : 'Goal (works until it is met)'],
      ['Account', accountName(current.account_label)],
      ['Models', m.models && m.models.length ? m.models.join(', ') : '—'],
      ['Turns', m.turns === null || m.turns === undefined ? '—' : int(m.turns)],
      ['Account switches', String(m.switches ?? (detail.switches || []).length ?? 0)],
      ['Branch', current.branch ? h('code', null, current.branch) : 'not created yet'],
      ['Pull request', pr ? prChip(pr) : 'none yet'],
      ['Delivery', delivery ? h('span', null, stageLabel(delivery.stage), ' ', h('button', { type: 'button', class: 'link-btn small', onClick: () => openDeliveryDialog(ctx, current) }, 'Details…')) : '—'],
      ['Spec', detail.spec_path ? h('code', null, detail.spec_path) : '—'],
      ['Worktree', detail.worktree ? h('code', null, detail.worktree) : '—'],
    ];
    const switches = Array.isArray(detail.switches) ? detail.switches : [];

    setChildren(body,
      facts,
      h('div', { class: 'ti-columns' },
        h('section', { class: 'card ti-card', 'aria-labelledby': 'ti-plan' }, h('h2', { class: 'ti-card-title', id: 'ti-plan' }, icon('tasks', { size: 14 }), 'Plan'), steps()),
        h('section', { class: 'card ti-card', 'aria-labelledby': 'ti-details' }, h('h2', { class: 'ti-card-title', id: 'ti-details' }, icon('target', { size: 14 }), 'Details'),
          h('dl', { class: 'kv ti-kv' }, rows.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)])))),
      h('section', { class: 'card ti-card', 'aria-labelledby': 'ti-sessions' }, h('h2', { class: 'ti-card-title', id: 'ti-sessions' }, icon('terminal', { size: 14 }), 'Sessions'), sessions()),
      switches.length ? h('section', { class: 'card ti-card', 'aria-labelledby': 'ti-switches' }, h('h2', { class: 'ti-card-title', id: 'ti-switches' }, icon('refresh', { size: 14 }), 'Account switches'),
        h('ul', { class: 'list-plain small' }, switches.map((sw) => h('li', null, `${relLong(sw.at)}: ${sw.from} → ${sw.to}`, sw.reason ? h('span', { class: 'muted' }, ` (${sw.reason})`) : null)))) : null);
  }

  let alive = true;
  async function load() {
    try {
      const fresh = await ctx.api.get(`/api/tasks/${enc(task.id)}`, { signal: reads.signal });
      if (!alive) return;
      detail = fresh;
      current = { ...current, ...fresh };
    } catch (error) {
      if (!alive || error.code === 'aborted') return;
      detailError = error;
    }
    render();
  }

  // Accounts as the Status panel sees them: emails come only to administrators (docs/api.md).
  async function loadAccounts() {
    try {
      const status = await ctx.api.get('/api/status', { signal: reads.signal });
      if (!alive) return;
      accounts = new Map((Array.isArray(status.accounts) ? status.accounts : []).map((a) => [a.label, a]));
      if (detail) render();
    } catch { /* labels stay */ }
  }

  const unwatchPulls = ctx.store.watchPulls(sandbox.id);
  const offs = [
    ctx.store.on('pulls', (sid) => { if (sid === sandbox.id) render(); }),
    // The task moves on while you read: its state, steps and times follow.
    ctx.store.on('task', (t) => { if (t.id === task.id) { current = { ...current, ...t }; render(); } }),
    ctx.store.on('steps', (taskId) => { if (taskId === task.id) load(); }),
  ];
  const el = h('div', { class: 'page ti-page' }, h('div', { class: 'page-inner ti-inner' }, head, body));
  renderHead();
  load();
  loadAccounts();

  return {
    el,
    destroy() { alive = false; reads.abort(); unwatchPulls(); offs.forEach((off) => off()); },
  };
}
