// Skills page (SKL-005): catalog with source badges, enable toggle and content viewer.

import { enc } from '../api.js';
import { h, badge, openModal, toggle, toast, errorToast, setChildren, placeholder } from '../dom.js';
import { allows } from '../status.js';
import { pageFrame, loader } from './common.js';

const SOURCES = [['', 'All sources'], ['repository', 'Repository'], ['instance', 'Instance'], ['open-mercato', 'Open Mercato']];
const SOURCE_TONE = { repository: '', instance: 'violet', 'open-mercato': 'accent' };

/** Mount the Skills page; ?source=instance pre-filters (admin "Instance skills" entry). */
export function mountSkillsPage(ctx, sandbox, route) {
  const write = allows(sandbox.access, 'write');
  const frame = pageFrame({ title: 'Skills', sub: 'one catalog for Claude Code — activated automatically by relevance' });
  const source = h('select', { class: 'select', 'aria-label': 'Source filter' }, SOURCES.map(([v, l]) => h('option', { value: v }, l)));
  source.value = route.query.get('source') || '';
  const search = h('input', { class: 'input', type: 'search', placeholder: 'Search skills…', 'aria-label': 'Search skills' });
  const host = h('div');
  frame.body.append(h('div', { class: 'toolbar' }, search, source), host);
  let skills = [];

  function render() {
    const q = search.value.trim().toLowerCase();
    const shown = skills.filter((s) => (!source.value || s.source === source.value) && (!q || `${s.name} ${s.description}`.toLowerCase().includes(q)));
    frame.sub.textContent = `${shown.length} of ${skills.length} · activated automatically by relevance`;
    if (shown.length === 0) return h('div', { class: 'card placeholder' }, skills.length ? 'No skill matches.' : 'No skills found. Repository skills live in .agents/skills or .claude/skills.');
    return h('div', { class: 'card list' }, shown.map((skill) => {
      // Repository skills are always available inside the repository (PUT → 422);
      // instance skills are toggled by instance administrators.
      const repository = skill.source === 'repository';
      const canToggle = write && !repository && (skill.source !== 'instance' || ctx.isInstanceAdmin());
      const why = repository ? 'Repository skills are always available inside the repository' : !write ? 'Requires write access' : skill.source === 'instance' && !ctx.isInstanceAdmin() ? 'Instance skills are managed by instance administrators' : null;
      return h('div', { class: 'list-item' },
        h('div', { class: 'list-main' },
          h('div', { class: 'list-title' }, h('button', { type: 'button', class: 'btn-ghost btn btn-sm', style: { padding: 0, height: 'auto', fontWeight: 600, color: 'var(--text)' }, onClick: () => view(skill) }, skill.name), badge(skill.source, SOURCE_TONE[skill.source] || '')),
          h('div', { class: 'list-sub', style: { whiteSpace: 'normal' } }, skill.description || ''),
          skill.path ? h('div', { class: 'list-sub mono' }, skill.path) : null),
        h('div', { class: 'list-actions' },
          h('button', { type: 'button', class: 'btn btn-sm', onClick: () => view(skill) }, 'View'),
          h('span', { title: why }, toggle({ checked: skill.enabled, disabled: !canToggle, label: `${skill.enabled ? 'Disable' : 'Enable'} ${skill.name}`, onChange: async (checked, input) => {
            try {
              await ctx.api.put(`/api/sandboxes/${enc(sandbox.id)}/skills/${enc(skill.name)}`, { enabled: checked });
              skill.enabled = checked;
              toast(`${skill.name} ${checked ? 'enabled' : 'disabled'}`, { kind: 'ok' });
            } catch (error) {
              input.checked = !checked;
              errorToast(error, 'Could not change skill');
            }
          } }))));
    }));
  }

  const reload = loader(host, () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/skills`), (list) => {
    skills = Array.isArray(list) ? list : [];
    return render();
  });
  const rerender = () => setChildren(host, render());
  search.addEventListener('input', rerender);
  source.addEventListener('change', rerender);

  function view(skill) {
    const content = h('pre', { class: 'code-view' }, '');
    const modal = openModal({ title: `${skill.name} · ${skill.source}`, wide: true, body: h('div', { class: 'stack' }, skill.path ? h('div', { class: 'muted small mono' }, skill.path) : null, placeholder('Loading…', 'loading')), actions: [{ label: 'Close' }] });
    ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/skills/${enc(skill.name)}`).then((data) => {
      content.textContent = data && data.content ? data.content : '';
      setChildren(modal.body, skill.path ? h('div', { class: 'muted small mono', style: { marginBottom: '8px' } }, skill.path) : null, content);
    }).catch((error) => setChildren(modal.body, placeholder(error.message, 'error')));
  }

  return { el: frame.el, key: `skills:${sandbox.id}:${route.query.get('source') || ''}`, reload };
}
