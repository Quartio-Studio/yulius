// Shared page scaffolding: frame, tables, async loading, pages that cannot be shown.
//
// One header pattern (#147): every page has a title and a subtitle that says what the page is for,
// in a few lowercase words (never the sandbox, which the tab row and the window title name, nor a
// count, which the page shows itself); actions on the right. A primary button that creates
// something starts with "+"; a key hint appears only on the button whose shortcut exists (New task).

import { h, setChildren, placeholder, loadError, keepFocus, emptyState, icon } from '../dom.js';
import { href } from '../router.js';

/** Page frame: title row with actions, then the content area. Returns {el, body, actions}. */
export function pageFrame({ title, sub, actions = [] }) {
  const actionsEl = h('div', { class: 'page-actions' }, actions);
  const body = h('div', { class: 'stack' });
  const subEl = h('span', { class: 'page-sub' }, sub || '');
  const el = h('div', { class: 'page' },
    h('div', { class: 'page-inner' },
      h('div', { class: 'page-head' }, h('h1', { class: 'page-title' }, title), subEl, actionsEl),
      body));
  return { el, body, actions: actionsEl, sub: subEl };
}

/** A primary "create" button: "+" and its label (the header pattern above). */
export function createButton(label, onClick, attrs = {}) {
  return h('button', { type: 'button', class: 'btn btn-primary', onClick, ...attrs }, icon('plus', { size: 14 }), label);
}

/**
 * A page that cannot be shown (#147): the missing sandbox, task, editor item or address. A heading
 * that says what is missing, one sentence of why, and the way back. Returns the page's inner
 * column; app.js puts it in a page, an editor in its own.
 */
export function notFound({ title, text, back: [label, path], level = 1 }) {
  return h('div', { class: 'page-inner not-found' },
    h('div', { class: 'page-head' }, h(`h${level}`, { class: 'page-title' }, title)),
    emptyState({ iconName: 'search', title: text, action: h('a', { class: 'btn', href: href(path), dataset: { route: path } }, icon('back', { size: 14 }), label) }));
}

/**
 * Data table. columns: [{label, render(row) → node|string, class, phoneHide}].
 * options: {onRowClick(row), empty (an emptyState, or a sentence), rowClass(row)}.
 */
export function dataTable(columns, rows, { onRowClick, empty = 'Nothing here yet.', rowClass } = {}) {
  if (!rows || rows.length === 0) return typeof empty === 'string' ? h('div', { class: 'card' }, placeholder(empty)) : empty;
  return h('div', { class: 'table-wrap' },
    h('table', { class: 'table' },
      h('thead', null, h('tr', null, columns.map((c) => h('th', { class: [c.class, c.phoneHide && 'hide-phone'] }, c.label || h('span', { class: 'sr-only' }, 'Actions'))))),
      h('tbody', null, rows.map((row) => {
        const tr = h('tr', { class: [onRowClick && 'clickable', rowClass && rowClass(row)] },
          columns.map((c) => h('td', { class: [c.class, c.phoneHide && 'hide-phone'] }, c.render(row))));
        if (onRowClick) {
          tr.tabIndex = 0;
          tr.addEventListener('click', (event) => { if (!event.target.closest('a, button, input, select, label')) onRowClick(row); });
          tr.addEventListener('keydown', (event) => { if (event.key === 'Enter' && event.target === tr) onRowClick(row); });
        }
        return tr;
      }))));
}

/**
 * Load data into a container: shows loading, then render(data) or the error, which names what
 * failed to load (`what`, such as "the users": "Couldn’t load the users: …").
 * Returns a reload function. Concurrent reloads keep only the latest result. `reload({ quiet })`
 * refreshes in place for live pages: no dimming, and a failure keeps what is shown.
 */
export function loader(container, what, fetcher, render) {
  let generation = 0;
  let hasContent = false;
  async function reload({ quiet = false } = {}) {
    generation += 1;
    const mine = generation;
    if (!hasContent) setChildren(container, placeholder(`Loading ${what}…`, 'loading'));
    else if (!quiet) container.style.opacity = '0.6';
    try {
      const data = await fetcher();
      if (mine !== generation) return;
      // A reload after an action (a switch, Run now) keeps keyboard focus on the same control.
      keepFocus(container, () => setChildren(container, render(data)));
      hasContent = true;
    } catch (error) {
      if (mine !== generation) return;
      if (quiet && hasContent) return;
      // Say it inline with a way out, like the Git page (UX2-07).
      setChildren(container, loadError(`Couldn’t load ${what}: ${error.message || String(error)}`, () => reload()));
      hasContent = false;
    } finally {
      if (mine === generation) container.style.opacity = '';
    }
  }
  reload();
  return reload;
}
