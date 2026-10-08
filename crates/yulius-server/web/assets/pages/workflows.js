// Workflows page (AUT-004): reusable task recipes — initial steps, instructions, gates. A card opens
// the workflow in the builder (workflowedit.js, UI-034); so does New workflow, starting from a template.

import { enc } from '../api.js';
import { h, toast, confirmDialog, errorToast, iconButton, iconBadge, itemCard, emptyState } from '../dom.js';
import { navigate } from '../router.js';
import { allows } from '../status.js';
import { pageFrame, loader } from './common.js';

/** Mount the Workflows page. */
export function mountWorkflowsPage(ctx, sandbox) {
  const write = allows(sandbox.access, 'write');
  const instanceAdmin = ctx.isInstanceAdmin();
  const base = `/s/${encodeURIComponent(sandbox.slug)}/workflows`;
  const create = () => navigate(`${base}/new`);
  const frame = pageFrame({
    title: 'Workflows',
    sub: 'selectable when starting a task',
    actions: [h('button', { type: 'button', class: 'btn btn-primary', disabled: !write, title: write ? null : 'Requires write access', onClick: create }, 'New workflow')],
  });
  const host = h('div');
  frame.body.append(host);

  const reload = loader(host, () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/workflows`), (list) => {
    const workflows = Array.isArray(list) ? list : [];
    if (workflows.length === 0) return emptyState({ iconName: 'workflow', title: 'No workflows yet', text: 'A workflow seeds a task’s steps, instructions and required gates — pick one when you start a task.', action: write ? h('button', { type: 'button', class: 'btn btn-primary', onClick: create }, 'New workflow') : null });
    return h('div', { class: 'item-list' }, workflows.map((wf) => {
      const canEdit = write && (wf.scope !== 'instance' || instanceAdmin);
      const gates = wf.gates || {};
      const steps = Array.isArray(wf.steps) ? wf.steps : [];
      return itemCard({
        iconName: 'workflow',
        title: wf.name,
        badges: [
          wf.scope === 'instance' ? iconBadge('globe', 'Whole instance', 'violet', 'Available in every sandbox') : iconBadge('folder', 'This sandbox', '', 'Available in this sandbox only'),
          gates.require_spec ? iconBadge('book', 'Spec required', '', 'The task must write a spec before it can be delivered') : null,
          gates.require_tests ? iconBadge('check', 'Tests required', '', 'The task must add or run tests before it can be delivered') : null,
        ],
        description: wf.description || null,
        extra: steps.length
          ? h('ol', { class: 'steps-line', 'aria-label': 'Steps' }, steps.map((step, i) => h('li', null, h('span', { class: 'step-num' }, String(i + 1)), step)))
          : h('div', { class: 'item-meta' }, 'No starting steps'),
        controls: canEdit ? [iconButton('trash', `Delete ${wf.name}`, () => remove(wf), { class: 'btn-quiet-danger' })] : [],
        onOpen: () => navigate(`${base}/${encodeURIComponent(wf.id)}`),
        openLabel: canEdit ? `Edit workflow ${wf.name}` : `Open workflow ${wf.name}`,
      });
    }));
  });

  async function remove(wf) {
    if (!(await confirmDialog({ title: `Delete workflow “${wf.name}”?`, message: 'Existing tasks keep their steps. New tasks can no longer select it.', confirmLabel: 'Delete', danger: true }))) return;
    try {
      await ctx.api.del(`/api/workflows/${enc(wf.id)}`);
      toast('Workflow deleted.', { kind: 'ok' });
      reload();
    } catch (error) {
      errorToast(error, 'Could not delete workflow');
    }
  }

  return { el: frame.el, key: `workflows:${sandbox.id}` };
}
