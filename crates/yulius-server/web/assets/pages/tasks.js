// Tasks overview (SPEC-001 §6.2) and Archived tasks (UI-006, AC-ARC-1).

import { h, setChildren, iconButton, badge, toast, errorToast } from '../dom.js';
import { relShort, dateTime } from '../format.js';
import { statusInfo, statusGroup, harnessInfo, allows } from '../status.js';
import { openTaskMenu, restoreTask } from '../actions.js';
import { href } from '../router.js';
import { pageFrame, dataTable, loader } from './common.js';

function statusCell(task) {
  const info = statusInfo(task);
  return h('span', { class: 'row nowrap' }, h('span', { class: `glyph tone-${info.tone}`, 'aria-hidden': 'true' }, info.glyph), info.label);
}

function stepsCell(task) {
  const steps = task.steps || {};
  if (!steps.total) return h('span', { class: 'muted' }, '—');
  const pct = Math.round(((steps.done || 0) / steps.total) * 100);
  return h('span', { class: 'mini-progress', title: steps.current || '' }, h('span', { class: 'track' }, h('span', { class: 'fill', style: { width: `${pct}%`, display: 'block' } })), `${steps.done}/${steps.total}`);
}

function taskLink(sandbox, task) {
  const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`;
  return h('a', { href: href(path), dataset: { route: path } }, task.title || `Task #${task.number}`);
}

/** All non-archived tasks of the sandbox with filters. */
export function mountTasksPage(ctx, sandbox) {
  const frame = pageFrame({ title: 'Tasks', sub: sandbox.name });
  const search = h('input', { class: 'input', type: 'search', placeholder: 'Search title, branch, author…', 'aria-label': 'Search tasks' });
  const group = h('select', { class: 'select', 'aria-label': 'Status filter' },
    [['', 'All statuses'], ['active', 'Active'], ['attention', 'Needs attention'], ['closed', 'Closed'], ['stopped', 'Stopped'], ['failed', 'Failed']].map(([v, l]) => h('option', { value: v }, l)));
  const harness = h('select', { class: 'select', 'aria-label': 'Harness filter' }, h('option', { value: '' }, 'All harnesses'), h('option', { value: 'claude' }, 'Claude Code'), h('option', { value: 'codex' }, 'Codex'));
  const origin = h('select', { class: 'select', 'aria-label': 'Origin filter' }, h('option', { value: '' }, 'All origins'), ['ui', 'automation', 'import', 'onboarding', 'agent'].map((o) => h('option', { value: o }, o)));
  const tableHost = h('div');
  frame.body.append(h('div', { class: 'toolbar' }, search, group, harness, origin), tableHost);

  function render() {
    const tasks = ctx.store.tasksOf(sandbox.id);
    if (tasks === null) { setChildren(tableHost, h('div', { class: 'placeholder placeholder-loading' }, 'Loading…')); return; }
    const q = search.value.trim().toLowerCase();
    const rows = tasks.filter((t) => (!group.value || statusGroup(t) === group.value)
      && (!harness.value || t.harness === harness.value)
      && (!origin.value || t.origin === origin.value)
      && (!q || `${t.number} ${t.title} ${t.branch} ${t.created_by && t.created_by.login}`.toLowerCase().includes(q)));
    frame.sub.textContent = `${sandbox.name} · ${rows.length} of ${tasks.length}`;
    setChildren(tableHost, dataTable([
      { label: 'Status', render: statusCell },
      { label: '#', class: 'num', render: (t) => String(t.number) },
      { label: 'Title', class: 'title-cell', render: (t) => taskLink(sandbox, t) },
      { label: 'Harness', render: (t) => h('span', { class: 'row nowrap' }, h('span', { class: `task-harness ${t.harness}` }, harnessInfo(t.harness).glyph), harnessInfo(t.harness).name), phoneHide: true },
      { label: 'Author', render: (t) => (t.created_by && t.created_by.login) || '—', phoneHide: true },
      { label: 'Branch', render: (t) => h('code', null, t.branch || '—'), phoneHide: true },
      { label: 'Steps', render: stepsCell, phoneHide: true },
      { label: 'Activity', class: 'num', render: (t) => h('span', { title: dateTime(t.updated_at) }, relShort(t.updated_at)) },
      { label: '', render: (t) => iconButton('more', 'Task actions', (event) => openTaskMenu(ctx, event.currentTarget, t)) },
    ], rows, {
      empty: tasks.length ? 'No task matches the filters.' : 'No active tasks in this sandbox.',
      onRowClick: (t) => ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}/t/${t.number}`),
    }));
  }
  for (const control of [search, group, harness, origin]) control.addEventListener('input', render);
  const off = ctx.store.on('tasks', (sid) => { if (sid === sandbox.id) render(); });
  render();
  return { el: frame.el, key: `tasks:${sandbox.id}`, destroy: off };
}

/** Archived tasks across visible sandboxes, with restore. */
export function mountArchivedPage(ctx) {
  const frame = pageFrame({ title: 'Archived tasks', sub: 'across your sandboxes' });
  const host = h('div');
  frame.body.append(host);
  const reload = loader(host, () => ctx.api.get('/api/archived-tasks'), (tasks) => {
    const list = Array.isArray(tasks) ? tasks : [];
    frame.sub.textContent = `${list.length} archived across your sandboxes`;
    return dataTable([
      { label: 'Sandbox', render: (t) => { const sb = ctx.store.sandboxById(t.sandbox_id); return sb ? sb.name : badge('unknown'); } },
      { label: '#', class: 'num', render: (t) => String(t.number) },
      { label: 'Title', class: 'title-cell', render: (t) => { const sb = ctx.store.sandboxById(t.sandbox_id); return sb ? taskLink(sb, t) : t.title; } },
      { label: 'Status', render: statusCell, phoneHide: true },
      { label: 'Harness', render: (t) => harnessInfo(t.harness).name, phoneHide: true },
      { label: 'Archived', class: 'num', render: (t) => h('span', { title: dateTime(t.updated_at) }, relShort(t.updated_at)) },
      { label: '', render: (t) => {
        const sb = ctx.store.sandboxById(t.sandbox_id);
        const can = sb && allows(sb.access, 'write');
        return h('button', { type: 'button', class: 'btn btn-sm', disabled: !can, title: can ? 'Restore to the sidebar' : 'Requires write access', onClick: async () => {
          const updated = await restoreTask(ctx, t);
          if (updated) reload();
        } }, 'Restore');
      } },
    ], list, { empty: 'No archived tasks. Archive finished tasks from their action menu.' });
  });
  return { el: frame.el, key: 'archived' };
}

/** CLI device approval (docs/api.md device flow): approve or deny a user code. */
export function mountDevicePage(ctx, route) {
  const frame = pageFrame({ title: 'CLI device approval', sub: 'authorize `yulius import` and other CLI sessions' });
  const code = h('input', { class: 'input mono', placeholder: 'ABCD-EFGH', autocomplete: 'off', spellcheck: 'false', value: route.query.get('code') || route.query.get('user_code') || '', style: { maxWidth: '220px', textTransform: 'uppercase' }, 'aria-label': 'User code' });
  const result = h('div', { role: 'status' });
  const decide = (approve) => async () => {
    const userCode = code.value.trim().toUpperCase();
    if (!userCode) { code.focus(); return; }
    try {
      await ctx.api.post('/api/device/approve', { user_code: userCode, approve });
      setChildren(result, h('div', { class: `banner ${approve ? 'banner-info' : ''}` }, approve ? `Approved ${userCode}. The CLI receives a token on its next poll.` : `Denied ${userCode}.`));
      toast(approve ? 'Device approved' : 'Device denied', { kind: approve ? 'ok' : 'info' });
    } catch (error) {
      errorToast(error, 'Approval failed');
    }
  };
  frame.body.append(h('div', { class: 'card card-pad stack', style: { maxWidth: '560px' } },
    h('p', { class: 'text-2' }, 'Enter the code shown by the CLI. Approve only codes you started yourself; the CLI gets a token with your permissions.'),
    h('div', { class: 'row-wrap' }, code, h('button', { type: 'button', class: 'btn btn-primary', onClick: decide(true) }, 'Approve'), h('button', { type: 'button', class: 'btn', onClick: decide(false) }, 'Deny')),
    result));
  return { el: frame.el, key: 'device' };
}

