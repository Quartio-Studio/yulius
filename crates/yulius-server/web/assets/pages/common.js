// Shared page scaffolding: frame, tables, async loading, pages that cannot be shown.
//
// One header pattern (#147): every page has a title and a subtitle that says what the page is for,
// in a few lowercase words (never the sandbox, which the tab row and the window title name, nor a
// count, which the page shows itself); actions on the right. A primary button that creates
// something starts with "+"; a key hint appears only on the button whose shortcut exists (New task).

import { h, setChildren, placeholder, loadError, keepFocus, emptyState, icon } from '../dom.js';
import { href, currentLocation, forgetPlace, replaceQuietly } from '../router.js';

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
 * A page of sections chosen with the Secrets page's tabs (SPEC-151, SPEC-161): CI/CD and the
 * Dashboard. `sections` are `{ value, label, icon, sub, mount }`; the first one's `value` is '' (the
 * plain `path`), the others are `?section=<value>`, written quietly on a change. `mount(ctx,
 * sandbox, route, host, actions)` fills its own part and the header's actions and returns its
 * teardown; a section shown by a tab change gets no route (it starts fresh). An alias address
 * (`/cd`, `/pipeline`) is rewritten to `path` with its query. Returns {el, destroy}.
 */
export function sectionedPage(ctx, sandbox, route, { title, path, sections }) {
  const frame = pageFrame({ title });
  const asked = route.query.get('section');
  let current = sections.some((o) => o.value && o.value === asked) ? asked : '';
  if (route.path !== path) replaceQuietly(route.query.toString() ? `${path}?${route.query}` : path);
  const host = h('div');
  const tabs = h('div', { class: 'tl-tabs ftabs', role: 'radiogroup', 'aria-label': 'Section' });
  let teardown = null;

  function renderTabs() {
    const focused = tabs.contains(document.activeElement);
    setChildren(tabs, sections.map((o) => h('button', {
      type: 'button', role: 'radio', 'aria-checked': String(o.value === current), tabindex: o.value === current ? '0' : '-1', class: ['tl-tab', o.value === current && 'active'],
      onClick: () => {
        if (current === o.value) return;
        current = o.value;
        // The shown section goes first: its run dialog writes its own address as it closes.
        stop();
        replaceQuietly(current ? `${path}?section=${current}` : path);
        renderTabs();
        show(null);
      },
      onKeydown: (event) => {
        if (!['ArrowRight', 'ArrowLeft', 'ArrowUp', 'ArrowDown'].includes(event.key)) return;
        event.preventDefault();
        const buttons = [...tabs.children];
        const step = ['ArrowRight', 'ArrowDown'].includes(event.key) ? 1 : -1;
        buttons[(buttons.indexOf(event.currentTarget) + step + buttons.length) % buttons.length].click();
      },
    }, h('span', { class: 'tl-tab-icon' }, icon(o.icon, { size: 14 })), o.label)));
    if (focused) tabs.querySelector('[aria-checked="true"]')?.focus();
  }

  function stop() {
    if (teardown) teardown();
    teardown = null;
  }

  /** Mounts the current section in a part of its own, so a late answer for the other one lands nowhere. */
  function show(shownRoute) {
    const section = sections.find((o) => o.value === current);
    const part = h('div', { class: 'stack' });
    setChildren(host, part);
    frame.sub.textContent = section.sub;
    teardown = section.mount(ctx, sandbox, shownRoute, part, frame.actions);
  }

  renderTabs();
  frame.body.append(tabs, host);
  // The address's own state (`?run=`, `?days=`, filters) is the section's it names.
  show(route);
  return { el: frame.el, destroy: stop };
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
  // Nothing to come back to: a sandbox's tab opens its composer next time (SPEC-113 KST-004).
  forgetPlace(currentLocation().full);
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
 * Load data into a container: shows loading (in the `shape` of what render draws, SPEC-120), then
 * render(data) or the error, which names what failed to load (`what`, such as "the users":
 * "Couldn’t load the users: …").
 * Returns a reload function. Concurrent reloads keep only the latest result. `reload({ quiet })`
 * refreshes in place for live pages: no dimming, and a failure keeps what is shown.
 */
export function loader(container, what, fetcher, render, { shape = 'lines' } = {}) {
  let generation = 0;
  let hasContent = false;
  async function reload({ quiet = false } = {}) {
    generation += 1;
    const mine = generation;
    if (!hasContent) setChildren(container, placeholder(`Loading ${what}…`, 'loading', shape));
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
