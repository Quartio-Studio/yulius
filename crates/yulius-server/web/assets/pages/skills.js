// Skills page (SKL-005): catalog with source badges, enable toggle and content viewer.

import { enc } from '../api.js';
import { h, openModal, toggle, toast, errorToast, setChildren, placeholder, iconBadge, itemCard, emptyState } from '../dom.js';
import { allows } from '../status.js';
import { pageFrame, loader } from './common.js';

const SOURCES = [['', 'All sources'], ['repository', 'Repository'], ['instance', 'Instance'], ['open-mercato', 'Open Mercato']];
/** How each source reads: icon, label and tone of its badge. */
const SOURCE_BADGE = { repository: ['branch', 'Repository', ''], instance: ['globe', 'Whole instance', 'violet'], 'open-mercato': ['book', 'Open Mercato', ''] };

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
    if (shown.length === 0) return skills.length
      ? emptyState({ iconName: 'book', title: 'No skill matches', text: 'Try another word or source.' })
      : emptyState({ iconName: 'book', title: 'No skills yet', text: 'Skills are instructions Claude Code loads when they are relevant. Add them to the repository under .agents/skills or .claude/skills.' });
    return h('div', { class: 'item-list' }, shown.map((skill) => {
      // Repository skills are always available inside the repository (PUT → 422);
      // instance skills are toggled by instance administrators.
      const repository = skill.source === 'repository';
      const canToggle = write && !repository && (skill.source !== 'instance' || ctx.isInstanceAdmin());
      const why = repository ? 'Repository skills are always available inside the repository' : !write ? 'Requires write access' : skill.source === 'instance' && !ctx.isInstanceAdmin() ? 'Instance skills are managed by instance administrators' : null;
      const [badgeIcon, badgeLabel, badgeTone] = SOURCE_BADGE[skill.source] || ['book', skill.source, ''];
      return itemCard({
        iconName: 'book',
        title: skill.name,
        badges: [iconBadge(badgeIcon, badgeLabel, badgeTone)],
        description: skill.description || null,
        extra: skill.path ? h('div', { class: 'item-meta mono' }, skill.path) : null,
        onOpen: () => view(skill),
        openLabel: `Open skill ${skill.name}`,
        dim: !skill.enabled,
        controls: [h('span', { title: why || (skill.enabled ? 'On: Claude Code may use it' : 'Off') }, toggle({ checked: skill.enabled, disabled: !canToggle, label: `${skill.name} enabled`, onChange: async (checked, input) => {
            try {
              await ctx.api.put(`/api/sandboxes/${enc(sandbox.id)}/skills/${enc(skill.name)}`, { enabled: checked });
              skill.enabled = checked;
              toast(`${skill.name} ${checked ? 'enabled' : 'disabled'}`, { kind: 'ok' });
            } catch (error) {
              input.checked = !checked;
              errorToast(error, 'Could not change skill');
            }
          } }))],
      });
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
      setChildren(modal.body, skill.path ? h('div', { class: 'muted small mono space-bottom' }, skill.path) : null, content);
    }).catch((error) => setChildren(modal.body, placeholder(error.message, 'error')));
  }

  return { el: frame.el, key: `skills:${sandbox.id}:${route.query.get('source') || ''}`, reload };
}
