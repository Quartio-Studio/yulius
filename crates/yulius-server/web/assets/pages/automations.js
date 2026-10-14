// Automations page (AUT-001..005): cron schedules with UTC offset, run history, Run now. An
// automation opens in its builder (automationedit.js, UI-039); so does New automation. Below the
// list, the ready-made automations not added yet, each enabled with one click (SPEC-042 AUT-009).

import { enc } from '../api.js';
import { h, badge, iconBadge, itemCard, iconButton, openModal, toast, confirmDialog, errorToast, toggle, setChildren, placeholder, emptyState } from '../dom.js';
import { relLong, dateTime, describeCron, utcOffset, startPhrase } from '../format.js';
import { allows } from '../status.js';
import { href, navigate } from '../router.js';
import { pageFrame, loader, dataTable, createButton } from './common.js';

// AutomationRun.state (docs/api.md "Skills, workflows, automations, MCP").
const RUN_TONE = { scheduled: 'warn', task_created: 'ok', skipped_previous_active: '', skipped_disabled: '', failed: 'danger' };
const RUN_LABEL = { scheduled: 'Scheduled', task_created: 'Task created', skipped_previous_active: 'Skipped: previous task still active', skipped_disabled: 'Skipped: disabled', failed: 'Failed' };

const runBadge = (state) => badge(RUN_LABEL[state] || state || 'Unknown', RUN_TONE[state] || '');

/** Mount the Automations page. */
export function mountAutomationsPage(ctx, sandbox) {
  const write = allows(sandbox.access, 'write');
  const base = `/s/${encodeURIComponent(sandbox.slug)}/automations`;
  const create = () => navigate(`${base}/new`);
  const frame = pageFrame({
    title: 'Automations',
    sub: 'scheduled tasks — agents can create them too',
    actions: [createButton('New automation', create, { disabled: !write, title: write ? null : 'Requires write access' })],
  });
  const host = h('div');
  frame.body.append(host);

  const taskLink = (taskId) => {
    const task = taskId ? ctx.store.findTask(taskId) : null;
    if (!task) return taskId ? h('span', { class: 'muted' }, 'a task') : null;
    const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`;
    return h('a', { href: href(path), dataset: { route: path } }, `#${task.number}`);
  };

  // The suggestions are optional: when they cannot be read, the page still shows the list.
  const fetchAll = () => Promise.all([
    ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/automations`),
    ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/automation-templates`).catch(() => []),
  ]);
  const reload = loader(host, 'the automations', fetchAll, ([list, templates]) => {
    const items = Array.isArray(list) ? list : [];
    const offered = (Array.isArray(templates) ? templates : []).filter((t) => !t.automation_id);
    return h('div', { class: 'stack' }, automationList(items), offered.length ? readyMade(offered) : null);
  });

  function readyMade(templates) {
    return h('section', { class: 'stack-sm', 'aria-labelledby': 'automations-ready-made' },
      h('h2', { class: 'page-section-title', id: 'automations-ready-made' }, 'Ready-made automations', h('span', { class: 'muted' }, 'enable in one click, edit it afterwards')),
      h('div', { class: 'item-list' }, templates.map((t) => itemCard({
        iconName: 'zap',
        title: t.name,
        description: t.description,
        extra: h('div', { class: 'item-meta' }, `${describeCron(t.cron)} · ${utcOffset(t.utc_offset_minutes)}`),
        controls: [h('button', { type: 'button', class: 'btn btn-sm btn-primary', disabled: !write, title: write ? null : 'Requires write access', 'aria-label': `Enable ${t.name}`, onClick: (event) => enableTemplate(t, event.currentTarget) }, 'Enable')],
      }))));
  }

  async function enableTemplate(t, button) {
    button.disabled = true; // one automation per press
    try {
      await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/automation-templates/${enc(t.id)}/enable`);
      toast(`${t.name} enabled.`, { kind: 'ok' });
      reload();
    } catch (error) {
      button.disabled = false;
      errorToast(error, 'Couldn’t enable automation');
    }
  }

  function automationList(items) {
    if (items.length === 0) return emptyState({ iconName: 'zap', title: 'No automations yet', text: 'Start tasks on a schedule — for example, every day at 09:00 a task that works through yesterday’s customer feedback.', action: write ? createButton('New automation', create) : null });
    return h('div', { class: 'item-list' }, items.map((a) => itemCard({
      iconName: 'zap',
      title: a.name,
      badges: [a.created_via === 'agent' ? iconBadge('tools', 'Created by an agent', 'violet', 'An agent created this automation') : null],
      description: [`${describeCron(a.cron)} · ${utcOffset(a.utc_offset_minutes)}${startPhrase(a) ? ` · ${startPhrase(a)}` : ''} `, h('code', { class: 'muted' }, a.cron)],
      extra: h('div', { class: 'item-meta' },
        a.enabled && a.next_run_at ? h('span', { title: dateTime(a.next_run_at) }, `Next run ${relLong(a.next_run_at)}`) : h('span', null, a.enabled ? 'Not scheduled' : 'Paused'),
        a.last_run ? [' · last ', h('span', { title: dateTime(a.last_run.scheduled_for) }, relLong(a.last_run.scheduled_for)), ': ', runBadge(a.last_run.state), ' ', taskLink(a.last_run.task_id)] : ' · never ran'),
      controls: [
        toggle({ checked: a.enabled, disabled: !write, label: `${a.name} runs on schedule`, onChange: async (checked, input) => {
          try {
            await ctx.api.put(`/api/automations/${enc(a.id)}`, { ...editable(a), enabled: checked });
            a.enabled = checked;
            input.closest('.item-card').classList.toggle('off', !checked);
            toast(`${a.name} ${checked ? 'enabled' : 'paused'}.`, { kind: 'ok' });
          } catch (error) {
            input.checked = !checked;
            errorToast(error, 'Couldn’t update automation');
          }
        } }),
        h('button', { type: 'button', class: 'btn btn-sm', disabled: !write, title: write ? 'Create a task from this automation now' : 'Requires write access', onClick: () => runNow(a) }, 'Run now'),
        iconButton('clock', `Run history of ${a.name}`, () => history(a)),
        iconButton('trash', `Delete ${a.name}`, () => remove(a), { class: 'btn-quiet-danger', disabled: !write }),
      ],
      onOpen: () => navigate(`${base}/${encodeURIComponent(a.id)}`),
      openLabel: write ? `Edit ${a.name}` : `Open ${a.name}`,
      dim: !a.enabled,
    })));
  }

  function editable(a) {
    // A PUT replaces every field: the start choice goes along (SPEC-078).
    return { name: a.name, cron: a.cron, utc_offset_minutes: a.utc_offset_minutes, prompt: a.prompt, workflow_id: a.workflow_id || null, enabled: a.enabled, supertask: Boolean(a.supertask), after_task_id: a.after_task_id || null, start_rule: a.start_rule || null };
  }

  /** One run now, also when disabled; the response is the AutomationRun. */
  const running = new Set();
  async function runNow(a) {
    if (running.has(a.id)) return; // one run per press
    running.add(a.id);
    try { await runNowOnce(a); } finally { running.delete(a.id); }
  }
  async function runNowOnce(a) {
    try {
      const run = await ctx.api.post(`/api/automations/${enc(a.id)}/run`);
      const state = run && run.state;
      const task = run && run.task_id ? ctx.store.findTask(run.task_id) : null;
      if (state === 'failed') toast(`${a.name}: run failed${run.error ? ` — ${run.error}` : ''}`, { kind: 'error' });
      else toast(`${a.name}: ${(RUN_LABEL[state] || state || 'run recorded').toLowerCase()}${task ? ` (#${task.number})` : ''}.`, { kind: state === 'task_created' ? 'ok' : 'info' });
      if (run && run.task_id) ctx.store.loadTasks(sandbox.id).catch(() => null);
      reload();
    } catch (error) {
      errorToast(error, 'Run refused');
    }
  }

  function history(a) {
    const modal = openModal({ title: `Runs · ${a.name}`, wide: true, body: placeholder('Loading the runs…', 'loading'), actions: [{ label: 'Close' }] });
    ctx.api.get(`/api/automations/${enc(a.id)}/runs`).then((runs) => {
      setChildren(modal.body, dataTable([
        { label: 'Scheduled for', render: (r) => dateTime(r.scheduled_for) },
        { label: 'State', render: (r) => runBadge(r.state) },
        { label: 'Task', render: (r) => taskLink(r.task_id) || '—' },
        { label: 'Error', render: (r) => (r.error ? h('span', { class: 'field-error' }, r.error) : '—') },
      ], Array.isArray(runs) ? runs : [], { empty: 'No runs yet.' }));
    }).catch((error) => setChildren(modal.body, placeholder(error.message, 'error')));
  }

  async function remove(a) {
    if (!(await confirmDialog({ title: `Delete automation “${a.name}”?`, message: 'Future runs are cancelled. Tasks it already created stay.', confirmLabel: 'Delete', danger: true }))) return;
    try {
      await ctx.api.del(`/api/automations/${enc(a.id)}`);
      toast('Automation deleted.', { kind: 'ok' });
      reload();
    } catch (error) {
      errorToast(error, 'Couldn’t delete automation');
    }
  }

  const off = ctx.store.on('automation-run', () => reload());
  return { el: frame.el, key: `automations:${sandbox.id}`, destroy: off };
}
