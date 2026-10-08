// Tiny DOM toolkit: element builder, icons, toasts, dialogs and menus.
// Every piece of server or user data is inserted with textContent or attributes,
// never through innerHTML, so API content cannot inject markup.

import { BRAND } from './brand.js';

const SVG_NS = 'http://www.w3.org/2000/svg';

/** Apply attributes/properties/listeners to an element. */
function applyAttrs(el, attrs) {
  if (!attrs) return;
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === null || value === false) continue;
    if (key === 'value') continue; // applied after children (select needs its options first)
    if (key === 'class') {
      el.className = Array.isArray(value) ? value.filter(Boolean).join(' ') : String(value);
    } else if (key === 'style' && typeof value === 'object') {
      for (const [prop, v] of Object.entries(value)) {
        if (prop.startsWith('--')) el.style.setProperty(prop, String(v));
        else el.style[prop] = v;
      }
    } else if (key === 'dataset') {
      Object.assign(el.dataset, value);
    } else if (key.startsWith('on') && typeof value === 'function') {
      el.addEventListener(key.slice(2).toLowerCase(), value);
    } else if (['checked', 'disabled', 'selected', 'hidden', 'indeterminate', 'multiple', 'required', 'readOnly'].includes(key)) {
      el[key] = Boolean(value);
    } else if (value === true) {
      el.setAttribute(key, '');
    } else {
      el.setAttribute(key, String(value));
    }
  }
}

/** Append children (nodes, strings, numbers, nested arrays; null/false skipped). */
export function append(el, ...children) {
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false || child === true) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

/** Create an HTML element: h('button', {class: 'btn', onClick}, 'Label'). */
export function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  applyAttrs(el, attrs);
  append(el, children);
  if (attrs && attrs.value !== undefined && attrs.value !== null) el.value = attrs.value;
  return el;
}

/** Replace all children of an element. */
export function setChildren(el, ...children) {
  el.replaceChildren();
  return append(el, children);
}

// Simple geometric icons drawn for Yulius (24×24 grid, stroked).
const ICONS = {
  globe: [['circle', { cx: 12, cy: 12, r: 9 }], ['ellipse', { cx: 12, cy: 12, rx: 4, ry: 9 }], ['path', { d: 'M3 12h18' }]],
  code: [['path', { d: 'M8 6l-6 6 6 6M16 6l6 6-6 6' }]],
  terminal: [['rect', { x: 2.5, y: 4, width: 19, height: 16, rx: 2 }], ['path', { d: 'M6.5 9l3.5 3-3.5 3M12 15.5h5' }]],
  mail: [['rect', { x: 3, y: 5, width: 18, height: 14, rx: 2 }], ['path', { d: 'M3.5 6.5l8.5 6.5 8.5-6.5' }]],
  menu: [['path', { d: 'M4 6h16M4 12h16M4 18h16' }]],
  more: [['circle', { cx: 12, cy: 5, r: 1.3, fill: 'currentColor' }], ['circle', { cx: 12, cy: 12, r: 1.3, fill: 'currentColor' }], ['circle', { cx: 12, cy: 19, r: 1.3, fill: 'currentColor' }]],
  plus: [['path', { d: 'M12 5v14M5 12h14' }]],
  x: [['path', { d: 'M6 6l12 12M18 6L6 18' }]],
  down: [['path', { d: 'M6 9l6 6 6-6' }]],
  right: [['path', { d: 'M9 6l6 6-6 6' }]],
  left: [['path', { d: 'M15 6l-6 6 6 6' }]],
  back: [['path', { d: 'M11 5l-7 7 7 7M4 12h16' }]],
  forward: [['path', { d: 'M13 5l7 7-7 7M20 12H4' }]],
  refresh: [['path', { d: 'M20 12a8 8 0 1 1-2.34-5.66' }], ['path', { d: 'M20 4v5h-5' }]],
  external: [['path', { d: 'M14 4h6v6M10 14L20 4M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5' }]],
  pulse: [['path', { d: 'M3 12h4l3-7 4 14 3-7h4' }]],
  sidebar: [['rect', { x: 3, y: 4, width: 18, height: 16, rx: 2 }], ['path', { d: 'M9 4v16' }]],
  tools: [['rect', { x: 3, y: 4, width: 18, height: 16, rx: 2 }], ['path', { d: 'M15 4v16' }]],
  logo: [['path', { d: BRAND.mark }]],
  /* task states (status.js): one shape per state so colour is never the only cue */
  'st-working': [['circle', { cx: 12, cy: 12, r: 6, fill: 'currentColor', stroke: 'none' }]],
  'st-input': [['path', { d: 'M5 6.5A2.5 2.5 0 0 1 7.5 4h9A2.5 2.5 0 0 1 19 6.5v7a2.5 2.5 0 0 1-2.5 2.5H11l-4 3.5V16h0.5A2.5 2.5 0 0 1 5 13.5z' }]],
  'st-attention': [['circle', { cx: 12, cy: 12, r: 8.5 }], ['path', { d: 'M12 7.5v5.5' }], ['circle', { cx: 12, cy: 16.2, r: 1.1, fill: 'currentColor', stroke: 'none' }]],
  'st-stopped': [['rect', { x: 7, y: 7, width: 10, height: 10, rx: 2, fill: 'currentColor', stroke: 'none' }]],
  'st-crashed': [['path', { d: 'M12 4l9 15.5H3z' }], ['path', { d: 'M12 10v4.2' }], ['circle', { cx: 12, cy: 17, r: 1, fill: 'currentColor', stroke: 'none' }]],
  'st-failed': [['circle', { cx: 12, cy: 12, r: 8.5 }], ['path', { d: 'M9 9l6 6M15 9l-6 6' }]],
  'st-closed': [['circle', { cx: 12, cy: 12, r: 8.5 }], ['path', { d: 'M8.5 12.2l2.4 2.4 4.6-5' }]],
  tasks: [['path', { d: 'M9 6h11M9 12h11M9 18h11' }], ['path', { d: 'M3.5 6l1.2 1.2L7 5M3.5 12l1.2 1.2L7 11M3.5 18l1.2 1.2L7 17' }]],
  branch: [['circle', { cx: 6, cy: 5.5, r: 2.2 }], ['circle', { cx: 6, cy: 18.5, r: 2.2 }], ['circle', { cx: 18, cy: 8, r: 2.2 }], ['path', { d: 'M6 7.7v8.6M18 10.2c0 4-4.5 3.8-9.8 6.6' }]],
  zap: [['path', { d: 'M13 2.5L4.5 13.5H12l-1 8 8.5-11H12z' }]],
  chart: [['path', { d: 'M4 20h16M7 16.5V11M12 16.5V6M17 16.5v-8' }]],
  archive: [['rect', { x: 3, y: 4, width: 18, height: 5, rx: 1.5 }], ['path', { d: 'M5 9v9a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9M10 13h4' }]],
  key: [['circle', { cx: 8, cy: 15, r: 4 }], ['path', { d: 'M10.8 12.2L20 3M16 7l3 3M14 9l2 2' }]],
  gear: [['circle', { cx: 12, cy: 12, r: 3 }], ['path', { d: 'M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z' }]],
  users: [['circle', { cx: 9, cy: 8, r: 3.5 }], ['path', { d: 'M2.5 20c1.2-3.4 3.7-5.2 6.5-5.2s5.3 1.8 6.5 5.2M16 4.6a3.5 3.5 0 0 1 0 6.8M18 14.9c1.6.6 2.8 2.3 3.5 5.1' }]],
  plug: [['path', { d: 'M9 3v5M15 3v5M6 8h12v3a6 6 0 0 1-12 0zM12 17v4' }]],
  book: [['path', { d: 'M4 5.5A2.5 2.5 0 0 1 6.5 3H20v15H6.5A2.5 2.5 0 0 0 4 20.5zM4 20.5A2.5 2.5 0 0 0 6.5 23H20v-5' }]],
  logout: [['path', { d: 'M15 4h3a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-3M10 17l5-5-5-5M15 12H4' }]],
  cpu: [['rect', { x: 6, y: 6, width: 12, height: 12, rx: 2 }], ['rect', { x: 9.5, y: 9.5, width: 5, height: 5, rx: 0.5 }], ['path', { d: 'M9.5 3v3M14.5 3v3M9.5 18v3M14.5 18v3M3 9.5h3M3 14.5h3M18 9.5h3M18 14.5h3' }]],
  memory: [['rect', { x: 3, y: 7, width: 18, height: 9, rx: 1.5 }], ['path', { d: 'M7.5 10v3M12 10v3M16.5 10v3M6 16v3M10 16v3M14 16v3M18 16v3' }]],
  disk: [['rect', { x: 3, y: 5, width: 18, height: 14, rx: 2 }], ['path', { d: 'M3 13.5h18M7 16.5h4' }], ['circle', { cx: 17, cy: 16.5, r: 0.9, fill: 'currentColor' }]],
  clock: [['circle', { cx: 12, cy: 12, r: 9 }], ['path', { d: 'M12 7v5l3 2' }]],
  shield: [['path', { d: 'M12 3l7 3v5.5c0 4.5-3 7.8-7 9.5-4-1.7-7-5-7-9.5V6z' }]],
  folder: [['path', { d: 'M3 7.5A2.5 2.5 0 0 1 5.5 5H9l2 2.2h7.5A2.5 2.5 0 0 1 21 9.7v7.8a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5z' }]],
  user: [['circle', { cx: 12, cy: 8, r: 3.8 }], ['path', { d: 'M4.5 20c1.4-3.6 4.3-5.5 7.5-5.5s6.1 1.9 7.5 5.5' }]],
  contrast: [['circle', { cx: 12, cy: 12, r: 9 }], ['path', { d: 'M12 3a9 9 0 0 0 0 18z', fill: 'currentColor', stroke: 'none' }]],
  target: [['circle', { cx: 12, cy: 12, r: 8 }], ['circle', { cx: 12, cy: 12, r: 3.5 }]],
  workflow: [['circle', { cx: 6, cy: 6, r: 2.5 }], ['circle', { cx: 18, cy: 18, r: 2.5 }], ['path', { d: 'M8.5 6H14a3 3 0 0 1 3 3v6.5' }]],
  up: [['path', { d: 'M12 19V5M6 11l6-6 6 6' }]],
  github: [['path', { d: 'M12 .3a12 12 0 0 0-3.8 23.38c.6.12.82-.26.82-.58v-2.04c-3.34.73-4.04-1.61-4.04-1.61-.55-1.39-1.34-1.76-1.34-1.76-1.09-.74.08-.73.08-.73 1.2.08 1.84 1.24 1.84 1.24 1.07 1.83 2.8 1.3 3.49 1 .1-.78.42-1.3.76-1.6-2.67-.3-5.47-1.33-5.47-5.93 0-1.31.47-2.38 1.24-3.22-.14-.3-.54-1.52.1-3.18 0 0 1-.32 3.3 1.23a11.5 11.5 0 0 1 6 0c2.28-1.55 3.29-1.23 3.29-1.23.64 1.66.24 2.88.12 3.18.77.84 1.23 1.91 1.23 3.22 0 4.61-2.8 5.63-5.48 5.92.42.36.81 1.1.81 2.22v3.29c0 .32.21.7.82.58A12 12 0 0 0 12 .3', fill: 'currentColor', stroke: 'none' }]],
  bell: [['path', { d: 'M6 9a6 6 0 0 1 12 0c0 6 2.5 8 2.5 8h-17S6 15 6 9' }], ['path', { d: 'M10.3 20.5a2 2 0 0 0 3.4 0' }]],
  maximize: [['path', { d: 'M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5' }]],
  minimize: [['path', { d: 'M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5' }]],
  check: [['path', { d: 'M5 12.5l4.5 4.5L19 7.5' }]],
  warn: [['path', { d: 'M12 3l10 18H2z' }], ['path', { d: 'M12 10v5M12 18v.5' }]],
  lock: [['rect', { x: 5, y: 11, width: 14, height: 9, rx: 2 }], ['path', { d: 'M8 11V8a4 4 0 0 1 8 0v3' }]],
  send: [['path', { d: 'M4 12l16-8-6 16-2.5-6.5z' }]],
  download: [['path', { d: 'M12 4v11M7 10l5 5 5-5M5 20h14' }]],
};

/** Inline SVG icon by name. Decorative unless a label is given. */
export function icon(name, { size = 16, label } = {}) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('width', String(size));
  svg.setAttribute('height', String(size));
  svg.setAttribute('fill', 'none');
  svg.setAttribute('stroke', 'currentColor');
  svg.setAttribute('stroke-width', '1.8');
  svg.setAttribute('stroke-linecap', 'round');
  svg.setAttribute('stroke-linejoin', 'round');
  svg.setAttribute('class', 'icon');
  if (label) {
    svg.setAttribute('role', 'img');
    svg.setAttribute('aria-label', label);
  } else {
    svg.setAttribute('aria-hidden', 'true');
  }
  for (const [tag, attrs] of ICONS[name] || []) {
    const node = document.createElementNS(SVG_NS, tag);
    for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
    svg.append(node);
  }
  return svg;
}

/** Create an SVG element (charts). */
export function svgEl(tag, attrs, ...children) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === undefined || v === null) continue;
    node.setAttribute(k, String(v));
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

/** Icon-only button with an accessible label and tooltip. */
export function iconButton(name, label, onClick, attrs = {}) {
  return h('button', { type: 'button', class: ['btn-icon', attrs.class], 'aria-label': label, title: label, onClick, ...withoutClass(attrs) }, icon(name));
}

function withoutClass(attrs) {
  const copy = { ...attrs };
  delete copy.class;
  return copy;
}

// ---------------------------------------------------------------- toasts

const MAX_TOASTS = 5;

function toastHost() {
  let host = document.getElementById('toasts');
  if (!host) {
    host = h('div', { id: 'toasts', class: 'toasts', role: 'status', 'aria-live': 'polite' });
    document.body.append(host);
  }
  return host;
}

/** Show a transient message. kind: info | ok | warn | error. */
export function toast(message, { kind = 'info', action, timeout = 5000, detail } = {}) {
  const host = toastHost();
  while (host.children.length >= MAX_TOASTS) host.firstElementChild.remove();
  const close = () => el.remove();
  const el = h('div', { class: ['toast', `toast-${kind}`] },
    h('div', { class: 'toast-body' }, h('div', { class: 'toast-msg' }, message), detail ? h('div', { class: 'toast-detail' }, detail) : null),
    action ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { close(); action.run(); } }, action.label) : null,
    iconButton('x', 'Dismiss', close, { class: 'toast-x' }),
  );
  host.append(el);
  if (timeout > 0) setTimeout(close, timeout);
  return close;
}

/** Toast for a failed request: shows the server's error message and code. */
export function errorToast(error, prefix) {
  const message = error && error.message ? error.message : String(error);
  const code = error && error.code ? error.code : null;
  toast(prefix ? `${prefix}: ${message}` : message, { kind: 'error', detail: code && code !== 'error' ? `code: ${code}` : null, timeout: 8000 });
}

// ---------------------------------------------------------------- dialogs

/**
 * Open a modal dialog. actions: [{label, kind, run(modal) -> false keeps it open}].
 * Returns {dialog, body, close(result), result: Promise}.
 */
export function openModal({ title, body, actions = [], wide = false, full = false, onClose, dismissible = true, className }) {
  let resolve;
  const result = new Promise((r) => { resolve = r; });
  const bodyEl = h('div', { class: 'modal-body' }, body);
  const footer = h('div', { class: 'modal-actions' });
  const dialog = h('dialog', { class: ['modal', wide && 'modal-wide', full && 'modal-full', className], 'aria-label': title },
    h('div', { class: 'modal-head' }, h('h2', { class: 'modal-title' }, title), dismissible ? iconButton('x', 'Close', () => close(undefined)) : null),
    bodyEl,
    footer,
  );
  let closed = false;
  function close(value) {
    if (closed) return;
    closed = true;
    if (dialog.open) dialog.close();
    dialog.remove();
    if (onClose) onClose(value);
    resolve(value);
  }
  dialog.addEventListener('cancel', (event) => {
    event.preventDefault();
    if (dismissible) close(undefined);
  });
  const modal = { dialog, body: bodyEl, footer, close, result, setActions };
  function setActions(list) {
    footer.replaceChildren();
    for (const action of list) {
      const button = h('button', { type: action.submit ? 'submit' : 'button', class: ['btn', action.kind && `btn-${action.kind}`], disabled: action.disabled, onClick: async () => {
        if (!action.run) return close(action.value);
        button.disabled = true;
        button.classList.add('busy');
        try {
          const keep = await action.run(modal);
          if (keep !== false) close(action.value === undefined ? true : action.value);
        } catch (error) {
          errorToast(error);
        } finally {
          button.disabled = false;
          button.classList.remove('busy');
        }
      } }, action.label);
      if (action.ref) action.ref(button);
      footer.append(button);
    }
    footer.hidden = list.length === 0;
  }
  setActions(actions);
  document.body.append(dialog);
  dialog.showModal();
  const autofocus = dialog.querySelector('[autofocus]') || dialog.querySelector('input, textarea, select');
  if (autofocus) autofocus.focus();
  return modal;
}

/**
 * Ask for confirmation. With `checkbox`, the confirm button stays disabled until the
 * box is ticked (explicit confirmation for deploys and destructive actions).
 * With `typeToConfirm`, the user must type that exact text.
 */
export function confirmDialog({ title, message, confirmLabel = 'Confirm', danger = false, checkbox, typeToConfirm, details }) {
  let confirmButton;
  const check = checkbox ? h('input', { type: 'checkbox', id: 'confirm-check' }) : null;
  const typed = typeToConfirm ? h('input', { type: 'text', class: 'input', autocomplete: 'off', 'aria-label': `Type ${typeToConfirm} to confirm` }) : null;
  const update = () => {
    const ok = (!check || check.checked) && (!typed || typed.value === typeToConfirm);
    if (confirmButton) confirmButton.disabled = !ok;
  };
  if (check) check.addEventListener('change', update);
  if (typed) typed.addEventListener('input', update);
  const body = h('div', { class: 'stack' },
    typeof message === 'string' ? h('p', null, message) : message,
    details || null,
    check ? h('label', { class: 'check', for: 'confirm-check' }, check, h('span', null, checkbox)) : null,
    typed ? h('label', { class: 'field' }, h('span', { class: 'field-label' }, `Type “${typeToConfirm}” to confirm`), typed) : null,
  );
  const modal = openModal({
    title,
    body,
    actions: [
      { label: 'Cancel', value: false },
      { label: confirmLabel, kind: danger ? 'danger' : 'primary', value: true, ref: (b) => { confirmButton = b; } },
    ],
  });
  update();
  return modal.result.then((v) => v === true);
}

// ---------------------------------------------------------------- menus

let openMenuEl = null;

/** Close the open context/kebab menu, if any. */
export function closeMenu() {
  if (openMenuEl) {
    openMenuEl.cleanup();
    openMenuEl = null;
  }
}

/**
 * Show a menu at an anchor element or {x, y} point.
 * items: [{label, onSelect, disabled, danger, hint, description, checked, icon, header, separator, href}]
 * `description` adds a second line; `checked` (true/false) makes the item a choice with a check mark.
 */
export function openMenu(anchor, items, { align = 'start' } = {}) {
  closeMenu();
  const menu = h('div', { class: 'menu', role: 'menu' });
  for (const item of items) {
    if (!item) continue;
    if (item.separator) { menu.append(h('div', { class: 'menu-sep', role: 'separator' })); continue; }
    if (item.header) { menu.append(h('div', { class: 'menu-header' }, item.header)); continue; }
    const choice = typeof item.checked === 'boolean';
    const label = item.description
      ? h('span', { class: 'menu-text' }, h('span', { class: 'menu-label' }, item.label), h('span', { class: 'menu-desc' }, item.description))
      : h('span', { class: 'menu-label' }, item.label);
    const el = h('button', { type: 'button', role: choice ? 'menuitemradio' : 'menuitem', 'aria-checked': choice ? String(item.checked) : null, class: ['menu-item', item.danger && 'danger', item.description && 'rich'], disabled: item.disabled, title: item.title || null, onClick: () => { closeMenu(); item.onSelect && item.onSelect(); } },
      choice ? h('span', { class: 'menu-check' }, item.checked ? icon('check', { size: 14 }) : null) : null,
      item.icon ? h('span', { class: 'menu-icon' }, icon(item.icon, { size: 15 })) : null,
      label, item.hint ? h('span', { class: 'menu-hint' }, item.hint) : null);
    menu.append(el);
  }
  document.body.append(menu);
  const rect = anchor instanceof Element ? anchor.getBoundingClientRect() : { left: anchor.x, right: anchor.x, top: anchor.y, bottom: anchor.y };
  const mw = menu.offsetWidth;
  const mh = menu.offsetHeight;
  let left = align === 'end' ? rect.right - mw : rect.left;
  let top = rect.bottom + 4;
  if (left + mw > window.innerWidth - 8) left = window.innerWidth - mw - 8;
  if (left < 8) left = 8;
  if (top + mh > window.innerHeight - 8) top = Math.max(8, rect.top - mh - 4);
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
  const onDoc = (event) => { if (!menu.contains(event.target)) closeMenu(); };
  const onKey = (event) => {
    if (event.key === 'Escape') { closeMenu(); if (anchor instanceof HTMLElement) anchor.focus(); return; }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const buttons = [...menu.querySelectorAll('.menu-item:not(:disabled)')];
      const index = buttons.indexOf(document.activeElement);
      const next = event.key === 'ArrowDown' ? (index + 1) % buttons.length : (index - 1 + buttons.length) % buttons.length;
      if (buttons[next]) buttons[next].focus();
    }
  };
  const onResize = () => closeMenu();
  setTimeout(() => document.addEventListener('pointerdown', onDoc, true), 0);
  document.addEventListener('keydown', onKey, true);
  window.addEventListener('resize', onResize);
  window.addEventListener('blur', onResize);
  openMenuEl = {
    cleanup() {
      document.removeEventListener('pointerdown', onDoc, true);
      document.removeEventListener('keydown', onKey, true);
      window.removeEventListener('resize', onResize);
      window.removeEventListener('blur', onResize);
      menu.remove();
    },
  };
  const first = menu.querySelector('.menu-item:not(:disabled)');
  if (first) first.focus({ preventScroll: true });
  return menu;
}

// ---------------------------------------------------------------- forms

/** Labelled form field wrapper. */
export function field(label, control, hint) {
  return h('label', { class: 'field' }, h('span', { class: 'field-label' }, label), control, hint ? h('span', { class: 'field-hint' }, hint) : null);
}

/** Toggle switch (checkbox styled as a switch). */
export function toggle({ checked, onChange, label, disabled }) {
  const input = h('input', { type: 'checkbox', class: 'switch-input', checked, disabled, 'aria-label': label, onChange: (e) => onChange(e.target.checked, input) });
  return h('span', { class: 'switch' }, input, h('span', { class: 'switch-track', 'aria-hidden': 'true' }));
}

/** A task state mark: the state's SVG icon in its tone (statusInfo from status.js). */
export function statusMark(info, size = 14) {
  return h('span', { class: ['glyph', `tone-${info.tone}`], role: 'img', 'aria-label': info.label, title: info.label }, icon(info.icon, { size }));
}

/** Small colored badge. */
export function badge(text, tone = '', title) {
  return h('span', { class: ['badge', tone && `badge-${tone}`], title: title || null }, text);
}

/** Loading / empty / error placeholders. */
export function placeholder(text, kind = 'empty') {
  return h('div', { class: ['placeholder', `placeholder-${kind}`] }, text);
}

/** Usage/percentage meter. `percent` null renders "unknown" (never zero). */
export function meter(percent, { label, detail } = {}) {
  const known = typeof percent === 'number' && Number.isFinite(percent);
  const value = known ? Math.max(0, Math.min(100, percent)) : 0;
  const tone = !known ? 'unknown' : value >= 90 ? 'danger' : value >= 70 ? 'warn' : 'ok';
  return h('div', { class: ['meter', `meter-${tone}`] },
    h('div', { class: 'meter-row' }, h('span', { class: 'meter-label' }, label || ''), h('span', { class: 'meter-value' }, known ? `${value.toFixed(value < 10 ? 1 : 0)}%` : 'unknown')),
    h('div', { class: 'meter-track', role: 'meter', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': known ? value : null, 'aria-label': label || 'usage' },
      h('div', { class: 'meter-fill', style: { width: `${value}%` } })),
    detail ? h('div', { class: 'meter-detail' }, detail) : null,
  );
}

const URL_RE = /\bhttps?:\/\/[^\s<>"'`]+/g;

/** Whether `uri` points at the VM itself (localhost, loopback): such links open in the sandbox
 *  browser, every other link in a new tab of the user's browser (UI-018). */
export function isLocalUrl(uri) {
  try {
    const host = new URL(uri).hostname.toLowerCase().replace(/^\[|\]$/g, '');
    return host === 'localhost' || host.endsWith('.localhost') || host === '0.0.0.0' || host === '::1' || /^127\.\d+\.\d+\.\d+$/.test(host);
  } catch {
    return false;
  }
}

/**
 * Turn http(s) URLs in plain text into links. `onLink(url, event)` decides how a click
 * is handled (Yulius browser links open the browser panel; others a new tab).
 */
export function linkify(text, onLink) {
  const frag = document.createDocumentFragment();
  let last = 0;
  for (const match of String(text).matchAll(URL_RE)) {
    let url = match[0];
    // Strip trailing punctuation that is almost never part of a URL.
    while (/[.,;:!?)\]}]$/.test(url)) {
      const ch = url.at(-1);
      const opener = { ')': '(', ']': '[', '}': '{' }[ch];
      if (opener && url.split(opener).length > url.split(ch).length - 1) break;
      url = url.slice(0, -1);
    }
    frag.append(text.slice(last, match.index));
    const a = h('a', { href: url, target: '_blank', rel: 'noopener noreferrer', class: 'link' }, url);
    if (onLink) a.addEventListener('click', (event) => { if (event.button === 0 && !event.metaKey && !event.ctrlKey) { event.preventDefault(); onLink(url, event); } });
    frag.append(a);
    last = match.index + url.length;
  }
  frag.append(text.slice(last));
  return frag;
}
