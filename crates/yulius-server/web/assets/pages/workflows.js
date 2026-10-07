// Workflows page (AUT-004): reusable task recipes — initial steps, instructions, gates.

import { enc } from '../api.js';
import { h, badge, openModal, field, toast, confirmDialog, errorToast } from '../dom.js';
import { allows } from '../status.js';
import { pageFrame, loader } from './common.js';

/** Mount the Workflows page. */
export function mountWorkflowsPage(ctx, sandbox) {
  const write = allows(sandbox.access, 'write');
  const instanceAdmin = ctx.isInstanceAdmin();
  const frame = pageFrame({
    title: 'Workflows',
    sub: 'selectable when starting a task',
    actions: [h('button', { type: 'button', class: 'btn btn-primary btn-sm', disabled: !write, title: write ? null : 'Requires write access', onClick: () => edit(null) }, 'New workflow')],
  });
  const host = h('div');
  frame.body.append(host);

  const reload = loader(host, () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/workflows`), (list) => {
    const workflows = Array.isArray(list) ? list : [];
    if (workflows.length === 0) return h('div', { class: 'card placeholder' }, 'No workflows yet. A workflow seeds a task’s steps, instructions and required gates.');
    return h('div', { class: 'card list' }, workflows.map((wf) => {
      const canEdit = write && (wf.scope !== 'instance' || instanceAdmin);
      const gates = wf.gates || {};
      return h('div', { class: 'list-item' },
        h('div', { class: 'list-main' },
          h('div', { class: 'list-title' }, wf.name, badge(wf.scope || 'sandbox', wf.scope === 'instance' ? 'violet' : ''),
            gates.require_spec ? badge('spec required', 'accent') : null, gates.require_tests ? badge('tests required', 'accent') : null),
          h('div', { class: 'list-sub' }, wf.description || ''),
          h('div', { class: 'list-sub' }, Array.isArray(wf.steps) && wf.steps.length ? wf.steps.map((s, i) => `${i + 1}. ${s}`).join('   ') : 'No initial steps')),
        h('div', { class: 'list-actions' },
          h('button', { type: 'button', class: 'btn btn-sm', disabled: !canEdit, title: canEdit ? null : 'Instance workflows are managed by instance administrators', onClick: () => edit(wf) }, 'Edit'),
          h('button', { type: 'button', class: 'btn btn-sm btn-danger', disabled: !canEdit, onClick: () => remove(wf) }, 'Delete')));
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
          if (!name.value.trim()) { name.focus(); toast('Name is required.', { kind: 'warn' }); return false; }
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
