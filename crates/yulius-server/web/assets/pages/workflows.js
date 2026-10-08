// Workflows page (AUT-004): reusable task recipes — initial steps, instructions, gates.

import { enc } from '../api.js';
import { h, openModal, field, toast, confirmDialog, errorToast, iconButton, iconBadge, itemCard, emptyState, invalid } from '../dom.js';
import { allows } from '../status.js';
import { pageFrame, loader } from './common.js';

/** Mount the Workflows page. */
export function mountWorkflowsPage(ctx, sandbox) {
  const write = allows(sandbox.access, 'write');
  const instanceAdmin = ctx.isInstanceAdmin();
  const frame = pageFrame({
    title: 'Workflows',
    sub: 'selectable when starting a task',
    actions: [h('button', { type: 'button', class: 'btn btn-primary', disabled: !write, title: write ? null : 'Requires write access', onClick: () => edit(null) }, 'New workflow')],
  });
  const host = h('div');
  frame.body.append(host);

  const reload = loader(host, () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/workflows`), (list) => {
    const workflows = Array.isArray(list) ? list : [];
    if (workflows.length === 0) return emptyState({ iconName: 'workflow', title: 'No workflows yet', text: 'A workflow seeds a task’s steps, instructions and required gates — pick one when you start a task.', action: write ? h('button', { type: 'button', class: 'btn btn-primary', onClick: () => edit(null) }, 'New workflow') : null });
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
        onOpen: canEdit ? () => edit(wf) : null,
        openLabel: `Edit workflow ${wf.name}`,
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

  function edit(wf) {
    const name = h('input', { class: 'input', value: wf ? wf.name : '', maxlength: 100, autofocus: true });
    const description = h('input', { class: 'input', value: wf ? wf.description : '', maxlength: 500 });
    const steps = h('textarea', { class: 'textarea', rows: 6, placeholder: 'One step per line, e.g.\nWrite spec\nImplement\nTest\nOpen PR', value: wf && Array.isArray(wf.steps) ? wf.steps.join('\n') : '' });
    const instructions = h('textarea', { class: 'textarea', rows: 6, placeholder: 'Extra instructions given to the agent at start', value: wf ? wf.instructions || '' : '' });
    const requireSpec = h('input', { type: 'checkbox', checked: wf ? Boolean(wf.gates && wf.gates.require_spec) : true });
    const requireTests = h('input', { type: 'checkbox', checked: wf ? Boolean(wf.gates && wf.gates.require_tests) : true });
    const scope = h('select', { class: 'select', disabled: Boolean(wf) }, h('option', { value: 'sandbox' }, `This sandbox (${sandbox.name})`), instanceAdmin ? h('option', { value: 'instance' }, 'Whole instance') : null);
    scope.value = wf ? wf.scope || 'sandbox' : 'sandbox';
    openModal({
      title: wf ? `Edit workflow · ${wf.name}` : 'New workflow',
      wide: true,
      body: h('div', { class: 'stack' },
        h('div', { class: 'form-grid' }, field('Name', name), field('Scope', scope)),
        field('Description', description),
        field('Initial steps', steps),
        field('Instructions', instructions),
        h('div', { class: 'row-wrap' },
          h('label', { class: 'check' }, requireSpec, h('span', null, 'Require a valid task spec before delivery')),
          h('label', { class: 'check' }, requireTests, h('span', null, 'Require tests'))))
      ,
      actions: [
        { label: 'Cancel', value: false },
        { label: wf ? 'Save' : 'Create', kind: 'primary', run: async () => {
          if (!name.value.trim()) return invalid(name, 'Give the workflow a name.');
          const body = {
            name: name.value.trim(),
            description: description.value.trim(),
            steps: steps.value.split('\n').map((s) => s.trim()).filter(Boolean),
            instructions: instructions.value,
            gates: { require_spec: requireSpec.checked, require_tests: requireTests.checked },
            scope: scope.value,
          };
          if (wf) await ctx.api.put(`/api/workflows/${enc(wf.id)}`, body);
          else await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/workflows`, body);
          toast(wf ? 'Workflow saved.' : 'Workflow created.', { kind: 'ok' });
          reload();
          return true;
        } },
      ],
    });
  }

  return { el: frame.el, key: `workflows:${sandbox.id}` };
}
