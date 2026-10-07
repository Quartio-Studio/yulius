// Tiny DOM toolkit: element builder, icons, toasts, dialogs and menus.
// Every piece of server or user data is inserted with textContent or attributes,
// never through innerHTML, so API content cannot inject markup.

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
 * items: [{label, onSelect, disabled, danger, hint, header, separator, href}]
 */
export function openMenu(anchor, items, { align = 'start' } = {}) {
  closeMenu();
  const menu = h('div', { class: 'menu', role: 'menu' });
  for (const item of items) {
    if (!item) continue;
    if (item.separator) { menu.append(h('div', { class: 'menu-sep', role: 'separator' })); continue; }
    if (item.header) { menu.append(h('div', { class: 'menu-header' }, item.header)); continue; }
    const el = h('button', { type: 'button', role: 'menuitem', class: ['menu-item', item.danger && 'danger'], disabled: item.disabled, title: item.title || null, onClick: () => { closeMenu(); item.onSelect && item.onSelect(); } },
      h('span', { class: 'menu-label' }, item.label), item.hint ? h('span', { class: 'menu-hint' }, item.hint) : null);
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
