// Keyboard shortcuts (SPEC-001 UI-029): the platform's key names and one sheet that lists every
// shortcut, opened with ⌘/ (Ctrl+/) or from Settings. Keep the sheet in step with the handlers in
// app.js (new task, home, F6, this sheet), taskview.js (composer), pages/tasks.js (single keys), dom.js
// (menus), editorview.js (save, word wrap) and splitter.js (column edges).

import { h, openModal } from './dom.js';
import { readJson } from './store.js';

export const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '');
const MOD = IS_MAC ? '⌘' : 'Ctrl+';

/** The new-task shortcut as shown ("⌘⇧O" / "Ctrl+Shift+O") and for aria-keyshortcuts. */
export const NEW_TASK_KEYS = IS_MAC ? '⌘⇧O' : 'Ctrl+Shift+O';
export const NEW_TASK_ARIA = IS_MAC ? 'Meta+Shift+O' : 'Control+Shift+O';
/** All sandboxes, the home screen ("⌘⇧H" / "Ctrl+Shift+H"). */
export const HOME_KEYS = IS_MAC ? '⌘⇧H' : 'Ctrl+Shift+H';
export const HOME_ARIA = IS_MAC ? 'Meta+Shift+H' : 'Control+Shift+H';
/** This sheet's own shortcut. */
export const SHEET_KEYS = `${MOD}/`;
/** Saving in the editor. */
export const SAVE_KEYS = `${MOD}S`;
/** Word wrap on/off in the editor, as in VS Code: ⌥Z / Alt+Z (ARIA names the Option key Alt). */
export const WRAP_KEYS = IS_MAC ? '⌥Z' : 'Alt+Z';
export const WRAP_ARIA = 'Alt+Z';

function groups() {
  const singleKeys = readJson('yulius.singleKeyShortcuts', true) !== false;
  return [
    ['Everywhere', [
      [IS_MAC ? [NEW_TASK_KEYS, '⌘N'] : [NEW_TASK_KEYS], IS_MAC ? 'New task (⌘N where the browser passes it on)' : 'New task'],
      [[HOME_KEYS], 'All sandboxes (home)'],
      [[SHEET_KEYS], 'Keyboard shortcuts (this list)'],
      [['F6', 'Shift+F6'], 'Next / previous area — also leaves the terminal and the browser view'],
      [['Esc'], 'Close a menu, dialog or the phone drawer'],
      [['Enter'], 'In a dialog’s field: confirm the dialog'],
    ]],
    ['New task', [
      [['Enter'], 'Start the task'],
      [['Shift+Enter'], 'New line'],
    ]],
    [singleKeys ? 'Tasks page' : 'Tasks page (single-key shortcuts are off in Settings)', [
      [['/'], 'Search'],
      [['j', 'k'], 'Next / previous task'],
      [['↓', '↑'], 'Next / previous task, once one is focused'],
      [['.'], 'The focused task’s menu'],
    ]],
    ['Search', [
      [['↑', '↓'], 'Choose a result'],
      [['Enter'], 'Open it'],
      [[`${MOD}Enter`], 'Go to the task itself (not its overview)'],
    ]],
    ['Menus', [
      [['↑', '↓'], 'Move'],
      [['Home', 'End'], 'First / last item'],
      [['a–z'], 'Next item starting with the letter'],
      [['Tab'], 'Close the menu'],
    ]],
    ['Editor', [
      [[SAVE_KEYS], 'Save the file'],
      [[WRAP_KEYS], 'Word wrap on / off — every open file'],
    ]],
    ['Browser, editor and shell tabs', [
      [['←', '→'], 'Move between tabs'],
      [['Home', 'End'], 'First / last tab'],
      [['Enter', 'Space'], 'Show the focused tab'],
      [['Delete', 'Backspace'], 'Close the focused tab'],
      [['F2'], 'Rename the focused shell'],
    ]],
    ['Column edges (sidebar, explorer, panel)', [
      [['←', '→'], 'Narrower / wider — Shift for bigger steps'],
      [['Home', 'End'], 'Narrowest / widest'],
      [['Double-click'], 'Back to the default width'],
    ]],
  ];
}

let open = null;

/** Opens the shortcut sheet (or closes it when it is already open). */
export function showShortcuts() {
  if (open) { open.close(); return; }
  const body = h('div', { class: 'shortcuts' }, groups().map(([title, rows]) => h('section', null,
    h('h3', { class: 'shortcuts-title' }, title),
    h('dl', { class: 'shortcuts-list' }, rows.flatMap(([keys, what]) => [
      h('dt', null, keys.flatMap((key, index) => [index ? h('span', { class: 'muted' }, ' / ') : null, h('kbd', { class: 'kbd' }, key)])),
      h('dd', null, what),
    ])))));
  open = openModal({ title: 'Keyboard shortcuts', wide: true, body, actions: [{ label: 'Close' }], onClose: () => { open = null; } });
}
