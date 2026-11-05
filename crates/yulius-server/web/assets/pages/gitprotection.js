// The Git page's Production protection card (SPEC-423 PPR-005, ADR-0267): whether the production
// branch must be protected from agents with a GitHub ruleset (ADR-0114). A sandbox admin switches
// the requirement off when the GitHub plan has no rulesets (a private repository without GitHub
// Pro, Team or Enterprise), accepting that agents could write the branch; onboarding then stops
// asking for the ruleset. The switch is `profile.protect_production`, changed with the sandbox
// PATCH like the Vidoc switch. Onboarding's warnings and `production_protect`'s refusals link here
// (`?show=production-protection`, a query: in-app links and the sign-in redirect keep it).

import { enc } from '../api.js';
import { h, icon, toggle, toast, errorToast } from '../dom.js';
import { allows } from '../status.js';
import { BRAND } from '../brand.js';

export const PROTECTION_ID = 'production-protection';

// Per sandbox, the switch being sent ({ on }): a repaint while it is on its way shows it, disabled.
const pending = new Map();
// Per sandbox, the Git pages showing its card: each one's redraw, called once a change is
// answered, so a card drawn while it was on its way never stays (also on a page left and visited
// again meanwhile).
const redraws = new Map();

/**
 * Registers `redraw()`, which draws `sandboxId`'s card again from the stored sandbox at once, for
 * as long as the page shows it. Returns the unregister.
 */
export function watchProductionProtection(sandboxId, redraw) {
  if (!redraws.has(sandboxId)) redraws.set(sandboxId, new Set());
  redraws.get(sandboxId).add(redraw);
  return () => {
    const set = redraws.get(sandboxId);
    if (!set) return;
    set.delete(redraw);
    if (!set.size) redraws.delete(sandboxId);
  };
}

/** The card for `sandbox` (not a playground: it has no GitHub branch). */
export function productionProtectionCard(ctx, sandbox) {
  const branch = (sandbox.profile && sandbox.profile.production_branch) || 'production';
  const required = !sandbox.profile || sandbox.profile.protect_production !== false;
  const admin = allows(sandbox.access, 'admin');
  const sending = pending.get(sandbox.id);
  const label = `Require a GitHub ruleset on ${branch}`;
  const control = toggle({
    checked: sending ? sending.on : required,
    disabled: Boolean(sending) || !admin,
    label,
    onChange: async (on, input) => {
      // The switch is disabled while the change is sent (which drops focus) and drawn again
      // after it, as the Vidoc card does.
      const focused = document.activeElement === input;
      pending.set(sandbox.id, { on });
      input.disabled = true;
      try {
        const updated = await ctx.api.patch(`/api/sandboxes/${enc(sandbox.id)}`, { protect_production: on });
        pending.delete(sandbox.id);
        ctx.store.upsertSandbox(updated);
        toast(on ? `${BRAND.name} asks for a ruleset on ${branch} again.` : `Production protection is off: ${BRAND.name} no longer asks for a ruleset on ${branch}.`, { kind: 'ok' });
      } catch (error) {
        pending.delete(sandbox.id);
        errorToast(error, 'Couldn’t change the production protection');
      }
      for (const redraw of [...(redraws.get(sandbox.id) || [])]) redraw();
      const now = document.querySelector(`#${PROTECTION_ID} input[role="switch"]`);
      const lost = !document.activeElement || document.activeElement === document.body;
      if (focused && lost && now && !now.disabled) now.focus();
    },
  });
  const rules = sandbox.repo && sandbox.repo.url ? `${sandbox.repo.url}/settings/rules` : null;
  const notes = [h('p', { class: 'muted xs' }, required
    ? `On: agents must not be able to push to, merge into or delete ${branch}. Onboarding checks it on GitHub and asks for a branch ruleset (Restrict updates, Restrict deletions, Block force pushes, only repository admins bypass) until one is in place.`
    : `Off: this project accepted that agents could push to or merge into ${branch}. ${BRAND.name} no longer warns about it or asks for a ruleset. Switch it on once the repository can have rulesets (GitHub Pro, Team or Enterprise, or a public repository).`)];
  if (required) {
    notes.push(h('p', { class: 'muted xs' }, 'Without GitHub Pro, Team or Enterprise a private repository cannot have rulesets: switch this off to accept the risk instead.'));
  }
  if (!admin) {
    notes.push(h('p', { class: 'muted xs' }, `Only this sandbox’s admins (admin on ${sandbox.repo.owner}/${sandbox.repo.name} in GitHub) or a superadmin can change it.`));
  }
  return h('section', { class: 'card card-pad stack-sm vidoc-card', id: PROTECTION_ID, 'aria-label': 'Production protection' },
    h('h2', { class: 'card-title' }, icon('shield', { size: 16 }), 'Production protection'),
    h('div', { class: 'row vidoc-project' }, h('span', { class: 'vidoc-project-name' }, label), control),
    notes,
    rules ? h('a', { class: 'small', href: rules, target: '_blank', rel: 'noopener noreferrer' }, 'Rulesets on GitHub', icon('external', { size: 12 })) : null);
}
