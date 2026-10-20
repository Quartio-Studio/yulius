// Connecting a Claude account comes first (SPEC-246): agents run on the instance's Claude
// subscriptions, so until one is usable nothing that starts an agent is offered, and the step that
// connects one is shown in its place.

import { h, icon } from './dom.js';
import { href } from './router.js';

/** Where an instance administrator connects a Claude account (Subscriptions, its sign-in open). */
export const CONNECT_PATH = '/admin/accounts?add=1';

/** The server's error code for an agent start refused without a usable account (CLA-002). */
export const ACCOUNT_REQUIRED = 'claude_account_required';

/** Whether a Claude account is usable (`/api/me` `instance.claude_connected`); unknown is yes. */
export function claudeConnected(me) {
  return !(me && me.instance && me.instance.claude_connected === false);
}

const isAdmin = (me) => Boolean(me && me.user && (me.user.is_superadmin || me.user.is_instance_admin));

/**
 * The required step while no Claude account is usable: a Connect Claude button for an instance
 * administrator, else who connects one. `what` says what waits for it.
 */
export function connectClaudeStep(me, what = 'Tasks and onboarding start once one is connected.') {
  return h('div', { class: 'banner banner-warn claude-connect', role: 'status' },
    icon('warn'),
    h('span', { class: 'banner-text' },
      h('strong', null, 'Connect Claude first. '),
      `Agents run on a Claude subscription, and none is connected yet. ${what}`,
      isAdmin(me) ? null : ' An instance administrator connects one in Subscriptions.'),
    isAdmin(me) ? h('a', { class: 'btn btn-sm btn-primary', href: href(CONNECT_PATH), dataset: { route: CONNECT_PATH } }, 'Connect Claude') : null);
}

/** The toast action for a refused start (CLA-007): Connect Claude, for those who can. */
export function connectAction(me, navigate) {
  return isAdmin(me) ? { label: 'Connect Claude', run: () => navigate(CONNECT_PATH) } : null;
}
