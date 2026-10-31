// The CI tab's Security scan card (SPEC-359 VSB-004): the sandbox's Vidoc switch, the one the
// Instance page's Vidoc card and the Add sandbox dialog set (SPEC-338), for the sandbox's admins;
// what the instance switch (SPEC-356) and the repository's committed `.config/vidoc.toml`
// (ADR-0107) say; who changed it last. It reads `GET /api/sandboxes/{sid}/vidoc` and changes the
// switch with the sandbox PATCH, like the Instance card.

import { enc } from '../api.js';
import { h, icon, toggle, bar } from '../dom.js';
import { relLong, dateTime } from '../format.js';
import { setVidocScans } from '../instancekeys.js';
import { loader } from './common.js';

const TITLE = 'Security scan (Vidoc)';
const LABEL = 'Scan this project with Vidoc';

/** Mount the card for `sandbox` (nothing for a playground). Returns `{ el, destroy }`. */
export function mountVidocCard(ctx, sandbox) {
  const el = h('div');
  if (sandbox.playground) return { el, destroy() {} };
  let shownProject = null;
  const reload = loader(el, 'the security scan', () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/vidoc`), render, { shape: loading });
  // The same switch changed elsewhere (the Instance card, another tab, a live update).
  const off = ctx.store.on('sandboxes', () => {
    const now = (ctx.store.state.sandboxes || []).find((sb) => sb.id === sandbox.id);
    const on = !now || !now.profile || now.profile.vidoc !== false;
    if (shownProject !== null && on !== shownProject) reload({ quiet: true });
  });
  return { el, destroy: off };

  function loading() {
    return h('section', { class: 'card card-pad stack-sm' }, h('div', { class: 'card-title' }, bar('lg')), bar(60));
  }

  function render(view) {
    shownProject = view.project;
    const repositoryOff = view.repository === 'off';
    const locked = repositoryOff || !view.instance;
    const control = toggle({
      checked: repositoryOff ? false : view.project,
      disabled: locked || !view.can_change,
      label: LABEL,
      onChange: async (on, input) => {
        // The switch is disabled while the change is sent (which drops focus) and drawn again
        // after it: the keyboard comes back to the new switch.
        const focused = document.activeElement === input;
        if (await setVidocScans(ctx, sandbox, on, { input })) await reload({ quiet: true });
        const now = el.querySelector('input[role="switch"]');
        const lost = !document.activeElement || document.activeElement === document.body;
        if (focused && lost && now && !now.disabled) now.focus();
      },
    });
    const notes = [h('p', { class: 'muted xs' }, state(view))];
    if (view.repository === 'unreadable') {
      notes.push(h('p', { class: 'xs vidoc-warn' }, icon('warn', { size: 14 }), ` ${view.repository_problem || '.config/vidoc.toml can’t be read'} (on ${view.branch}): scans end “not run” until it is fixed.`));
    }
    if (!view.can_change && !locked) {
      notes.push(h('p', { class: 'muted xs' }, `Only this sandbox’s admins (admin on ${sandbox.repo.owner}/${sandbox.repo.name} in GitHub) or a superadmin can change it.`));
    }
    if (view.changed_by) {
      notes.push(h('p', { class: 'muted xs' }, `Last changed by ${view.changed_by} `, h('span', { title: dateTime(view.changed_at) }, relLong(view.changed_at)), '.'));
    }
    return h('section', { class: 'card card-pad stack-sm vidoc-card', id: 'vidoc', 'aria-label': TITLE },
      h('h2', { class: 'card-title' }, icon('shield', { size: 16 }), TITLE),
      h('div', { class: 'row vidoc-project' }, h('span', { class: 'vidoc-project-name' }, LABEL), control),
      notes);
  }
}

/** What the switch means now, most binding first: the repository, the instance, the project. */
function state(view) {
  if (view.repository === 'off') {
    return `The repository turned Vidoc off: .config/vidoc.toml on ${view.branch} sets enabled = false, which wins over this switch. Nothing is scanned or sent to Vidoc; a reviewed pull request that sets enabled = true turns it on again.`;
  }
  if (!view.instance) {
    return `A superadmin switched Vidoc off for the whole instance: no project scans until it is switched on again in Settings, Instance. This project’s own choice is kept: ${view.project ? 'on' : 'off'}.`;
  }
  if (!view.project) {
    return 'Off: agents and CI scan nothing and send nothing to Vidoc, they report the scan as disabled, and nobody is asked about the key or its quota, until it is switched on again.';
  }
  if (!view.key) {
    return 'On, but no Vidoc key reaches this project (neither the instance’s nor its own VIDOC_API_KEY secret), so scans end “not run”. Switch it off to have them say “disabled”.';
  }
  return 'On: agents scan their changes before delivery and CI’s security job scans each pull request. Switch it off while Vidoc refuses the key or its quota is used up, and on again once it works.';
}
