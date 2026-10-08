// Automations page (AUT-001..005): cron schedules with UTC offset, run history, Run now.

import { enc } from '../api.js';
import { h, badge, iconBadge, itemCard, iconButton, openModal, field, toast, confirmDialog, errorToast, toggle, setChildren, placeholder } from '../dom.js';
import { relLong, dateTime, describeCron, cronValid, utcOffset } from '../format.js';
import { allows } from '../status.js';
import { href } from '../router.js';
import { pageFrame, loader, dataTable } from './common.js';

// AutomationRun.state (docs/api.md "Skills, workflows, automations, MCP").
const RUN_TONE = { scheduled: 'warn', task_created: 'ok', skipped_previous_active: '', skipped_disabled: '', failed: 'danger' };
const RUN_LABEL = { scheduled: 'scheduled', task_created: 'task created', skipped_previous_active: 'skipped: previous task still active', skipped_disabled: 'skipped: disabled', failed: 'failed' };

const runBadge = (state) => badge(RUN_LABEL[state] || state || 'unknown', RUN_TONE[state] || '');

/** Quarter-hour offsets from UTC−12 to UTC+14, plus `current` when it is another valid value (±1080). */
function offsetOptions(current) {
  const out = [];
  for (let m = -12 * 60; m <= 14 * 60; m += 15) out.push(m);
  if (Number.isInteger(current) && !out.includes(current)) out.push(current);
  return out.sort((a, b) => a - b);
}

/** Mount the Automations page. */
export function mountAutomationsPage(ctx, sandbox) {
  const write = allows(sandbox.access, 'write');
  const frame = pageFrame({
    title: 'Automations',
    sub: 'scheduled tasks — agents can create them too',
    actions: [h('button', { type: 'button', class: 'btn btn-primary btn-sm', disabled: !write, title: write ? null : 'Requires write access', onClick: () => edit(null) }, 'New automation')],
  });
  const host = h('div');
  frame.body.append(host);
  let workflows = [];
  ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/workflows`).then((list) => { workflows = Array.isArray(list) ? list : []; }).catch(() => {});

  const taskLink = (taskId) => {
    const task = taskId ? ctx.store.findTask(taskId) : null;
    if (!task) return taskId ? h('span', { class: 'muted mono xs' }, String(taskId).slice(0, 8)) : null;
    const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`;
    return h('a', { href: href(path), dataset: { route: path } }, `#${task.number}`);
  };

  const reload = loader(host, () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/automations`), (list) => {
    const items = Array.isArray(list) ? list : [];
    if (items.length === 0) return h('div', { class: 'card placeholder' }, 'No automations. Example: every day at 09:00, create a task that implements yesterday’s customer feedback.');
    return h('div', { class: 'item-list' }, items.map((a) => itemCard({
      iconName: 'zap',
      title: a.name,
      badges: [a.created_via === 'agent' ? iconBadge('tools', 'Created by an agent', 'violet', 'An agent created this automation') : null],
      description: [`${describeCron(a.cron)} · ${utcOffset(a.utc_offset_minutes)} `, h('code', { class: 'muted' }, a.cron)],
      extra: h('div', { class: 'item-meta' },
        a.enabled && a.next_run_at ? h('span', { title: dateTime(a.next_run_at) }, `Next run ${relLong(a.next_run_at)}`) : h('span', null, a.enabled ? 'Not scheduled' : 'Paused'),
        a.last_run ? [' · last ', h('span', { title: dateTime(a.last_run.scheduled_for) }, relLong(a.last_run.scheduled_for)), ': ', runBadge(a.last_run.state), ' ', taskLink(a.last_run.task_id)] : ' · never ran'),
      controls: [
        toggle({ checked: a.enabled, disabled: !write, label: `${a.name} runs on schedule`, onChange: async (checked, input) => {
          try {
            await ctx.api.put(`/api/automations/${enc(a.id)}`, { ...editable(a), enabled: checked });
            a.enabled = checked;
            input.closest('.item-card').classList.toggle('off', !checked);
            toast(`${a.name} ${checked ? 'enabled' : 'paused'}`, { kind: 'ok' });
          } catch (error) {
            input.checked = !checked;
            errorToast(error, 'Could not update automation');
          }
        } }),
        h('button', { type: 'button', class: 'btn btn-sm', disabled: !write, title: write ? 'Create a task from this automation now' : 'Requires write access', onClick: () => runNow(a) }, 'Run now'),
        iconButton('clock', `Run history of ${a.name}`, () => history(a)),
        iconButton('trash', `Delete ${a.name}`, () => remove(a), { class: 'btn-quiet-danger', disabled: !write }),
      ],
      onOpen: write ? () => edit(a) : () => history(a),
      openLabel: write ? `Edit ${a.name}` : `Run history of ${a.name}`,
      dim: !a.enabled,
    })));
  });

  function editable(a) {
    return { name: a.name, cron: a.cron, utc_offset_minutes: a.utc_offset_minutes, prompt: a.prompt, workflow_id: a.workflow_id || null, enabled: a.enabled };
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
      else toast(`${a.name}: ${RUN_LABEL[state] || state || 'run recorded'}${task ? ` (#${task.number})` : ''}`, { kind: state === 'task_created' ? 'ok' : 'info' });
      if (run && run.task_id) ctx.store.loadTasks(sandbox.id).catch(() => null);
      reload();
    } catch (error) {
      errorToast(error, 'Run refused');
    }
  }

  function history(a) {
    const modal = openModal({ title: `Runs · ${a.name}`, wide: true, body: placeholder('Loading…', 'loading'), actions: [{ label: 'Close' }] });
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
      errorToast(error, 'Could not delete automation');
    }
  }

  function edit(a) {
    const name = h('input', { class: 'input', value: a ? a.name : '', maxlength: 100, autofocus: true });
    const cron = h('input', { class: 'input mono', value: a ? a.cron : '0 9 * * *', placeholder: 'min hour day month weekday' });
    const cronHint = h('span', { class: 'field-hint' });
    const offset = h('select', { class: 'select' }, offsetOptions(a ? a.utc_offset_minutes : null).map((m) => h('option', { value: String(m) }, utcOffset(m))));
    offset.value = String(a ? a.utc_offset_minutes : -new Date().getTimezoneOffset());
    const prompt = h('textarea', { class: 'textarea', rows: 6, placeholder: 'What the created task should do, e.g. “Implement yesterday’s customer feedback from the Quartio MCP.”', value: a ? a.prompt : '' });
    const workflow = h('select', { class: 'select' }, h('option', { value: '' }, 'No workflow'), workflows.map((w) => h('option', { value: w.id }, w.name)));
    workflow.value = a && a.workflow_id ? a.workflow_id : '';
    const enabled = h('input', { type: 'checkbox', checked: a ? a.enabled : true });
    const updateHint = () => {
      const ok = cronValid(cron.value);
      cron.setAttribute('aria-invalid', ok ? 'false' : 'true');
      cronHint.textContent = ok ? `${describeCron(cron.value)} (${utcOffset(Number(offset.value))})` : 'Five fields: minute hour day-of-month month day-of-week.';
    };
    cron.addEventListener('input', updateHint);
    offset.addEventListener('change', updateHint);
    updateHint();
    openModal({
      title: a ? `Edit automation · ${a.name}` : 'New automation',
      wide: true,
      body: h('div', { class: 'stack' },
        h('div', { class: 'form-grid' }, field('Name', name), field('Workflow', workflow)),
        h('div', { class: 'form-grid' }, h('label', { class: 'field' }, h('span', { class: 'field-label' }, 'Cron schedule'), cron, cronHint), field('Time zone (UTC offset)', offset)),
        field('Prompt', prompt),
        h('label', { class: 'check' }, enabled, h('span', null, 'Enabled'))),
      actions: [
        { label: 'Cancel', value: false },
        { label: a ? 'Save' : 'Create', kind: 'primary', run: async () => {
          if (!name.value.trim() || !prompt.value.trim()) { toast('Name and prompt are required.', { kind: 'warn' }); return false; }
          if (!cronValid(cron.value)) { cron.focus(); toast('The cron expression needs five fields.', { kind: 'warn' }); return false; }
          const body = { name: name.value.trim(), cron: cron.value.trim().split(/\s+/).join(' '), utc_offset_minutes: Number(offset.value), prompt: prompt.value.trim(), workflow_id: workflow.value || null, enabled: enabled.checked };
          if (a) await ctx.api.put(`/api/automations/${enc(a.id)}`, body);
          else await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/automations`, body);
          toast(a ? 'Automation saved.' : 'Automation created.', { kind: 'ok' });
          reload();
          return true;
        } },
      ],
    });
  }

  const off = ctx.store.on('automation-run', () => reload());
  return { el: frame.el, key: `automations:${sandbox.id}`, destroy: off };
}
