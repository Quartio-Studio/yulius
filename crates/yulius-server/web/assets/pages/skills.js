// Skills page (SKL-005, SKL-006): two groups instead of filters. Included: the small curated set
// that ships with the server (read-only, with its source and a thank-you to Open Mercato, whose
// skills it adapts); each can be switched off for this sandbox. Custom: this repository's skills
// (.agents/skills, .claude/skills; always on, edited in place) and the instance's. A skill opens in
// its editor (pages/skilledit.js, UI-033); New skill opens pages/skillnew.js (UI-035).

import { enc } from '../api.js';
import { h, toggle, toast, errorToast, iconBadge, itemCard, emptyState } from '../dom.js';
import { navigate } from '../router.js';
import { allows } from '../status.js';
import { pageFrame, loader } from './common.js';
import { BRAND } from '../brand.js';

/** How each source reads: icon, label and tone of its badge (any other source ships with the server). */
export const SOURCE_BADGE = { repository: ['branch', 'Repository', ''], instance: ['globe', 'Whole instance', 'violet'], 'open-mercato': ['lock', 'Included', ''] };
const CUSTOM = new Set(['repository', 'instance']);
const OPEN_MERCATO_SKILLS = 'https://github.com/open-mercato/skills';
const SUPERPOWERS_SKILLS = 'https://github.com/obra/superpowers';

/** Mount the Skills page; ?source=instance (admin "Instance skills" entry) starts at Custom. */
export function mountSkillsPage(ctx, sandbox, route) {
  const write = allows(sandbox.access, 'write');
  const frame = pageFrame({
    title: 'Skills',
    sub: 'Claude Code uses them by itself when they are relevant',
    actions: [h('button', { type: 'button', class: 'btn btn-primary', disabled: !write, title: write ? null : 'Requires write access', onClick: () => navigate(`/s/${encodeURIComponent(sandbox.slug)}/skills/new`) }, 'New skill')],
  });
  const host = h('div');
  frame.body.append(host);

  function card(skill) {
    // Repository skills are always available inside the repository (PUT → 422); instance skills
    // are switched by instance administrators.
    const repository = skill.source === 'repository';
    const canToggle = write && !repository && (skill.source !== 'instance' || ctx.isInstanceAdmin());
    const why = repository ? 'Repository skills are always on inside the repository' : !write ? 'Requires write access' : skill.source === 'instance' && !ctx.isInstanceAdmin() ? 'Instance skills are managed by instance administrators' : null;
    const [badgeIcon, badgeLabel, badgeTone] = SOURCE_BADGE[skill.source] || ['lock', 'Included', ''];
    return itemCard({
      iconName: 'book',
      title: skill.name,
      badges: [iconBadge(badgeIcon, badgeLabel, badgeTone, CUSTOM.has(skill.source) ? null : `Ships with ${BRAND.name}; read-only`)],
      description: skill.description || null,
      extra: CUSTOM.has(skill.source) && skill.path ? h('div', { class: 'item-meta mono' }, skill.path) : null,
      onOpen: () => navigate(`/s/${encodeURIComponent(sandbox.slug)}/skills/${encodeURIComponent(skill.name)}`),
      openLabel: `Open skill ${skill.name}`,
      dim: !skill.enabled,
      controls: repository ? [] : [h('span', { title: why || (skill.enabled ? 'On: Claude Code may use it in this sandbox' : 'Off in this sandbox') }, toggle({ checked: skill.enabled, disabled: !canToggle, label: `${skill.name} on`, onChange: async (checked, input) => {
        try {
          await ctx.api.put(`/api/sandboxes/${enc(sandbox.id)}/skills/${enc(skill.name)}`, { enabled: checked });
          skill.enabled = checked;
          input.closest('.item-card').classList.toggle('off', !checked);
          toast(`${skill.name} ${checked ? 'on' : 'off'} in ${sandbox.name}`, { kind: 'ok' });
        } catch (error) {
          input.checked = !checked;
          errorToast(error, 'Could not change skill');
        }
      } }))],
    });
  }

  const section = (id, title, hint, ...content) => h('section', { class: 'stack-sm', id: `${id}-section`, 'aria-labelledby': id },
    h('h2', { class: 'page-section-title', id }, title, h('span', { class: 'muted' }, hint)), ...content);

  loader(host, () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/skills`), (list) => {
    const skills = Array.isArray(list) ? list : [];
    const included = skills.filter((s) => !CUSTOM.has(s.source));
    const custom = skills.filter((s) => CUSTOM.has(s.source));
    const view = h('div', { class: 'stack' },
      section('skills-included', 'Included', `curated, kept current by ${BRAND.name}`,
        h('p', { class: 'page-section-note' }, `${BRAND.name}'s own delivery skills, and skills adapted from the `, h('a', { href: OPEN_MERCATO_SKILLS, target: '_blank', rel: 'noopener noreferrer' }, 'Open Mercato skills'),
          ' and ', h('a', { href: SUPERPOWERS_SKILLS, target: '_blank', rel: 'noopener noreferrer' }, 'obra/superpowers'),
          ', rewritten to fit any project. Thank you to the Open Mercato team and the superpowers authors for sharing them.'),
        included.length ? h('div', { class: 'item-list' }, included.map(card)) : h('p', { class: 'muted small' }, 'None in this version.')),
      section('skills-custom', 'Custom', 'this repository and the instance',
        custom.length ? h('div', { class: 'item-list' }, custom.map(card))
          : emptyState({ iconName: 'book', title: 'No custom skills yet', text: 'A skill is instructions Claude Code loads when they are relevant. Create one here, or add it to the repository under .agents/skills or .claude/skills.' })));
    if (route.query.get('source') === 'instance') requestAnimationFrame(() => view.querySelector('#skills-custom-section')?.scrollIntoView({ block: 'start' }));
    return view;
  });

  return { el: frame.el, key: `skills:${sandbox.id}:${route.query.get('source') || ''}` };
}
