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

async function taskPost(ctx, task, action, body, okMessage) {
  try {
    const updated = await ctx.api.post(`/api/tasks/${enc(task.id)}/${action}`, body);
    if (updated && updated.id) ctx.store.upsertTask(updated);
    if (okMessage) toast(okMessage, { kind: 'ok' });
    return updated;
  } catch (error) {
    errorToast(error, `Could not ${action} task #${task.number}`);
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
  if (ok) await taskPost(ctx, task, 'stop', undefined, `Stopping #${task.number}…`);
}

/** Resume a closed/stopped task on the same session lineage (TSK-007). */
export function continueTask(ctx, task) {
  return taskPost(ctx, task, 'continue', {}, `Continuing #${task.number}…`);
}

/** Archive is a one-click action with Undo (UI-006). */
export async function archiveTask(ctx, task) {
  const updated = await taskPost(ctx, task, 'archive');
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
  const updated = await taskPost(ctx, task, 'restore', undefined, `Restored #${task.number}`);
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
        h('option', { value: a.id, disabled: !a.enabled || a.status === 'error' }, `${a.label} · ${a.status}${a.usage && a.usage.primary ? ` · ${Math.round(a.usage.primary.used_percent)}%` : ''}`)));
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
        const updated = await ctx.api.post(`/api/tasks/${enc(task.id)}/${live ? 'switch' : 'continue'}`, payload);
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

/** PR line with state and checks badges. */
export function prLine(pr) {
  if (!pr) return h('span', { class: 'muted' }, 'No pull request');
  const checks = { success: ['checks passed', 'ok'], failure: ['checks failed', 'danger'], pending: ['checks running', 'warn'], none: ['no checks', ''] }[pr.checks] || [pr.checks, ''];
  const state = { open: 'accent', merged: 'violet', closed: '' }[pr.state] || '';
  return h('span', { class: 'row-wrap' },
    h('a', { href: pr.url, target: '_blank', rel: 'noopener noreferrer' }, `#${pr.number}`),
    h('span', { class: 'ellipsis' }, pr.title),
    badge(pr.state, state),
    badge(checks[0], checks[1]),
    h('span', { class: 'muted small mono' }, `${pr.base} ← ${pr.head}`));
}

/** Delivery panel for one task: stage, spec validity, PR and next actions (DLV, GIT-002). */
export function openDeliveryDialog(ctx, task) {
  const sandbox = ctx.store.sandboxById(task.sandbox_id);
  const modal = openModal({ title: `Deliver #${task.number} ${task.title}`, body: placeholder('Loading delivery…', 'loading'), wide: true, actions: [{ label: 'Close' }] });
  const load = async () => {
    try {
      const delivery = await ctx.api.get(`/api/tasks/${enc(task.id)}/delivery`);
      setChildren(modal.body, renderDelivery(delivery));
    } catch (error) {
      setChildren(modal.body, placeholder(error.message, 'error'));
    }
  };
  const renderDelivery = (d) => {
    const stageIndex = STAGES.findIndex(([s]) => s === d.stage || (s === 'env_done' && String(d.stage).startsWith('env_')));
    const actions = (d.next_actions || []).map((action) => {
      if (action === 'open_pr') return h('button', { type: 'button', class: 'btn btn-primary', disabled: !d.spec_valid, title: d.spec_valid ? null : 'The task spec must be valid first', onClick: () => openPrDialog(ctx, task, load) }, 'Open pull request');
      if (action === 'merge') return h('button', { type: 'button', class: 'btn btn-primary', onClick: async () => {
        const ok = await confirmDialog({ title: `Merge PR #${d.pr ? d.pr.number : ''}?`, message: `Merge into ${d.pr ? d.pr.base : 'the integration branch'} using the profile's merge method. Checks must be green.`, confirmLabel: 'Merge' });
        if (!ok) return;
        try { await ctx.api.post(`/api/tasks/${enc(task.id)}/delivery/merge`); toast('Merged.', { kind: 'ok' }); } catch (error) { errorToast(error, 'Merge refused'); }
        load();
      } }, 'Merge');
      if (action.startsWith('run_env:')) {
        const env = action.slice('run_env:'.length);
        return h('button', { type: 'button', class: 'btn', onClick: async () => { if (sandbox) { await runDeploy(ctx, sandbox, env); load(); } } }, `Run deploy: ${env}`);
      }
      if (action === 'promote') return h('button', { type: 'button', class: 'btn', onClick: async () => { if (sandbox) { await promote(ctx, sandbox); load(); } } }, 'Promote');
      return h('button', { type: 'button', class: 'btn', disabled: true }, action);
    });
    return h('div', { class: 'stack' },
      h('div', { class: 'stage-track' }, STAGES.map(([stage, label], i) => h('span', { class: ['stage', i < stageIndex && 'done', i === stageIndex && 'current'] }, i < stageIndex ? icon('check', { size: 11 }) : null, label))),
      h('div', { class: 'card card-pad stack-sm' },
        h('div', { class: 'row' }, h('strong', null, 'Pull request')),
        prLine(d.pr)),
      h('div', { class: 'card card-pad stack-sm' },
        h('div', { class: 'row' }, h('strong', null, 'Task spec'), d.spec_valid ? badge('valid', 'ok') : badge('needs work', 'danger')),
        d.spec_valid ? null : h('ul', { class: 'small', style: { margin: 0, paddingLeft: '18px' } }, (d.spec_problems || []).map((p) => h('li', null, p))),
        d.steps_open ? h('div', { class: 'banner banner-warn small' }, icon('warn', { size: 14 }), `${d.steps_open} step${d.steps_open === 1 ? '' : 's'} still open in the plan.`) : null),
      h('div', { class: 'row-wrap' }, actions.length ? actions : h('span', { class: 'muted' }, 'No delivery action is available right now.')),
      h('p', { class: 'muted xs' }, 'Merges, deploys and promotions only happen on your explicit action (or an explicit profile policy).'),
    );
  };
  load();
  return modal;
}

function openPrDialog(ctx, task, onDone) {
  const title = h('input', { class: 'input', placeholder: task.title, maxlength: 200 });
  const body = h('textarea', { class: 'textarea', rows: 6, placeholder: 'Optional description (defaults to the task spec summary)' });
  openModal({
    title: 'Open pull request',
    body: h('div', { class: 'stack' }, field('Title', title), field('Body', body)),
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

/** Explicitly confirmed environment deploy, then a live run viewer (DLV-003). */
export async function runDeploy(ctx, sandbox, envName) {
  const env = ((sandbox.profile && sandbox.profile.environments) || []).find((e) => e.name === envName);
  const ok = await confirmDialog({
    title: `Run deploy: ${envName}`,
    message: h('div', { class: 'stack-sm' },
      h('p', null, `This runs the “${envName}” environment step for ${sandbox.name} on the server now.`),
      env && env.command ? h('pre', { class: 'code-view' }, env.command) : h('p', { class: 'muted small' }, 'This environment has no command; the step is recorded as a manual verification.')),
    checkbox: `I want to run the ${envName} deploy now.`,
    confirmLabel: 'Run deploy',
    danger: true,
  });
  if (!ok) return null;
  try {
    const result = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/delivery/environments/${enc(envName)}/runs`, { confirm: true });
    openRunViewer(ctx, sandbox, result.run_id, envName);
    return result;
  } catch (error) {
    errorToast(error, `Deploy ${envName} refused`);
    return null;
  }
}

const RUN_POLL_MS = 2000;
const RUN_POLL_MAX = 900; // 30 minutes

/** Poll a delivery run and show its output until it finishes (bounded). */
export function openRunViewer(ctx, sandbox, runId, envName) {
  const stateEl = badge('starting', 'warn');
  const output = h('pre', { class: 'code-view run-output', 'aria-live': 'polite' }, '');
  const exitEl = h('span', { class: 'muted small' });
  let polls = 0;
  let timer = null;
  const modal = openModal({
    title: `Deploy ${envName} · run ${runId}`,
    wide: true,
    body: h('div', { class: 'stack' }, h('div', { class: 'row' }, stateEl, exitEl), output),
    actions: [{ label: 'Close' }],
    onClose: () => clearTimeout(timer),
  });
  const poll = async () => {
    polls += 1;
    try {
      const run = await ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/delivery/runs/${enc(runId)}`);
      const done = ['succeeded', 'success', 'failed', 'failure', 'error', 'cancelled', 'done', 'completed'].includes(run.state);
      stateEl.textContent = run.state;
      stateEl.className = `badge badge-${done ? (run.exit_code === 0 || run.state === 'succeeded' || run.state === 'success' ? 'ok' : 'danger') : 'warn'}`;
      const atBottom = output.scrollTop + output.clientHeight >= output.scrollHeight - 8;
      output.textContent = run.output || '';
      if (atBottom) output.scrollTop = output.scrollHeight;
      exitEl.textContent = run.exit_code === null || run.exit_code === undefined ? '' : `exit code ${run.exit_code}`;
      if (done) return;
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
    message: `Promotion method: ${profile.promote || 'per profile'}. This updates the production branch of ${sandbox.repo ? `${sandbox.repo.owner}/${sandbox.repo.name}` : sandbox.name}.`,
    confirmLabel: 'Promote',
  });
  if (!ok) return null;
  try {
    const result = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/delivery/promote`);
    toast(`Promotion: ${result.result}${result.sha ? ` · ${String(result.sha).slice(0, 8)}` : ''}`, { kind: 'ok' });
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
  const tabImport = h('input', { type: 'radio', name: 'sb-mode', value: 'import', checked: mode === 'import' });
  const tabCreate = h('input', { type: 'radio', name: 'sb-mode', value: 'create', checked: mode === 'create' });
  const content = h('div', { class: 'stack' });
  const body = h('div', { class: 'stack' },
    h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Mode' }, h('label', null, tabImport, 'Import repository'), h('label', null, tabCreate, 'Create project')),
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
    setChildren(content, search, list, h('p', { class: 'muted xs' }, 'Importing needs admin permission on the repository. The repository must belong to the instance owner.'));
    const draw = () => {
      if (!repos) return;
      const q = search.value.trim().toLowerCase();
      const shown = repos.filter((r) => `${r.owner}/${r.name}`.toLowerCase().includes(q));
      if (shown.length === 0) { setChildren(list, placeholder(repos.length ? 'No repository matches.' : 'No repositories visible to you.')); return; }
      setChildren(list, shown.map((r) => {
        const full = `${r.owner}/${r.name}`;
        const disabled = r.already_sandbox || r.access !== 'admin';
        const radio = h('input', { type: 'radio', name: 'sb-repo', value: full, disabled, checked: selectedRepo === full });
        radio.addEventListener('change', () => { selectedRepo = full; updateSubmit(); });
        return h('label', { class: 'list-item', style: { cursor: disabled ? 'not-allowed' : 'pointer', opacity: disabled ? 0.55 : 1 } },
          radio,
          h('div', { class: 'list-main' }, h('div', { class: 'list-title' }, full, r.private ? badge('private') : badge('public', 'accent'))),
          r.already_sandbox ? badge('already a sandbox', 'ok') : badge(r.access, r.access === 'admin' ? '' : 'warn', r.access === 'admin' ? null : 'Admin permission required'));
      }));
    };
    search.addEventListener('input', draw);
    if (!repos) {
      try {
        repos = await ctx.api.get('/api/github/repositories');
      } catch (error) {
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
      h('p', { class: 'muted xs' }, 'Yulius creates the GitHub repository, scaffolds the documentation format, configures branch protection and opens the new tab.'));
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
        toast(tabImport.checked ? `Imported ${sandbox.name}` : `Created ${sandbox.name}`, { kind: 'ok' });
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
    message: 'Running sessions are stopped and task history is exported. The GitHub repository itself is not deleted or changed.',
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
