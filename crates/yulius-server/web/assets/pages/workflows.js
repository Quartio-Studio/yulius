// Workflows page (AUT-004, AUT-006): reusable task recipes — initial steps, instructions, gates.
// The defaults ship with the server (catalog/workflows/*.md), come first, are read-only and kept
// current by it; Duplicate starts a custom copy in the builder (workflowedit.js, UI-034), as does
// New workflow, starting from a default or Blank. A card opens the workflow.

import { enc } from '../api.js';
import { h, toast, confirmDialog, errorToast, iconButton, iconBadge, itemCard, emptyState } from '../dom.js';
import { navigate } from '../router.js';
import { allows } from '../status.js';
import { pageFrame, loader, createButton } from './common.js';
import { BRAND } from '../brand.js';

/** Icon of a workflow: the defaults have their own, the rest share one. */
export function workflowIcon(wf) {
  return { 'wf-feature': 'zap', 'wf-fix': 'target', 'wf-research': 'search' }[wf.id] || 'workflow';
}

/** Mount the Workflows page. */
export function mountWorkflowsPage(ctx, sandbox) {
  const write = allows(sandbox.access, 'write');
  const instanceAdmin = ctx.isInstanceAdmin();
  const base = `/s/${encodeURIComponent(sandbox.slug)}/workflows`;
  const create = () => navigate(`${base}/new`);
  const duplicate = (wf) => navigate(`${base}/new?from=${encodeURIComponent(wf.id)}`);
  const frame = pageFrame({
    title: 'Workflows',
    sub: 'picked when starting a task',
    actions: [createButton('New workflow', create, { disabled: !write, title: write ? null : 'Requires write access' })],
  });
  const host = h('div');
  frame.body.append(host);

  function card(wf) {
    const canEdit = write && !wf.builtin && (wf.scope !== 'instance' || instanceAdmin);
    const gates = wf.gates || {};
    const steps = Array.isArray(wf.steps) ? wf.steps : [];
    return itemCard({
      iconName: workflowIcon(wf),
      title: wf.name,
      badges: [
        wf.builtin ? iconBadge('lock', 'Default', 'violet', `Ships with ${BRAND.name} and stays current with it; duplicate it to customise`)
          : wf.scope === 'instance' ? iconBadge('globe', 'Every sandbox', 'violet', 'Available in every sandbox') : iconBadge('folder', 'This sandbox', '', 'Available in this sandbox only'),
        gates.require_spec ? iconBadge('book', 'Spec required', '', 'The task writes a spec before it is delivered') : null,
        gates.require_tests ? iconBadge('check', 'Tests required', '', 'The task adds or runs tests before it is delivered') : null,
      ],
      description: wf.description || null,
      extra: [
        wf.when_to_use ? h('div', { class: 'item-meta' }, `Use when: ${wf.when_to_use}`) : null,
        steps.length
          ? h('ol', { class: 'steps-line', 'aria-label': 'Steps' }, steps.map((step, i) => h('li', null, h('span', { class: 'step-num' }, String(i + 1)), step)))
          : h('div', { class: 'item-meta' }, 'No starting steps'),
      ],
      controls: [
        wf.builtin && write ? h('button', { type: 'button', class: 'btn btn-sm', title: `A custom copy of ${wf.name} to change`, onClick: () => duplicate(wf) }, 'Duplicate') : null,
        canEdit ? iconButton('trash', `Delete ${wf.name}`, () => remove(wf), { class: 'btn-quiet-danger' }) : null,
      ].filter(Boolean),
      onOpen: () => navigate(`${base}/${encodeURIComponent(wf.id)}`),
      openLabel: canEdit ? `Edit workflow ${wf.name}` : `Open workflow ${wf.name}`,
    });
  }

  const reload = loader(host, 'the workflows', () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/workflows`), (list) => {
    const workflows = Array.isArray(list) ? list : [];
    const defaults = workflows.filter((wf) => wf.builtin);
    const custom = workflows.filter((wf) => !wf.builtin);
    const section = (id, title, hint, items) => h('section', { class: 'stack-sm', 'aria-labelledby': id },
      h('h2', { class: 'page-section-title', id }, title, h('span', { class: 'muted' }, hint)),
      items);
    return h('div', { class: 'stack' },
      defaults.length ? section('wf-defaults', 'Defaults', `kept current by ${BRAND.name}`, h('div', { class: 'item-list' }, defaults.map(card))) : null,
      section('wf-custom', 'Custom', 'yours to change', custom.length
        ? h('div', { class: 'item-list' }, custom.map(card))
        : emptyState({ iconName: 'workflow', title: 'No custom workflows yet', text: 'Duplicate a default to change it, or start one from Blank.', action: write ? createButton('New workflow', create) : null })));
  });

  async function remove(wf) {
    if (!(await confirmDialog({ title: `Delete workflow “${wf.name}”?`, message: 'Existing tasks keep their steps. New tasks can no longer select it.', confirmLabel: 'Delete', danger: true }))) return;
    try {
      await ctx.api.del(`/api/workflows/${enc(wf.id)}`);
      toast('Workflow deleted.', { kind: 'ok' });
      reload();
    } catch (error) {
      errorToast(error, 'Couldn’t delete workflow');
    }
  }

  return { el: frame.el, key: `workflows:${sandbox.id}` };
}
