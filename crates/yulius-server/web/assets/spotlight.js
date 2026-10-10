// Spotlight-style search (SPEC-001 UI-036): "/" opens a floating search over the page, like
// Spotlight on a Mac. Results update as you type; ↑/↓ choose, Enter opens, ⌘/Ctrl+Enter takes the
// alternative action (when the result has one), Esc or a click outside closes. A combobox with a
// listbox (WAI-ARIA APG); focus returns where it was.

import { h, icon, setChildren } from './dom.js';
import { IS_MAC } from './shortcuts.js';

let open = null; // the dialog that is showing, so "/" twice does not stack two

/**
 * Open the search. `search(q)` returns results: { title, sub?, meta?, mark?, onPick, onAlt? }.
 * `more(q)` may return one last row (e.g. "Filter the list by …") shown after the results.
 */
export function openSpotlight({ label, placeholder, search, more, initial = '' }) {
  if (open) { open.querySelector('input').select(); return; }
  const returnTo = document.activeElement;
  let results = [];
  let active = 0;
  const id = `spot-${Math.random().toString(36).slice(2, 8)}`;
  const input = h('input', {
    class: 'spot-input', type: 'text', value: initial, placeholder, autocomplete: 'off', spellcheck: 'false',
    role: 'combobox', 'aria-expanded': 'true', 'aria-controls': `${id}-list`, 'aria-autocomplete': 'list', 'aria-label': label,
  });
  const list = h('ul', { class: 'spot-list', id: `${id}-list`, role: 'listbox', 'aria-label': 'Results' });
  const status = h('div', { class: 'sr-only', role: 'status' });
  const alt = IS_MAC ? '⌘' : 'Ctrl';
  const foot = h('div', { class: 'spot-foot', 'aria-hidden': 'true' },
    h('span', null, h('span', { class: 'kbd' }, '↑'), h('span', { class: 'kbd' }, '↓'), 'choose'),
    h('span', null, h('span', { class: 'kbd' }, '↵'), 'open'),
    h('span', { class: 'spot-foot-alt' }, h('span', { class: 'kbd' }, `${alt} ↵`), 'go to task'),
    h('span', null, h('span', { class: 'kbd' }, 'esc'), 'close'));
  const dialog = h('dialog', { class: 'spot', 'aria-label': label },
    h('div', { class: 'spot-field' }, icon('search', { size: 20 }), input),
    list, foot, status);

  function render() {
    const q = input.value.trim();
    results = search(q);
    const last = more ? more(q) : null;
    if (last) results = [...results, { ...last, more: true }];
    active = Math.min(active, Math.max(0, results.length - 1));
    setChildren(list, results.length ? results.map((r, i) => h('li', {
      id: `${id}-${i}`, role: 'option', 'aria-selected': String(i === active), class: ['spot-item', r.more && 'spot-more'],
      onPointermove: () => { if (active !== i) { active = i; mark(); } },
      onClick: (e) => pick(i, e.metaKey || e.ctrlKey),
    },
    h('span', { class: 'spot-mark' }, r.mark || icon(r.more ? 'search' : 'right', { size: 16 })),
    h('span', { class: 'spot-text' }, h('span', { class: 'spot-title' }, r.title), r.sub ? h('span', { class: 'spot-sub' }, r.sub) : null),
    r.meta ? h('span', { class: 'spot-meta' }, r.meta) : null)) : h('li', { class: 'spot-empty', role: 'presentation' }, q ? `Nothing matches “${q}”` : 'Nothing here yet'));
    const found = results.filter((r) => !r.more).length;
    status.textContent = q ? `${found} ${found === 1 ? 'result' : 'results'}` : '';
    foot.querySelector('.spot-foot-alt').hidden = !results.some((r) => r.onAlt);
    mark();
  }

  function mark() {
    list.querySelectorAll('.spot-item').forEach((el, i) => el.setAttribute('aria-selected', String(i === active)));
    const current = list.querySelector(`#${id}-${active}`);
    if (current) { input.setAttribute('aria-activedescendant', current.id); current.scrollIntoView({ block: 'nearest' }); } else input.removeAttribute('aria-activedescendant');
  }

  function close() {
    if (!dialog.open) return;
    dialog.close();
  }

  function pick(i, alternative) {
    const r = results[i];
    if (!r) return;
    close();
    if (alternative && r.onAlt) r.onAlt(); else r.onPick();
  }

  input.addEventListener('input', () => { active = 0; render(); });
  input.addEventListener('keydown', (event) => {
    const step = { ArrowDown: 1, ArrowUp: -1 }[event.key];
    if (step && results.length) { event.preventDefault(); active = (active + step + results.length) % results.length; mark(); return; }
    if (event.key === 'Home' && event.ctrlKey) { active = 0; mark(); return; }
    if (event.key === 'Enter') { event.preventDefault(); pick(active, event.metaKey || event.ctrlKey); }
  });
  // A click on the backdrop (outside the panel) closes.
  dialog.addEventListener('click', (event) => {
    const r = dialog.getBoundingClientRect();
    if (event.target === dialog && (event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom)) close();
  });
  dialog.addEventListener('close', () => {
    dialog.remove();
    open = null;
    if (returnTo && returnTo.isConnected && document.contains(returnTo)) returnTo.focus({ preventScroll: true });
  });

  document.body.append(dialog);
  open = dialog;
  render();
  dialog.showModal();
  input.focus();
  input.select();
  return { close };
}
