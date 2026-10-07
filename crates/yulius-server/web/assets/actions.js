// Task actions (UI-013, §6.3) and shared dialogs: switch, feedback, delivery, deploy
// runs, promotion, sandbox create/import and removal.

import { enc } from './api.js';
import { h, openModal, confirmDialog, openMenu, toast, errorToast, field, badge, setChildren, icon, placeholder, linkify } from './dom.js';
import { taskActions, harnessInfo, statusInfo, allows } from './status.js';
import { relLong, dateTime } from './format.js';

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
    { label: 'Switch harness / account…', disabled: !can.switch, title: can.switch ? null : why, onSelect: () => openSwitchDialog(ctx, task) },
    { label: 'Deliver…', disabled: !can.deliver, title: can.deliver ? null : why, onSelect: () => openDeliveryDialog(ctx, task) },
    { label: 'Feedback…', disabled: !can.feedback, onSelect: () => openFeedbackDialog(ctx, task) },
    { separator: true },
    task.archived
      ? { label: 'Restore', disabled: !can.restore, onSelect: () => restoreTask(ctx, task) }
      : { label: 'Archive', disabled: !can.archive, title: can.archive ? null : (allows(access, 'write') ? 'Stop the task before archiving' : why), onSelect: () => archiveTask(ctx, task) },
  ], { align: anchor instanceof Element ? 'end' : 'start' });
}

// Every API path is a literal at its call site so tests/ui_contract.rs can check it
// against the server's router.
async function taskPost(ctx, task, verb, request, okMessage) {
  try {
    const updated = await request();
    if (updated && updated.id) ctx.store.upsertTask(updated);
    if (okMessage) toast(okMessage, { kind: 'ok' });
    return updated;
  } catch (error) {
    errorToast(error, `Could not ${verb} task #${task.number}`);
    return null;
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

/** Archive is a one-click action with Undo (UI-006). */
export async function archiveTask(ctx, task) {
  const updated = await taskPost(ctx, task, 'archive', () => ctx.api.post(`/api/tasks/${enc(task.id)}/archive`));
  if (updated) {
    ctx.store.removeTask(task.id);
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

/** Switch harness/account; for an ended task this continues it with the chosen target. */
export async function openSwitchDialog(ctx, task) {
  const live = task.status === 'running' || task.status === 'waiting_capacity';
  let accounts = [];
  try {
    accounts = await ctx.api.get('/api/admin/accounts', { allow401: true });
  } catch {
    accounts = []; // Only instance admins can list accounts; others pick a harness only.
  }
  const harnessInputs = ['claude', 'codex'].map((value) => h('input', { type: 'radio', name: 'switch-harness', value, checked: value === task.harness }));
  const accountSelect = h('select', { class: 'select' });
  const renderAccounts = () => {
    const harness = harnessInputs.find((i) => i.checked).value;
    const provider = harness === 'codex' ? 'openai' : 'anthropic';
    setChildren(accountSelect,
      h('option', { value: '' }, 'Automatic (rotation picks the best account)'),
      (Array.isArray(accounts) ? accounts : []).filter((a) => a.provider === provider).map((a) =>
        h('option', { value: a.id, disabled: !a.enabled || a.status === 'error' || a.status === 'pending_login' }, `${a.label} · ${a.status}${a.usage && a.usage.primary && Number.isFinite(a.usage.primary.used_percent) ? ` · ${Math.round(a.usage.primary.used_percent)}%` : ''}`)));
  };
  harnessInputs.forEach((i) => i.addEventListener('change', renderAccounts));
  renderAccounts();
  const body = h('div', { class: 'stack' },
    h('p', { class: 'text-2' }, live
      ? 'The session moves to the chosen harness or account at the next turn boundary. A provider switch translates the conversation; the terminal shows the new harness.'
      : 'The task is not running; it will continue on the chosen harness or account.'),
    field('Harness', h('div', { class: 'seg', role: 'radiogroup' }, harnessInputs.map((input) => h('label', null, input, harnessInfo(input.value).name)))),
    field('Account', accountSelect, accounts.length ? null : 'Only instance administrators can pick a specific account.'),
  );
  openModal({
    title: `Switch task #${task.number}`,
    body,
    actions: [
      { label: 'Cancel', value: false },
      { label: live ? 'Switch' : 'Continue on target', kind: 'primary', run: async () => {
        const payload = { harness: harnessInputs.find((i) => i.checked).value };
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
  ['none', 'Not delivered'],
  ['pr_open', 'PR open'],
  ['merged_integration', 'Merged to integration'],
  ['env_done', 'Environment done'],
  ['promoted', 'Promoted'],
  ['production_done', 'Production done'],
];

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

/** Notes from Git/GitHub reads and policy outcomes (reads degrade instead of failing). */
export function notesBanner(notes) {
  const list = Array.isArray(notes) ? notes.filter(Boolean) : [];
  if (list.length === 0) return null;
  return h('div', { class: 'banner banner-warn small' }, icon('warn', { size: 14 }),
    h('div', { class: 'banner-text stack-sm' }, list.map((note) => h('span', null, note))));
}

/** Label for a delivery action wire name (`run_env:<name>`, `confirm_env:<name>`, …). */
function actionLabel(action) {
  if (action === 'open_pr') return 'Open pull request';
  if (action === 'merge') return 'Merge';
  if (action === 'promote') return 'Promote';
  if (action.startsWith('run_env:')) return `Run ${action.slice('run_env:'.length)}`;
  if (action.startsWith('confirm_env:')) return `Confirm ${action.slice('confirm_env:'.length)}`;
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
        d.spec_valid ? null : h('ul', { class: 'small', style: { margin: 0, paddingLeft: '18px' } }, (d.spec_problems || []).map((p) => h('li', null, p))),
        d.steps_open ? h('div', { class: 'banner banner-warn small' }, icon('warn', { size: 14 }), `${d.steps_open} step${d.steps_open === 1 ? '' : 's'} still open in the plan.`) : null),
      h('div', { class: 'row-wrap' },
        next.map((action, i) => actionButton(action, i === 0)),
        blocked.map((b) => h('button', { type: 'button', class: 'btn', disabled: true, title: b.reason }, actionLabel(b.action))),
        next.length || blocked.length ? null : h('span', { class: 'muted' }, 'No delivery action applies at this stage.')),
      blocked.length ? h('ul', { class: 'small muted', style: { margin: 0, paddingLeft: '18px' } },
        blocked.map((b) => h('li', null, h('strong', null, `${actionLabel(b.action)}: `), b.reason))) : null,
      h('p', { class: 'muted xs' }, 'Merges, deploys and promotions only happen on your explicit action (or an explicit profile policy such as automatic promotion).'),
    );
  };
  load();
  return modal;
}

async function mergeTask(ctx, task, onDone) {
  const ok = await confirmDialog({ title: `Merge #${task.number} ${task.title}?`, message: 'Merge the pull request into the integration branch using the profile’s merge method. The PR head must match the task worktree; checks must pass when the profile requires them.', confirmLabel: 'Merge' });
  if (!ok) return;
  try {
    const delivery = await ctx.api.post(`/api/tasks/${enc(task.id)}/delivery/merge`);
    const notes = delivery && Array.isArray(delivery.notes) && delivery.notes.length ? delivery.notes.join(' ') : null;
    toast('Merged.', { kind: 'ok', detail: notes });
  } catch (error) {
    errorToast(error, 'Merge refused');
  }
  onDone();
}

function openPrDialog(ctx, task, onDone) {
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
    title: command ? `Run ${envName}` : `Confirm ${envName}`,
    message: h('div', { class: 'stack-sm' },
      command
        ? h('p', null, `This runs the “${envName}” command for ${sandbox.name} on the server now (as /bin/sh -c in the sandbox clone, outside sandbox containment).`)
        : h('p', null, `“${envName}” has no command. Confirming records that you verified it.`),
      command ? h('pre', { class: 'code-view' }, command) : null,
      env && env.verify === 'manual' && command ? h('p', { class: 'muted small' }, 'This environment is verified manually: confirm it after checking the result.') : null),
    checkbox: command ? `I want to run ${envName} now.` : `I verified ${envName}.`,
    confirmLabel: command ? `Run ${envName}` : `Confirm ${envName}`,
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
    title: `Confirm ${envName}`,
    message: `Record that you verified “${envName}” for ${sandbox.name}. Tasks waiting for this environment advance.`,
    checkbox: `I verified ${envName}.`,
    confirmLabel: `Confirm ${envName}`,
  });
  if (!ok) return null;
  try {
    const run = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/delivery/environments/${enc(envName)}/confirm`, { confirm: true });
    toast(`${envName} confirmed`, { kind: 'ok' });
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
    title: `${envName} · run ${runId}`,
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
      setChildren(meta, [run.kind, run.commit ? `at ${String(run.commit).slice(0, 8)}` : null, run.requested_by ? `by ${run.requested_by}` : null, run.started_at ? `started ${relLong(run.started_at)}` : null].filter(Boolean).join(' · '));
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

/** Promote integration → production after confirmation. */
export async function promote(ctx, sandbox) {
  const profile = sandbox.profile || {};
  const ok = await confirmDialog({
    title: `Promote ${profile.integration_branch || 'integration'} → ${profile.production_branch || 'production'}?`,
    message: profile.promote === 'merge_commit'
      ? `Promotion uses a merge-commit pull request: the first promotion opens it, a later one merges it once its checks allow. This updates the production branch of ${sandbox.repo ? `${sandbox.repo.owner}/${sandbox.repo.name}` : sandbox.name}.`
      : `Promotion fast-forwards ${profile.production_branch || 'production'} to the integration head of ${sandbox.repo ? `${sandbox.repo.owner}/${sandbox.repo.name}` : sandbox.name}.`,
    confirmLabel: 'Promote',
  });
  if (!ok) return null;
  try {
    const result = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/delivery/promote`);
    if (result && result.result === 'pr_opened') {
      const url = result.sha && /^https?:\/\//.test(result.sha) ? result.sha : null;
      toast('Promotion pull request opened. Promote again to merge it once its checks allow.', { kind: 'ok', timeout: 10000, action: url ? { label: 'Open PR', run: () => window.open(url, '_blank', 'noopener,noreferrer') } : undefined });
    } else {
      toast(`Promoted${result && result.sha ? ` · ${String(result.sha).slice(0, 8)}` : ''}`, { kind: 'ok' });
    }
    return result;
  } catch (error) {
    errorToast(error, 'Promotion refused');
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
    h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Mode' }, h('label', null, tabImport, 'Import repository'), h('label', { title: canCreate ? null : 'Only instance administrators can create projects' }, tabCreate, 'Create project')),
    content);
  let submitButton;
  let selectedRepo = null;
  let repos = null;
  let templates = null;
  const nameInput = h('input', { class: 'input', placeholder: 'my-project', maxlength: 100, autocomplete: 'off' });
  const descInput = h('input', { class: 'input', placeholder: 'One sentence about the project (optional)', maxlength: 300 });
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
    const list = h('div', { class: 'card list', style: { maxHeight: '320px', overflow: 'auto' } }, placeholder('Loading repositories…', 'loading'));
    setChildren(content, search, list, h('p', { class: 'muted xs' }, 'Importing needs admin or maintain permission on the repository, which must belong to the instance owner. Yulius clones it and checks the documentation format; a project that does not follow it gets a preparation task first.'));
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
        return h('label', { class: 'list-item', style: { cursor: disabled ? 'not-allowed' : 'pointer', opacity: disabled ? 0.55 : 1 } },
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
      h('p', { class: 'muted xs' }, 'Instance administrators only. Yulius creates a private GitHub repository, pushes the template to main and dev, adds rulesets requiring pull requests and opens the new tab.'));
    if (!templates) {
      try { templates = await ctx.api.get('/api/templates'); } catch (error) { setChildren(templateList, placeholder(error.message, 'error')); return; }
    }
    if (!templateValue && templates.length) templateValue = templates[0].id;
    setChildren(templateList, templates.map((t) => {
      const radio = h('input', { type: 'radio', name: 'sb-template', value: t.id, checked: t.id === templateValue });
      radio.addEventListener('change', () => { templateValue = t.id; updateSubmit(); });
      return h('label', { class: 'list-item', style: { cursor: 'pointer' } }, radio, h('div', { class: 'list-main' }, h('div', { class: 'list-title' }, t.name), h('div', { class: 'list-sub', style: { whiteSpace: 'normal' } }, t.description)));
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
    title: `Remove ${sandbox.name} from Yulius?`,
    message: 'Live and queued tasks are stopped. The GitHub repository, the server’s clone and the task records are kept; the sandbox disappears from Yulius for everyone.',
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
