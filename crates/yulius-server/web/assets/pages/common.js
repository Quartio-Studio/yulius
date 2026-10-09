// Shared page scaffolding: frame, tables, async loading.

import { h, setChildren, placeholder, loadError, keepFocus } from '../dom.js';

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

/**
 * Data table. columns: [{label, render(row) → node|string, class, phoneHide}].
 * options: {onRowClick(row), empty, rowClass(row)}.
 */
export function dataTable(columns, rows, { onRowClick, empty = 'Nothing here yet.', rowClass } = {}) {
  if (!rows || rows.length === 0) return h('div', { class: 'card' }, placeholder(empty));
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
 * Load data into a container: shows loading, then render(data) or the error.
 * Returns a reload function. Concurrent reloads keep only the latest result.
 */
export function loader(container, fetcher, render) {
  let generation = 0;
  let hasContent = false;
  async function reload() {
    generation += 1;
    const mine = generation;
    if (!hasContent) setChildren(container, placeholder('Loading…', 'loading'));
    else container.style.opacity = '0.6';
    try {
      const data = await fetcher();
      if (mine !== generation) return;
      // A reload after an action (a switch, Run now) keeps keyboard focus on the same control.
      keepFocus(container, () => setChildren(container, render(data)));
      hasContent = true;
    } catch (error) {
      if (mine !== generation) return;
      // Say it inline with a way out, like the Git page (UX2-07).
      setChildren(container, loadError(`Couldn’t load this: ${error.message || String(error)}`, () => reload()));
      hasContent = false;
    } finally {
      if (mine === generation) container.style.opacity = '';
    }
  }
  reload();
  return reload;
}
