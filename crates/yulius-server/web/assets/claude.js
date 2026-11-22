// Connecting a Claude account comes first (SPEC-246): agents run on the instance's Claude
// subscriptions, so until one is usable nothing that starts an agent is offered, and the step that
// connects one is shown in its place.

import { h, icon } from './dom.js';
import { href } from './router.js';

/** Where an instance administrator connects a Claude account (Subscriptions, its sign-in open). */
export const CONNECT_PATH = '/admin/accounts?add=1';
/** Where an instance administrator connects a Codex account (the Add dialog on Codex, SPEC-503). */
export const CONNECT_CODEX_PATH = '/admin/accounts?add=codex';
/** Where an instance administrator connects an Antigravity account (SPEC-505). */
export const CONNECT_ANTIGRAVITY_PATH = '/admin/accounts?add=antigravity';

/** The server's error code for an agent start refused without a usable account (CLA-002). */
export const ACCOUNT_REQUIRED = 'claude_account_required';

/** Whether a Claude account is usable (`/api/me` `instance.claude_connected`); unknown is yes. */
export function claudeConnected(me) {
  return !(me && me.instance && me.instance.claude_connected === false);
}

/** Whether a Codex account is usable (`/api/me` `instance.codex_connected`, SPEC-503 CDX-014). */
export function codexConnected(me) {
  return Boolean(me && me.instance && me.instance.codex_connected);
}

/** Whether an Antigravity account is usable (`/api/me` `instance.antigravity_connected`, SPEC-505). */
export function antigravityConnected(me) {
  return Boolean(me && me.instance && me.instance.antigravity_connected);
}

const isAdmin = (me) => Boolean(me && me.user && (me.user.is_superadmin || me.user.is_instance_admin));

const CONNECTIONS = {
  claude: { name: 'Claude', subscription: 'a Claude subscription', path: CONNECT_PATH },
  codex: { name: 'Codex', subscription: 'a Codex subscription', path: CONNECT_CODEX_PATH },
  antigravity: { name: 'Antigravity', subscription: 'an Antigravity subscription', path: CONNECT_ANTIGRAVITY_PATH },
};

/** The subscription to connect while no agent account is usable. Home and onboarding need Claude. */
export function connectAccountStep(me, what = 'Tasks and onboarding start once one is usable.', harness = 'claude') {
  const { name, subscription, path } = CONNECTIONS[harness];
  return h('div', { class: 'banner banner-warn claude-connect', role: 'status' },
    icon('warn'),
    h('span', { class: 'banner-text' },
      h('strong', null, `Connect ${name} first. `),
      // None connected, or every one must sign in again: either is done in Subscriptions.
      isAdmin(me)
        ? `Agents run on ${subscription}, and none is usable yet: connect one, or sign one in again. ${what}`
        : `Agents run on ${subscription}, and none is usable yet. An instance administrator connects one in Subscriptions. ${what}`),
    isAdmin(me) ? h('a', { class: 'btn btn-sm btn-primary', href: href(path), dataset: { route: path } }, `Connect ${name}`) : null);
}

/**
 * The toast action for a refused start (CLA-007): Connect Claude, for those who can, or Connect
 * Codex or Antigravity when the refusal was for want of such an account (SPEC-503, SPEC-505).
 */
export function connectAction(me, navigate, error) {
  if (!isAdmin(me)) return null;
  const said = (error && error.message) || '';
  if (/\bCodex account\b/.test(said)) return { label: 'Connect Codex', run: () => navigate(CONNECT_CODEX_PATH) };
  if (/\bAntigravity account\b/.test(said)) return { label: 'Connect Antigravity', run: () => navigate(CONNECT_ANTIGRAVITY_PATH) };
  return { label: 'Connect Claude', run: () => navigate(CONNECT_PATH) };
}
