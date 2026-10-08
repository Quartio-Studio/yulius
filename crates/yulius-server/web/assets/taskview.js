// Center views: the selected task (step strip + native terminal + phone quick input)
// and the new-task composer shown when no task is selected (SPEC-001 §6.1, STP-003).

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, toast, errorToast, field, confirmDialog, placeholder } from './dom.js';
import { createTerminalView } from './terminal.js';
import { statusInfo, harnessInfo, isLive, allows, taskActions } from './status.js';
import { openTaskMenu, continueTask } from './actions.js';
import { relLong, dateTime } from './format.js';
import { readJson, writeJson } from './store.js';
import { href } from './router.js';

const STEP_MARK = { done: '✓', in_progress: '▸', pending: '·', skipped: '–', blocked: '✕' };
const STRIP_KEY = 'yulius.stepsOpen';

// Phone quick-input keys (SPEC-001 §6.4).
const QUICK_KEYS = [
  ['Esc', '\x1b'],
  ['Ctrl-C', '\x03'],
  ['Tab', '\t'],
  ['↑', '\x1b[A'],
  ['↓', '\x1b[B'],
  ['Enter', '\r'],
];

function readOnlyText(sandbox) {
  return sandbox && sandbox.access === 'read' ? 'Read-only (GitHub: read)' : 'Read-only';
}

/** Mount the task view for one task. Returns {el, key, update(task), destroy()}. */
export function mountTaskView(ctx, sandbox, task) {
  let current = task;
  let detail = null;
  let stripOpen = Boolean(readJson(STRIP_KEY, false));
  const destroyers = [];

  // ---- step strip
  const stripLabel = h('span', { class: 'step-label' });
  const progressFill = h('div', { class: 'step-progress-fill', style: { width: '0%' } });
  const progress = h('div', { class: 'step-progress', role: 'progressbar', 'aria-valuemin': 0, 'aria-valuemax': 100 }, progressFill);
  const statusEl = h('span', { class: 'strip-status' });
  const toggleButton = h('button', { type: 'button', class: 'stepstrip-toggle', 'aria-expanded': String(stripOpen) }, icon('right', { size: 14 }), stripLabel);
  const kebab = iconButton('more', 'Task actions', () => openTaskMenu(ctx, kebab, current));
  const stepList = h('div', { class: 'steplist', hidden: !stripOpen });
  const strip = h('div', { class: ['stepstrip', stripOpen && 'open'] },
    h('div', { class: 'stepstrip-main' }, toggleButton, progress, statusEl, kebab),
    stepList);
  toggleButton.addEventListener('click', () => {
    stripOpen = !stripOpen;
    writeJson(STRIP_KEY, stripOpen);
    strip.classList.toggle('open', stripOpen);
    stepList.hidden = !stripOpen;
    toggleButton.setAttribute('aria-expanded', String(stripOpen));
    if (stripOpen) renderStepList();
    terminal.refit();
  });

  // ---- terminal
  const terminal = createTerminalView({
    transport: ctx.transport,
    path: `/api/tasks/${enc(task.id)}/terminal`,
    onLink: (uri, event) => ctx.openLink(uri, event),
    readOnlyLabel: () => readOnlyText(ctx.store.sandboxById(sandbox.id) || sandbox),
    exitActions: () => (taskActions(current, sandbox.access).continue || allows(sandbox.access, 'write'))
      ? [{ label: 'Continue', run: () => continueTask(ctx, current) }]
      : [],
  });

  // ---- phone quick input
  const quickInput = h('input', { class: 'input', type: 'text', placeholder: 'Message the agent…', autocomplete: 'off', autocapitalize: 'off', enterkeyhint: 'send', 'aria-label': 'Quick input' });
  const sendQuick = () => {
    const text = quickInput.value;
    if (!text) { terminal.sendKeys('\r'); return; }
    if (terminal.sendKeys(text)) {
      quickInput.value = '';
      // Submit separately so TUIs treat the text as typed input, not a pasted newline.
      setTimeout(() => terminal.sendKeys('\r'), 60);
    }
  };
  quickInput.addEventListener('keydown', (event) => { if (event.key === 'Enter') { event.preventDefault(); sendQuick(); } });
  const quickbar = h('div', { class: 'quickbar' },
    h('div', { class: 'quickbar-input' }, quickInput, h('button', { type: 'button', class: 'btn btn-primary', onClick: sendQuick }, 'Send')),
    h('div', { class: 'quickbar-keys', role: 'group', 'aria-label': 'Keys' }, QUICK_KEYS.map(([label, seq]) => h('button', { type: 'button', class: 'btn', onClick: () => terminal.sendKeys(seq), 'aria-label': `Send ${label}` }, label))));

  const el = h('div', { class: 'taskview' }, strip, terminal.el, quickbar);

  function renderStrip() {
    const steps = current.steps || { done: 0, total: 0, current: null };
    const info = statusInfo(current);
    const hi = harnessInfo(current.harness);
    const total = steps.total || 0;
    const done = steps.done || 0;
    let n = total ? Math.min(total, done + (steps.current ? 1 : 0)) : 0;
    if (detail && Array.isArray(detail.steps_list)) {
      const ordered = [...detail.steps_list].sort((a, b) => a.ordinal - b.ordinal);
      const index = ordered.findIndex((s) => s.status === 'in_progress');
      if (index >= 0) n = index + 1;
    }
    setChildren(stripLabel,
      h('span', { class: 'step-task', title: current.title }, `#${current.number} ${current.title}`),
      total
        ? [h('span', { class: 'step-count' }, `Step ${n || done}/${total}`), h('span', { class: 'step-current' }, steps.current ? `· ${steps.current}` : done === total ? '· All steps done' : '')]
        : h('span', { class: 'step-current muted' }, 'No plan steps yet'));
    const pct = total ? Math.round((done / total) * 100) : 0;
    progressFill.style.width = `${pct}%`;
    progress.setAttribute('aria-valuenow', String(pct));
    progress.setAttribute('aria-label', `${done} of ${total} steps done`);
    progress.hidden = !total;
    setChildren(statusEl,
      h('span', { class: `glyph tone-${info.tone}`, 'aria-hidden': 'true' }, info.glyph),
      h('span', null, info.label),
      h('span', { class: `task-harness ${current.harness}`, title: hi.name }, hi.glyph),
      current.mode === 'goal' ? h('span', { class: 'chip', title: 'Runs as a goal (/goal): the agent keeps working until it is met' }, 'Goal') : null,
      current.account_label ? h('span', { class: 'muted xs' }, current.account_label) : null);
  }

  function renderStepList() {
    if (!stripOpen) return;
    if (!detail) { setChildren(stepList, placeholder('Loading steps…', 'loading')); return; }
    const steps = Array.isArray(detail.steps_list) ? [...detail.steps_list].sort((a, b) => a.ordinal - b.ordinal) : [];
    if (steps.length === 0) {
      setChildren(stepList, h('div', { class: 'muted small', style: { padding: '6px 0' } }, 'The agent has not written a plan yet. Steps appear here as soon as the task spec has a Plan checklist.'));
      return;
    }
    setChildren(stepList,
      steps.map((step) => h('div', { class: ['step-item', `st-${step.status}`] },
        h('span', { class: 'step-mark', 'aria-label': step.status }, STEP_MARK[step.status] || '·'),
        h('span', { class: 'step-title' }, step.title),
        h('span', { class: 'step-audit', title: dateTime(step.updated_at) }, [step.updated_by, step.updated_at ? relLong(step.updated_at) : null].filter(Boolean).join(' · ')),
        step.note ? h('span', { class: 'step-note' }, step.note) : null)),
      detail.spec_path ? h('div', { class: 'muted xs', style: { paddingTop: '6px' } }, 'Spec: ', h('code', null, detail.spec_path)) : null);
  }

  async function loadDetail() {
    try {
      detail = await ctx.api.get(`/api/tasks/${enc(current.id)}`);
      if (detail && detail.id === current.id) {
        current = { ...current, ...pickSummary(detail) };
        renderStrip();
      }
    } catch (error) {
      if (error.status !== 404) errorToast(error, 'Could not load task details');
    }
    renderStepList();
  }

  destroyers.push(ctx.store.on('steps', (taskId) => { if (taskId === current.id) loadDetail(); }));
  destroyers.push(ctx.store.on('resync', () => loadDetail()));

  renderStrip();
  loadDetail();

  return {
    el,
    key: `task:${task.id}`,
    start() {
      terminal.start();
      if (!window.matchMedia('(max-width: 767px)').matches) setTimeout(() => terminal.focus(), 50);
    },
    update(next) {
      const wasLive = isLive(current.status);
      current = { ...current, ...next };
      renderStrip();
      // A continued task gets a new attempt: reconnect if the socket had ended.
      if (!wasLive && isLive(current.status) && terminal.socket.status !== 'open') terminal.reconnect();
      // The user left the agent session (e.g. /exit): back to the task list (HRN-002).
      if (wasLive && current.status === 'closed' && ctx.onTaskClosed) ctx.onTaskClosed(current);
    },
    get task() { return current; },
    get detail() { return detail; },
    destroy() {
      destroyers.forEach((off) => off());
      terminal.destroy();
    },
  };
}

function pickSummary(detail) {
  const keys = ['status', 'activity', 'title', 'steps', 'unread_messages', 'account_label', 'harness', 'branch', 'updated_at', 'archived', 'mode'];
  const out = {};
  for (const key of keys) if (key in detail) out[key] = detail[key];
  return out;
}

// ---------------------------------------------------------------- composer

/** New-task composer (J3): prompt, mode, workflow, optional model/effort, Start (Claude Code). */
export function mountComposer(ctx, sandbox) {
  const destroyers = [];
  const write = allows(sandbox.access, 'write');
  const prompt = h('textarea', { class: 'textarea composer-prompt', placeholder: 'Describe what you want to build or change — e.g. “make me an online store”…', 'aria-label': 'Task prompt', maxlength: 100000 });
  // Goal mode is the default (TSK-010): the prompt becomes the session's /goal.
  const modeInputs = [['goal', 'Goal'], ['prompt', 'Prompt']].map(([value]) => h('input', { type: 'radio', name: 'composer-mode', value, checked: value === 'goal' }));
  const modeHint = h('span', { class: 'muted xs' });
  const renderModeHint = () => {
    modeHint.textContent = modeInputs[0].checked
      ? 'The agent keeps working until the goal is met (Claude Code\u2019s /goal).'
      : 'Sent as a normal first message.';
  };
  modeInputs.forEach((input) => input.addEventListener('change', renderModeHint));
  renderModeHint();
  const workflow = h('select', { class: 'select', 'aria-label': 'Workflow' }, h('option', { value: '' }, 'No workflow'));
  const model = h('input', { class: 'input', placeholder: 'default', autocomplete: 'off', 'aria-label': 'Model' });
  const effort = h('select', { class: 'select', 'aria-label': 'Effort' }, h('option', { value: '' }, 'default'), ['low', 'medium', 'high'].map((v) => h('option', { value: v }, v)));
  const title = h('input', { class: 'input', placeholder: 'derived from the prompt', maxlength: 200, 'aria-label': 'Title' });
  const banner = h('div', { class: 'stack-sm' });
  const startButton = h('button', { type: 'button', class: 'btn btn-primary' }, 'Start task');
  const hint = h('span', { class: 'muted xs' }, h('span', { class: 'kbd' }, navigator.platform && navigator.platform.includes('Mac') ? '⌘' : 'Ctrl'), ' + ', h('span', { class: 'kbd' }, 'Enter'), ' to start');

  const el = h('div', { class: 'composer' },
    h('div', { class: 'composer-card' },
      h('div', { class: 'row' }, h('h1', { class: 'composer-title' }, 'New task'), h('span', { class: 'composer-repo muted small' }, 'in', sandbox.repo ? icon('github', { size: 14 }) : null, h('span', { class: 'ellipsis' }, sandbox.repo ? `${sandbox.repo.owner}/${sandbox.repo.name}` : sandbox.name))),
      banner,
      prompt,
      h('div', { class: 'composer-row' },
        field('Mode', h('div', { class: 'stack-sm' }, h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Mode' }, modeInputs.map((input) => h('label', null, input, input.value === 'goal' ? 'Goal' : 'Prompt'))), modeHint)),
        field('Workflow', workflow)),
      h('details', null,
        h('summary', null, 'Model, effort and title (optional)'),
        h('div', { class: 'form-grid' }, field('Model', model, 'Passed to Claude Code, e.g. opus, sonnet or a full model id'), field('Effort', effort), field('Title', title))),
      h('div', { class: 'composer-foot' }, startButton, hint)));

  // The superadmin is reminded while no recovery code exists (ACC-011).
  let recovery = null;
  const me0 = ctx.store.state.me;
  if (me0 && me0.user && me0.user.is_superadmin) {
    ctx.api.get('/api/auth/recovery').then((state) => { recovery = state; renderBanner(); }).catch(() => {});
  }

  function renderBanner() {
    const sb = ctx.store.sandboxById(sandbox.id) || sandbox;
    const items = [];
    const me = ctx.store.state.me;
    if (me && me.user && me.user.is_superadmin && recovery && !recovery.available) {
      items.push(h('div', { class: 'banner banner-danger' }, icon('warn'),
        h('span', { class: 'banner-text' }, h('strong', null, recovery.locked ? 'Password recovery is disabled. ' : 'No recovery code yet. '), 'Without a recovery code a forgotten superadmin password cannot be reset.'),
        h('a', { class: 'btn btn-sm', href: href('/admin/users'), dataset: { route: '/admin/users' } }, 'Generate one')));
    }
    // Onboarding (docs/api.md "Sandboxes"): `pending` waits for a person to approve the
    // resting preparation task (write), `preparing` runs it; admins may skip either.
    const blocked = sb.onboarding === 'pending' || sb.onboarding === 'preparing';
    if (blocked) {
      const prep = (ctx.store.tasksOf(sb.id) || []).find((t) => t.origin === 'onboarding');
      const pending = sb.onboarding === 'pending';
      items.push(h('div', { class: 'banner banner-info' }, icon('refresh'),
        h('span', { class: 'banner-text' },
          h('strong', null, pending ? 'This project needs preparation. ' : 'Preparing this project. '),
          pending
            ? 'The repository does not follow the Yulius documentation format yet. Start the preparation task to adapt it; new tasks can start once it lands.'
            : 'A preparation task adapts the imported repository to the Yulius format; new tasks can start once it lands.',
          prep ? [' ', h('a', { href: href(`/s/${encodeURIComponent(sb.slug)}/t/${prep.number}`), dataset: { route: `/s/${encodeURIComponent(sb.slug)}/t/${prep.number}` } }, `Open #${prep.number}`)] : null),
        pending && allows(sb.access, 'write') ? h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: approveOnboarding }, 'Start preparation') : null,
        allows(sb.access, 'admin') ? h('button', { type: 'button', class: 'btn btn-sm', onClick: skipOnboarding }, 'Skip for now') : null));
    }
    if (!write) items.push(h('div', { class: 'banner' }, icon('lock'), h('span', { class: 'banner-text' }, 'Read-only (GitHub: read). You can watch tasks but not start them.')));
    setChildren(banner, items);
    const disabled = !write || blocked;
    startButton.disabled = disabled;
    prompt.disabled = disabled;
    startButton.title = blocked ? 'Blocked until project preparation finishes' : !write ? 'Requires write access' : '';
  }

  async function approveOnboarding() {
    try {
      const updated = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/onboarding/approve`);
      if (updated && updated.id) ctx.store.upsertSandbox(updated);
      await ctx.store.loadTasks(sandbox.id).catch(() => null);
      toast('Preparation started.', { kind: 'ok' });
    } catch (error) {
      errorToast(error, 'Could not start preparation');
    }
  }

  async function skipOnboarding() {
    const ok = await confirmDialog({
      title: 'Skip project preparation?',
      message: 'Tasks may start before the repository follows the Yulius documentation format. The preparation task keeps its history.',
      confirmLabel: 'Skip for now',
    });
    if (!ok) return;
    try {
      const updated = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/onboarding/skip`);
      if (updated && updated.id) ctx.store.upsertSandbox(updated);
      toast('Preparation skipped; tasks can start now.', { kind: 'ok' });
    } catch (error) {
      errorToast(error, 'Could not skip preparation');
    }
  }

  async function start() {
    const text = prompt.value.trim();
    if (!text) { prompt.focus(); toast('Write a prompt first.', { kind: 'warn' }); return; }
    const body = { prompt: text, mode: modeInputs.find((i) => i.checked).value };
    if (workflow.value) body.workflow_id = workflow.value;
    if (model.value.trim()) body.model = model.value.trim();
    if (effort.value) body.effort = effort.value;
    if (title.value.trim()) body.title = title.value.trim();
    startButton.disabled = true;
    startButton.classList.add('busy');
    try {
      const task = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/tasks`, body);
      ctx.store.upsertTask(task);
      prompt.value = '';
      ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`);
    } catch (error) {
      errorToast(error, 'Could not start the task');
    } finally {
      startButton.classList.remove('busy');
      renderBanner();
    }
  }

  startButton.addEventListener('click', start);
  prompt.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) { event.preventDefault(); start(); }
  });

  ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/workflows`).then((list) => {
    for (const wf of Array.isArray(list) ? list : []) workflow.append(h('option', { value: wf.id }, `${wf.name}${wf.scope === 'instance' ? ' (instance)' : ''}`));
  }).catch(() => { /* workflows are optional */ });

  destroyers.push(ctx.store.on('sandboxes', renderBanner));
  destroyers.push(ctx.store.on('tasks', (sid) => { if (sid === sandbox.id) renderBanner(); }));
  renderBanner();

  return {
    el,
    key: `composer:${sandbox.id}`,
    start() { if (!prompt.disabled && !window.matchMedia('(max-width: 767px)').matches) prompt.focus(); },
    destroy() { destroyers.forEach((off) => off()); },
  };
}
