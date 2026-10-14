// Center views: the selected task (step strip + native terminal + phone quick input)
// and the new-task composer shown when no task is selected (SPEC-001 §6.1, STP-003).

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, toast, errorToast, skeleton, openMenu, statusMark, singleFlight, keepFocus } from './dom.js';
import { openRuleEditor, ruleText } from './ruleeditor.js';
import { openStartMenu, renderStartChip, startKind, SUPERTASK_HELP, WHEN_I_DECIDE_HELP } from './startchoice.js';
import { createTerminalView } from './terminal.js';
import { taskInfo, isLive, allows, taskActions, otherOwner, endedMessage, pausedLine, resetAround, shellsLine, heldBy, supertaskLine, preparedLine } from './status.js';
import { heldPromptCard, splitCard, looksSeveral, intakeStatus, SPLIT_MIN_WORDS } from './intake.js';
import { attachMarkdownInput } from './mdinput.js';
import { createMarkdownEditor } from './mdeditor.js';
import { SUBMIT_KEYS, SUBMIT_ARIA } from './shortcuts.js';
import { openTaskMenu, continueTask, deliveryStrip, startNow, stopTask, requestControl, dropControlRequest, handOverTask, openScheduleDialog, rescheduleTask } from './actions.js';
import { attachTerminalDrop, attachPromptDrop, draftAttachments, taskAttachments, withAttachments, openAttachment } from './filedrop.js';
import { createPromptInput, ATOM, FILE_MARK, FILE_MARK_END, placeFiles, withoutMarks } from './promptinput.js';
import { relLong, dateTime, whenLabel } from './format.js';
import { readJson, writeJson } from './store.js';
import { href } from './router.js';
import { findModel, findEffort, modelMenuItems, effortMenuItems, MODEL_PREF_KEY, EFFORT_PREF_KEY } from './models.js';
import { randomQuote } from './quotes.js';
import { brandSmoke } from './brandfx.js';
import { BRAND } from './brand.js';
import { isPhone } from './theme.js';
import { draftFor, saveDraft, clearDraft, attachToDraft, onDraftChange } from './drafts.js';

const STEP_MARK = { done: '✓', in_progress: '▸', pending: '·', skipped: '–', blocked: '✕' };
const STEP_LABEL = { done: 'Done', in_progress: 'In progress', pending: 'To do', blocked: 'Blocked', skipped: 'Skipped' };
const STRIP_KEY = 'yulius.stepsOpen';
// Waiting states in which a task that never started takes queued messages (SPEC-052).
const WAITING = new Set(['queued', 'chained', 'prepared', 'waiting_capacity']);
// States only a task with a run is in.
const RAN = new Set(['running', 'switching', 'stopping']);

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
 * empty states instead of an empty black screen); null once it should show the session. `busy`
 * states shimmer like the composer's working state (F 2026-10-10).
 */
function emptyTerminalMessage(task) {
  switch (task.status) {
    case 'queued': return { title: 'Queued', text: 'The agent starts as soon as a session slot is free.', busy: true };
    case 'starting': return { title: 'Starting Claude Code…', text: 'The session appears here in a moment.', busy: true };
    case 'switching': return { title: 'Continuing…', text: 'The session picks up where it left off in a moment.', busy: true };
    case 'waiting_capacity': return { title: 'Paused', text: `${pausedLine(task)}. It continues by itself.` };
    case 'running': return { title: 'Claude Code is starting…', text: 'The session appears here in a moment.', busy: true };
    case 'chained': {
      const after = task.after;
      if (after && after.blocked) return { title: 'Waiting for you', text: after.blocked };
      if (after && after.rule) return { title: 'Queued by a rule', text: `It starts by itself when ${after.rule}.` };
      return { title: after ? `Queued after #${after.number}` : 'Queued after another task', text: after ? `It starts by itself when #${after.number} is done.` : 'It starts by itself when that task is done.' };
    }
    case 'prepared': return { title: task.start_at ? 'Scheduled' : 'Prepared', text: preparedLine(task) };
    default: return null;
  }
}

/** Mount the task view for one task. Returns {el, key, update(task), destroy()}. */
export function mountTaskView(ctx, sandbox, task) {
  let current = task;
  let detail = null;
  let stripOpen = Boolean(readJson(STRIP_KEY, false));
  const destroyers = [];
  // Only the task's owner changes it (SPEC-070): `owner()` names another owner, else null.
  const me = () => ctx.store.state.me?.user?.login;
  const owner = () => otherOwner(current, me());

  // ---- step strip
  const stripLabel = h('span', { class: 'step-label' });
  const progressFill = h('div', { class: 'step-progress-fill' });
  const progress = h('div', { class: 'step-progress', role: 'progressbar', 'aria-valuemin': 0, 'aria-valuemax': 100 }, progressFill);
  // The session's state in words, coloured by tone; announced when it changes. The mode stays
  // beside it, in the strip's own colours; the subscription is not a property of the task (F
  // 2026-10-10) and shows only in Task details.
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
    // Without write access to the sandbox that is the reason; with it, the task's owner (SPEC-070).
    readOnlyLabel: () => {
      const box = ctx.store.sandboxById(sandbox.id) || sandbox;
      return owner() && allows(box.access, 'write') ? `Read-only: @${owner()} owns this task` : readOnlyText(box);
    },
    exitMessage: (exit) => endedMessage(exit.status || current.status),
    emptyMessage: () => emptyTerminalMessage(current),
    exitActions: (exit) => {
      const message = endedMessage(exit.status || current.status);
      const actions = [];
      // Administrators go to the Subscriptions page, where a subscription signs in again; everyone
      // else sees the subscriptions' state in the Status panel (#147).
      if (message.accounts) actions.push({ label: 'Check subscriptions', run: () => (ctx.isInstanceAdmin() ? ctx.navigate('/admin/accounts') : ctx.openStatus && ctx.openStatus()) });
      // The exit can arrive before the task's new status: write access is enough, but an archived
      // task is restored first, as in its menu (taskActions).
      if (allows(sandbox.access, 'write') && !owner() && !current.archived) {
        actions.push({ label: message.retry ? 'Try again' : 'Continue', run: () => continueTask(ctx, current) });
      }
      return actions;
    },
    // Paths of files in the task's worktree open in the Editor (F 2026-10-10). The files API needs
    // write access, as the editor does.
    fileLinks: allows(sandbox.access, 'write') ? {
      list: (dir) => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/files?root=${encodeURIComponent(`task:${task.id}`)}&path=${encodeURIComponent(dir)}`).then((listing) => listing.entries || []),
      roots: () => (detail && detail.worktree ? [detail.worktree] : []),
      open: (file) => ctx.openFile && ctx.openFile(sandbox, { root: `task:${task.id}`, ...file }),
    } : null,
  });
  // Files dropped (or an image pasted) here reach Claude by path, as in a local terminal (UI-048).
  attachTerminalDrop(ctx, terminal, { label: 'Drop to send to Claude', upload: taskAttachments(ctx, task.id) });

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
  // Where the user answers the agent: its terminal (the quick input on phones).
  const reply = () => (isPhone() ? quickInput.focus() : terminal.focus());
  function renderAttention() {
    const pr = ctx.store.pullsOf(current.sandbox_id).get(current.id);
    const info = taskInfo(current, ctx.store.state.actionRequests, pr);
    let content = null;
    const access = (ctx.store.sandboxById(sandbox.id) || sandbox).access;
    const can = taskActions(current, access, pr, me());
    const request = current.control_request;
    // Control of the task (SPEC-070), on its own row above what else the task needs.
    let control = null;
    if (owner() && allows(access, 'write')) {
      // Someone else's task: it acts on GitHub as them; its owner may hand it over. Asking again
      // takes control at once once the owner can no longer act.
      const mine = request && request.by.login === me();
      control = [icon('lock', { size: 16 }), h('span', { class: 'attention-text' },
        mine ? [h('strong', null, `You asked @${owner()} for control. `), 'Until they hand it over, the task acts on GitHub as them.']
          : [h('strong', null, `@${owner()} owns this task: `), 'it acts on GitHub as them.', request ? ` @${request.by.login} asked for control.` : '']),
      mine ? h('button', { type: 'button', class: 'btn btn-sm', title: `Takes control at once when @${owner()} can no longer act`, onClick: () => requestControl(ctx, current) }, 'Ask again') : null,
      mine ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => dropControlRequest(ctx, current) }, 'Withdraw')
        : request ? null : h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: () => requestControl(ctx, current) }, 'Request control')];
    } else if (!owner() && request && allows(access, 'write')) {
      control = [icon('st-attention', { size: 16 }), h('span', { class: 'attention-text' }, h('strong', null, `@${request.by.login} asks to take control of this task: `), 'it will act on GitHub as them.'),
        h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: () => handOverTask(ctx, current) }, 'Hand over'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => dropControlRequest(ctx, current) }, 'Decline')];
    }
    if (current.status === 'chained') {
      // Waiting in a chain (TSK-012): the way out is always here.
      const after = current.after;
      const label = after && after.blocked
        ? [h('strong', null, 'Waiting for you. '), after.blocked]
        : after && after.rule ? [h('strong', null, 'Queued by a rule. '), `It starts by itself when ${after.rule}.`]
        : [h('strong', null, after ? `Queued after #${after.number}. ` : 'Queued. '), after ? `It starts by itself when #${after.number} is done.` : ''];
      content = [icon(after && after.blocked ? 'st-attention' : 'clock', { size: 16 }), h('span', { class: 'attention-text' }, label),
        can.startNow ? h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: () => startNow(ctx, current) }, 'Start now') : null,
        can.stop ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => stopTask(ctx, current) }, 'Cancel') : null];
    } else if (current.status === 'prepared') {
      // Written now, started later (SPEC-105): Start now, a time, or Cancel.
      content = [icon(current.start_at ? 'clock' : 'st-prepared', { size: 16 }), h('span', { class: 'attention-text' },
        h('strong', null, current.start_at ? 'Scheduled. ' : 'Prepared. '), preparedLine(current)),
      can.startNow ? h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: () => startNow(ctx, current) }, 'Start now') : null,
      can.schedule ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => rescheduleTask(ctx, current) }, current.start_at ? 'Change time…' : 'Schedule…') : null,
      can.stop ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => stopTask(ctx, current) }, 'Cancel') : null];
    } else if (current.status === 'queued' && heldBy(current).length) {
      // Held by a supertask (SPEC-071): it starts by itself; Cancel is the way out.
      const held = heldBy(current);
      content = [icon('clock', { size: 16 }), h('span', { class: 'attention-text' },
        h('strong', null, current.supertask ? 'Supertask. ' : `Queued after supertask #${held[0]}. `), supertaskLine(current)),
      can.stop ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => stopTask(ctx, current) }, 'Cancel') : null];
    } else if (info.held) {
      content = null; // the held-prompt card says it
    } else if (info.request) {
      content = [icon('globe', { size: 16 }), h('span', { class: 'attention-text' }, h('strong', null, 'Action needed in the browser: '), info.request.reason || 'the agent asked you to do something there.'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => ctx.openBrowserTab && ctx.openBrowserTab(sandbox, info.request.tab_id) }, 'Open browser tab')];
    } else if (current.status === 'running' && current.activity === 'needs_attention') {
      content = [icon('st-attention', { size: 16 }), h('span', { class: 'attention-text' }, h('strong', null, 'The agent needs your attention. '), 'Its question is in the terminal below.'),
        owner() ? null : h('button', { type: 'button', class: 'btn btn-sm', onClick: reply }, 'Reply')];
    } else if (current.status === 'running' && current.activity === 'waiting' && info.group === 'needs') {
      content = [icon('st-input', { size: 16 }), h('span', { class: 'attention-text' }, h('strong', null, 'The agent is waiting for your reply. '), 'Type it in the terminal below.'),
        owner() ? null : h('button', { type: 'button', class: 'btn btn-sm', onClick: reply }, 'Reply')];
    }
    attention.hidden = !content && !control;
    attention.classList.toggle('waiting', !control && !info.request && current.activity === 'waiting' && info.group === 'needs');
    attention.classList.toggle('stacked', Boolean(control && content));
    setChildren(attention, control && content
      ? [h('div', { class: 'attention-row' }, control), h('div', { class: 'attention-row' }, content)]
      : control || content);
  }
  // A browser request changes the bar and the state in the strip ("Needs you in the browser").
  destroyers.push(ctx.store.on('browser-action', (event) => { if (event.sandbox_id === sandbox.id) renderStrip(); }));
  // So does its pull request ("Done · PR #…"), read on its own and often after this view (#294).
  destroyers.push(ctx.store.on('pulls', (sandboxId) => { if (sandboxId === sandbox.id) renderStrip(); }));

  // The view's heading for screen readers and the page title (the strip shows it visually).
  const heading = h('h1', { class: 'sr-only' }, `#${task.number} ${task.title || ''}`);
  // A prompt typed into the session that Yulius held before it reached the agent (TSK-013):
  // the choices float over the terminal, which keeps its size.
  const held = h('div', { class: 'held-slot', hidden: true });
  let heldKey = '';
  function renderHeld() {
    const prompt = current.held_prompt;
    const key = prompt ? `${prompt.id}:${current.status}:${detail ? (detail.steps_list || []).map((s) => s.status).join() : ''}` : '';
    if (key === heldKey) return;
    heldKey = key;
    held.hidden = !prompt;
    setChildren(held, prompt ? heldPromptCard(ctx, sandbox, current, prompt, detail) : null);
  }

  // ---- queued messages (SPEC-052): a task that waits and never started takes messages into its
  // request, so a person can correct it before the agent sees it. The card sits at the bottom of
  // the empty terminal, where a message is typed; the request shows as the agent will get it.
  const queueSlot = h('div', { class: 'queue-slot', hidden: true });
  const requestEl = h('div', { class: 'queue-request', role: 'region', 'aria-label': 'Request', tabindex: 0 });
  const queueBox = h('textarea', { class: 'input queue-input', rows: 2, placeholder: 'Queue a message for the agent…', 'aria-label': 'Message for the agent', 'aria-keyshortcuts': SUBMIT_ARIA, maxlength: 65536 });
  const queueButton = h('button', { type: 'button', class: 'btn btn-primary', title: `Queue (${SUBMIT_KEYS})`, onClick: () => queueMessage() }, 'Queue');
  const queueError = h('div', { class: 'field-error', role: 'alert' });
  const queueNote = h('span', { class: 'muted' });
  // A card kept after the start (below) goes once the person has their text (SPEC-056 WUI-106).
  const queueDismiss = h('button', { type: 'button', class: 'btn', hidden: true, onClick: () => {
    queueBox.value = '';
    queueError.textContent = '';
    renderQueue();
    if (!isPhone()) terminal.focus();
  } }, 'Dismiss');
  setChildren(queueSlot, h('section', { class: 'queue-card', role: 'region', 'aria-label': 'Queued messages' },
    h('div', { class: 'queue-head' }, icon('send', { size: 16 }), h('strong', null, 'Queued messages'), queueNote),
    requestEl,
    h('div', { class: 'queue-compose' }, queueBox, queueButton, queueDismiss),
    queueError));
  attachMarkdownInput(queueBox, { onSubmit: () => queueMessage() });
  let queueing = false;
  async function queueMessage() {
    const text = queueBox.value;
    if (queueing || !text.trim() || queueBox.readOnly) return;
    queueing = true;
    queueButton.disabled = true;
    queueError.textContent = '';
    try {
      const out = await ctx.api.post(`/api/tasks/${enc(current.id)}/queued-messages`, { text });
      if (!alive) return;
      queueBox.value = '';
      if (detail) detail = { ...detail, request: out.request };
      renderQueue();
    } catch (error) {
      // The text stays in the box; the reason shows in place (UI-050).
      if (alive) queueError.textContent = error.message || 'Couldn’t queue the message.';
    } finally {
      queueing = false;
      if (alive) renderQueue();
    }
  }
  /** Whether the task waits, has never started and takes queued messages from this person. */
  function queueOpen() {
    const access = (ctx.store.sandboxById(sandbox.id) || sandbox).access;
    return Boolean(detail && detail.id === current.id && detail.started === false
      && WAITING.has(current.status) && !current.archived && allows(access, 'write') && !owner());
  }
  function renderQueue() {
    const waiting = queueOpen();
    // The task started while the box held text or a refusal: the card stays with them, so the
    // person can copy the text into the session (SPEC-052 QMS-006, SPEC-056 WUI-106). A task that
    // ended or was archived instead takes the card away.
    const started = current.status === 'starting' || RAN.has(current.status);
    const kept = !waiting && started && !current.archived && !queueSlot.hidden && Boolean(queueBox.value.trim() || queueError.textContent);
    // Back to waiting before it ran: a refusal from the start no longer holds.
    if (waiting && !queueDismiss.hidden) queueError.textContent = '';
    queueSlot.hidden = !waiting && !kept;
    if (queueSlot.hidden) return;
    queueNote.textContent = kept ? 'The task has started: copy your message into its session.' : 'The agent gets them with the request when the task starts.';
    queueBox.readOnly = kept;
    queueButton.disabled = queueing || kept;
    queueDismiss.hidden = !kept;
    if (kept) return;
    const text = detail.request || '';
    if (requestEl.textContent !== text) {
      requestEl.textContent = text;
      requestEl.scrollTop = requestEl.scrollHeight; // the latest message in view
    }
  }

  const el = h('div', { class: 'taskview' }, heading, strip, h('div', { class: 'taskview-term' }, terminal.el, attention, held, queueSlot), quickbar);

  function renderStrip() {
    renderAttention();
    quickbar.hidden = Boolean(owner());
    renderHeld();
    renderQueue();
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
    // The server weighs steps by their declared effort (STP-006); older servers send no percent.
    const pct = !total ? 0 : Number.isFinite(steps.percent) ? steps.percent : Math.floor((done / total) * 100);
    progressFill.style.width = `${pct}%`;
    progress.setAttribute('aria-valuenow', String(pct));
    progress.setAttribute('aria-label', `${done} of ${total} steps done, ${pct}% of the plan`);
    progress.hidden = !total;
    statusEl.dataset.tone = info.tone;
    const reset = current.status === 'waiting_capacity' ? resetAround(current.resumes_at) : '';
    statusEl.title = [info.label, reset, info.detail].filter(Boolean).join(' · ');
    setChildren(statusEl,
      statusMark(info),
      h('span', { class: 'strip-state' }, info.label, reset ? h('span', { class: 'strip-reset' }, ` · ${reset}`) : null,
        info.shells ? h('span', { class: 'strip-reset' }, ` · ${shellsLine(info.shells, info.shellsSince)}`) : null));
    // The chain shows on both tasks (TSK-012): what this task runs after, what runs after it.
    const slug = encodeURIComponent((ctx.store.sandboxById(sandbox.id) || sandbox).slug);
    const chainLink = (link, text, title) => (link.task_id
      ? h('a', { class: 'chip strip-extra chain-chip', href: href(`/s/${slug}/t/${link.number}`), dataset: { route: `/s/${slug}/t/${link.number}` }, title }, icon('branch', { size: 12 }), text)
      : h('span', { class: 'chip strip-extra chain-chip', title }, icon('branch', { size: 12 }), text));
    const after = current.after ? (current.after.rule
      ? chainLink(current.after, `When ${current.after.rule}`, `Starts by itself when ${current.after.rule}`)
      : chainLink(current.after, `After #${current.after.number}`, current.after.title ? `Runs after #${current.after.number} ${current.after.title}` : `#${current.after.number} was deleted`)) : null;
    const next = (current.next || []).map((link) => chainLink(link, `Then #${link.number}`, `#${link.number} ${link.title || ''} runs after this task`));
    // How the task runs: "Auto → Goal · Feature" when Auto chose it (TSK-015), else Goal.
    const auto = autoChoiceLabel(current);
    const supertask = current.supertask
      ? h('span', { class: 'chip strip-extra supertask-chip', title: 'A supertask: it runs alone in its project, the other tasks wait for it' }, icon('shield', { size: 12 }), 'Supertask')
      : null;
    setChildren(extrasEl,
      supertask,
      auto ? h('span', { class: 'chip strip-extra auto-chip', title: auto.title }, icon('zap', { size: 12 }), auto.text)
        : current.mode === 'goal' ? h('span', { class: 'chip strip-extra', title: 'Runs as a goal (/goal): the agent keeps working until it is met' }, 'Goal') : null,
      after,
      next);
  }

  function renderStepList() {
    if (!stripOpen) return;
    if (!detail) { setChildren(stepList, skeleton('Loading steps…', 3)); return; }
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
  // Reads overlap (every status change reads again): an answer older than one already applied is
  // dropped, and only the latest read's answer sets the summary, so an older answer landing last
  // never brings back an older status. A newer read that fails leaves an older answer's details
  // standing (SPEC-056 WUI-105).
  let detailRead = 0;
  let appliedRead = 0;
  async function loadDetail() {
    const read = ++detailRead;
    try {
      const fresh = await ctx.api.get(`/api/tasks/${enc(current.id)}`, { signal: reads.signal });
      if (!alive || read < appliedRead) return; // the view was left, or a newer answer was applied
      appliedRead = read;
      detail = fresh;
      if (detail && detail.id === current.id) {
        if (read === detailRead) current = { ...current, ...pickSummary(detail) };
        heldKey = '';
        renderStrip();
      }
    } catch (error) {
      if (alive && read === detailRead && error.status !== 404) errorToast(error, 'Couldn’t load task details');
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
      const previousUpdate = current.updated_at;
      const previousOwner = owner();
      current = { ...current, ...next };
      // Whether the task started is the detail's: a status with a run means it has, and any status
      // change reads the detail again (a start that found no capacity has not). A message someone
      // else queued changes the request: read it again too.
      if (detail && RAN.has(current.status)) detail = { ...detail, started: true };
      if (current.status !== previousStatus || (queueOpen() && current.updated_at !== previousUpdate)) loadDetail();
      renderStrip();
      if (current.status !== previousStatus) terminal.refreshEmpty();
      if (current.status !== previousStatus) loadDelivery(); // a finished task may now be ready for a pull request
      else renderDelivery();
      // A continued task gets a new attempt: reconnect if the socket had ended.
      if (!wasLive && isLive(current.status) && terminal.socket.status !== 'open') terminal.reconnect();
      // Control passed (SPEC-070): the terminal asks again whether this viewer may type.
      else if (owner() !== previousOwner) terminal.reconnect();
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
  const keys = ['status', 'activity', 'title', 'steps', 'account_label', 'harness', 'branch', 'updated_at', 'archived', 'mode', 'after', 'next', 'held_prompt', 'auto', 'owner', 'control_request'];
  const out = {};
  for (const key of keys) if (key in detail) out[key] = detail[key];
  return out;
}

// ---------------------------------------------------------------- composer

const AUTO = 'auto';
// The sandboxes whose Start onboarding was pressed in this tab and has not answered yet: a composer
// mounted again meanwhile (the person left and came back) still shows it loading (SPEC-111).
const onboardingStarts = new Set();
const onboardingStartsChanged = new EventTarget();
// Auto is the default for the mode and the workflow (TSK-015): Jev decides when the task is created.
const MODES = [
  { id: AUTO, name: 'Auto mode', icon: 'zap', description: 'Jev decides: a goal for work to build, change or fix; a prompt for questions and research.' },
  { id: 'goal', name: 'Goal', icon: 'target', description: 'The agent keeps working until the goal is met.' },
  { id: 'prompt', name: 'Prompt', icon: 'send', description: 'Sent as a normal first message.' },
];

/** "Auto → Goal · Feature" for a task whose mode or workflow Auto chose (TSK-015), with its why. */
export function autoChoiceLabel(task) {
  const auto = task && task.auto;
  if (!auto) return null;
  const mode = task.mode === 'prompt' ? 'Prompt' : 'Goal';
  const workflow = auto.workflow_name || 'No workflow';
  const why = (by) => (by === 'jev' ? 'chosen by Jev' : by === 'default' ? 'the default (Jev was unsure or is not set up)' : 'picked when it was created');
  return {
    text: `Auto → ${mode} · ${workflow}`,
    title: `Mode: ${mode}, ${why(auto.mode)}. Workflow: ${workflow}, ${why(auto.workflow)}.`,
  };
}

/**
 * New-task composer (J3, UI-021, F 2026-10-10): a rounded prompt box whose bottom edge holds the
 * task's choices — mode and workflow (Auto by default: Jev decides), run after, model and effort
 * — and the ↑ Start button. Enter adds a line and continues Markdown lists (mdinput.js);
 * ⌘/Ctrl+Enter or ↑ starts. While intake decides, a calm status shimmers below the box, and the
 * split card takes its place there: nothing above it moves. Model and effort are remembered per
 * browser.
 */
export function mountComposer(ctx, sandbox) {
  const destroyers = [];
  const write = allows(sandbox.access, 'write');
  // What was typed, attached and chosen here waits in the sandbox's draft (SPEC-026).
  const userId = ctx.store.state.me && ctx.store.state.me.user ? ctx.store.state.me.user.id : null;
  const draft = draftFor(userId, sandbox.id);
  const remember = (options) => saveDraft(userId, sandbox.id, options);
  // Files added to the prompt sit inside its text (SPEC-124): an image as a thumbnail drawn from
  // its file (an object URL of this view), or from its stored thumbnail after a reload; another
  // file as a chip with its name. `known` holds every file this view has seen, so one deleted
  // and brought back (undo, a cut and paste) is the same file; the draft keeps the ones in the
  // text. A file opened in a tab has its own URL (`tabUrls`).
  const known = new Map();
  const urls = new Map();
  const tabUrls = new Map();
  const learn = () => { for (const item of draft.attached) known.set(item.path, item); };
  learn();
  const imageOf = (item) => {
    if (item.file && /^image\//.test(item.type || item.file.type || '')) {
      if (!urls.has(item.path)) urls.set(item.path, URL.createObjectURL(item.file));
      return urls.get(item.path);
    }
    return item.thumb || null;
  };
  const prompt = createPromptInput({
    label: 'Task prompt',
    placeholder: 'Describe what you want to build, fix or find out. Paste or drop images where they belong. Lists and Markdown work.',
    keyshortcuts: SUBMIT_ARIA,
    face: (path) => { const item = known.get(path); return item ? { name: item.name, image: imageOf(item) } : null; },
    known: (path) => known.has(path),
    onChange: () => {
      // A composer gone (an upload that ended after the person left) no longer speaks for the draft.
      if (destroyed) return;
      draft.text = prompt.value;
      keepAttached();
      remember();
      // Editing the prompt withdraws a split proposal made for the old text.
      if (split) clearSplit();
    },
    onSubmit: () => start(),
    onOpen: (path) => openFile(path),
  });
  /** Opens a file of the prompt: an image previews, another file opens in a tab or downloads. */
  function openFile(path) {
    const item = known.get(path);
    if (!item) return;
    const image = imageOf(item);
    if (!image && !item.file) return;
    openAttachment(item, { image, url: (blob) => {
      if (!tabUrls.has(path)) tabUrls.set(path, URL.createObjectURL(blob));
      return tabUrls.get(path);
    } });
  }
  prompt.setValue(draft.text, draft.attached.map((item) => item.path));
  /** The draft keeps the files its text holds, in order. */
  function keepAttached() {
    const paths = [...new Set(prompt.paths())];
    const next = paths.map((path) => known.get(path)).filter(Boolean);
    if (next.length !== draft.attached.length || next.some((item, i) => item !== draft.attached[i])) draft.attached = next;
  }
  keepAttached();
  draft.text = prompt.value;
  // A rendered, read-only view of the prompt (Preview), made on demand.
  const previewEl = h('div', { class: 'composer-preview', role: 'region', hidden: true, tabindex: 0, 'aria-label': 'Prompt preview' });
  let preview = null;
  let mode = MODES.some((m) => m.id === draft.mode) ? draft.mode : AUTO;
  let workflowId = draft.workflowId ?? AUTO;
  let workflows = [];
  // Run after another task of this sandbox (TSK-012): the new task waits until it is Done.
  let afterTaskId = draft.afterTaskId;
  // Or wait for a logical sentence over the sandbox's tasks (SPEC-017), like `#12 done and #13 done`,
  // picked as conditions in the rule editor (SPEC-022): `{ join, conditions }` or null.
  let startRule = draft.startRule;
  // Or run as a supertask (SPEC-071): after the project's running tasks, then alone.
  let supertask = Boolean(draft.supertask);
  // Or prepare it (SPEC-105): `{ at: null }` waits until the person starts it ("When I decide"),
  // `{ at: iso }` starts by itself then; null starts as chosen above.
  let prepare = draft.prepare || null;
  let split = null; // the split card while it shows (TSK-011)
  let modelId = findModel(readJson(MODEL_PREF_KEY, null)) ? readJson(MODEL_PREF_KEY, null) : null;
  let effortId = findEffort(readJson(EFFORT_PREF_KEY, null)) ? readJson(EFFORT_PREF_KEY, null) : null;

  const chip = (label, onOpen, extra = {}) => {
    // The visible value is the accessible name, with a hidden "Mode: " prefix (WCAG 2.5.3).
    const button = h('button', { type: 'button', class: ['chip-btn', extra.class], 'aria-haspopup': 'menu', 'aria-expanded': 'false', title: label });
    button.addEventListener('click', () => onOpen(button));
    return button;
  };
  // The sandbox's workflows (instance first), read once; Auto chooses among them on the server.
  const workflowsRead = ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/workflows`).then((list) => {
    workflows = Array.isArray(list) ? list : [];
    renderChips();
  }).catch(() => { /* workflows are optional */ });
  const modeChip = chip('Mode', openModeMenu);
  const workflowChip = chip('Workflow', openWorkflowMenu);
  const afterChip = chip('Run after', openAfterMenu);
  // Below the box: the intake status, then the split card, in one place (TSK-011).
  const intakeSlot = h('div', { class: 'intake-slot' });
  const modelChip = chip('Model', openModelMenu, { class: 'plain' });
  const effortChip = chip('Effort', openEffortMenu, { class: 'plain muted-chip' });
  const previewButton = h('button', { type: 'button', class: 'btn-icon composer-preview-toggle', 'aria-pressed': 'false', 'aria-label': 'Preview', title: 'Preview: the prompt formatted' }, icon('eye', { size: 16 }));
  const modeHint = h('span');
  const banner = h('div', { class: 'stack-sm' });
  const box = h('div', { class: 'composer-box' });
  const startButton = h('button', { type: 'button', class: 'btn btn-primary composer-send', 'aria-label': 'Start task', 'aria-keyshortcuts': SUBMIT_ARIA, title: `Start task (${SUBMIT_KEYS})` }, icon('up', { size: 18 }));
  // The keyboard way to start, shown quietly where there is a keyboard (not on phones).
  const hint = h('span', { class: 'composer-kbd' }, h('span', { class: 'kbd' }, SUBMIT_KEYS), ' to start');

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
  function forgetUrls() {
    for (const map of [urls, tabUrls]) { for (const url of map.values()) URL.revokeObjectURL(url); map.clear(); }
  }
  setChildren(box,
    prompt.el,
    previewEl,
    h('div', { class: 'composer-tools' }, modeChip, workflowChip, afterChip, h('div', { class: 'composer-right' }, previewButton, modelChip, effortChip, startButton)));
  // While the sandbox awaits onboarding, a centred message takes the card's place (SPEC-096).
  const gate = h('section', { class: 'onboarding-gate', hidden: true, 'aria-labelledby': `onboarding-gate-${sandbox.id}` });
  // Says what changed when someone else starts onboarding or it lands (the gate's text is rebuilt).
  // A live region without the status role: the page's own status messages stay the only ones.
  const gateNews = h('span', { class: 'sr-only', 'aria-live': 'polite', 'aria-atomic': 'true' });
  const card = h('div', { class: 'composer-card' },
      // A brand's niche for its mark (hidden unless a brand shows it; styles.css "Brand details").
      niche,
      h('figure', { class: 'composer-quote' }, h('blockquote', null, `“${quote.text}”`), h('figcaption', null, `— ${quote.author}`)),
      banner,
      box,
      h('div', { class: 'composer-hint muted xs' }, modeHint, h('span', { class: 'composer-hint-sep', 'aria-hidden': 'true' }, ' · '), hint),
      intakeSlot);
  const el = h('div', { class: 'composer' }, repoBar, card, gate, gateNews);
  // Files dropped (or an image pasted) on the prompt reach the new task's agent by path.
  attachPromptDrop(ctx, { box, prompt, label: 'Drop to add to the prompt', upload: draftAttachments(ctx, sandbox.id), onAttach: (items, place) => {
    // At the place kept when they came, in this composer; an upload that ends after the person
    // left still joins the draft (DRF-002), after the text of the composer open then.
    const here = place && !destroyed;
    if (here) arriving = new Set(items.map((item) => item.path));
    attachToDraft(userId, sandbox.id, draft, items);
    arriving = null;
    if (here && items.every((item) => known.has(item.path))) place.insert(items.map((item) => item.path));
  } });
  // Files being put in their place right now: not appended as late ones.
  let arriving = null;
  /** The draft changed elsewhere: files it gained follow the text, a cleared draft empties it. */
  function draftChanged() {
    learn();
    if (!draft.text && !draft.attached.length) {
      if (prompt.value) prompt.setValue('');
      return;
    }
    const shown = new Set(prompt.paths());
    const late = draft.attached.map((item) => item.path).filter((path) => !shown.has(path) && !(arriving && arriving.has(path)));
    if (late.length) prompt.append(late);
    else prompt.refresh(); // a thumbnail came
  }

  function renderChips() {
    const m = MODES.find((x) => x.id === mode);
    setChildren(modeChip, icon(m.icon, { size: 14 }), h('span', { class: 'sr-only' }, 'Mode: '), h('span', { class: 'chip-label' }, m.name), icon('down', { size: 12 }));
    modeChip.title = m.description;
    modeHint.textContent = mode !== AUTO ? m.description
      : workflowId === AUTO ? 'Jev picks goal or prompt mode and the workflow as the task starts.' : 'Jev picks goal or prompt mode as the task starts.';
    const wf = workflows.find((w) => w.id === workflowId);
    if (workflowId && workflowId !== AUTO && !wf && workflows.length) workflowId = AUTO; // deleted meanwhile
    const wfLabel = workflowId === AUTO ? 'Auto workflow' : wf ? wf.name : workflowId ? 'Workflow' : 'No workflow';
    setChildren(workflowChip, icon(workflowId === AUTO ? 'zap' : 'workflow', { size: 14 }), h('span', { class: 'sr-only' }, 'Workflow: '), h('span', { class: 'chip-label' }, wfLabel), icon('down', { size: 12 }));
    workflowChip.title = workflowId === AUTO ? 'Jev picks the workflow that fits, or none: features get a spec, research and questions do not' : wfLabel;
    const tasks = ctx.store.tasksOf(sandbox.id);
    const after = (tasks || []).find((t) => t.id === afterTaskId);
    if (afterTaskId && tasks && !after) afterTaskId = ''; // gone (a kept draft waits for the list)
    const rule = startRule ? ruleText(startRule) : '';
    // A kept time that has passed (a draft from yesterday) waits for the person instead: sending
    // it would be refused, and starting it now was not asked for (SPEC-105).
    if (prepare && prepare.at && Date.parse(prepare.at) < Date.now() - 60e3) prepare = { at: null };
    const kind = startKind({ supertask, afterTaskId: after && after.id, rule, prepare });
    renderStartChip(afterChip, { kind, number: after && after.number, rule, at: prepare && prepare.at, prefix: 'Run after: ' });
    const at = prepare && prepare.at ? whenLabel(prepare.at) : '';
    afterChip.title = prepare ? (at ? `Starts by itself ${at}` : WHEN_I_DECIDE_HELP) : supertask ? SUPERTASK_HELP : rule ? `Starts by itself when ${rule}` : after ? `Runs after #${after.number} ${after.title} is done` : 'Starts right away; or pick a task to run after, or give a rule';
    // The send button says what it does: a prepared task is not started (SPEC-105).
    startButton.setAttribute('aria-label', startAction());
    startButton.title = write ? `${startAction()} (${SUBMIT_KEYS})` : 'Requires write access';
    const found = findModel(modelId);
    setChildren(modelChip, h('span', { class: 'sr-only' }, 'Model: '), h('span', { class: 'chip-label' }, found ? found.model.name : 'Default model'), icon('down', { size: 12 }));
    modelChip.title = found ? `${found.provider.name} · ${found.model.name}` : 'Model: default, Opus 5.5';
    // A model without effort levels (Haiku) hides the effort choice and sends none.
    const effortless = Boolean(found && found.model.effort === false);
    effortChip.hidden = effortless;
    const effort = findEffort(effortId);
    setChildren(effortChip, h('span', { class: 'sr-only' }, 'Effort: '), h('span', { class: 'chip-label' }, effort ? effort.name : 'Default effort'), icon('down', { size: 12 }));
    // Every choice shown here is the draft's (DRF-003).
    if (draft.mode !== mode || draft.workflowId !== workflowId || draft.afterTaskId !== afterTaskId || draft.startRule !== startRule || draft.supertask !== supertask || draft.prepare !== prepare) {
      Object.assign(draft, { mode, workflowId, afterTaskId, startRule, supertask, prepare });
      remember();
    }
  }

  function openModeMenu(anchor) {
    openMenu(anchor, MODES.map((m) => ({ label: m.name, icon: m.icon, description: m.description, checked: m.id === mode, onSelect: () => { mode = m.id; renderChips(); prompt.focus(); } })));
  }

  async function openWorkflowMenu(anchor) {
    // A menu opened right away still lists the sandbox's workflows: wait for their first read.
    await workflowsRead;
    if (!anchor.isConnected) return;
    const slug = encodeURIComponent(sandbox.slug);
    openMenu(anchor, [
      { label: 'Auto workflow', icon: 'zap', description: 'Jev picks the workflow that fits, or none: features get a spec, research and questions do not', checked: workflowId === AUTO, onSelect: () => { workflowId = AUTO; renderChips(); } },
      { label: 'No workflow', icon: 'x', description: 'The agent plans the work itself', checked: !workflowId, onSelect: () => { workflowId = ''; renderChips(); } },
      ...workflows.map((wf) => ({ label: `${wf.name}${wf.builtin ? ' (default)' : wf.scope === 'instance' ? ' (instance)' : ''}`, icon: 'workflow', description: wf.description || (Array.isArray(wf.steps) ? wf.steps.join(' → ') : null), checked: wf.id === workflowId, onSelect: () => { workflowId = wf.id; renderChips(); } })),
      { separator: true },
      { label: 'Manage workflows…', icon: 'gear', onSelect: () => ctx.navigate(`/s/${slug}/workflows`) },
    ]);
  }

  function openAfterMenu(anchor) {
    openStartMenu(anchor, ctx, sandbox, {
      kind: startKind({ supertask, afterTaskId, rule: startRule, prepare }),
      afterTaskId,
      at: prepare && prepare.at,
      onPick: ({ kind, task }) => {
        afterTaskId = kind === 'after' ? task.id : '';
        startRule = null;
        supertask = kind === 'supertask';
        prepare = kind === 'prepared' ? { at: null } : null;
        renderChips();
        prompt.focus();
      },
      onRule: openRuleDialog,
      onTime: openTimeDialog,
    });
  }

  /** A start rule (SPEC-017) picked as conditions (SPEC-022); the server checks it. */
  function openRuleDialog() {
    openRuleEditor({
      tasks: ctx.store.tasksOf(sandbox.id),
      value: startRule,
      onUse: (rule) => {
        startRule = rule;
        afterTaskId = '';
        supertask = false;
        prepare = null;
        renderChips();
        prompt.focus();
      },
    });
  }

  /** What the send button does with the start choice (SPEC-105). */
  function startAction() {
    if (!prepare) return 'Start task';
    return prepare.at ? 'Schedule task' : 'Prepare task';
  }

  /** At a time… (SPEC-105): the task is prepared and starts by itself then. */
  function openTimeDialog() {
    openScheduleDialog({
      value: prepare && prepare.at,
      onUse: (at) => {
        prepare = { at };
        afterTaskId = '';
        startRule = null;
        supertask = false;
        renderChips();
        prompt.focus();
      },
    });
  }

  function openModelMenu(anchor) {
    openMenu(anchor, modelMenuItems(modelId, (id) => { modelId = id; writeJson(MODEL_PREF_KEY, id); renderChips(); prompt.focus(); }));
  }

  function openEffortMenu(anchor) {
    openMenu(anchor, effortMenuItems(effortId, (id) => { effortId = id; writeJson(EFFORT_PREF_KEY, id); renderChips(); prompt.focus(); }));
  }


  // Preview: the prompt formatted, read-only, in the box's place; Edit (or a click) goes back.
  function setPreview(on) {
    if (on && !prompt.value.trim()) { toast('Nothing to preview yet.', { kind: 'info' }); return; }
    if (preview) { preview.destroy(); preview = null; }
    if (on) {
      // Each file shows where it sits in the formatted text (SPEC-124).
      const { plain, paths } = prompt.model();
      let n = 0;
      const markdown = withoutMarks(plain).replace(new RegExp(ATOM, 'g'), () => `${FILE_MARK}${n++}${FILE_MARK_END}`);
      preview = createMarkdownEditor({ markdown, readOnly: true, frontmatter: false });
      // Each file shows where it sits in the formatted text (SPEC-124), and opens from there.
      placeFiles(preview.el, (i) => {
        const item = known.get(paths[i]);
        if (!item) return null;
        const image = imageOf(item);
        const face = image ? h('img', { class: 'pi-preview-image', src: image, alt: item.name })
          : [icon('file', { size: 14 }), h('span', { class: 'pi-file-name' }, item.name)];
        return image || item.file
          ? h('button', { type: 'button', class: ['pi-preview-file', !image && 'pi-atom pi-file'], 'aria-label': `Open ${item.name}`, onClick: () => openFile(item.path) }, face)
          : h('span', { class: 'pi-atom pi-file', title: `${item.name}: attach it again to open it here` }, face);
      });
      setChildren(previewEl, preview.el);
      previewEl.style.minHeight = `${prompt.el.offsetHeight}px`;
    } else {
      setChildren(previewEl, null);
    }
    previewEl.hidden = !on;
    prompt.hidden = on;
    box.classList.toggle('previewing', on);
    previewButton.setAttribute('aria-pressed', String(on));
    previewButton.title = on ? 'Back to editing the prompt' : 'Preview: the prompt formatted';
    if (!on) prompt.focus();
  }
  previewButton.addEventListener('click', () => setPreview(prompt.hidden === false));
  previewEl.addEventListener('dblclick', () => setPreview(false));
  previewEl.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' || (event.key === 'Enter' && !event.metaKey && !event.ctrlKey)) { event.preventDefault(); setPreview(false); }
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) { event.preventDefault(); start(); }
  });

  // The superadmin is reminded while no recovery code exists (ACC-011).
  let recovery = null;
  const me0 = ctx.store.state.me;
  if (me0 && me0.user && me0.user.is_superadmin) {
    ctx.api.get('/api/auth/recovery').then((state) => { recovery = state; renderBanner(); }).catch(() => {});
  }

  let gateKey = '';
  let opening = false;
  let sentToTask = false; // this view sent the page to the onboarding task
  // The gate offered Start onboarding in this view: opening the task then is news to say.
  let offered = false;
  function renderBanner() {
    const sb = ctx.store.sandboxById(sandbox.id) || sandbox;
    const items = [];
    const me = ctx.store.state.me;
    if (me && me.user && me.user.is_superadmin && recovery && !recovery.available) {
      items.push(h('div', { class: 'banner banner-danger' }, icon('warn'),
        h('span', { class: 'banner-text' }, h('strong', null, recovery.locked ? 'Password recovery is disabled. ' : 'No recovery code yet. '), 'Without a recovery code a forgotten superadmin password cannot be reset.'),
        h('a', { class: 'btn btn-sm', href: href('/admin/users'), dataset: { route: '/admin/users' } }, 'Generate one')));
    }
    // Onboarding is mandatory (SPEC-096, ADR-0076): while it is `pending` or `preparing` the page
    // shows only the onboarding message, never the task input; the composer stays built, hidden.
    const preparing = sb.onboarding === 'pending' || sb.onboarding === 'preparing';
    const wasGated = !gate.hidden;
    // The onboarding task: `undefined` while the sandbox's tasks are not known (not read yet, or
    // reading them failed), `null` when they are and it is not among them.
    const list = ctx.store.tasksOf(sb.id);
    const prep = !preparing ? null : !list || ctx.store.tasksError(sb.id) ? undefined : list.find((t) => t.origin === 'onboarding') || null;
    if (sb.onboarding === 'pending') loadPlan(prep);
    // Rebuilt only when what it says changes (task events are frequent), and a rebuild keeps the
    // keyboard on its action.
    // While onboarding runs the page opens its task, where the session is (SPEC-111): no message
    // stands in between, only a loading state until the task is known.
    const shown = sb.onboarding === 'pending'
      ? [onboardingStarts.has(sb.id) || [prep && prep.title, plans.get(planKey(prep)), branchOf(sb)]]
      : [prep === undefined ? (ctx.store.tasksError(sb.id) ? 'unknown' : 'loading') : prep ? 'open' : 'missing'];
    const key = preparing ? JSON.stringify([sb.onboarding, sb.name, sb.slug, sb.access, prep && prep.number, ...shown]) : '';
    if (key !== gateKey) {
      const before = gateKey ? JSON.parse(gateKey)[0] : null;
      const hadFocus = gate.contains(document.activeElement);
      setChildren(gate, preparing ? onboardingGate(sb, prep) : null);
      if (hadFocus && preparing) focusGate();
      // A start from this page has its own toast: the status speaks for someone else's.
      if (before && before !== (preparing ? sb.onboarding : null)) {
        gateNews.textContent = preparing ? '' : 'Onboarding finished: the task box is ready.';
      }
    }
    gateKey = key;
    gate.hidden = !preparing;
    gate.setAttribute('aria-busy', String(Boolean(gate.querySelector('.onboarding-gate-loading'))));
    if (sb.onboarding === 'pending') offered = true;
    if (sb.onboarding === 'preparing' && prep) openOnboarding(sb, prep);
    card.hidden = preparing;
    el.classList.toggle('gated', preparing);
    if (!write) items.push(h('div', { class: 'banner' }, icon('lock'), h('span', { class: 'banner-text' }, 'Read-only (GitHub: read). You can watch tasks but not start them.')));
    setChildren(banner, items);
    startButton.disabled = !write;
    prompt.disabled = !write;
    startButton.title = !write ? 'Requires write access' : `${startAction()} (${SUBMIT_KEYS})`;
    // Onboarding landed while the page was open: the task box is ready for typing, unless the
    // person is busy elsewhere on the page.
    if (wasGated && !preparing && el.isConnected) {
      const idle = document.activeElement === document.body || el.contains(document.activeElement) || !document.activeElement;
      if (idle && !prompt.disabled && !isPhone()) prompt.focus();
    }
  }

  const branchOf = (sb) => (sb.profile && sb.profile.integration_branch) || baseBranch;

  /**
   * Opens the running onboarding's task in this page's place (SPEC-111), once. Later than the
   * render that asks for it: the router mounts views synchronously, and this one may be mid-mount.
   */
  function openOnboarding(sb, prep) {
    if (opening) return;
    opening = true;
    setTimeout(() => {
      if (destroyed || !el.isConnected) { opening = false; return; }
      // Started here or elsewhere while this page offered it: a toast outlives the page it leaves.
      if (offered) toast(`Onboarding started: opened #${prep.number}.`, { kind: 'ok' });
      ctx.navigate(`/s/${encodeURIComponent(sb.slug)}/t/${prep.number}`, { replace: true });
      sentToTask = true;
      // A view kept after the navigation (none is today) may open it again.
      if (!destroyed) opening = false;
    }, 0);
  }

  // What exactly onboarding will do, `{ title, steps }`, whichever kind it is (`onboarding_prompt`
  // or `environment_prompt`): its task's own plan, or before that task exists (it is created when
  // onboarding starts, SPEC-097) the plan it will get. Per task id, or `''` while there is no task.
  // `null` while it loads, `false` when it could not be read.
  const plans = new Map();
  const planKey = (task) => (task ? task.id : '');
  let destroyed = false;
  function loadPlan(task) {
    const key = planKey(task);
    if (plans.has(key)) return;
    plans.set(key, null);
    ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/onboarding/plan`)
      .then((plan) => {
        const steps = plan && Array.isArray(plan.steps) ? plan.steps.filter(Boolean) : [];
        plans.set(key, steps.length ? { title: plan.title, steps } : false);
      })
      .catch(() => plans.set(key, false))
      .finally(() => { if (!destroyed) renderBanner(); });
  }

  /**
   * What onboarding will do and its one action, Start onboarding; then a loading state until its
   * task opens (SPEC-111). Only a task that cannot be read or is missing is said in words.
   */
  function onboardingGate(sb, prep) {
    const titleId = `onboarding-gate-${sandbox.id}`;
    const path = prep ? `/s/${encodeURIComponent(sb.slug)}/t/${prep.number}` : null;
    const ready = `The task box appears here by itself once it has finished and the repository passes the ${BRAND.name} format check.`;
    const loading = (title, line) => [
      h('h2', { id: titleId, class: 'onboarding-gate-title', tabindex: -1 }, title),
      h('p', null, line),
      // The region is `aria-busy` and its line says what loads: the bars are drawn only.
      h('div', { class: 'onboarding-gate-loading' }, skeleton(line, 3, { announce: false })),
    ];
    if (sb.onboarding === 'pending' && onboardingStarts.has(sb.id)) {
      return loading(`Starting onboarding ${sb.name}`, `${BRAND.name} is starting the agent’s session; it opens here in a moment.`);
    }
    if (sb.onboarding === 'preparing') {
      if (prep) return loading(`Opening onboarding ${sb.name}`, `Opening #${prep.number}, where the agent prepares this sandbox.`);
      if (prep === undefined && !ctx.store.tasksError(sb.id)) return loading(`Opening onboarding ${sb.name}`, 'Opening the onboarding task.');
      const title = h('h2', { id: titleId, class: 'onboarding-gate-title', tabindex: -1 }, `Onboarding ${sb.name}`);
      // The server's own reading of a preparing sandbox (`onboarding_status`), in the page's words.
      if (prep === undefined) return [title, h('p', null, `An agent is setting up this sandbox and adapting the repository to the ${BRAND.name} standards. ${ready}`)];
      return [title,
        h('p', null, 'The onboarding task is missing, so onboarding cannot finish by itself.'),
        h('p', { class: 'muted' }, allows(sb.access, 'admin')
          ? `As an administrator you can end the wait through the API: POST /api/sandboxes/${sb.id}/onboarding/skip.`
          : 'An administrator of this sandbox can end the wait.')];
    }
    const plan = plans.get(planKey(prep));
    const intro = 'Onboarding is one task an agent runs before any other in this sandbox';
    return [
      h('h2', { id: titleId, class: 'onboarding-gate-title', tabindex: -1 }, `Onboard ${sb.name} first`),
      h('p', null, prep
        ? [`${intro}: `, h('a', { href: href(path), dataset: { route: path } }, `#${prep.number} ${prep.title}`), '. Its plan:']
        : plan ? `${intro}: ${plan.title}. Its plan:` : `${intro}.`),
      plan ? h('ol', { class: 'onboarding-gate-steps' }, plan.steps.map((step) => h('li', null, step)))
        : plan === null ? skeleton('Loading what onboarding will do…', 4) : null,
      h('p', null, `It changes documentation, configuration and CI only, never application code, and any change reaches ${branchOf(sb)} through a pull request. ${ready}`),
      allows(sb.access, 'write')
        ? h('button', { type: 'button', class: 'btn btn-primary', onClick: () => approveOnboarding() }, 'Start onboarding')
        : h('p', { class: 'muted' }, 'Someone with write access to the repository starts it here.'),
    ];
  }

  // One start per press: a double click must not send two approvals (singleFlight).
  // The gate loads at once and nothing in it can be pressed again; the approval answers once the
  // task's run has started, and the page then opens that task (SPEC-111).
  const approveOnboarding = singleFlight(async () => {
    const away = () => !gate.contains(document.activeElement) && document.activeElement !== document.body && document.activeElement;
    if (onboardingStarts.has(sandbox.id)) return;
    const changed = () => onboardingStartsChanged.dispatchEvent(new Event('change'));
    onboardingStarts.add(sandbox.id);
    changed();
    try {
      const updated = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/onboarding/approve`);
      await ctx.store.loadTasks(sandbox.id).catch(() => null);
      if (updated && updated.id) ctx.store.upsertSandbox(updated);
      // Opening the task says so itself; otherwise (the task could not be read, or the person went
      // elsewhere meanwhile) this does.
      if (!opening && !sentToTask) toast('Onboarding started.', { kind: 'ok' });
    } catch (error) {
      errorToast(error, 'Couldn’t start onboarding');
    } finally {
      onboardingStarts.delete(sandbox.id);
      changed();
    }
    // Back as it was (a failure), or a task that could not be read: the keyboard goes back to the
    // gate, unless the person moved it elsewhere while the start ran.
    if (!destroyed && !opening && !away()) focusGate();
  });

  /** Puts the keyboard on the gate's action, else its heading. */
  function focusGate() {
    const target = gate.querySelector('.btn') || gate.querySelector('h2');
    if (target) target.focus();
  }

  /**
   * The request body of a new task with the composer's choices (`auto`: Jev decides, TSK-015);
   * `text` is the prompt as sent, its files in it.
   */
  function bodyFor(text, title) {
    const body = { prompt: text, mode };
    if (title) body.title = title;
    if (workflowId) body.workflow_id = workflowId;
    const found = findModel(modelId);
    if (found) {
      body.model = found.model.id;
      if (found.provider.harness !== 'claude') body.harness = found.provider.harness;
    }
    if (effortId && !(found && found.model.effort === false)) body.effort = effortId;
    return body;
  }

  async function createTask(body) {
    const task = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/tasks`, body);
    ctx.store.upsertTask(task);
    return task;
  }

  function opened(task) {
    prompt.setValue('');
    mode = AUTO;
    workflowId = AUTO;
    afterTaskId = '';
    startRule = null;
    supertask = false;
    prepare = null;
    clearDraft(userId, sandbox.id);
    ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`);
  }

  function clearSplit() {
    if (split) split.cancel();
    split = null;
    setChildren(intakeSlot, null);
  }

  // ---- working: the ↑ button spins at once; after a moment the box shimmers and a status line
  // below it says what is happening, so a quick answer never flashes and a slow one never looks
  // frozen. The prompt stays as it is (read-only) while it is being read.
  let status = null;
  let showTimer = 0;
  const autoStage = () => (mode === AUTO || workflowId === AUTO ? 'Choosing mode and workflow…' : prepare ? 'Preparing the task…' : 'Starting the task…');
  function working(text) {
    draft.starting = true;
    startButton.disabled = true;
    startButton.classList.add('busy');
    prompt.readOnly = true;
    if (status) { status.stage(text); return; }
    status = intakeStatus(text);
    clearTimeout(showTimer);
    showTimer = setTimeout(() => {
      if (!status) return;
      box.classList.add('thinking');
      setChildren(intakeSlot, status.el);
    }, 150);
  }
  function idle() {
    draft.starting = false;
    clearTimeout(showTimer);
    box.classList.remove('thinking');
    prompt.readOnly = false;
    startButton.classList.remove('busy');
    if (status && intakeSlot.contains(status.el)) setChildren(intakeSlot, null);
    status = null;
    renderBanner();
  }

  /** Runs a creation chosen on the split card, with the working state. */
  async function busyRun(stage, work) {
    clearSplit();
    working(stage);
    try {
      await work();
    } catch (error) {
      errorToast(error, 'Couldn’t start the task');
    } finally {
      idle();
    }
  }

  /**
   * The gentle split card (TSK-011), below the box: parallel tasks, a chain, or one task. `text`
   * is the request's words (its files left out), `value` the prompt as sent; split tasks each get
   * the files after their text.
   */
  function showSplit(text, value) {
    split = splitCard(ctx, sandbox, text, {
      create: (list, chain) => busyRun(chain ? `Creating a chain of ${list.length} tasks…` : `Creating ${list.length} tasks…`, async () => {
        let first = null;
        let previous = null;
        for (const item of list) {
          const body = bodyFor(withAttachments(item.prompt, draft.attached), item.title);
          if (chain && previous) body.after_task_id = previous.id;
          const task = await createTask(body);
          first = first || task;
          previous = task;
        }
        toast(chain ? `Created a chain of ${list.length} tasks: each starts when the one before is done.` : `Created ${list.length} tasks.`, { kind: 'ok' });
        opened(first);
      }),
      keep: () => busyRun(autoStage(), async () => opened(await createTask(bodyFor(value)))),
      close: () => clearSplit(),
    });
    setChildren(intakeSlot, split.el);
    const calm = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    split.el.scrollIntoView({ block: 'nearest', behavior: calm ? 'auto' : 'smooth' });
  }

  // One task per press: two presses must not start two sessions (singleFlight).
  const start = singleFlight(async () => {
    const text = prompt.text().trim();
    const value = prompt.value.trim();
    if (!text) { if (prompt.hidden) setPreview(false); prompt.focus(); toast('Write a prompt first.', { kind: 'warn' }); return; }
    if (split || prompt.disabled) return; // the card is open (its buttons decide), or starting is blocked
    // Started from this composer before the person left and came back: not twice.
    if (draft.starting) { toast('This task is already starting.', { kind: 'info' }); return; }
    try {
      // A request mixing separate workstreams gets the split card first; a chosen "Run after"
      // already decided how the work is scheduled, and a supertask or a prepared task is one task
      // by intent.
      // The check never blocks for long (≤ 4 s).
      if (!afterTaskId && !startRule && !supertask && !prepare && text.split(/\s+/).filter(Boolean).length >= SPLIT_MIN_WORDS) {
        working('Reading your request…');
        if (await looksSeveral(ctx, sandbox, text)) {
          showSplit(text, value);
          return;
        }
      }
      working(autoStage());
      const body = bodyFor(value);
      if (afterTaskId) body.after_task_id = afterTaskId;
      if (startRule) body.start_rule = ruleText(startRule);
      if (supertask) body.supertask = true;
      if (prepare && prepare.at) body.start_at = prepare.at;
      else if (prepare) body.prepared = true;
      opened(await createTask(body));
    } catch (error) {
      errorToast(error, 'Couldn’t start the task');
    } finally {
      idle();
    }
  });

  startButton.addEventListener('click', start);


  renderChips();

  destroyers.push(ctx.store.on('sandboxes', renderBanner));
  // A kept "Run after" shows once the sandbox's tasks are read.
  destroyers.push(ctx.store.on('tasks', (sid) => { if (sid === sandbox.id) { renderBanner(); if (afterTaskId) renderChips(); } }));
  destroyers.push(onDraftChange(userId, sandbox.id, draftChanged));
  const onStarts = () => { if (!destroyed) renderBanner(); };
  onboardingStartsChanged.addEventListener('change', onStarts);
  destroyers.push(() => onboardingStartsChanged.removeEventListener('change', onStarts));
  renderBanner();

  return {
    el,
    key: `composer:${sandbox.id}`,
    start() {
      // While onboarding comes first the keyboard lands on its action (SPEC-096).
      if (!isPhone()) {
        if (card.hidden) focusGate();
        else if (!prompt.disabled) prompt.focus();
      }
      if (!stopSmoke) stopSmoke = brandSmoke(niche); // the brand's smoke, when its niche shows (brandfx.js)
    },
    destroy() { destroyed = true; forgetUrls(); remember({ now: true }); destroyers.forEach((off) => off()); if (stopSmoke) stopSmoke(); clearTimeout(showTimer); clearSplit(); if (preview) preview.destroy(); },
  };
}
