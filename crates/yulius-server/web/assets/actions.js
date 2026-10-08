// Task actions (UI-013, §6.3) and shared dialogs: switch, feedback, delivery, deploy
// runs, promotion, sandbox create/import and removal.

import { enc } from './api.js';
import { h, openModal, confirmDialog, openMenu, toast, errorToast, field, badge, setChildren, icon, placeholder, linkify } from './dom.js';
import { taskActions, statusInfo, allows } from './status.js';
import { relLong, dateTime } from './format.js';
import { BRAND } from './brand.js';

// ---------------------------------------------------------------- task menu

/** Kebab / right-click menu for a task. */
export function openTaskMenu(ctx, anchor, task) {
  const sandbox = ctx.store.sandboxById(task.sandbox_id);
  const access = sandbox ? sandbox.access : 'read';
  const can = taskActions(task, access);
  const why = allows(access, 'write') ? 'Not available in this state' : 'Requires write access on GitHub';
  openMenu(anchor, [
    { header: `#${task.number} · ${statusInfo(task).label}` },
    { label: 'Stop', disabled: !can.stop, title: can.stop ? null : why, onSelect: () => stopTask(ctx, task) },
    { label: 'Continue', disabled: !can.continue, title: can.continue ? null : why, onSelect: () => continueTask(ctx, task) },
    { label: 'Switch account…', disabled: !can.switch, title: can.switch ? null : why, onSelect: () => openSwitchDialog(ctx, task) },
    { label: 'Deliver…', disabled: !can.deliver, title: can.deliver ? null : why, onSelect: () => openDeliveryDialog(ctx, task) },
    { label: 'Feedback…', disabled: !can.feedback, onSelect: () => openFeedbackDialog(ctx, task) },
    { separator: true },
    task.archived
      ? { label: 'Restore', disabled: !can.restore, onSelect: () => restoreTask(ctx, task) }
      : { label: 'Archive', disabled: !can.archive, title: can.archive ? null : (allows(access, 'write') ? 'Stop the task before archiving' : why), onSelect: () => archiveTask(ctx, task) },
    { label: 'Delete permanently…', danger: true, disabled: !can.delete, title: can.delete ? null : (allows(access, 'write') ? 'Stop the task before deleting it' : why), onSelect: () => deleteTask(ctx, task) },
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
    errorToast(error, `Could not ${verb} task #${task.number}`);
    return null;
  } finally {
    inFlight.delete(key);
  }
}

/** Stop a live task (confirmation: stopping interrupts the agent). */
export async function stopTask(ctx, task) {
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

/** Permanent deletion (TSK-009): typed confirmation; cannot be undone. */
export async function deleteTask(ctx, task) {
  const ok = await confirmDialog({
    title: `Delete task #${task.number} permanently?`,
    message: `"${task.title}" and its history, messages, steps and metrics are deleted, and its local worktree and branch are removed, including uncommitted changes. Pushed branches and pull requests on GitHub stay. This cannot be undone.`,
    confirmLabel: 'Delete permanently',
    danger: true,
  });
  if (!ok) return;
  try {
    await ctx.api.del(`/api/tasks/${enc(task.id)}`);
  } catch (error) {
    errorToast(error, `Could not delete task #${task.number}`);
    return;
  }
  ctx.store.removeTask(task.id, { byUser: true });
  toast(`Deleted #${task.number} ${task.title}`, { kind: 'ok' });
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
    toast(`Archived #${task.number} ${task.title}`, { kind: 'ok', action: { label: 'Undo', run: () => restoreTask(ctx, task) } });
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

// ---------------------------------------------------------------- switch

/** Switch to another Claude account; for an ended task this continues it on that account. */
export async function openSwitchDialog(ctx, task) {
  const live = task.status === 'running' || task.status === 'waiting_capacity';
  let accounts = [];
  try {
    accounts = await ctx.api.get('/api/admin/accounts', { allow401: true });
  } catch {
    accounts = []; // Only instance admins can list accounts; others let rotation choose.
  }
  const accountSelect = h('select', { class: 'select' },
    h('option', { value: '' }, 'Automatic (rotation picks the best account)'),
    // Accounts that cannot take the task are listed but disabled, with the reason in words.
    (Array.isArray(accounts) ? accounts : []).map((a) => {
      const used = a.usage && a.usage.primary && Number.isFinite(a.usage.primary.used_percent) ? Math.round(a.usage.primary.used_percent) : null;
      const why = !a.enabled ? 'turned off' : a.status === 'relogin' || a.status === 'pending_login' ? 'sign in again first' : a.status === 'limited' || (used !== null && used >= 99) ? 'limit reached' : a.status === 'error' ? 'not working' : null;
      const label = `${a.label}${why ? ` — ${why}` : used !== null ? ` · ${used}% used` : ''}`;
      return h('option', { value: a.id, disabled: Boolean(why) }, label);
    }));
  const body = h('div', { class: 'stack' },
    h('p', { class: 'text-2' }, live
      ? 'The session moves to the chosen account at the next turn boundary and resumes the same conversation.'
      : 'The task is not running; it will continue on the chosen account.'),
    field('Account', accountSelect, accounts.length ? null : 'Only instance administrators can pick a specific account.'),
  );
  openModal({
    title: `Switch task #${task.number}`,
    body,
    actions: [
      { label: 'Cancel', value: false },
      { label: live ? 'Switch' : 'Continue on account', kind: 'primary', run: async () => {
        const payload = {};
        if (accountSelect.value) payload.account_id = accountSelect.value;
        const updated = live
          ? await ctx.api.post(`/api/tasks/${enc(task.id)}/switch`, payload)
          : await ctx.api.post(`/api/tasks/${enc(task.id)}/continue`, payload);
        if (updated && updated.id) ctx.store.upsertTask(updated);
        toast(live ? `Switching #${task.number}…` : `Continuing #${task.number}…`, { kind: 'ok' });
      } },
    ],
  });
}

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
    badge(checks[0], checks[1], pr.checks === 'unknown' ? 'GitHub checks could not be read' : null),
    h('span', { class: 'muted small mono' }, `${pr.base} ← ${pr.head}`));
}

const CHECK_ICONS = { success: 'check', failure: 'x', pending: 'clock' };

/**
 * The task view's delivery line (UX2-18): the pull request with its checks, opening Deliver…, and
 * the next step as a button when it is to open the pull request (once the task is closed) or to
 * merge. `canAct` hides the buttons from read-only members; `reload` reads the delivery again.
 */
export function deliveryStrip(ctx, task, delivery, { canAct = true, reload = () => {} } = {}) {
  if (!delivery) return [];
  const pr = delivery.pr;
  const next = Array.isArray(delivery.next_actions) ? delivery.next_actions : [];
  const done = task.status === 'closed'; // as the Git page: a pull request once the task is finished
  const out = [];
  if (pr) {
    const checks = pr.state === 'open' ? CHECKS[pr.checks] : null;
    const glyph = checks ? CHECK_ICONS[pr.checks] : null;
    const label = `Pull request #${pr.number}, ${pr.state}${checks ? `, ${checks[0]}` : ''} — delivery details`;
    out.push(h('button', { type: 'button', class: ['tl-pr-chip', 'strip-pr', pr.state === 'merged' && 'merged', pr.state === 'closed' && 'closed'], title: label, 'aria-label': label, onClick: () => openDeliveryDialog(ctx, task).result.then(reload) },
      icon('branch', { size: 12 }), `#${pr.number}`, glyph ? h('span', { class: `tone-${checks[1]}` }, icon(glyph, { size: 12 })) : null));
  }
  if (canAct && next.includes('merge')) out.push(h('button', { type: 'button', class: 'btn btn-sm strip-extra', onClick: () => mergeTask(ctx, task, reload) }, 'Merge…'));
  else if (canAct && done && next.includes('open_pr')) out.push(h('button', { type: 'button', class: 'btn btn-sm strip-extra', onClick: () => openPrDialog(ctx, task, reload) }, 'Open pull request…'));
  return out;
}

/** Notes from Git/GitHub reads and policy outcomes (reads degrade instead of failing). */
export function notesBanner(notes) {
  const list = Array.isArray(notes) ? notes.filter(Boolean) : [];
  if (list.length === 0) return null;
  return h('div', { class: 'banner banner-warn small' }, icon('warn', { size: 14 }),
    h('div', { class: 'banner-text stack-sm' }, list.map((note) => h('span', null, note))));
}

/** Label for a delivery action wire name (`run_env:<name>`, `confirm_env:<name>`, …). */
export function actionLabel(action) {
  if (action === 'open_pr') return 'Open pull request';
  if (action === 'merge') return 'Merge';
  if (action === 'promote') return 'Release to production';
  if (action.startsWith('run_env:')) return `Deploy to ${action.slice('run_env:'.length)}`;
  if (action.startsWith('confirm_env:')) return `Mark ${action.slice('confirm_env:'.length)} verified`;
  return action;
}

/** Delivery panel for one task: stage, spec validity, PR, next and blocked actions (DLV, GIT-002). */
export function openDeliveryDialog(ctx, task) {
  const sandbox = ctx.store.sandboxById(task.sandbox_id);
  const modal = openModal({ title: `Deliver #${task.number} ${task.title}`, body: placeholder('Loading delivery…', 'loading'), wide: true, actions: [{ label: 'Close' }] });
  const load = async () => {
    try {
      const delivery = await ctx.api.get(`/api/tasks/${enc(task.id)}/delivery`);
      setChildren(modal.body, renderDelivery(delivery || {}));
    } catch (error) {
      setChildren(modal.body, placeholder(error.message, 'error'));
    }
  };
  const actionButton = (action, primary) => {
    const label = actionLabel(action);
    const cls = ['btn', primary && 'btn-primary'];
    if (action === 'open_pr') return h('button', { type: 'button', class: cls, onClick: () => openPrDialog(ctx, task, load) }, label);
    if (action === 'merge') return h('button', { type: 'button', class: cls, onClick: () => mergeTask(ctx, task, load) }, label);
    if (action === 'promote') return h('button', { type: 'button', class: cls, onClick: async () => { if (sandbox) { await promote(ctx, sandbox); load(); } } }, label);
    if (action.startsWith('run_env:')) {
      const env = action.slice('run_env:'.length);
      return h('button', { type: 'button', class: cls, onClick: async () => { if (sandbox) { await runEnvironment(ctx, sandbox, env); load(); } } }, label);
    }
    if (action.startsWith('confirm_env:')) {
      const env = action.slice('confirm_env:'.length);
      return h('button', { type: 'button', class: cls, onClick: async () => { if (sandbox) { await confirmEnvironment(ctx, sandbox, env); load(); } } }, label);
    }
    return h('button', { type: 'button', class: 'btn', disabled: true, title: 'This version of the UI does not know this action' }, label);
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
      h('div', { class: 'card card-pad stack-sm' },
        h('div', { class: 'row' }, h('strong', null, 'Pull request')),
        prLine(d.pr)),
      h('div', { class: 'card card-pad stack-sm' },
        h('div', { class: 'row' }, h('strong', null, 'Task spec'), d.spec_valid ? badge('valid', 'ok') : badge('needs work', 'danger')),
        d.spec_path ? h('div', { class: 'muted small' }, h('code', null, d.spec_path)) : h('div', { class: 'muted small' }, 'No spec registered for this task yet.'),
        d.spec_valid ? null : h('ul', { class: 'small list-plain' }, (d.spec_problems || []).map((p) => h('li', null, p))),
        d.steps_open ? h('div', { class: 'banner banner-warn small' }, icon('warn', { size: 14 }), `${d.steps_open} step${d.steps_open === 1 ? '' : 's'} still open in the plan.`) : null),
      h('div', { class: 'row-wrap' },
        next.map((action, i) => actionButton(action, i === 0)),
        blocked.map((b) => h('button', { type: 'button', class: 'btn', disabled: true, title: b.reason }, actionLabel(b.action))),
        next.length || blocked.length ? null : h('span', { class: 'muted' }, 'No delivery action applies at this stage.')),
      blocked.length ? h('ul', { class: 'small muted list-plain' },
        blocked.map((b) => h('li', null, h('strong', null, `${actionLabel(b.action)}: `), b.reason))) : null,
      h('p', { class: 'muted xs' }, 'Merges, deploys and promotions only happen on your explicit action (or an explicit profile policy such as automatic promotion).'),
    );
  };
  load();
  return modal;
}

/** Merge a task's pull request into the integration branch after confirmation. */
export async function mergeTask(ctx, task, onDone = () => {}) {
  // Say where the change ends up: with automatic release, merging puts it live (UX2-04).
  const profile = (ctx.store.sandboxById(task.sandbox_id) || {}).profile || {};
  const int = profile.integration_branch || 'the integration branch';
  const prod = profile.production_branch || 'production';
  const live = Boolean(profile.auto_promote);
  const ok = await confirmDialog({
    title: live ? `Merge and release #${task.number} ${task.title}?` : `Merge #${task.number} ${task.title}?`,
    message: live
      ? `The pull request merges into ${int}, and ${prod} moves forward right away: this change goes live in production. To undo it, revert on GitHub.`
      : `The pull request merges into ${int}. Nothing goes live until someone releases ${int} to ${prod}.`,
    confirmLabel: live ? 'Merge and release' : 'Merge',
  });
  if (!ok) return;
  try {
    const delivery = await ctx.api.post(`/api/tasks/${enc(task.id)}/delivery/merge`);
    const notes = delivery && Array.isArray(delivery.notes) && delivery.notes.length ? delivery.notes.join(' ') : null;
    const released = delivery && (delivery.stage === 'promoted' || delivery.stage === 'production_done');
    toast(released ? `Merged and released to ${prod}.` : `Merged into ${int}.`, { kind: 'ok', detail: notes });
  } catch (error) {
    errorToast(error, 'Merge refused');
  }
  onDone();
}

/** Open a pull request for a task (title and body optional). */
export function openPrDialog(ctx, task, onDone = () => {}) {
  const title = h('input', { class: 'input', placeholder: task.title, maxlength: 200 });
  const body = h('textarea', { class: 'textarea', rows: 6, placeholder: 'Optional description (defaults to the spec’s goal, plan and steps, plus a link to the task)' });
  openModal({
    title: 'Open pull request',
    body: h('div', { class: 'stack' }, field('Title', title), field('Body', body), h('p', { class: 'muted xs' }, 'Refused without a valid spec or with uncommitted changes in the task worktree.')),
    actions: [
      { label: 'Cancel', value: false },
      { label: 'Open PR', kind: 'primary', run: async () => {
        const payload = {};
        if (title.value.trim()) payload.title = title.value.trim();
        if (body.value.trim()) payload.body = body.value.trim();
        const delivery = await ctx.api.post(`/api/tasks/${enc(task.id)}/delivery/pr`, payload);
        toast(delivery && delivery.pr ? `Opened PR #${delivery.pr.number}` : 'Pull request opened', { kind: 'ok' });
        onDone();
      } },
    ],
  });
}

function environmentOf(sandbox, envName) {
  return ((sandbox.profile && sandbox.profile.environments) || []).find((e) => e.name === envName) || null;
}

/**
 * Run an environment step after explicit confirmation (DLV-003). With a command, the run
 * starts in the background and a viewer polls it; without one, the same request records the
 * person's confirmation as a `manual` run.
 */
export async function runEnvironment(ctx, sandbox, envName) {
  const env = environmentOf(sandbox, envName);
  const command = env && env.command;
  const ok = await confirmDialog({
    // One verb per action, from the button to the dialog to the toast: Deploy, Mark verified.
    title: command ? `Deploy to ${envName}?` : `Mark ${envName} verified?`,
    message: h('div', { class: 'stack-sm' },
      command
        ? h('p', null, `This runs ${envName}’s deploy command for ${sandbox.name} on the server now. It runs on the server itself, outside the sandbox’s isolation.`)
        : h('p', null, `${envName} has no deploy command. This records that you checked it works.`),
      command ? h('pre', { class: 'code-view' }, command) : null,
      env && env.verify === 'manual' && command ? h('p', { class: 'muted small' }, `After it finishes, check ${envName} and mark it verified.`) : null),
    checkbox: command ? `I want to deploy to ${envName} now.` : `I checked that ${envName} works.`,
    confirmLabel: command ? `Deploy to ${envName}` : 'Mark verified',
    danger: Boolean(command),
  });
  if (!ok) return null;
  try {
    const run = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/delivery/environments/${enc(envName)}/runs`, { confirm: true });
    if (run && run.state === 'running') openRunViewer(ctx, sandbox, run.run_id, envName);
    else toast(`${envName}: ${run ? run.state : 'recorded'}`, { kind: run && run.state === 'succeeded' ? 'ok' : 'warn' });
    return run;
  } catch (error) {
    errorToast(error, `${envName} refused`);
    return null;
  }
}

/** Confirm a manual verification (`confirm_env:<name>`): POST …/environments/{env}/confirm. */
export async function confirmEnvironment(ctx, sandbox, envName) {
  const ok = await confirmDialog({
    title: `Mark ${envName} verified?`,
    message: `This records that you checked ${envName} works for ${sandbox.name}. The tasks waiting there move on.`,
    checkbox: `I checked that ${envName} works.`,
    confirmLabel: 'Mark verified',
  });
  if (!ok) return null;
  try {
    const run = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/delivery/environments/${enc(envName)}/confirm`, { confirm: true });
    toast(`${envName} marked verified`, { kind: 'ok' });
    return run;
  } catch (error) {
    errorToast(error, `Confirming ${envName} refused`);
    return null;
  }
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

/**
 * Release integration → production after confirmation (promotion). `waiting` lists the tasks
 * that go live ({number, title, pr}), so the dialog says what changes, not only which branch.
 */
export async function promote(ctx, sandbox, { waiting } = {}) {
  const profile = sandbox.profile || {};
  const production = profile.production_branch || 'production';
  const integration = profile.integration_branch || 'integration';
  const list = Array.isArray(waiting) ? waiting : null;
  const compare = sandbox.repo && sandbox.repo.url ? `${sandbox.repo.url}/compare/${encodeURIComponent(production)}...${encodeURIComponent(integration)}` : null;
  const ok = await confirmDialog({
    title: `Release to production (${production})?`,
    message: h('div', { class: 'stack-sm' },
      h('p', null, profile.promote === 'merge_commit'
        ? `This goes through a release pull request from ${integration} into ${production}: the first click opens it, the next merges it once its checks allow.`
        : `${production} moves forward to match ${integration} exactly.`),
      list ? h('div', { class: 'release-list' },
        h('div', { class: 'release-head' }, list.length ? `What goes live · ${list.length} task${list.length === 1 ? '' : 's'}` : 'What goes live'),
        list.length
          ? h('ul', null, list.map((t) => h('li', null, h('span', { class: 'muted' }, `#${t.number}`), ` ${t.title}`, t.pr ? h('span', { class: 'muted' }, ` · PR #${t.pr.number}`) : null)))
          : h('p', { class: 'muted small' }, 'No task is waiting; commits made outside tasks may still go live.'),
        compare ? h('a', { href: compare, target: '_blank', rel: 'noopener noreferrer', class: 'small' }, `Compare ${production}…${integration} on GitHub`) : null) : null,
      h('p', { class: 'muted small' }, `If something breaks, undo it with a revert on GitHub; ${BRAND.name} records who released what and when.`)),
    confirmLabel: list && list.length ? `Release ${list.length} task${list.length === 1 ? '' : 's'}` : 'Release',
  });
  if (!ok) return null;
  try {
    const result = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/delivery/promote`);
    if (result && result.result === 'pr_opened') {
      const url = result.sha && /^https?:\/\//.test(result.sha) ? result.sha : null;
      toast('Release pull request opened. Release again to merge it once its checks allow.', { kind: 'ok', timeout: 10000, action: url ? { label: 'Open PR', run: () => window.open(url, '_blank', 'noopener,noreferrer') } : undefined });
    } else {
      toast(`Released to ${production}${result && result.sha ? ` · ${String(result.sha).slice(0, 8)}` : ''}`, { kind: 'ok' });
    }
    return result;
  } catch (error) {
    errorToast(error, 'Release refused');
    return null;
  }
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
        // 403: no GitHub-linked account; 409: no GitHub App. The server's message says which.
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
    toast(`Removed ${sandbox.name}`, { kind: 'ok' });
    const current = ctx.currentSandbox();
    if (!current || current.id === sandbox.id) ctx.navigate('/');
  } catch (error) {
    errorToast(error, 'Could not remove sandbox');
  }
}

/** One message rendered with clickable links (UI-011). */
export function messageView(ctx, message, myLogin) {
  const from = message.from || {};
  const mine = from.login && myLogin && from.login === myLogin && !from.task_id;
  const kindTone = { question: 'warn', request: 'accent', scope: 'violet', info: '' }[message.kind] || '';
  return h('article', { class: ['msg', !message.read && 'unread', mine && 'mine'] },
    h('div', { class: 'msg-head' },
      h('strong', { class: 'ellipsis' }, from.title || from.login || 'unknown'),
      badge(message.kind || 'info', kindTone),
      h('span', { class: 'spacer' }),
      h('span', { class: 'xs muted', title: dateTime(message.created_at) }, relLong(message.created_at))),
    message.subject ? h('div', { class: 'msg-subject' }, message.subject) : null,
    h('div', { class: 'msg-body' }, linkify(message.body || '', (url, event) => ctx.openLink(url, event))));
}
