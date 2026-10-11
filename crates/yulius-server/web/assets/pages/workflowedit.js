// Workflows → one workflow (SPEC-001 UI-034, AUT-006): a builder with as little typing as
// possible. A workflow is a task recipe picked when a task starts: its steps seed the task's plan,
// its instructions go to the agent, its gates hold delivery until the task has a valid spec and/or
// tests, and its scope says where it can be picked. A new one starts from a default workflow (a
// custom copy: Duplicate on a default opens this page with ?from=<id>) or Blank, then click or drag
// steps from the library, drag to reorder (Alt+↑/↓ from the keyboard), switch the gates, click
// instruction snippets; the summary says what a task started with it gets. A default workflow
// opens read-only: it ships with the server and stays current with it.

import { enc } from '../api.js';
import { h, icon, iconButton, setChildren, placeholder, loadError, confirmDialog, toast, invalid } from '../dom.js';
import { href, navigate, currentLocation } from '../router.js';
import { allows } from '../status.js';
import { BRAND } from '../brand.js';
import { workflowIcon } from './workflows.js';

const BLANK = { id: 'blank', name: 'Blank', icon: 'plus', description: 'Start from nothing.', copyName: '', steps: [], gates: { require_spec: false, require_tests: false }, instructions: '' };

/** A workflow as a starting point: its content, named as a custom copy. */
function asTemplate(wf) {
  return { id: wf.id, name: wf.name, icon: workflowIcon(wf), description: wf.description || '', copyName: `${wf.name} (custom)`, steps: [...(wf.steps || [])], gates: { require_spec: Boolean(wf.gates && wf.gates.require_spec), require_tests: Boolean(wf.gates && wf.gates.require_tests) }, instructions: wf.instructions || '' };
}
const STEP_LIBRARY = ['Write task spec', 'Plan', 'Reproduce with a failing test', 'Implement', 'Fix', 'Write tests', 'Run tests', 'Review the diff', 'Update docs', 'QA in the browser', 'Investigate', 'Write findings', 'Open PR'];
const SNIPPETS = ['Follow AGENTS.md.', 'Keep the spec Plan checklist in sync.', 'Ask before changing public APIs or the database schema.', 'Prefer small commits with clear messages.', 'Check the change in the sandbox browser before opening the PR.'];

/** Mount the builder: `id` is a workflow id, or 'new'. */
export function mountWorkflowEditor(ctx, sandbox, id) {
  const base = `/s/${encodeURIComponent(sandbox.slug)}/workflows`;
  const creating = id === 'new';
  const write = allows(sandbox.access, 'write');
  const instanceAdmin = ctx.isInstanceAdmin();
  let wf = null; // the stored workflow when editing
  let state = null; // { name, description, steps, instructions, gates, scope }
  let template = null;
  let templates = [BLANK]; // the defaults, then Blank (filled by load)
  let saved = '';
  let canEdit = write;
  let alive = true;
  let editing = -1; // the step whose name is being edited
  let dragFrom = null; // { index } or { add: label }
  const live = h('div', { class: 'sr-only', role: 'status' });

  const nameInput = h('input', { class: 'wf-name', placeholder: 'Untitled workflow', 'aria-label': 'Workflow name', maxlength: 100, onInput: (e) => { state.name = e.target.value; changed(); } });
  const descInput = h('input', { class: 'wf-desc', placeholder: 'One line on when to use it (optional)', 'aria-label': 'Description', maxlength: 500, onInput: (e) => { state.description = e.target.value; changed(); } });
  const save = h('button', { type: 'button', class: 'btn btn-primary', onClick: () => doSave() }, creating ? 'Create workflow' : 'Save');
  const del = creating ? null : h('button', { type: 'button', class: 'btn btn-quiet-danger', onClick: () => doDelete() }, icon('trash', { size: 14 }), 'Delete');
  const body = h('div', { class: 'wf-body' }, placeholder('Loading…', 'loading'));
  const note = h('div');
  const head = h('header', { class: 'gt-head wf-head' },
    h('a', { class: 'gt-back', href: href(base), dataset: { route: base } }, icon('back', { size: 14 }), 'Workflows'),
    h('div', { class: 'gt-title-row' }, h('div', { class: 'wf-titles' }, nameInput, descInput), del, save),
    note);
  const el = h('div', { class: 'page wf-page' }, h('div', { class: 'page-inner wf-inner' }, h('h1', { class: 'sr-only' }, creating ? 'New workflow' : 'Edit workflow'), head, body, live));

  const snapshot = () => JSON.stringify(state);
  const dirty = () => state && snapshot() !== saved;
  function changed() { renderSummary(); save.disabled = !canEdit || (!creating && !dirty()); }

  // ---- start from a default or Blank (new workflows)
  function useTemplate(t) {
    template = t.id;
    const typed = state.name && !templates.some((x) => x.copyName === state.name);
    state = { ...state, name: typed ? state.name : t.copyName, description: t.id === 'blank' ? '' : t.description, steps: [...t.steps], instructions: t.instructions, gates: { ...t.gates } };
    nameInput.value = state.name;
    descInput.value = state.description;
    render();
    live.textContent = `Started from ${t.name}`;
  }

  // ---- steps
  const stepList = h('ol', { class: 'wf-steps', 'aria-label': 'Steps' });
  const dropLine = h('li', { class: 'wf-drop-line', 'aria-hidden': 'true' });

  function moveStep(from, to) {
    if (to < 0 || to >= state.steps.length || from === to) return;
    const [step] = state.steps.splice(from, 1);
    state.steps.splice(to, 0, step);
    renderSteps(to);
    live.textContent = `Moved ${step} to position ${to + 1} of ${state.steps.length}`;
    changed();
  }
  function addStep(label, at = state.steps.length) {
    const text = String(label || '').trim();
    if (!text) return;
    state.steps.splice(at, 0, text);
    renderSteps(at);
    live.textContent = `Added ${text} as step ${at + 1}`;
    changed();
  }
  function removeStep(i) {
    const [step] = state.steps.splice(i, 1);
    renderSteps(Math.min(i, state.steps.length - 1));
    live.textContent = `Removed ${step}`;
    changed();
  }

  /** Where a drop at pointer y lands among the steps. */
  function dropIndex(y) {
    const items = [...stepList.querySelectorAll('.wf-step')];
    for (let i = 0; i < items.length; i += 1) {
      const r = items[i].getBoundingClientRect();
      if (y < r.top + r.height / 2) return i;
    }
    return items.length;
  }

  function renderSteps(focusIndex) {
    const items = state.steps.map((text, i) => {
      const finish = (value) => {
        if (editing !== i) return; // Enter and the blur that follows finish once
        editing = -1;
        if (value !== null && value.trim()) state.steps[i] = value.trim();
        renderSteps(i);
        changed();
      };
      const name = editing === i
        ? h('input', { class: 'input wf-step-input', value: text, 'aria-label': `Step ${i + 1} name`,
          onKeydown: (e) => { if (e.key === 'Enter') { e.preventDefault(); finish(e.target.value); } if (e.key === 'Escape') { e.preventDefault(); finish(null); } },
          onBlur: (e) => finish(e.target.value) })
        : h('button', { type: 'button', class: 'wf-step-text', disabled: !canEdit, title: canEdit ? 'Rename' : null, onClick: () => { editing = i; renderSteps(); stepList.querySelector('.wf-step-input')?.focus(); } }, text);
      const item = h('li', { class: 'wf-step', draggable: canEdit ? 'true' : 'false', dataset: { index: String(i) },
        onKeydown: (e) => {
          if (!canEdit || !e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
          e.preventDefault();
          moveStep(i, i + (e.key === 'ArrowUp' ? -1 : 1));
        } },
        h('span', { class: 'wf-handle', 'aria-hidden': 'true', title: 'Drag to reorder' }, icon('grip', { size: 14 })),
        h('span', { class: 'wf-num', 'aria-hidden': 'true' }, String(i + 1)),
        name,
        canEdit ? h('span', { class: 'wf-step-actions' },
          iconButton('up', `Move ${text} up`, () => moveStep(i, i - 1), { disabled: i === 0 }),
          iconButton('down', `Move ${text} down`, () => moveStep(i, i + 1), { disabled: i === state.steps.length - 1 }),
          iconButton('x', `Remove ${text}`, () => removeStep(i))) : null);
      item.addEventListener('dragstart', (e) => { dragFrom = { index: i }; e.dataTransfer.effectAllowed = 'move'; e.dataTransfer.setData('text/plain', text); item.classList.add('dragging'); });
      item.addEventListener('dragend', () => { dragFrom = null; item.classList.remove('dragging'); dropLine.remove(); });
      return item;
    });
    setChildren(stepList, items.length ? items : h('li', { class: 'wf-empty' }, canEdit ? 'No steps yet: click or drag one from below.' : 'No steps.'));
    if (focusIndex !== undefined && focusIndex >= 0) stepList.querySelectorAll('.wf-step .wf-step-text')[focusIndex]?.focus({ preventScroll: true });
    renderLibrary();
    renderSummary();
  }

  stepList.addEventListener('dragover', (e) => {
    if (!dragFrom || !canEdit) return;
    e.preventDefault();
    const at = dropIndex(e.clientY);
    const items = stepList.querySelectorAll('.wf-step');
    if (at < items.length) items[at].before(dropLine); else stepList.append(dropLine);
  });
  stepList.addEventListener('dragleave', (e) => { if (!stepList.contains(e.relatedTarget)) dropLine.remove(); });
  stepList.addEventListener('drop', (e) => {
    if (!dragFrom || !canEdit) return;
    e.preventDefault();
    const at = dropIndex(e.clientY);
    dropLine.remove();
    if (dragFrom.add) addStep(dragFrom.add, at);
    else moveStep(dragFrom.index, at > dragFrom.index ? at - 1 : at);
    dragFrom = null;
  });

  // The library offers the steps not yet in the list, then a field for your own.
  const custom = h('input', { class: 'input wf-custom', placeholder: 'Your own step…', 'aria-label': 'Add your own step', maxlength: 120,
    onKeydown: (e) => { if (e.key === 'Enter') { e.preventDefault(); addStep(e.target.value); e.target.value = ''; custom.focus(); } } });
  const chips = h('div', { class: 'wf-chips' });
  function renderLibrary() {
    const used = new Set(state.steps.map((s) => s.toLowerCase()));
    setChildren(chips, STEP_LIBRARY.filter((label) => !used.has(label.toLowerCase())).map((label) => {
      const chip = h('button', { type: 'button', class: 'wf-chip', draggable: 'true', title: `Add “${label}” (or drag it into place)`, onClick: () => addStep(label) }, icon('plus', { size: 12 }), label);
      chip.addEventListener('dragstart', (e) => { dragFrom = { add: label }; e.dataTransfer.effectAllowed = 'copy'; e.dataTransfer.setData('text/plain', label); });
      chip.addEventListener('dragend', () => { dragFrom = null; dropLine.remove(); });
      return chip;
    }), custom);
  }
  const library = () => h('div', { class: 'wf-library' }, h('span', { class: 'wf-label' }, 'Add a step'), chips);

  // ---- gates, instructions, scope
  function gateCard(key, title, text, iconName) {
    const input = h('input', { type: 'checkbox', role: 'switch', class: 'switch-input', checked: Boolean(state.gates[key]), disabled: !canEdit, onChange: (e) => { state.gates[key] = e.target.checked; gate.classList.toggle('on', e.target.checked); changed(); } });
    const gate = h('label', { class: ['wf-gate', state.gates[key] && 'on'] }, h('span', { class: 'wf-gate-icon' }, icon(iconName, { size: 16 })), h('span', { class: 'wf-gate-text' }, h('strong', null, title), h('span', { class: 'muted small' }, text)), h('span', { class: 'switch' }, input, h('span', { class: 'switch-track', 'aria-hidden': 'true' })));
    return gate;
  }

  function instructionsBox() {
    const area = h('textarea', { class: 'textarea wf-instructions', rows: 3, value: state.instructions || '', readOnly: !canEdit, placeholder: 'Anything else the agent should know (optional)', 'aria-label': 'Instructions for the agent', onInput: (e) => { state.instructions = e.target.value; renderSnippets(); changed(); } });
    const snippets = h('div', { class: 'wf-chips' });
    function renderSnippets() {
      setChildren(snippets, SNIPPETS.map((text) => {
        const on = (state.instructions || '').includes(text);
        return h('button', { type: 'button', class: ['wf-chip', on && 'on'], 'aria-pressed': String(on), disabled: !canEdit, onClick: () => {
          state.instructions = on ? state.instructions.replace(text, '').replace(/\s{2,}/g, ' ').trim() : `${(state.instructions || '').trim()} ${text}`.trim();
          area.value = state.instructions;
          renderSnippets();
          changed();
        } }, icon(on ? 'check' : 'plus', { size: 12 }), text);
      }));
    }
    renderSnippets();
    return h('div', { class: 'wf-instr' }, snippets, area);
  }

  function scopeSwitch() {
    const option = (value, label) => h('label', null, h('input', { type: 'radio', name: 'wf-scope', value, checked: state.scope === value, disabled: !canEdit || !creating || (value === 'instance' && !instanceAdmin), onChange: () => { state.scope = value; changed(); } }), label);
    return h('div', { class: 'wf-scope' },
      h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Available in' }, option('sandbox', `This sandbox (${sandbox.name})`), option('instance', 'Every sandbox')),
      !creating ? h('span', { class: 'muted xs' }, 'Set when the workflow is created.') : !instanceAdmin ? h('span', { class: 'muted xs' }, 'Every sandbox: instance administrators only.') : null);
  }

  // ---- the summary: what a task started with it gets
  const summary = h('aside', { class: 'card wf-summary', 'aria-label': 'What a task gets' });
  function renderSummary() {
    if (!state) return;
    const gates = [state.gates.require_spec && 'a valid task spec', state.gates.require_tests && 'tests'].filter(Boolean);
    setChildren(summary,
      h('h2', { class: 'ti-card-title' }, icon('target', { size: 14 }), 'A task started with it'),
      h('p', { class: 'small' }, 'Begins with this plan:'),
      state.steps.length ? h('ol', { class: 'wf-summary-steps' }, state.steps.map((s) => h('li', null, s))) : h('p', { class: 'muted small' }, 'No plan yet: the agent writes its own.'),
      h('p', { class: 'small' }, 'The agent is told:'),
      h('p', { class: 'wf-summary-quote small' }, (state.instructions || '').trim() || h('span', { class: 'muted' }, 'nothing extra')),
      h('p', { class: 'small' }, gates.length ? ['Can be delivered once it has ', h('strong', null, gates.join(' and ')), '.'] : 'Can be delivered without a spec or tests.'),
      h('p', { class: 'muted xs' }, state.scope === 'instance' ? 'Can be picked in every sandbox.' : `Can be picked in ${sandbox.name}.`));
  }

  function section(title, hint, content) {
    return h('section', { class: 'wf-section' }, h('h2', { class: 'wf-section-title' }, title, hint ? h('span', { class: 'muted small' }, hint) : null), content);
  }

  function render() {
    renderSummary();
    setChildren(body,
      h('div', { class: 'wf-grid' },
        h('div', { class: 'wf-main' },
          creating ? section('Start from', 'a default, or nothing', h('div', { class: 'wf-templates', role: 'radiogroup', 'aria-label': 'Template' }, templates.map((t) => h('button', {
            type: 'button', role: 'radio', 'aria-checked': String(template === t.id), class: ['wf-template', template === t.id && 'on'], onClick: () => useTemplate(t),
          }, h('span', { class: 'wf-template-icon' }, icon(t.icon, { size: 16 })), h('span', { class: 'wf-template-name' }, t.name), h('span', { class: 'muted xs' }, t.description))))) : null,
          section('Steps', canEdit ? 'drag to reorder · Alt+↑/↓' : null, [stepList, canEdit ? library() : null]),
          section('Before delivery', null, h('div', { class: 'wf-gates' },
            gateCard('require_spec', 'A valid task spec', 'The task writes a spec with its goal and plan before it can be merged.', 'book'),
            gateCard('require_tests', 'Tests', 'The task adds or runs tests before it can be merged.', 'check'))),
          section('Instructions', 'click to add', instructionsBox()),
          section('Available in', null, scopeSwitch())),
        summary));
    renderSteps();
    changed();
  }

  // ---- save, delete, leave
  async function doSave() {
    if (!state || !canEdit) return;
    if (!state.name.trim()) { invalid(nameInput, 'Give the workflow a name.'); return; }
    const payload = { name: state.name.trim(), description: state.description.trim(), steps: state.steps, instructions: state.instructions || '', gates: state.gates, scope: state.scope };
    save.classList.add('busy');
    try {
      if (creating) await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/workflows`, payload);
      else await ctx.api.put(`/api/workflows/${enc(id)}`, payload);
      saved = snapshot();
      toast(creating ? `Created ${payload.name}` : `Saved ${payload.name}`, { kind: 'ok' });
      navigate(base);
    } catch (error) {
      toast(`Couldn’t save: ${error.message}`, { kind: 'error' });
    } finally {
      save.classList.remove('busy');
    }
  }

  async function doDelete() {
    if (!(await confirmDialog({ title: `Delete workflow “${state.name}”?`, message: 'Tasks started with it keep their steps. New tasks can no longer pick it.', confirmLabel: 'Delete', danger: true }))) return;
    try {
      await ctx.api.del(`/api/workflows/${enc(id)}`);
      saved = snapshot();
      toast('Workflow deleted.', { kind: 'ok' });
      navigate(base);
    } catch (error) {
      toast(`Couldn’t delete: ${error.message}`, { kind: 'error' });
    }
  }

  const onBeforeUnload = (event) => { if (dirty()) { event.preventDefault(); event.returnValue = ''; } };
  const onLink = async (event) => {
    const link = event.target.closest && event.target.closest('a[data-route]');
    if (!link || !dirty() || event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    if (await confirmDialog({ title: 'Leave without saving?', message: 'Your changes to this workflow are not saved.', confirmLabel: 'Leave', danger: true })) { saved = snapshot(); navigate(link.dataset.route); }
  };
  window.addEventListener('beforeunload', onBeforeUnload);
  document.addEventListener('click', onLink, true);

  // A default workflow is read-only: Duplicate opens a custom copy in this builder.
  const duplicate = h('button', { type: 'button', class: 'btn btn-primary', hidden: true, onClick: () => navigate(`${base}/new?from=${encodeURIComponent(id)}`) }, 'Duplicate to customise');
  head.querySelector('.gt-title-row').append(duplicate);

  async function load() {
    try {
      const list = await ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/workflows`);
      if (!alive) return;
      const all = Array.isArray(list) ? list : [];
      templates = [...all.filter((w) => w.builtin).map(asTemplate), BLANK];
      if (!creating) {
        wf = all.find((w) => w.id === id);
        if (!wf) { setChildren(body, loadError('This workflow no longer exists.', () => navigate(base))); return; }
      }
      canEdit = write && (!wf || (!wf.builtin && (wf.scope !== 'instance' || instanceAdmin)));
      state = wf
        ? { name: wf.name || '', description: wf.description || '', steps: [...(wf.steps || [])], instructions: wf.instructions || '', gates: { require_spec: Boolean(wf.gates && wf.gates.require_spec), require_tests: Boolean(wf.gates && wf.gates.require_tests) }, scope: wf.scope || 'sandbox' }
        : { name: '', description: '', steps: [], instructions: '', gates: { require_spec: false, require_tests: false }, scope: 'sandbox' };
      saved = snapshot();
      nameInput.value = state.name;
      descInput.value = state.description;
      nameInput.readOnly = !canEdit;
      descInput.readOnly = !canEdit;
      save.hidden = !canEdit;
      if (del) del.hidden = !canEdit;
      duplicate.hidden = !(wf && wf.builtin && write);
      const why = wf && wf.builtin ? `A default workflow: it ships with ${BRAND.name} and stays current with each release. Duplicate it to change it.`
        : !write ? 'Read-only: changing workflows needs write access to this sandbox.' : 'Read-only: workflows for every sandbox are changed by instance administrators.';
      setChildren(note, canEdit ? null : h('p', { class: 'banner small' }, icon('lock', { size: 14 }), h('span', { class: 'banner-text' }, why)));
      if (creating) {
        const from = currentLocation().query.get('from');
        const source = from && all.find((w) => w.id === from);
        useTemplate(source ? asTemplate(source) : templates[0]);
      } else render();
      if (creating) saved = snapshot(); // a template alone is not a change worth guarding
    } catch (error) {
      if (alive) setChildren(body, loadError(`Couldn’t load the workflow: ${error.message}`, load));
    }
  }
  load();

  return {
    el,
    start() { if (creating) nameInput.focus(); },
    destroy() { alive = false; window.removeEventListener('beforeunload', onBeforeUnload); document.removeEventListener('click', onLink, true); },
  };
}
