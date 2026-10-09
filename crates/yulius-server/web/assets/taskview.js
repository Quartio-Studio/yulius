// Center views: the selected task (step strip + native terminal + phone quick input)
// and the new-task composer shown when no task is selected (SPEC-001 §6.1, STP-003).

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, toast, errorToast, confirmDialog, placeholder, openMenu, statusMark, singleFlight, keepFocus } from './dom.js';
import { createTerminalView } from './terminal.js';
import { taskInfo, isLive, allows, taskActions, endedMessage } from './status.js';
import { openTaskMenu, continueTask, deliveryStrip } from './actions.js';
import { relLong, dateTime, resetLabel } from './format.js';
import { readJson, writeJson } from './store.js';
import { href } from './router.js';
import { findModel, findEffort, modelMenuItems, effortMenuItems, MODEL_PREF_KEY, EFFORT_PREF_KEY } from './models.js';
import { randomQuote } from './quotes.js';
import { brandSmoke } from './brandfx.js';
import { BRAND } from './brand.js';
import { isPhone } from './theme.js';

const STEP_MARK = { done: '✓', in_progress: '▸', pending: '·', skipped: '–', blocked: '✕' };
const STEP_LABEL = { done: 'Done', in_progress: 'In progress', pending: 'To do', blocked: 'Blocked', skipped: 'Skipped' };
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

/**
 * What a blank terminal means while the agent has not drawn anything yet (F 2026-10-09: helpful
 * empty states instead of an empty black screen); null once it should show the session.
 */
function emptyTerminalMessage(task) {
  switch (task.status) {
    case 'queued': return { title: 'Queued', text: 'The agent starts as soon as a session slot is free.' };
    case 'starting': return { title: 'Starting Claude Code…', text: 'The session appears here in a moment.' };
    case 'switching': return { title: 'Moving to another AI account…', text: 'The session continues here in a moment.' };
    case 'waiting_capacity': {
      const reset = task.resumes_at ? resetLabel(task.resumes_at) : '';
      return { title: 'Usage limit reached', text: `Every AI account is at its limit${reset ? ` · ${reset}` : ''}. The agent continues by itself.` };
    }
    case 'running': return { title: 'Claude Code is starting…', text: 'The session appears here in a moment.' };
    default: return null;
  }
}

/** Mount the task view for one task. Returns {el, key, update(task), destroy()}. */
export function mountTaskView(ctx, sandbox, task) {
  let current = task;
  let detail = null;
  let stripOpen = Boolean(readJson(STRIP_KEY, false));
  const destroyers = [];

  // ---- step strip
  const stripLabel = h('span', { class: 'step-label' });
  const progressFill = h('div', { class: 'step-progress-fill' });
  const progress = h('div', { class: 'step-progress', role: 'progressbar', 'aria-valuemin': 0, 'aria-valuemax': 100 }, progressFill);
  // The session's state in words, coloured by tone; announced when it changes. Mode and account
  // stay beside it, in the strip's own colours.
  const statusEl = h('span', { class: 'strip-status', role: 'status' });
  const extrasEl = h('span', { class: 'strip-extras' });
  // The pull request, its checks and the next delivery step, beside the state (UX2-18).
  const deliveryEl = h('span', { class: 'strip-delivery' });
  const toggleButton = h('button', { type: 'button', class: 'stepstrip-toggle', 'aria-expanded': String(stripOpen) }, icon('right', { size: 14 }), stripLabel);
  const kebab = iconButton('more', 'Task actions', () => openTaskMenu(ctx, kebab, current), { 'aria-haspopup': 'menu' });
  const stepList = h('div', { class: 'steplist', hidden: !stripOpen });
  const strip = h('div', { class: ['stepstrip', stripOpen && 'open'] },
    h('div', { class: 'stepstrip-main' }, toggleButton, progress, statusEl, extrasEl, deliveryEl, kebab),
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
    exitMessage: (exit) => endedMessage(exit.status || current.status),
    emptyMessage: () => emptyTerminalMessage(current),
    exitActions: (exit) => {
      const message = endedMessage(exit.status || current.status);
      const actions = [];
      if (message.accounts && ctx.openStatus) actions.push({ label: 'Check AI accounts', run: () => ctx.openStatus() });
      if (taskActions(current, sandbox.access).continue || allows(sandbox.access, 'write')) {
        actions.push({ label: message.retry ? 'Try again' : 'Continue', run: () => continueTask(ctx, current) });
      }
      return actions;
    },
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

  // ---- attention bar (UX-03): what the agent needs from the user, with the way to do it. It
  // floats over the top of the terminal: coming and going with every turn, it must not resize
  // the terminal (each resize makes Claude Code redraw its whole screen, F 2026-10-09).
  const attention = h('div', { class: 'attention-bar', role: 'status', hidden: true });
  function renderAttention() {
    const info = taskInfo(current, ctx.store.state.actionRequests, ctx.store.pullsOf(current.sandbox_id).get(current.id));
    const reply = () => (isPhone() ? quickInput.focus() : terminal.focus());
    let content = null;
    if (info.request) {
      content = [icon('globe', { size: 16 }), h('span', { class: 'attention-text' }, h('strong', null, 'Action needed in the browser: '), info.request.reason || 'the agent asked you to do something there.'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => ctx.openBrowserTab && ctx.openBrowserTab(sandbox, info.request.tab_id) }, 'Open browser tab')];
    } else if (current.status === 'running' && current.activity === 'needs_attention') {
      content = [icon('st-attention', { size: 16 }), h('span', { class: 'attention-text' }, h('strong', null, 'The agent needs your attention. '), 'Its question is in the terminal below.'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: reply }, 'Reply')];
    } else if (current.status === 'running' && current.activity === 'waiting') {
      content = [icon('st-input', { size: 16 }), h('span', { class: 'attention-text' }, h('strong', null, 'The agent is waiting for your reply. '), 'Type it in the terminal below.'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: reply }, 'Reply')];
    }
    attention.hidden = !content;
    attention.classList.toggle('waiting', !info.request && current.activity === 'waiting');
    setChildren(attention, content);
  }
  // A browser request changes the bar and the state in the strip ("Needs you in the browser").
  destroyers.push(ctx.store.on('browser-action', (event) => { if (event.sandbox_id === sandbox.id) renderStrip(); }));

  // The view's heading for screen readers and the page title (the strip shows it visually).
  const heading = h('h1', { class: 'sr-only' }, `#${task.number} ${task.title || ''}`);
  const el = h('div', { class: 'taskview' }, heading, strip, h('div', { class: 'taskview-term' }, terminal.el, attention), quickbar);

  function renderStrip() {
    renderAttention();
    const steps = current.steps || { done: 0, total: 0, current: null };
    // The session's state as the sidebar says it (pull request, browser request, usage limit).
    const info = taskInfo(current, ctx.store.state.actionRequests, ctx.store.pullsOf(current.sandbox_id).get(current.id));
    const total = steps.total || 0;
    const done = steps.done || 0;
    let n = total ? Math.min(total, done + (steps.current ? 1 : 0)) : 0;
    if (detail && Array.isArray(detail.steps_list)) {
      const ordered = [...detail.steps_list].sort((a, b) => a.ordinal - b.ordinal);
      const index = ordered.findIndex((s) => s.status === 'in_progress');
      if (index >= 0) n = index + 1;
    }
    setChildren(stripLabel,
      h('span', { class: 'step-task', title: current.title }, h('span', { class: 'step-task-num' }, `#${current.number}`), h('span', { class: 'sr-only' }, ' '), current.title),
      total
        ? [h('span', { class: 'step-count' }, `Step ${n || done}/${total}`), h('span', { class: 'step-current' }, steps.current ? `· ${steps.current}` : done === total ? '· All steps done' : '')]
        : h('span', { class: 'step-current muted' }, 'No plan steps yet'));
    const pct = total ? Math.round((done / total) * 100) : 0;
    progressFill.style.width = `${pct}%`;
    progress.setAttribute('aria-valuenow', String(pct));
    progress.setAttribute('aria-label', `${done} of ${total} steps done`);
    progress.hidden = !total;
    statusEl.dataset.tone = info.tone;
    const reset = current.status === 'waiting_capacity' && current.resumes_at ? resetLabel(current.resumes_at) : '';
    statusEl.title = [info.label, reset, info.detail].filter(Boolean).join(' · ');
    setChildren(statusEl,
      statusMark(info),
      h('span', { class: 'strip-state' }, info.label, reset ? h('span', { class: 'strip-reset' }, ` · ${reset}`) : null));
    setChildren(extrasEl,
      current.mode === 'goal' ? h('span', { class: 'chip strip-extra', title: 'Runs as a goal (/goal): the agent keeps working until it is met' }, 'Goal') : null,
      current.account_label ? h('span', { class: 'muted xs strip-extra' }, current.account_label) : null);
  }

  function renderStepList() {
    if (!stripOpen) return;
    if (!detail) { setChildren(stepList, placeholder('Loading steps…', 'loading')); return; }
    const steps = Array.isArray(detail.steps_list) ? [...detail.steps_list].sort((a, b) => a.ordinal - b.ordinal) : [];
    if (steps.length === 0) {
      setChildren(stepList, h('div', { class: 'muted small steplist-note' }, 'The agent has not written a plan yet. Steps appear here as soon as the task spec has a Plan checklist.'));
      return;
    }
    setChildren(stepList,
      steps.map((step) => h('div', { class: ['step-item', `st-${step.status}`] },
        h('span', { class: 'step-mark', role: 'img', 'aria-label': STEP_LABEL[step.status] || step.status }, STEP_MARK[step.status] || '·'),
        h('span', { class: 'step-title' }, step.title),
        h('span', { class: 'step-audit', title: dateTime(step.updated_at) }, [step.updated_by, step.updated_at ? relLong(step.updated_at) : null].filter(Boolean).join(' · ')),
        step.note ? h('span', { class: 'step-note' }, step.note) : null)),
      detail.spec_path ? h('div', { class: 'muted xs steplist-spec' }, 'Spec: ', h('code', null, detail.spec_path)) : null);
  }

  let alive = true;
  const reads = new AbortController(); // leaving the task cancels its reads in flight (PERF-13)
  async function loadDetail() {
    try {
      const fresh = await ctx.api.get(`/api/tasks/${enc(current.id)}`, { signal: reads.signal });
      if (!alive) return; // the view was left while the read was in flight
      detail = fresh;
      if (detail && detail.id === current.id) {
        current = { ...current, ...pickSummary(detail) };
        renderStrip();
      }
    } catch (error) {
      if (alive && error.status !== 404) errorToast(error, 'Could not load task details');
    }
    if (alive) renderStepList();
  }

  let delivery = null;
  const renderDelivery = () => keepFocus(deliveryEl, () => setChildren(deliveryEl, deliveryStrip(ctx, current, delivery, { canAct: allows(sandbox.access, 'write'), reload: loadDelivery })));
  // Optional: the strip works without it (no GitHub, no branch yet), so a failed read stays quiet.
  const loadDelivery = singleFlight(async () => {
    try {
      const fresh = await ctx.api.get(`/api/tasks/${enc(current.id)}/delivery`, { signal: reads.signal });
      if (!alive) return;
      delivery = fresh;
    } catch {
      delivery = null;
    }
    if (alive) renderDelivery();
  });

  destroyers.push(ctx.store.on('steps', (taskId) => { if (taskId === current.id) loadDetail(); }));
  destroyers.push(ctx.store.on('resync', () => { loadDetail(); loadDelivery(); }));

  renderStrip();
  loadDetail();
  loadDelivery();

  return {
    el,
    key: `task:${task.id}`,
    start() {
      terminal.start();
      // Focus lands in the terminal as soon as it shows (the keyboard is the agent's).
      if (!isPhone()) terminal.focus();
    },
    update(next) {
      const wasLive = isLive(current.status);
      const previousStatus = current.status;
      current = { ...current, ...next };
      renderStrip();
      if (current.status !== previousStatus) terminal.refreshEmpty();
      if (current.status !== previousStatus) loadDelivery(); // a finished task may now be ready for a pull request
      else renderDelivery();
      // A continued task gets a new attempt: reconnect if the socket had ended.
      if (!wasLive && isLive(current.status) && terminal.socket.status !== 'open') terminal.reconnect();
      // The user left the agent session (e.g. /exit): back to the task list (HRN-002).
      if (wasLive && current.status === 'closed' && ctx.onTaskClosed) ctx.onTaskClosed(current);
    },
    get task() { return current; },
    get detail() { return detail; },
    destroy() {
      alive = false;
      reads.abort();
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

const MODES = [
  { id: 'goal', name: 'Goal', icon: 'target', description: 'The agent keeps working until the goal is met.' },
  { id: 'prompt', name: 'Prompt', icon: 'send', description: 'Sent as a normal first message.' },
];

/**
 * New-task composer (J3, UI-021): a rounded prompt box whose bottom edge holds the task's
 * choices — mode, workflow, model (picked from a list grouped by provider) and effort — and
 * Start. Model and effort are remembered per browser.
 */
export function mountComposer(ctx, sandbox) {
  const destroyers = [];
  const write = allows(sandbox.access, 'write');
  const prompt = h('textarea', { class: 'composer-prompt', rows: 4, placeholder: 'Describe what you want to build or change — e.g. “make me an online store”…', 'aria-label': 'Task prompt', maxlength: 100000 });
  // Goal mode is the default (TSK-010): the prompt becomes the session's /goal.
  let mode = 'goal';
  let workflowId = '';
  let workflows = [];
  let modelId = findModel(readJson(MODEL_PREF_KEY, null)) ? readJson(MODEL_PREF_KEY, null) : null;
  let effortId = findEffort(readJson(EFFORT_PREF_KEY, null)) ? readJson(EFFORT_PREF_KEY, null) : null;

  const chip = (label, onOpen, extra = {}) => {
    // The visible value is the accessible name, with a hidden "Mode: " prefix (WCAG 2.5.3).
    const button = h('button', { type: 'button', class: ['chip-btn', extra.class], 'aria-haspopup': 'menu', 'aria-expanded': 'false', title: label });
    button.addEventListener('click', () => onOpen(button));
    return button;
  };
  const modeChip = chip('Mode', openModeMenu);
  const workflowChip = chip('Workflow', openWorkflowMenu);
  const modelChip = chip('Model', openModelMenu, { class: 'plain' });
  const effortChip = chip('Effort', openEffortMenu, { class: 'plain muted-chip' });
  const modeHint = h('span');
  const banner = h('div', { class: 'stack-sm' });
  const startButton = h('button', { type: 'button', class: 'btn btn-primary composer-send', 'aria-label': 'Start task', title: 'Start task' }, icon('up', { size: 18 }));
  const hint = h('span', { class: 'composer-kbd' }, ' · ', h('span', { class: 'kbd' }, 'Enter'), ' to start, ', h('span', { class: 'kbd' }, 'Shift'), ' + ', h('span', { class: 'kbd' }, 'Enter'), ' for a new line');

  const quote = randomQuote();
  const niche = h('div', { class: 'brand-niche', 'aria-hidden': 'true' }, icon('logo', { size: 64 }));
  let stopSmoke = null;
  // The top bar: where you are (sandbox › New task, the page's heading) on the left; the GitHub
  // repository (opening it) and the branch new tasks start from on the right.
  const baseBranch = (sandbox.profile && sandbox.profile.integration_branch) || sandbox.default_branch || 'main';
  const tasksPath = `/s/${encodeURIComponent(sandbox.slug)}/tasks`;
  const repoBar = h('div', { class: 'composer-top' },
    h('nav', { class: 'composer-crumbs', 'aria-label': 'Breadcrumb' }, h('ol', null,
      h('li', null, h('a', { href: href(tasksPath), dataset: { route: tasksPath } }, sandbox.name)),
      h('li', { 'aria-current': 'page' }, icon('right', { size: 14 }), h('h1', { class: 'composer-title' }, 'New task')))),
    sandbox.repo ? h('div', { class: 'composer-repo-bar' },
      h('a', { class: 'composer-repo-link', href: sandbox.repo.url, target: '_blank', rel: 'noopener noreferrer', title: `Open ${sandbox.repo.owner}/${sandbox.repo.name} on GitHub` },
        icon('github', { size: 18 }), h('span', null, `${sandbox.repo.owner} / ${sandbox.repo.name}`)),
      h('span', { class: 'composer-branch', title: `New tasks branch from ${baseBranch}` }, icon('branch', { size: 16 }), h('span', null, baseBranch))) : null);
  const el = h('div', { class: 'composer' }, repoBar,
    h('div', { class: 'composer-card' },
      // A brand's niche for its mark (hidden unless a brand shows it; styles.css "Brand details").
      niche,
      h('figure', { class: 'composer-quote' }, h('blockquote', null, `“${quote.text}”`), h('figcaption', null, `— ${quote.author}`)),
      banner,
      h('div', { class: 'composer-box' },
        prompt,
        h('div', { class: 'composer-tools' }, modeChip, workflowChip, h('div', { class: 'composer-right' }, modelChip, effortChip, startButton))),
      h('div', { class: 'composer-hint muted xs' }, modeHint, hint)));

  function renderChips() {
    const m = MODES.find((x) => x.id === mode);
    setChildren(modeChip, icon(m.icon, { size: 14 }), h('span', { class: 'sr-only' }, 'Mode: '), h('span', { class: 'chip-label' }, m.name), icon('down', { size: 12 }));
    modeHint.textContent = m.description;
    const wf = workflows.find((w) => w.id === workflowId);
    setChildren(workflowChip, icon('workflow', { size: 14 }), h('span', { class: 'sr-only' }, 'Workflow: '), h('span', { class: 'chip-label' }, wf ? wf.name : 'No workflow'), icon('down', { size: 12 }));
    const found = findModel(modelId);
    setChildren(modelChip, h('span', { class: 'sr-only' }, 'Model: '), h('span', { class: 'chip-label' }, found ? found.model.name : 'Default model'), icon('down', { size: 12 }));
    modelChip.title = found ? `${found.provider.name} · ${found.model.name}` : 'Model: Claude Code’s default';
    // A model without effort levels (Haiku) hides the effort choice and sends none.
    const effortless = Boolean(found && found.model.effort === false);
    effortChip.hidden = effortless;
    const effort = findEffort(effortId);
    setChildren(effortChip, h('span', { class: 'sr-only' }, 'Effort: '), h('span', { class: 'chip-label' }, effort ? effort.name : 'Default effort'), icon('down', { size: 12 }));
  }

  function openModeMenu(anchor) {
    openMenu(anchor, MODES.map((m) => ({ label: m.name, icon: m.icon, description: m.description, checked: m.id === mode, onSelect: () => { mode = m.id; renderChips(); prompt.focus(); } })));
  }

  function openWorkflowMenu(anchor) {
    const slug = encodeURIComponent(sandbox.slug);
    openMenu(anchor, [
      { label: 'No workflow', icon: 'x', description: 'The agent plans the work itself', checked: !workflowId, onSelect: () => { workflowId = ''; renderChips(); } },
      ...workflows.map((wf) => ({ label: `${wf.name}${wf.scope === 'instance' ? ' (instance)' : ''}`, icon: 'workflow', description: wf.description || (Array.isArray(wf.steps) ? wf.steps.join(' → ') : null), checked: wf.id === workflowId, onSelect: () => { workflowId = wf.id; renderChips(); } })),
      { separator: true },
      { label: 'Manage workflows…', icon: 'gear', onSelect: () => ctx.navigate(`/s/${slug}/workflows`) },
    ]);
  }

  function openModelMenu(anchor) {
    openMenu(anchor, modelMenuItems(modelId, (id) => { modelId = id; writeJson(MODEL_PREF_KEY, id); renderChips(); prompt.focus(); }));
  }

  function openEffortMenu(anchor) {
    openMenu(anchor, effortMenuItems(effortId, (id) => { effortId = id; writeJson(EFFORT_PREF_KEY, id); renderChips(); prompt.focus(); }));
  }

  // The box grows with the prompt up to half the screen, then scrolls.
  prompt.addEventListener('input', () => {
    prompt.style.height = 'auto';
    prompt.style.height = `${prompt.scrollHeight}px`;
  });

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
          h('strong', null, pending ? 'This sandbox needs preparation. ' : 'Preparing this sandbox. '),
          pending
            ? `The repository does not follow the ${BRAND.name} documentation format yet. Start the preparation task to adapt it; new tasks can start once it lands.`
            : `A preparation task adapts the imported repository to the ${BRAND.name} format; new tasks can start once it lands.`,
          prep ? [' ', h('a', { href: href(`/s/${encodeURIComponent(sb.slug)}/t/${prep.number}`), dataset: { route: `/s/${encodeURIComponent(sb.slug)}/t/${prep.number}` } }, `Open #${prep.number}`)] : null),
        pending && allows(sb.access, 'write') ? h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: approveOnboarding }, 'Start preparation') : null,
        allows(sb.access, 'admin') ? h('button', { type: 'button', class: 'btn btn-sm', onClick: skipOnboarding }, 'Skip for now') : null));
    }
    if (!write) items.push(h('div', { class: 'banner' }, icon('lock'), h('span', { class: 'banner-text' }, 'Read-only (GitHub: read). You can watch tasks but not start them.')));
    setChildren(banner, items);
    const disabled = !write || blocked;
    startButton.disabled = disabled;
    prompt.disabled = disabled;
    startButton.title = blocked ? 'Blocked until the sandbox preparation finishes' : !write ? 'Requires write access' : '';
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
      title: 'Skip sandbox preparation?',
      message: `Tasks may start before the repository follows the ${BRAND.name} documentation format. The preparation task keeps its history.`,
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

  // One task per press: Enter twice must not start two sessions (singleFlight).
  const start = singleFlight(async () => {
    const text = prompt.value.trim();
    if (!text) { prompt.focus(); toast('Write a prompt first.', { kind: 'warn' }); return; }
    const body = { prompt: text, mode };
    if (workflowId) body.workflow_id = workflowId;
    const found = findModel(modelId);
    if (found) {
      body.model = found.model.id;
      if (found.provider.harness !== 'claude') body.harness = found.provider.harness;
    }
    if (effortId && !(found && found.model.effort === false)) body.effort = effortId;
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
  });

  startButton.addEventListener('click', start);
  // Enter starts (the Claude and Codex convention), Shift+Enter adds a line, ⌘/Ctrl+Enter also
  // starts. On touch screens Enter adds a line and the Start button sends. IME composition wins.
  const finePointer = window.matchMedia('(pointer: fine)').matches;
  prompt.addEventListener('keydown', (event) => {
    if (event.key !== 'Enter' || event.isComposing || event.keyCode === 229) return;
    if (event.metaKey || event.ctrlKey || (finePointer && !event.shiftKey && !event.altKey)) {
      event.preventDefault();
      start();
    }
  });

  ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/workflows`).then((list) => {
    workflows = Array.isArray(list) ? list : [];
    renderChips();
  }).catch(() => { /* workflows are optional */ });
  renderChips();

  destroyers.push(ctx.store.on('sandboxes', renderBanner));
  destroyers.push(ctx.store.on('tasks', (sid) => { if (sid === sandbox.id) renderBanner(); }));
  renderBanner();

  return {
    el,
    key: `composer:${sandbox.id}`,
    start() {
      if (!prompt.disabled && !isPhone()) prompt.focus();
      if (!stopSmoke) stopSmoke = brandSmoke(niche); // the brand's smoke, when its niche shows (brandfx.js)
    },
    destroy() { destroyers.forEach((off) => off()); if (stopSmoke) stopSmoke(); },
  };
}
