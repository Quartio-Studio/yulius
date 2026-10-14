// Task actions (UI-013, §6.3) and shared dialogs: feedback, delivery status, deploy output,
// sandbox create/import and removal. Delivery is shown, not driven: agents merge, deploy and
// release as part of their workflow (GIT-002), so no dialog here merges or releases. Tasks are
// subscription-agnostic (F 2026-10-10): the pool picks and changes a task's Claude subscription by
// itself, so no action chooses one.

import { enc } from './api.js';
import { h, openModal, confirmDialog, openMenu, toast, errorToast, field, invalid, badge, setChildren, icon, iconButton, placeholder } from './dom.js';
import { taskActions, taskInfo, allows, isLive, otherOwner } from './status.js';
import { relLong, whenLabel, localInputValue, fromLocalInput } from './format.js';
import { BRAND } from './brand.js';
import { createMarkdownEditor } from './mdeditor.js';
import { openAttachment } from './filedrop.js';

// ---------------------------------------------------------------- task menu

/** Kebab / right-click menu for a task. */
export function openTaskMenu(ctx, anchor, task) {
  const sandbox = ctx.store.sandboxById(task.sandbox_id);
  const access = sandbox ? sandbox.access : 'read';
  const pr = ctx.store.pullsOf(task.sandbox_id).get(task.id);
  const me = ctx.store.state.me?.user?.login;
  const can = taskActions(task, access, pr, me);
  const owner = otherOwner(task, me);
  const why = owner ? `@${owner} owns this task` : allows(access, 'write') ? 'Not available in this state' : 'Requires write access';
  openMenu(anchor, [
    // The same rule as the row it opens from (pull request, held prompt, browser request).
    { header: `#${task.number} · ${taskInfo(task, ctx.store.state.actionRequests, pr).label}` },
    task.status === 'chained' || task.status === 'prepared' ? { label: 'Start now', disabled: !can.startNow, title: can.startNow ? null : why, onSelect: () => startNow(ctx, task) } : null,
    task.status === 'prepared' ? { label: task.start_at ? 'Change time…' : 'Schedule…', disabled: !can.schedule, title: can.schedule ? null : why, onSelect: () => rescheduleTask(ctx, task) } : null,
    { label: task.status === 'chained' || task.status === 'prepared' ? 'Cancel' : 'Stop', disabled: !can.stop, title: can.stop ? null : why, onSelect: () => stopTask(ctx, task) },
    { label: 'Continue', disabled: !can.continue, title: can.continue ? null : why, onSelect: () => continueTask(ctx, task) },
    { label: 'See the original prompt', onSelect: () => openPromptDialog(ctx, task) },
    { label: 'Delivery…', onSelect: () => openDeliveryDialog(ctx, task) },
    { label: 'Feedback…', disabled: !can.feedback, onSelect: () => openFeedbackDialog(ctx, task) },
    { separator: true },
    task.archived
      ? { label: 'Restore', disabled: !can.restore, title: can.restore ? null : why, onSelect: () => restoreTask(ctx, task) }
      : { label: 'Archive', disabled: !can.archive, title: can.archive ? null : (allows(access, 'write') && !owner ? 'Still running: stop the task before archiving it' : why), onSelect: () => archiveTask(ctx, task) },
    { label: 'Delete permanently…', danger: true, disabled: !can.delete, title: can.delete ? null : (!allows(access, 'write') || owner ? why : task.status === 'stopping' ? 'Stopping…' : 'Still running: stop the task before deleting it'), onSelect: () => deleteTask(ctx, task) },
  ], { align: anchor instanceof Element ? 'end' : 'start' });
}

// Every API path is a literal at its call site so tests/ui_contract.rs can check it
// against the server's router.
const inFlight = new Set(); // "<task id>:<verb>" while that request runs: a repeat is ignored

async function taskPost(ctx, task, verb, request, okMessage) {
  const key = `${task.id}:${verb}`;
  if (inFlight.has(key)) return null;
  inFlight.add(key);
  try {
    const updated = await request();
    if (updated && updated.id) ctx.store.upsertTask(updated);
    if (okMessage) toast(okMessage, { kind: 'ok' });
    return updated;
  } catch (error) {
    errorToast(error, `Couldn’t ${verb} task #${task.number}`);
    return null;
  } finally {
    inFlight.delete(key);
  }
}

/**
 * When a task starts (SPEC-105): a date and time in the person's time zone, at most 366 days
 * ahead. `value` is the time picked before (ISO) or null; `onUse(iso)` gets the new time and may
 * throw to keep the dialog open with the reason. With `onClear`, a "When I decide" button clears
 * the time instead.
 */
export function openScheduleDialog({ value, onUse, onClear }) {
  const next = new Date();
  next.setHours(next.getHours() + 1, 0, 0, 0);
  const id = `schedule-${Math.random().toString(36).slice(2, 8)}`;
  const hintId = `${id}-hint`;
  const input = h('input', { id, class: 'input', type: 'datetime-local', value: localInputValue(value || next.toISOString()), min: localInputValue(new Date().toISOString()), required: true, 'aria-describedby': hintId });
  return openModal({
    title: 'Start at a time',
    className: 'schedule-dialog',
    body: h('div', { class: 'stack' },
      // A label of its own: a validation message joins the field, not the input's name.
      h('div', { class: 'field' }, h('label', { class: 'field-label', for: id }, 'Date and time'), input,
        h('span', { class: 'field-hint', id: hintId }, 'In your time zone. The task starts by itself within a minute of it.'))),
    actions: [
      { label: 'Cancel', value: false },
      onClear ? { label: 'When I decide', run: () => onClear() } : null,
      { label: 'Use this time', kind: 'primary', run: async () => {
        const at = fromLocalInput(input.value);
        const ahead = at ? Date.parse(at) - Date.now() : NaN;
        if (!at) { invalid(input, 'Pick a date and a time.'); return false; }
        if (ahead < -60e3) { invalid(input, 'That time has passed: pick a later one.'); return false; }
        if (ahead > 366 * 86400e3) { invalid(input, 'Pick a time at most 366 days ahead.'); return false; }
        await onUse(at);
        return true;
      } },
    ].filter(Boolean),
  });
}

/** Sets or clears when a prepared task starts by itself (SPEC-105); null: when I decide. */
export function scheduleTask(ctx, task, at) {
  return ctx.api.put(`/api/tasks/${enc(task.id)}/schedule`, { start_at: at }).then((updated) => {
    if (updated && updated.id) ctx.store.upsertTask(updated);
    toast(at ? `#${task.number} starts ${whenLabel(at)}.` : `#${task.number} waits until you start it.`, { kind: 'ok' });
    return updated;
  });
}

/** Schedule… / Change time… of a prepared task: the dialog, then the server. */
export function rescheduleTask(ctx, task) {
  return openScheduleDialog({
    value: task.start_at,
    onUse: (at) => scheduleTask(ctx, task, at),
    onClear: task.start_at ? () => scheduleTask(ctx, task, null) : null,
  });
}

/** A task waiting in a chain starts now, whatever its predecessor does (TSK-012). */
export function startNow(ctx, task) {
  return taskPost(ctx, task, 'start', () => ctx.api.post(`/api/tasks/${enc(task.id)}/start-now`), `Starting #${task.number}…`);
}

/** Stop a live task (confirmation: stopping interrupts the agent); cancel a chained one. */
export async function stopTask(ctx, task) {
  if (task.status === 'prepared') {
    const cancel = await confirmDialog({
      title: `Cancel task #${task.number}?`,
      message: `It is prepared${task.start_at ? ` to start ${whenLabel(task.start_at)}` : ''} and will not start. You can continue it later.`,
      confirmLabel: 'Cancel task',
      danger: true,
    });
    if (cancel) await taskPost(ctx, task, 'stop', () => ctx.api.post(`/api/tasks/${enc(task.id)}/stop`), `Cancelled #${task.number}`);
    return;
  }
  if (task.status === 'chained') {
    const cancel = await confirmDialog({
      title: `Cancel task #${task.number}?`,
      message: `It is queued ${task.after && task.after.rule ? `until ${task.after.rule}` : `after #${task.after ? task.after.number : '?'}`} and will not start. You can continue it later.`,
      confirmLabel: 'Cancel task',
      danger: true,
    });
    if (cancel) await taskPost(ctx, task, 'stop', () => ctx.api.post(`/api/tasks/${enc(task.id)}/stop`), `Cancelled #${task.number}`);
    return;
  }
  const ok = await confirmDialog({
    title: `Stop task #${task.number}?`,
    message: 'The agent session is interrupted and its process tree is stopped. You can continue the task later.',
    confirmLabel: 'Stop task',
    danger: true,
  });
  if (ok) await taskPost(ctx, task, 'stop', () => ctx.api.post(`/api/tasks/${enc(task.id)}/stop`), `Stopping #${task.number}…`);
}

/** Resume a closed/stopped task on the same session lineage (TSK-007). */
export function continueTask(ctx, task) {
  return taskPost(ctx, task, 'continue', () => ctx.api.post(`/api/tasks/${enc(task.id)}/continue`, {}), `Continuing #${task.number}…`);
}

/** Ask the task's owner for control: the task would act on GitHub as you (SPEC-070). */
export function requestControl(ctx, task) {
  return taskPost(ctx, task, 'ask for control of', () => ctx.api.post(`/api/tasks/${enc(task.id)}/control-request`), `Asked for control of #${task.number}`);
}

/** Withdraw your request for control, or, as the owner, decline someone's. */
export function dropControlRequest(ctx, task) {
  return taskPost(ctx, task, 'answer the request for', () => ctx.api.del(`/api/tasks/${enc(task.id)}/control-request`));
}

/** As the owner, hand the task to the person who asked: it acts on GitHub as them from now on. */
export function handOverTask(ctx, task) {
  const to = task.control_request && task.control_request.by.login;
  return taskPost(ctx, task, 'hand over', () => ctx.api.post(`/api/tasks/${enc(task.id)}/control-request/accept`), to ? `#${task.number} is @${to}'s now` : null);
}

// The server closes a finished task's session before deleting it and may wait for the close's
// escalation (25 s at most, SPEC-039): the request outlasts it.
const DELETE_TIMEOUT_MS = 40000;

/**
 * Permanent deletion (TSK-009): an explicit confirmation; cannot be undone. A Done task's idle
 * session is closed first (SPEC-039).
 */
export async function deleteTask(ctx, task) {
  const closing = isLive(task.status) ? ' Its finished session is closed first.' : '';
  const ok = await confirmDialog({
    title: `Delete task #${task.number} permanently?`,
    message: `“${task.title}” and its history, messages, steps and metrics are deleted, and its local worktree and branch are removed, including uncommitted changes. Pushed branches and pull requests on GitHub stay.${closing} This cannot be undone.`,
    confirmLabel: 'Delete permanently',
    danger: true,
  });
  // One request per task: closing a Done task takes a few seconds, and a second confirmation
  // meanwhile would end in "not found" (SPEC-065 END-007).
  const key = `${task.id}:delete`;
  if (!ok || inFlight.has(key)) return;
  inFlight.add(key);
  const progress = toast(`Deleting #${task.number}…`, { timeout: 0 });
  try {
    await ctx.api.del(`/api/tasks/${enc(task.id)}`, { timeoutMs: DELETE_TIMEOUT_MS });
  } catch (error) {
    errorToast(error, `Couldn’t delete task #${task.number}`);
    return;
  } finally {
    inFlight.delete(key);
    progress();
  }
  ctx.store.removeTask(task.id, { byUser: true });
  toast(`Task #${task.number} deleted.`, { kind: 'ok' });
  const route = ctx.route();
  if (route && route.name === 'task' && route.number === task.number) {
    const sandbox = ctx.store.sandboxById(task.sandbox_id);
    if (sandbox) ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}`);
  }
}

/** Archive is a one-click action with Undo (UI-006). */
export async function archiveTask(ctx, task) {
  const updated = await taskPost(ctx, task, 'archive', () => ctx.api.post(`/api/tasks/${enc(task.id)}/archive`));
  if (updated) {
    ctx.store.removeTask(task.id, { byUser: true });
    toast(`Task #${task.number} archived.`, { kind: 'ok', action: { label: 'Undo', run: () => restoreTask(ctx, task) } });
    const route = ctx.route();
    if (route && route.name === 'task' && route.number === task.number) {
      const sandbox = ctx.store.sandboxById(task.sandbox_id);
      if (sandbox) ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}`);
    }
  }
}

/** Restore an archived task back to the sidebar. */
export async function restoreTask(ctx, task) {
  const updated = await taskPost(ctx, task, 'restore', () => ctx.api.post(`/api/tasks/${enc(task.id)}/restore`), `Restored #${task.number}`);
  return updated;
}

// "Archive all" sends at most this many archive requests at a time (SPEC-062 QAR-002).
const BULK_CONCURRENCY = 4;

/** Runs `run` on every item, at most BULK_CONCURRENCY at a time; resolves to the items that failed with their errors. */
async function eachBounded(items, run) {
  const queue = [...items];
  const failed = [];
  const worker = async () => {
    for (let item = queue.shift(); item; item = queue.shift()) {
      try {
        await run(item);
      } catch (error) {
        failed.push({ item, error });
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(BULK_CONCURRENCY, queue.length) }, worker));
  return failed;
}

/**
 * Archive several tasks in one go (SPEC-062 QAR-002, QAR-003): each through the per-task archive,
 * so the server checks each one; rows leave as they succeed; one toast with one Undo for them all,
 * and one error toast for those that failed.
 */
export async function archiveAll(ctx, tasks, what) {
  // A task already being archived (its row's button, its menu) is left to that request; the
  // others are marked in flight, so those buttons send nothing more meanwhile.
  const own = tasks.filter((task) => !inFlight.has(`${task.id}:archive`));
  own.forEach((task) => inFlight.add(`${task.id}:archive`));
  const archived = [];
  const failed = await eachBounded(own, async (task) => {
    try {
      const updated = await ctx.api.post(`/api/tasks/${enc(task.id)}/archive`);
      if (updated && updated.id) ctx.store.upsertTask(updated);
      ctx.store.removeTask(task.id, { byUser: true });
      archived.push(task);
    } finally {
      inFlight.delete(`${task.id}:archive`);
    }
  });
  if (archived.length) {
    toast(`Archived ${counted(archived.length, what)}.`, { kind: 'ok', action: { label: 'Undo', run: () => restoreAll(ctx, archived) } });
    // The tasks are one sandbox's (one sidebar group): leave the page of one that was archived.
    const route = ctx.route();
    const sandbox = ctx.store.sandboxById(archived[0].sandbox_id);
    if (sandbox && route && route.name === 'task' && route.slug === sandbox.slug && archived.some((task) => task.number === route.number)) {
      ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}`);
    }
  }
  if (failed.length) errorToast(failed[0].error, `Couldn’t archive ${failed.length} of ${counted(own.length, 'task')}`);
}

/** Undo of "Archive all": restores the tasks it archived, with one toast. */
async function restoreAll(ctx, tasks) {
  const failed = await eachBounded(tasks, async (task) => {
    const updated = await ctx.api.post(`/api/tasks/${enc(task.id)}/restore`);
    if (updated && updated.id) ctx.store.upsertTask(updated);
  });
  const restored = tasks.length - failed.length;
  if (restored) toast(`Restored ${counted(restored, 'task')}.`, { kind: 'ok' });
  if (failed.length) errorToast(failed[0].error, `Couldn’t restore ${failed.length} of ${counted(tasks.length, 'task')}`);
}

/** "1 task", "2 tasks". */
const counted = (n, noun) => `${n} ${noun}${n === 1 ? '' : 's'}`;

// ---------------------------------------------------------------- feedback

/** Was the task done correctly? (ANL-002) */
export function openFeedbackDialog(ctx, task) {
  const verdicts = [['correct', 'Done correctly'], ['incorrect', 'Not correct']].map(([value, label]) => ({ value, label, input: h('input', { type: 'radio', name: 'verdict', value }) }));
  const comment = h('textarea', { class: 'textarea', rows: 4, placeholder: 'What went well or wrong? (optional)', maxlength: 4000 });
  let submit;
  const update = () => { if (submit) submit.disabled = !verdicts.some((v) => v.input.checked); };
  verdicts.forEach((v) => v.input.addEventListener('change', update));
  openModal({
    title: `Feedback on #${task.number} ${task.title}`,
    body: h('div', { class: 'stack' },
      h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Verdict' }, verdicts.map((v) => h('label', null, v.input, v.label))),
      field('Comment', comment)),
    actions: [
      { label: 'Cancel', value: false },
      { label: 'Send feedback', kind: 'primary', ref: (b) => { submit = b; b.disabled = true; }, run: async () => {
        const verdict = verdicts.find((v) => v.input.checked).value;
        await ctx.api.post(`/api/tasks/${enc(task.id)}/feedback`, { verdict, comment: comment.value.trim() });
        toast('Thanks — feedback recorded.', { kind: 'ok' });
      } },
    ],
  });
}

// ---------------------------------------------------------------- delivery

const STAGES = [
  ['none', 'Being built'],
  ['pr_open', 'In review'],
  ['merged_integration', 'Merged, not live'],
  ['env_done', 'Verified'],
  ['promoted', 'Released'],
  ['production_done', 'Live'],
];

/** A delivery stage in words ("In review", "Live"). */
export function stageLabel(stage) {
  return (STAGES.find(([s]) => s === stage) || [null, stage || 'Unknown'])[1];
}

/** Why "checks unknown" (#111): unreadable checks never count as passed. */
export const UNKNOWN_CHECKS = `${BRAND.name} can’t read this pull request’s checks, so they don’t count as passed: merges that require checks wait.`;

const CHECKS = {
  success: ['checks passed', 'ok'],
  failure: ['checks failed', 'danger'],
  pending: ['checks running', 'warn'],
  none: ['no checks', ''],
  unknown: ['checks unknown', 'warn'],
};

/** PR line with state and checks badges (`checks: unknown` when GitHub could not be read). */
export function prLine(pr) {
  if (!pr) return h('span', { class: 'muted' }, 'No pull request');
  const checks = CHECKS[pr.checks] || [pr.checks || 'checks unknown', 'warn'];
  const state = { open: 'accent', merged: 'violet', closed: '' }[pr.state] || '';
  return h('span', { class: 'row-wrap' },
    h('a', { href: pr.url, target: '_blank', rel: 'noopener noreferrer' }, `#${pr.number}`),
    h('span', { class: 'ellipsis' }, pr.title),
    badge(pr.state, state),
    badge(checks[0], checks[1], pr.checks === 'unknown' ? UNKNOWN_CHECKS : null),
    h('span', { class: 'muted small mono' }, `${pr.base} ← ${pr.head}`));
}

const CHECK_ICONS = { success: 'check', failure: 'x', pending: 'clock' };

/**
 * The task view's delivery line (UX2-18): the pull request with its checks, opening the delivery
 * details. Status only: the agent opens, merges and releases it (GIT-002). `reload` reads the
 * delivery again after the details close.
 */
export function deliveryStrip(ctx, task, delivery, { reload = () => {} } = {}) {
  if (!delivery) return [];
  const pr = delivery.pr;
  const out = [];
  if (pr) {
    const checks = pr.state === 'open' ? CHECKS[pr.checks] : null;
    const glyph = checks ? CHECK_ICONS[pr.checks] : null;
    const label = `Pull request #${pr.number}, ${pr.state}${checks ? `, ${checks[0]}` : ''} — delivery details`;
    out.push(h('button', { type: 'button', class: ['tl-pr-chip', 'strip-pr', pr.state === 'merged' && 'merged', pr.state === 'closed' && 'closed'], title: label, 'aria-label': label, onClick: () => openDeliveryDialog(ctx, task).result.then(reload) },
      icon('branch', { size: 12 }), `#${pr.number}`, glyph ? h('span', { class: `tone-${checks[1]}` }, icon(glyph, { size: 12 })) : null));
  }
  return out;
}

const CHECKS_ACCESS_KEY = 'yulius.checks-access.dismissed';

/**
 * The GitHub check results the App can't read for a repository and the permission it needs
 * (`checks_access`, #111): one quiet line for instance administrators, who can change the App,
 * with a link to its permission page. Dismissed, it stays hidden for this browser session.
 * Everyone else sees "checks unknown" on a pull request only when nothing could be read.
 */
export function checksAccessNote(ctx, access) {
  if (!access || !access.message || !(ctx.isInstanceAdmin && ctx.isInstanceAdmin())) return null;
  const key = `${CHECKS_ACCESS_KEY}:${access.repo}:${(access.missing || []).join(',')}`;
  try { if (sessionStorage.getItem(key)) return null; } catch { /* storage off: show it */ }
  const note = h('div', { class: 'checks-access small', role: 'note' },
    icon('key', { size: 14 }),
    h('span', { class: 'checks-access-text' }, access.message,
      access.settings_url ? [' ', h('a', { href: access.settings_url, target: '_blank', rel: 'noopener noreferrer' }, 'Open the App’s permissions')] : null),
    iconButton('x', 'Dismiss until the next session', () => {
      try { sessionStorage.setItem(key, '1'); } catch { /* hidden for this page only */ }
      note.remove();
    }));
  return note;
}

/** Notes from Git/GitHub reads and policy outcomes (reads degrade instead of failing). */
export function notesBanner(notes) {
  const list = Array.isArray(notes) ? notes.filter(Boolean) : [];
  if (list.length === 0) return null;
  return h('div', { class: 'banner banner-warn small' }, icon('warn', { size: 14 }),
    h('div', { class: 'banner-text stack-sm' }, list.map((note) => h('span', null, note))));
}

/** Label for a delivery step's wire name (`run_env:<name>`, `confirm_env:<name>`, …). */
export function actionLabel(action) {
  if (action === 'open_pr') return 'Open the pull request';
  if (action === 'merge') return 'Merge';
  if (action === 'promote') return 'Release to production';
  if (action.startsWith('run_env:')) return `Deploy to ${action.slice('run_env:'.length)}`;
  if (action.startsWith('confirm_env:')) return `Verify ${action.slice('confirm_env:'.length)}`;
  return action;
}

/**
 * Delivery details of one task (DLV, GIT-002): stage, pull request and checks, spec, what comes
 * next and what blocks it. Read-only: the agent delivers the task through its workflow.
 */
export function openDeliveryDialog(ctx, task) {
  const modal = openModal({ title: `Delivery · #${task.number} ${task.title}`, body: placeholder('Loading delivery…', 'loading'), wide: true, actions: [{ label: 'Close' }] });
  const load = async () => {
    try {
      const delivery = await ctx.api.get(`/api/tasks/${enc(task.id)}/delivery`);
      setChildren(modal.body, renderDelivery(delivery || {}));
    } catch (error) {
      setChildren(modal.body, placeholder(error.message, 'error'));
    }
  };
  const renderDelivery = (d) => {
    const stageIndex = STAGES.findIndex(([s]) => s === d.stage);
    const next = Array.isArray(d.next_actions) ? d.next_actions : [];
    const blocked = Array.isArray(d.blocked) ? d.blocked : [];
    return h('div', { class: 'stack' },
      h('div', { class: 'stage-track' }, STAGES.map(([stage, label], i) => h('span', { class: ['stage', i < stageIndex && 'done', i === stageIndex && 'current'] },
        i < stageIndex ? icon('check', { size: 11 }) : null,
        stage === 'env_done' && d.stage === 'env_done' && d.environment ? `${label}: ${d.environment}` : label))),
      stageIndex < 0 ? h('div', { class: 'muted small' }, `Stage: ${d.stage || 'unknown'}`) : null,
      notesBanner(d.notes),
      checksAccessNote(ctx, d.checks_access),
      h('div', { class: 'card card-pad stack-sm' },
        h('div', { class: 'row' }, h('strong', null, 'Pull request')),
        prLine(d.pr)),
      h('div', { class: 'card card-pad stack-sm' },
        h('div', { class: 'row' }, h('strong', null, 'Task spec'), d.spec_valid ? badge('valid', 'ok') : badge('needs work', 'danger')),
        d.spec_path ? h('div', { class: 'muted small' }, h('code', null, d.spec_path)) : h('div', { class: 'muted small' }, 'No spec registered for this task yet.'),
        d.spec_valid ? null : h('ul', { class: 'small list-plain' }, (d.spec_problems || []).map((p) => h('li', null, p))),
        d.steps_open ? h('div', { class: 'banner banner-warn small' }, icon('warn', { size: 14 }), `${d.steps_open} step${d.steps_open === 1 ? '' : 's'} still open in the plan.`) : null),
      h('div', { class: 'card card-pad stack-sm' },
        h('div', { class: 'row' }, h('strong', null, 'Next')),
        next.length || blocked.length
          ? h('ul', { class: 'small list-plain' },
            next.map((action) => h('li', { class: 'row-wrap' }, h('span', { class: 'tone-ok' }, icon('check', { size: 12 })), `${actionLabel(action)}: ready`)),
            blocked.map((b) => h('li', { class: 'row-wrap' }, h('span', { class: 'tone-warn' }, icon('clock', { size: 12 })), h('span', null, h('strong', null, `${actionLabel(b.action)}: `), b.reason))))
          : h('span', { class: 'muted small' }, 'Nothing is pending at this stage.')),
      h('p', { class: 'muted xs' }, 'Agents open, merge, deploy and release as part of their workflow, following this sandbox’s delivery profile. This shows where the task is.'),
    );
  };
  load();
  return modal;
}

/**
 * The prompt a task started with (SPEC-043): its text formatted as written, then the files
 * attached to it. An image shows as a thumbnail that opens full size, another file downloads, a
 * file that is gone (drafts are removed after a week) is named. Read when the dialog opens.
 */
export function openPromptDialog(ctx, task) {
  const controller = new AbortController();
  const urls = [];
  let editor = null;
  let text = '';
  const modal = openModal({
    title: `Original prompt · #${task.number}`,
    body: placeholder('Loading the prompt…', 'loading'),
    wide: true,
    className: 'prompt-modal',
    actions: [{ label: 'Close' }],
    onClose: () => {
      controller.abort();
      urls.forEach((url) => URL.revokeObjectURL(url));
      if (editor) editor.destroy();
    },
  });
  const copy = {
    label: 'Copy text',
    run: async () => {
      if (!navigator.clipboard) throw new Error('Copying needs a secure (HTTPS) page.');
      await navigator.clipboard.writeText(text);
      toast('Prompt copied.', { kind: 'ok' });
      return false;
    },
  };
  const fileUrl = (index) => `/api/tasks/${enc(task.id)}/prompt/attachments/${index}`;
  // An attached file can be as large as an upload (25 MiB by default): longer than a JSON read.
  const FILE_TIMEOUT_MS = 120000;
  const fileItem = (file, index) => {
    if (!file.available) {
      return h('li', { class: 'prompt-file gone' }, icon('file', { size: 16 }), h('span', { class: 'prompt-file-name' }, file.name), h('span', { class: 'muted xs' }, 'No longer on the server'));
    }
    if (!file.image) {
      const download = async () => {
        try {
          const url = URL.createObjectURL(await ctx.api.get(fileUrl(index), { as: 'blob', signal: controller.signal, timeoutMs: FILE_TIMEOUT_MS }));
          urls.push(url);
          const link = h('a', { href: url, download: file.name, hidden: true });
          document.body.append(link);
          link.click();
          link.remove();
        } catch (error) {
          if (error.code !== 'aborted') errorToast(error, `Couldn’t download ${file.name}`);
        }
      };
      return h('li', { class: 'prompt-file' }, h('button', { type: 'button', class: 'btn btn-ghost btn-sm', 'aria-label': `Download ${file.name}`, onClick: download },
        icon('download', { size: 16 }), h('span', { class: 'prompt-file-name' }, file.name)));
    }
    const img = h('img', { alt: file.name });
    const item = h('li', { class: 'prompt-file image' },
      h('button', { type: 'button', class: 'prompt-thumb', 'aria-label': `Open ${file.name}`, onClick: () => img.src && openAttachment({ name: file.name }, { image: img.src }) }, img),
      h('span', { class: 'prompt-file-name' }, file.name));
    ctx.api.get(fileUrl(index), { as: 'blob', signal: controller.signal, timeoutMs: FILE_TIMEOUT_MS }).then((blob) => {
      const url = URL.createObjectURL(blob);
      urls.push(url);
      img.src = url;
    }).catch((error) => {
      if (error.code !== 'aborted') item.replaceChildren(icon('file', { size: 16 }), h('span', { class: 'prompt-file-name' }, file.name), h('span', { class: 'muted xs' }, 'Couldn’t load it'));
    });
    return item;
  };
  (async () => {
    let prompt;
    try {
      prompt = await ctx.api.get(`/api/tasks/${enc(task.id)}/prompt`, { signal: controller.signal });
    } catch (error) {
      if (error.code !== 'aborted') setChildren(modal.body, placeholder(error.message, 'error'));
      return;
    }
    text = prompt.text || '';
    const files = Array.isArray(prompt.attachments) ? prompt.attachments : [];
    if (text) editor = createMarkdownEditor({ markdown: text, readOnly: true, frontmatter: false });
    setChildren(modal.body, h('div', { class: 'stack' },
      editor ? h('div', { class: 'prompt-text' }, editor.el) : h('p', { class: 'muted' }, 'No text: only files were attached.'),
      files.length ? h('ul', { class: 'prompt-files', 'aria-label': 'Attached files' }, files.map(fileItem)) : null));
    modal.setActions(text ? [copy, { label: 'Close' }] : [{ label: 'Close' }]);
  })();
  return modal;
}

const RUN_POLL_MS = 2000;
const RUN_POLL_MAX = 900; // 30 minutes, the server's run limit
const RUN_DONE = new Set(['succeeded', 'failed', 'error']);
const RUN_TONE = { running: 'warn', succeeded: 'ok', failed: 'danger', error: 'danger' };

/** Badge for an EnvironmentRun state. */
export function runStateBadge(state) {
  return badge(state || 'unknown', RUN_TONE[state] || '');
}

/** Poll an environment run and show its output until it finishes (bounded). */
export function openRunViewer(ctx, sandbox, runId, envName) {
  const stateEl = runStateBadge('running');
  const meta = h('span', { class: 'muted small' });
  const output = h('pre', { class: 'code-view run-output', 'aria-live': 'polite' }, '');
  const exitEl = h('span', { class: 'muted small' });
  let polls = 0;
  let timer = null;
  const modal = openModal({
    title: `Deploy to ${envName}`,
    wide: true,
    body: h('div', { class: 'stack' }, h('div', { class: 'row-wrap' }, stateEl, exitEl, meta), output),
    actions: [{ label: 'Close' }],
    onClose: () => clearTimeout(timer),
  });
  const poll = async () => {
    polls += 1;
    try {
      const run = await ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/delivery/runs/${enc(runId)}`);
      setChildren(stateEl, run.state || 'unknown');
      stateEl.className = `badge badge-${RUN_TONE[run.state] || 'warn'}`;
      setChildren(meta, [run.kind === 'manual' ? 'Marked verified' : 'Deploy', run.commit ? `of ${String(run.commit).slice(0, 7)}` : null, run.requested_by ? `by ${run.requested_by}` : null, run.started_at ? `started ${relLong(run.started_at)}` : null].filter(Boolean).join(' · '));
      const atBottom = output.scrollTop + output.clientHeight >= output.scrollHeight - 8;
      output.textContent = run.output || (run.kind === 'manual' ? 'Manual confirmation (no command output).' : '');
      if (atBottom) output.scrollTop = output.scrollHeight;
      exitEl.textContent = run.exit_code === null || run.exit_code === undefined ? '' : `exit code ${run.exit_code}`;
      if (RUN_DONE.has(run.state)) return;
    } catch (error) {
      exitEl.textContent = `poll failed: ${error.message}`;
    }
    if (polls < RUN_POLL_MAX && modal.dialog.isConnected) timer = setTimeout(poll, RUN_POLL_MS);
    else if (polls >= RUN_POLL_MAX) exitEl.textContent = 'Stopped watching after 30 minutes; the run continues on the server.';
  };
  poll();
  return modal;
}

// ---------------------------------------------------------------- sandboxes

const REPO_NAME_RE = /^[A-Za-z0-9._-]{1,100}$/;

/** "+" dialog: import an existing repository or create a new one from a template. */
export function openSandboxDialog(ctx, mode = 'import') {
  // Create mode is for instance administrators (docs/api.md "Sandboxes").
  const canCreate = ctx.isInstanceAdmin();
  const startMode = mode === 'create' && canCreate ? 'create' : 'import';
  const tabImport = h('input', { type: 'radio', name: 'sb-mode', value: 'import', checked: startMode === 'import' });
  const tabCreate = h('input', { type: 'radio', name: 'sb-mode', value: 'create', checked: startMode === 'create', disabled: !canCreate });
  const content = h('div', { class: 'stack' });
  const body = h('div', { class: 'stack' },
    h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Mode' }, h('label', null, tabImport, 'Import repository'), h('label', { title: canCreate ? null : 'Only instance administrators can create sandboxes from a template' }, tabCreate, 'Create from template')),
    content);
  let submitButton;
  let selectedRepo = null;
  let repos = null;
  let templates = null;
  const nameInput = h('input', { class: 'input', placeholder: 'my-sandbox', maxlength: 100, autocomplete: 'off' });
  const descInput = h('input', { class: 'input', placeholder: 'One sentence about the sandbox (optional)', maxlength: 300 });
  const nameError = h('span', { class: 'field-error' });
  let templateValue = null;

  const updateSubmit = () => {
    if (!submitButton) return;
    if (tabImport.checked) {
      submitButton.textContent = 'Import';
      submitButton.disabled = !selectedRepo;
    } else {
      submitButton.textContent = 'Create';
      const valid = REPO_NAME_RE.test(nameInput.value.trim());
      nameError.textContent = nameInput.value && !valid ? 'Use letters, digits, “.”, “-” or “_”.' : '';
      nameInput.setAttribute('aria-invalid', nameInput.value && !valid ? 'true' : 'false');
      submitButton.disabled = !valid || !templateValue;
    }
  };

  const renderImport = async () => {
    const search = h('input', { class: 'input', type: 'search', placeholder: 'Filter repositories…', 'aria-label': 'Filter repositories' });
    const list = h('div', { class: 'card list scroll-box' }, placeholder('Loading repositories…', 'loading'));
    setChildren(content, search, list, h('p', { class: 'muted xs' }, `Importing needs admin or maintain permission on the repository, which must belong to the instance owner. ${BRAND.name} clones it and checks the documentation format; a sandbox that does not follow it gets a preparation task first.`));
    const draw = () => {
      if (!repos) return;
      const q = search.value.trim().toLowerCase();
      const shown = repos.filter((r) => `${r.owner}/${r.name}`.toLowerCase().includes(q));
      if (shown.length === 0) { setChildren(list, placeholder(repos.length ? 'No repository matches.' : 'No repositories visible to you.')); return; }
      setChildren(list, shown.map((r) => {
        const full = `${r.owner}/${r.name}`;
        // GitHub "maintain" is listed as Yulius `write` but may import; the server decides.
        const disabled = r.already_sandbox || r.access === 'read';
        const radio = h('input', { type: 'radio', name: 'sb-repo', value: full, disabled, checked: selectedRepo === full });
        radio.addEventListener('change', () => { selectedRepo = full; updateSubmit(); });
        return h('label', { class: ['list-item', 'choice', disabled && 'disabled'] },
          radio,
          h('div', { class: 'list-main' }, h('div', { class: 'list-title' }, full, r.private ? badge('private') : badge('public', 'accent'))),
          r.already_sandbox ? badge('already a sandbox', 'ok') : badge(r.access, r.access === 'admin' ? '' : 'warn', r.access === 'admin' ? null : r.access === 'write' ? 'Import needs admin or maintain permission on GitHub' : 'Admin or maintain permission required'));
      }));
    };
    search.addEventListener('input', draw);
    if (!repos) {
      try {
        repos = await ctx.api.get('/api/github/repositories');
      } catch (error) {
        // 403: no GitHub-linked account (never for the superadmin, ACC-012); 409: no GitHub App.
        // The server's message says which.
        setChildren(list, placeholder(error.message, 'error'));
        return;
      }
    }
    draw();
    search.focus();
  };

  const renderCreate = async () => {
    const templateList = h('div', { class: 'card list' }, placeholder('Loading templates…', 'loading'));
    setChildren(content,
      field('Repository name', nameInput, 'Created under the instance’s GitHub owner.'), nameError,
      field('Description', descInput),
      h('div', { class: 'field' }, h('span', { class: 'field-label' }, 'Template'), templateList),
      h('p', { class: 'muted xs' }, `Instance administrators only. ${BRAND.name} creates a private GitHub repository, pushes the template to main and dev, adds rulesets requiring pull requests and opens the new tab.`));
    if (!templates) {
      try { templates = await ctx.api.get('/api/templates'); } catch (error) { setChildren(templateList, placeholder(error.message, 'error')); return; }
    }
    if (!templateValue && templates.length) templateValue = templates[0].id;
    setChildren(templateList, templates.map((t) => {
      const radio = h('input', { type: 'radio', name: 'sb-template', value: t.id, checked: t.id === templateValue });
      radio.addEventListener('change', () => { templateValue = t.id; updateSubmit(); });
      return h('label', { class: 'list-item choice' }, radio, h('div', { class: 'list-main' }, h('div', { class: 'list-title' }, t.name), h('div', { class: 'list-sub wrap-text' }, t.description)));
    }));
    updateSubmit();
    nameInput.focus();
  };

  nameInput.addEventListener('input', updateSubmit);
  const switchMode = () => { (tabImport.checked ? renderImport : renderCreate)(); updateSubmit(); };
  tabImport.addEventListener('change', switchMode);
  tabCreate.addEventListener('change', switchMode);

  openModal({
    title: 'Add sandbox',
    wide: true,
    body,
    actions: [
      { label: 'Cancel', value: false },
      { label: 'Import', kind: 'primary', ref: (b) => { submitButton = b; }, run: async () => {
        const payload = tabImport.checked
          ? { mode: 'import', repo: selectedRepo }
          : { mode: 'create', name: nameInput.value.trim(), template: templateValue, description: descInput.value.trim() };
        const sandbox = await ctx.api.post('/api/sandboxes', payload);
        ctx.store.upsertSandbox(sandbox);
        const preparing = sandbox.onboarding === 'pending' || sandbox.onboarding === 'preparing';
        toast(`${tabImport.checked ? 'Imported' : 'Created'} ${sandbox.name}${preparing ? ' — it needs preparation before tasks can start' : ''}`, { kind: 'ok' });
        ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}`);
      } },
    ],
  });
  switchMode();
}

/** Remove a sandbox from Yulius; the GitHub repository is untouched (PRJ-009). */
export async function confirmRemoveSandbox(ctx, sandbox) {
  const ok = await confirmDialog({
    title: `Remove ${sandbox.name} from ${BRAND.name}?`,
    message: `Live and queued tasks are stopped. The GitHub repository, the server’s clone and the task records are kept; the sandbox disappears from ${BRAND.name} for everyone.`,
    typeToConfirm: sandbox.name,
    confirmLabel: 'Remove sandbox',
    danger: true,
  });
  if (!ok) return;
  try {
    await ctx.api.del(`/api/sandboxes/${enc(sandbox.id)}`);
    ctx.store.removeSandbox(sandbox.id);
    toast(`Sandbox ${sandbox.name} removed.`, { kind: 'ok' });
    const current = ctx.currentSandbox();
    if (!current || current.id === sandbox.id) ctx.navigate('/');
  } catch (error) {
    errorToast(error, 'Couldn’t remove sandbox');
  }
}
