// Automations page (AUT-001..005): cron schedules with UTC offset, run history, Run now. An
// automation opens in its builder (automationedit.js, UI-039); so does New automation. As on the
// Workflows page, the automations Yulius ships come first as Defaults (SPEC-197 AUT-032..036): the
// project's automation of a shipped one's name, or the shipped one switched off until its switch
// enables it (SPEC-042 AUT-008); then the project's other automations, Custom.

import { enc } from '../api.js';
import { h, badge, iconBadge, itemCard, iconButton, openModal, toast, confirmDialog, errorToast, toggle, setChildren, placeholder, emptyState } from '../dom.js';
import { relLong, dateTime, describeCron, utcOffset, startPhrase } from '../format.js';
import { allows } from '../status.js';
import { href, navigate } from '../router.js';
import { pageFrame, loader, dataTable, createButton } from './common.js';
import { BRAND } from '../brand.js';

// AutomationRun.state (docs/api.md "Skills, workflows, automations, MCP").
const RUN_TONE = { scheduled: 'warn', task_created: 'ok', skipped_previous_active: '', skipped_disabled: '', failed: 'danger' };
const RUN_LABEL = { scheduled: 'Scheduled', task_created: 'Task created', skipped_previous_active: 'Skipped: previous task still active', skipped_disabled: 'Skipped: disabled', failed: 'Failed' };

/** What Delete says; deleting a default resets it (SPEC-197 AUT-035), here and in the builder. */
export function deleteMessage(isDefault) {
  return isDefault
    ? 'Future runs are cancelled. Tasks it already created stay. It stays among the defaults, off: switching it on again starts from the version shipped then, and changes made here are lost.'
    : 'Future runs are cancelled. Tasks it already created stay.';
}

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

  // The shipped list is optional: when it cannot be read, every automation shows under Custom.
  const fetchAll = () => Promise.all([
    ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/automations`),
    ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/automation-templates`).catch(() => []),
  ]);
  const reload = loader(host, 'the automations', fetchAll, ([list, templates]) => {
    const items = Array.isArray(list) ? list : [];
    const byId = new Map(items.map((a) => [a.id, a]));
    const defaults = (Array.isArray(templates) ? templates : []).map((t) => ({ template: t, automation: byId.get(t.automation_id) || null }));
    const taken = new Set(defaults.map((d) => d.automation && d.automation.id));
    const custom = items.filter((a) => !taken.has(a.id));
    const section = (id, title, hint, body) => h('section', { class: 'stack-sm', 'aria-labelledby': id },
      h('h2', { class: 'page-section-title', id }, title, h('span', { class: 'muted' }, hint)),
      body);
    return h('div', { class: 'stack' },
      defaults.length ? section('automations-defaults', 'Defaults', `ship with ${BRAND.name}, on or off per project`, h('div', { class: 'item-list' }, defaults.map(({ template, automation }) => (automation ? automationCard(automation, template) : shippedCard(template))))) : null,
      section('automations-custom', 'Custom', 'yours to change', custom.length
        ? h('div', { class: 'item-list' }, custom.map((a) => automationCard(a)))
        : emptyState({ iconName: 'zap', title: 'No custom automations yet', text: 'Start tasks on a schedule — for example, every day at 09:00 a task that works through yesterday’s customer feedback.', action: write ? createButton('New automation', create) : null })));
  }, { shape: 'items' });

  const defaultBadge = () => iconBadge('lock', 'Default', 'violet', `Ships with ${BRAND.name}; changes made here are kept`);
  const schedule = (a, extra = '') => [`${describeCron(a.cron)} · ${utcOffset(a.utc_offset_minutes)}${extra} `, h('code', { class: 'muted' }, a.cron)];

  /** A shipped automation not added to the project: off until its switch enables it. */
  function shippedCard(t) {
    return itemCard({
      iconName: 'zap',
      title: t.name,
      badges: [defaultBadge()],
      description: schedule(t),
      extra: [h('div', { class: 'item-meta' }, t.description), h('div', { class: 'item-meta' }, 'Off')],
      controls: [toggle({ checked: false, disabled: !write, label: `${t.name} runs on schedule`, onChange: (_checked, input) => enableShipped(t, input) })],
      dim: true,
    });
  }

  // One automation per switch: a press while it is being enabled keeps it on. The switch is not
  // disabled meanwhile, which would drop keyboard focus; the reload's keepFocus then finds the
  // added automation's switch by its label.
  const enabling = new Set();
  async function enableShipped(t, input) {
    if (enabling.has(t.id)) { input.checked = true; return; }
    if (!input.checked) return;
    enabling.add(t.id);
    input.setAttribute('aria-busy', 'true');
    try {
      await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/automation-templates/${enc(t.id)}/enable`);
      toast(`${t.name} enabled.`, { kind: 'ok' });
      await reload();
    } catch (error) {
      input.checked = false;
      errorToast(error, 'Couldn’t enable automation');
    } finally {
      enabling.delete(t.id);
      input.removeAttribute('aria-busy');
    }
  }

  /** A project automation; `template` when it is a shipped one (a default). */
  function automationCard(a, template = null) {
    const meta = h('div', { class: 'item-meta' },
      a.enabled && a.next_run_at ? h('span', { title: dateTime(a.next_run_at) }, `Next run ${relLong(a.next_run_at)}`) : h('span', null, a.enabled ? 'Not scheduled' : 'Paused'),
      a.last_run ? [' · last ', h('span', { title: dateTime(a.last_run.scheduled_for) }, relLong(a.last_run.scheduled_for)), ': ', runBadge(a.last_run.state), ' ', taskLink(a.last_run.task_id)] : ' · never ran');
    return itemCard({
      iconName: 'zap',
      title: a.name,
      badges: [template ? defaultBadge() : a.created_via === 'agent' ? iconBadge('tools', 'Created by an agent', 'violet', 'An agent created this automation') : null],
      description: schedule(a, startPhrase(a) ? ` · ${startPhrase(a)}` : ''),
      extra: template ? [h('div', { class: 'item-meta' }, template.description), meta] : meta,
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
        iconButton('trash', `Delete ${a.name}`, () => remove(a, template), { class: 'btn-quiet-danger', disabled: !write }),
      ],
      onOpen: () => navigate(`${base}/${encodeURIComponent(a.id)}`),
      openLabel: write ? `Edit ${a.name}` : `Open ${a.name}`,
      dim: !a.enabled,
    });
  }

  function editable(a) {
    // A PUT replaces every field: the start choice goes along (SPEC-078).
    return { name: a.name, cron: a.cron, utc_offset_minutes: a.utc_offset_minutes, prompt: a.prompt, workflow_id: a.workflow_id || null, enabled: a.enabled, supertask: Boolean(a.supertask), after_task_id: a.after_task_id || null, start_rule: a.start_rule || null, prepared: Boolean(a.prepared) };
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
    const modal = openModal({ title: `Runs · ${a.name}`, wide: true, body: placeholder('Loading the runs…', 'loading', ['table', 4]), actions: [{ label: 'Close' }] });
    ctx.api.get(`/api/automations/${enc(a.id)}/runs`).then((runs) => {
      setChildren(modal.body, dataTable([
        { label: 'Scheduled for', render: (r) => dateTime(r.scheduled_for) },
        { label: 'State', render: (r) => runBadge(r.state) },
        { label: 'Task', render: (r) => taskLink(r.task_id) || '—' },
        { label: 'Error', render: (r) => (r.error ? h('span', { class: 'field-error' }, r.error) : '—') },
      ], Array.isArray(runs) ? runs : [], { empty: 'No runs yet.' }));
    }).catch((error) => setChildren(modal.body, placeholder(error.message, 'error')));
  }

  async function remove(a, template) {
    if (!(await confirmDialog({ title: `Delete automation “${a.name}”?`, message: deleteMessage(Boolean(template)), confirmLabel: 'Delete', danger: true }))) return;
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
