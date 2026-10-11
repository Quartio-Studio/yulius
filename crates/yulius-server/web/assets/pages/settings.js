// Settings (SPEC-001 ACC-010 as changed by F 2026-10-08 UI): one page for the signed-in user's
// account, this browser's preferences and the way into instance administration. It gathers what
// exists; the administration pages themselves are unchanged.

import { h, icon, toggle, openMenu, toast } from '../dom.js';
import { href } from '../router.js';
import { readJson, writeJson } from '../store.js';
import { initials } from '../layout.js';
import { findModel, findEffort, modelMenuItems, effortMenuItems, MODEL_PREF_KEY, EFFORT_PREF_KEY } from '../models.js';
import { pageFrame } from './common.js';
import { DESKTOP_KEY } from '../notifications.js';
import { showShortcuts, SHEET_KEYS } from '../shortcuts.js';
import { THEME_CHOICES, themeChoice, setThemeChoice } from '../theme.js';
import { BRAND } from '../brand.js';

function section(iconName, title, description, ...content) {
  return h('section', { class: 'settings-section' },
    h('div', { class: 'settings-head' }, h('h2', { class: 'card-title' }, icon(iconName, { size: 16 }), title), description ? h('span', { class: 'muted small' }, description) : null),
    h('div', { class: 'settings-body' }, content));
}

function row(iconName, title, description, control) {
  return h('div', { class: 'settings-row' },
    h('span', { class: 'settings-row-icon' }, icon(iconName, { size: 16 })),
    h('div', { class: 'settings-row-text' }, h('div', null, title), description ? h('div', { class: 'muted xs' }, description) : null),
    h('div', { class: 'settings-row-control' }, control));
}

/** Settings page: Your account · Preferences (this browser) · Administration (admins). */
export function mountSettingsPage(ctx) {
  const frame = pageFrame({ title: 'Settings', sub: 'your account, this browser and the instance' });
  const me = ctx.store.state.me || {};
  const user = me.user || {};
  const superadmin = Boolean(user.is_superadmin);
  const instanceAdmin = superadmin || Boolean(user.is_instance_admin);
  const role = superadmin ? 'Superadmin' : instanceAdmin ? 'Instance administrator' : 'Member';
  const name = user.display_name || user.login || 'Signed in';
  // Each control is named for its row, so a list of links or buttons reads apart (WCAG 2.4.4).
  const open = (path, what) => h('a', { class: 'btn btn-sm', href: href(path), dataset: { route: path }, 'aria-label': `Open ${what}` }, 'Open');

  // Preferences: the composer's defaults, the tools rail and the notification history.
  let modelId = findModel(readJson(MODEL_PREF_KEY, null)) ? readJson(MODEL_PREF_KEY, null) : null;
  let effortId = findEffort(readJson(EFFORT_PREF_KEY, null)) ? readJson(EFFORT_PREF_KEY, null) : null;
  const modelButton = h('button', { type: 'button', class: 'chip-btn', 'aria-haspopup': 'menu' });
  const effortButton = h('button', { type: 'button', class: 'chip-btn', 'aria-haspopup': 'menu' });
  const renderPicks = () => {
    const found = findModel(modelId);
    modelButton.replaceChildren(h('span', { class: 'sr-only' }, 'Model for new tasks: '), h('span', { class: 'chip-label' }, found ? found.model.name : 'Default model'), icon('down', { size: 12 }));
    const effort = findEffort(effortId);
    effortButton.replaceChildren(h('span', { class: 'sr-only' }, 'Effort for new tasks: '), h('span', { class: 'chip-label' }, effort ? effort.name : 'Default effort'), icon('down', { size: 12 }));
  };
  modelButton.addEventListener('click', () => openMenu(modelButton, modelMenuItems(modelId, (id) => { modelId = id; writeJson(MODEL_PREF_KEY, id); renderPicks(); })));
  effortButton.addEventListener('click', () => openMenu(effortButton, effortMenuItems(effortId, (id) => { effortId = id; writeJson(EFFORT_PREF_KEY, id); renderPicks(); })));
  renderPicks();

  const appearance = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Appearance' }, THEME_CHOICES.map((choice) => h('label', null,
    h('input', { type: 'radio', name: 'appearance', value: choice.id, checked: themeChoice() === choice.id, onChange: () => setThemeChoice(choice.id) }),
    choice.name)));

  const railToggle = toggle({ checked: ctx.railVisible ? ctx.railVisible() : false, label: 'Show the tools rail', onChange: (checked) => { if (ctx.setRailVisible) ctx.setRailVisible(checked); } });
  const screenReaderToggle = toggle({ checked: readJson('yulius.terminalScreenReader', false) === true, label: 'Terminal screen reader support', onChange: (checked) => writeJson('yulius.terminalScreenReader', checked) });
  // WCAG 2.1.4: single-character shortcuts can be turned off (pages/tasks.js reads this key).
  const keysToggle = toggle({ checked: readJson('yulius.singleKeyShortcuts', true) !== false, label: 'Single-key shortcuts', onChange: (checked) => writeJson('yulius.singleKeyShortcuts', checked) });
  const notifCount = h('span', { class: 'muted xs' });
  const clearButton = h('button', { type: 'button', class: 'btn btn-sm', onClick: () => {
    if (ctx.notifications) ctx.notifications.clear();
    renderNotif();
    toast('Notification history cleared.', { kind: 'ok' });
  }, 'aria-label': 'Clear notification history' }, 'Clear');
  const renderNotif = () => {
    const n = ctx.notifications ? ctx.notifications.count() : 0;
    notifCount.textContent = n ? `${n} in history` : 'Empty';
    clearButton.disabled = n === 0;
  };
  renderNotif();
  // Desktop notifications need the browser's permission, asked only when switched on.
  const supported = typeof Notification !== 'undefined' && window.isSecureContext;
  const desktopNote = h('span', null, supported
    ? 'While this tab is in the background: tasks that need you, failures, messages'
    : 'Not available here: the browser needs HTTPS (or localhost) and notification support');
  const desktopToggle = toggle({
    checked: supported && readJson(DESKTOP_KEY, false) === true && Notification.permission === 'granted',
    disabled: !supported,
    label: 'Desktop notifications',
    onChange: async (checked, input) => {
      if (!checked) { writeJson(DESKTOP_KEY, false); return; }
      const permission = Notification.permission === 'granted' ? 'granted' : await Notification.requestPermission();
      if (permission !== 'granted') {
        input.checked = false;
        writeJson(DESKTOP_KEY, false);
        desktopNote.textContent = 'Blocked by the browser: allow notifications for this site in its settings, then switch this on again';
        return;
      }
      writeJson(DESKTOP_KEY, true);
    },
  });

  frame.body.append(
    section('user', 'Your account', null,
      h('div', { class: 'settings-identity' },
        h('span', { class: 'avatar settings-avatar', 'aria-hidden': 'true' }, initials(name)),
        h('div', null, h('div', { class: 'settings-name' }, name), h('div', { class: 'muted small' }, [user.login ? `@${user.login}` : null, role].filter(Boolean).join(' · ')))),
      row('archive', 'Archived tasks', 'Tasks you archived, with restore', open('/archived', 'archived tasks')),
      row('key', 'CLI device approval', 'Approve “yulius login” from a terminal', open('/device', 'CLI device approval')),
      row('logout', 'Sign out', `Sign out of ${BRAND.name} in this browser`, h('button', { type: 'button', class: 'btn btn-sm', onClick: () => ctx.signOut() }, 'Sign out'))),
    section('gear', 'Preferences', 'kept in this browser',
      row('contrast', 'Appearance', 'Dark, light, or follow your system', appearance),
      row('target', 'Default model for new tasks', 'What the composer starts with', modelButton),
      row('pulse', 'Default effort for new tasks', 'How hard the agent thinks by default', effortButton),
      row('tools', 'Show the tools rail', 'Browser, Editor, Shell and Messages on the right of a sandbox', railToggle)),
    section('key', 'Keyboard and accessibility', 'kept in this browser',
      row('key', 'Keyboard shortcuts', `Every shortcut in one list (${SHEET_KEYS})`, h('button', { type: 'button', class: 'btn btn-sm', onClick: showShortcuts }, 'Show')),
      row('target', 'Single-key shortcuts', 'On the Tasks page: / to search, j and k to move, . for a task’s menu', keysToggle),
      row('terminal', 'Terminal screen reader support', 'Lets screen readers read terminal output; applies to terminals opened next', screenReaderToggle)),
    section('bell', 'Notifications', 'kept in this browser',
      row('bell', 'Desktop notifications', desktopNote, desktopToggle),
      row('clock', 'Notification history', notifCount, clearButton)),
    section('book', 'Help', null,
      row('code', 'Design system', 'For developers: colours, type, components and themes in one place', open('/design', 'the design system'))),
  );

  if (instanceAdmin) {
    // Settings is outside any sandbox: the per-sandbox tiles open the sandbox used last.
    const sandbox = ctx.defaultSandbox ? ctx.defaultSandbox() : ctx.store.state.sandboxes[0] || null;
    const slug = sandbox ? encodeURIComponent(sandbox.slug) : null;
    const tile = (iconName, title, description, path) => path
      ? h('a', { class: 'settings-tile', href: href(path), dataset: { route: path } }, icon(iconName, { size: 18 }), h('div', null, h('div', { class: 'settings-tile-title' }, title), h('div', { class: 'muted xs' }, description)))
      // Unavailable: the reason is written on the tile (not only in a tooltip) and the text stays readable.
      : h('div', { class: 'settings-tile disabled', 'aria-disabled': 'true' }, icon(iconName, { size: 18 }), h('div', null, h('div', { class: 'settings-tile-title' }, title), h('div', { class: 'muted xs' }, `${description} — open a sandbox first`)));
    frame.body.append(section('shield', 'Administration', `this ${BRAND.name} instance`,
      row('user', 'Add a subscription', 'A Claude subscription for the agents; you finish Claude’s own sign-in in a terminal here', h('a', { class: 'btn btn-primary btn-sm', href: href('/admin/accounts?add=1'), dataset: { route: '/admin/accounts?add=1' } }, icon('plus', { size: 14 }), 'Add subscription')),
      h('div', { class: 'settings-tiles' },
        superadmin ? tile('users', 'Users', 'Who may sign in', '/admin/users') : null,
        tile('user', 'Subscriptions', 'Claude subscriptions, their usage and when to add more', '/admin/accounts'),
        superadmin ? tile('github', 'GitHub connection', 'The GitHub App and its installations', '/admin/github') : null,
        superadmin ? tile('download', 'Instance', 'Version, updates and the Jev key', '/admin/instance') : null,
        tile('plug', 'MCP for every sandbox', 'Tool servers attached in every sandbox', slug ? `/s/${slug}/mcp?scope=instance` : null),
        tile('book', 'Skills for every sandbox', 'Skills available in every sandbox', slug ? `/s/${slug}/skills?source=instance` : null))));
  }

  return { el: frame.el };
}
