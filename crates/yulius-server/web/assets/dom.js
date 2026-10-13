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
  // A button that opens a menu or panel says it is closed until it opens (ARIA 1.2 menu button).
  if (attrs && attrs['aria-haspopup'] && !el.hasAttribute('aria-expanded')) el.setAttribute('aria-expanded', 'false');
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
  note: [['rect', { x: 4.5, y: 3, width: 15, height: 18, rx: 2 }], ['path', { d: 'M8 8h8M8 12h8M8 16h5' }]],
  // The Cockpit (SPEC-021): a dial and its needle.
  gauge: [['path', { d: 'M3.5 17a8.5 8.5 0 1 1 17 0' }], ['path', { d: 'M12 17l4.5-5.5' }], ['circle', { cx: 12, cy: 17, r: 1.2 }]],
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
  tools: [['rect', { x: 3, y: 4, width: 18, height: 16, rx: 2 }], ['path', { d: 'M15 4v16' }]],
  logo: [['path', BRAND.markStyle === 'fill' ? { d: BRAND.mark, fill: 'currentColor', stroke: 'none' } : { d: BRAND.mark }]],
  /* task states (status.js): one shape per state so colour is never the only cue */
  // A spinner: a faint ring and a turning arc (styles.css spins it; still under reduced motion).
  'st-working': [['circle', { cx: 12, cy: 12, r: 9, opacity: 0.25, 'stroke-width': 3 }], ['path', { d: 'M12 3a9 9 0 0 1 7.79 13.5', 'stroke-width': 3, 'stroke-linecap': 'round' }]],
  'st-input': [['path', { d: 'M5 6.5A2.5 2.5 0 0 1 7.5 4h9A2.5 2.5 0 0 1 19 6.5v7a2.5 2.5 0 0 1-2.5 2.5H11l-4 3.5V16h0.5A2.5 2.5 0 0 1 5 13.5z' }]],
  'st-attention': [['circle', { cx: 12, cy: 12, r: 8.5 }], ['path', { d: 'M12 7.5v5.5' }], ['circle', { cx: 12, cy: 16.2, r: 1.1, fill: 'currentColor', stroke: 'none' }]],
  'st-stopped': [['rect', { x: 7, y: 7, width: 10, height: 10, rx: 2, fill: 'currentColor', stroke: 'none' }]],
  'st-crashed': [['path', { d: 'M12 4l9 15.5H3z' }], ['path', { d: 'M12 10v4.2' }], ['circle', { cx: 12, cy: 17, r: 1, fill: 'currentColor', stroke: 'none' }]],
  'st-failed': [['circle', { cx: 12, cy: 12, r: 8.5 }], ['path', { d: 'M9 9l6 6M15 9l-6 6' }]],
  'st-closed': [['circle', { cx: 12, cy: 12, r: 8.5 }], ['path', { d: 'M8.5 12.2l2.4 2.4 4.6-5' }]],
  // Waiting states (#147): in line for a slot (an hourglass), starting (a ring being drawn), paused
  // for capacity (pause bars) and queued after another task (a chain link).
  'st-queued': [['path', { d: 'M7 3.5h10M7 20.5h10M8.2 3.5c0 4.6 3.8 5.4 3.8 8.5s-3.8 3.9-3.8 8.5M15.8 3.5c0 4.6-3.8 5.4-3.8 8.5s3.8 3.9 3.8 8.5' }]],
  'st-starting': [['circle', { cx: 12, cy: 12, r: 8.5, 'stroke-dasharray': '2.6 3.1' }], ['circle', { cx: 12, cy: 12, r: 2, fill: 'currentColor', stroke: 'none' }]],
  'st-paused': [['circle', { cx: 12, cy: 12, r: 8.5 }], ['path', { d: 'M10 9v6M14 9v6' }]],
  'st-chained': [['path', { d: 'M9.5 14.5l5-5M11 7.6l1.7-1.7a3.6 3.6 0 0 1 5.1 5.1l-1.7 1.7M13 16.4l-1.7 1.7a3.6 3.6 0 0 1-5.1-5.1l1.7-1.7' }]],
  trash: [['path', { d: 'M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2' }]],
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
  file: [['path', { d: 'M6.5 3h7.5l4.5 4.5V21h-12z' }], ['path', { d: 'M14 3v4.5h4.5' }]],
  wrap: [['path', { d: 'M4 6h16M4 12h13a3 3 0 0 1 0 6h-4M4 18h5' }], ['path', { d: 'M15 16l-2 2 2 2' }]],
  folder: [['path', { d: 'M3 7.5A2.5 2.5 0 0 1 5.5 5H9l2 2.2h7.5A2.5 2.5 0 0 1 21 9.7v7.8a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5z' }]],
  user: [['circle', { cx: 12, cy: 8, r: 3.8 }], ['path', { d: 'M4.5 20c1.4-3.6 4.3-5.5 7.5-5.5s6.1 1.9 7.5 5.5' }]],
  contrast: [['circle', { cx: 12, cy: 12, r: 9 }], ['path', { d: 'M12 3a9 9 0 0 0 0 18z', fill: 'currentColor', stroke: 'none' }]],
  target: [['circle', { cx: 12, cy: 12, r: 8 }], ['circle', { cx: 12, cy: 12, r: 3.5 }]],
  search: [['circle', { cx: 11, cy: 11, r: 6.5 }], ['path', { d: 'M16 16l4.5 4.5' }]],
  // Sessions (SES-002): a speech bubble with lines of a transcript.
  chat: [['path', { d: 'M4 6.5A2.5 2.5 0 0 1 6.5 4h11A2.5 2.5 0 0 1 20 6.5v7a2.5 2.5 0 0 1-2.5 2.5H11l-4.5 4V16A2.5 2.5 0 0 1 4 13.5z' }], ['path', { d: 'M8 8.5h8M8 12h5' }]],
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
  upload: [['path', { d: 'M12 20V9M7 14l5-5 5 5M5 4h14' }]],
  grip: [[9, 6], [15, 6], [9, 12], [15, 12], [9, 18], [15, 18]].map(([cx, cy]) => ['circle', { cx, cy, r: 1.3, fill: 'currentColor', stroke: 'none' }]),
  // All sandboxes (the home screen): four tiles.
  grid: [[4, 4], [13.5, 4], [4, 13.5], [13.5, 13.5]].map(([x, y]) => ['rect', { x, y, width: 6.5, height: 6.5, rx: 1.6 }]),
  // The composer's Preview (the prompt formatted).
  eye: [['path', { d: 'M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z' }], ['circle', { cx: 12, cy: 12, r: 3 }]],
};

/** Every icon name (the /design page shows them all). */
export const ICON_NAMES = Object.keys(ICONS);

/**
 * Filter tabs with counts: pills in a radio group (arrows move, as in Tasks), the stand-in for a
 * filter <select>. `options` are { value, label, icon? }; '' is "All". Tabs with no items hide
 * unless chosen. Returns { el, value, setCounts(Map value → count, total) }.
 */
export function filterTabs({ label, options, value = '', onChange }) {
  const el = h('div', { class: 'tl-tabs ftabs', role: 'radiogroup', 'aria-label': label });
  let current = options.some((o) => o.value === value) ? value : '';
  let counts = new Map();
  let total = null;
  function render() {
    const focused = el.contains(document.activeElement);
    setChildren(el, options.filter((o) => o.value === '' || o.value === current || total === null || (counts.get(o.value) || 0) > 0).map((o) => {
      const on = o.value === current;
      const n = o.value === '' ? total : counts.get(o.value) || 0;
      return h('button', { type: 'button', role: 'radio', 'aria-checked': String(on), tabindex: on ? '0' : '-1', class: ['tl-tab', on && 'active'], dataset: { value: o.value },
        onClick: () => { if (current === o.value) return; current = o.value; render(); onChange(current); },
        onKeydown: (event) => {
          const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
          if (!step) return;
          event.preventDefault();
          const all = [...el.children];
          all[(all.indexOf(event.currentTarget) + step + all.length) % all.length].click();
          el.querySelector('[aria-checked="true"]').focus();
        } },
      o.icon ? h('span', { class: 'tl-tab-icon' }, icon(o.icon, { size: 14 })) : null, o.label, n === null ? null : h('span', { class: 'tl-tab-count' }, String(n)));
    }));
    if (focused) el.querySelector('[aria-checked="true"]')?.focus();
  }
  render();
  return {
    el,
    get value() { return current; },
    setCounts(next, all) { counts = next; total = all; render(); },
  };
}

/** Inline SVG icon by name. Decorative unless a label is given. */
export function icon(name, { size = 16, label } = {}) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('viewBox', name === 'logo' && BRAND.markViewBox ? BRAND.markViewBox : '0 0 24 24');
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
/**
 * Keyboard for a closable tab (browser and editor tabs, roving tabindex): Enter or Space selects,
 * Delete or Backspace closes, ←/→/Home/End move the focus along its tablist. The close mark inside
 * a tab is not a button: a tab presents its children as plain content, so a nested button would be
 * unreachable for assistive technology.
 */
export function closableTabKeys(event, { select, close }) {
  const tab = event.currentTarget;
  // A held key repeats: it must not close one tab after another or type into what follows (#80).
  if (event.repeat && ['Enter', ' ', 'Delete', 'Backspace'].includes(event.key)) {
    event.preventDefault();
    return;
  }
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault();
    select();
  } else if (event.key === 'Delete' || event.key === 'Backspace') {
    event.preventDefault();
    close();
  } else if (['ArrowRight', 'ArrowLeft', 'Home', 'End'].includes(event.key)) {
    event.preventDefault();
    const all = [...tab.parentElement.querySelectorAll('[role="tab"]')];
    const at = all.indexOf(tab);
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? all.length - 1
      : (at + (event.key === 'ArrowRight' ? 1 : -1) + all.length) % all.length;
    for (const other of all) other.tabIndex = other === all[next] ? 0 : -1;
    all[next].focus();
  }
}

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

/**
 * The toast region. While a modal dialog is open everything outside it is inert and below its
 * backdrop, so the region moves into the topmost open modal (and back to <body> when it closes):
 * an error raised by a dialog's action is seen, clickable and announced.
 */
function toastHost() {
  let host = document.getElementById('toasts');
  if (!host) {
    host = h('div', { id: 'toasts', class: 'toasts', role: 'status', 'aria-live': 'polite' });
    document.addEventListener('focusin', () => avoidFocus(host));
  }
  const modal = [...document.querySelectorAll('dialog[open]')].filter((d) => d.matches(':modal')).pop();
  const parent = modal || document.body;
  if (host.parentElement !== parent) parent.append(host);
  return host;
}

/** Moves the toasts to the other edge while they would cover the focused control (WCAG 2.4.11). */
function avoidFocus(host) {
  const target = document.activeElement;
  if (!host.children.length || !target || target === document.body || host.contains(target)) return;
  const covers = () => {
    const a = host.getBoundingClientRect();
    const b = target.getBoundingClientRect();
    return a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
  };
  if (covers()) host.classList.toggle('flipped');
}

/**
 * Show a transient message. kind: info | ok | warn | error. A toast with an action (Undo) stays
 * at least 10 s, and no toast disappears while the pointer or keyboard focus is on it (WCAG 2.2.1).
 */
export function toast(message, { kind = 'info', action, timeout = 5000, detail } = {}) {
  const host = toastHost();
  while (host.children.length >= MAX_TOASTS) host.firstElementChild.remove();
  const close = () => el.remove();
  const el = h('div', { class: ['toast', `toast-${kind}`], role: kind === 'error' ? 'alert' : null },
    h('div', { class: 'toast-body' }, h('div', { class: 'toast-msg' }, message), detail ? h('div', { class: 'toast-detail' }, detail) : null),
    action ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { close(); action.run(); } }, action.label) : null,
    iconButton('x', 'Dismiss', close, { class: 'toast-x' }),
  );
  host.append(el);
  avoidFocus(host);
  const wait = action && timeout > 0 ? Math.max(timeout, 10000) : timeout;
  if (wait > 0) {
    let timer = setTimeout(close, wait);
    const pause = () => clearTimeout(timer);
    const resume = () => { if (!el.matches(':hover') && !el.contains(document.activeElement)) { clearTimeout(timer); timer = setTimeout(close, Math.min(wait, 4000)); } };
    el.addEventListener('pointerenter', pause);
    el.addEventListener('focusin', pause);
    el.addEventListener('pointerleave', resume);
    el.addEventListener('focusout', () => setTimeout(resume, 0));
  }
  return close;
}

/** Toast for a failed request: shows the server's error message and code. */
export function errorToast(error, prefix) {
  const message = error && error.message ? error.message : String(error);
  // The server's sentence is shown; its machine code is not (it stays in the console for support).
  if (error && error.code) console.warn(`${prefix || 'Error'}: ${error.code}`, error);
  toast(prefix ? `${prefix}: ${message}` : message, { kind: 'error', timeout: 8000 });
}

// ---------------------------------------------------------------- dialogs

/**
 * Open a modal dialog. actions: [{label, kind, run(modal) -> false keeps it open}].
 * Returns {dialog, body, close(result), result: Promise}.
 */
export function openModal({ title, body, actions = [], wide = false, full = false, onClose, dismissible = true, className }) {
  // Focus returns where it was when the dialog opened (WCAG 2.4.3), or to the main heading.
  const returnTo = document.activeElement;
  let resolve;
  const result = new Promise((r) => { resolve = r; });
  const bodyEl = h('div', { class: 'modal-body' }, body);
  const footer = h('div', { class: 'modal-actions' });
  // A failed action says why inside the dialog, next to the button that failed (not in a toast).
  const errorEl = h('div', { class: 'modal-error', role: 'alert', hidden: true });
  const dialog = h('dialog', { class: ['modal', wide && 'modal-wide', full && 'modal-full', className], 'aria-label': title },
    h('div', { class: 'modal-head' }, h('h2', { class: 'modal-title' }, title), dismissible ? iconButton('x', 'Close', () => close(undefined)) : null),
    bodyEl,
    errorEl,
    footer,
  );
  let closed = false;
  function close(value) {
    if (closed) return;
    closed = true;
    if (dialog.open) dialog.close();
    const toasts = document.getElementById('toasts');
    if (toasts && dialog.contains(toasts)) toastHost();
    dialog.remove();
    if (returnTo instanceof HTMLElement && returnTo.isConnected && returnTo !== document.body) returnTo.focus({ preventScroll: true });
    else focusMainHeading();
    if (onClose) onClose(value);
    resolve(value);
  }
  dialog.addEventListener('cancel', (event) => {
    event.preventDefault();
    if (dismissible) close(undefined);
  });
  // A click outside the dialog (on its backdrop) closes it like Esc and ×, as every menu and
  // popover does. Only a press that also starts outside counts: selecting text in a field and
  // releasing past the edge keeps the dialog.
  const outside = (event) => {
    if (event.target !== dialog) return false;
    const r = dialog.getBoundingClientRect();
    return event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom;
  };
  let pressedOutside = false;
  dialog.addEventListener('pointerdown', (event) => { pressedOutside = outside(event); });
  dialog.addEventListener('click', (event) => {
    if (dismissible && pressedOutside && outside(event)) close(undefined);
    pressedOutside = false;
  });
  // Enter in a one-line field submits the dialog, as in every form: the primary action runs.
  let primary = null;
  dialog.addEventListener('keydown', (event) => {
    const field = event.target;
    if (event.key !== 'Enter' || event.defaultPrevented || event.isComposing || event.shiftKey || event.altKey || event.ctrlKey || event.metaKey) return;
    if (!(field instanceof HTMLInputElement) || ['checkbox', 'radio', 'button', 'submit', 'file', 'range', 'color'].includes(field.type)) return;
    if (!primary || primary.disabled || !primary.isConnected) return;
    event.preventDefault();
    primary.click();
  });
  const modal = { dialog, body: bodyEl, footer, close, result, setActions };
  function setActions(list) {
    footer.replaceChildren();
    primary = null;
    for (const action of list) {
      const button = h('button', { type: action.submit ? 'submit' : 'button', class: ['btn', action.kind && `btn-${action.kind}`], disabled: action.disabled, onClick: async () => {
        if (!action.run) return close(action.value);
        button.disabled = true;
        button.classList.add('busy');
        errorEl.hidden = true;
        try {
          const keep = await action.run(modal);
          if (keep !== false) close(action.value === undefined ? true : action.value);
        } catch (error) {
          if (error && error.code) console.warn(`${action.label}: ${error.code}`, error);
          const message = error && error.message ? error.message : String(error);
          errorEl.textContent = message.charAt(0).toUpperCase() + message.slice(1); // server sentences start lower-case
          errorEl.hidden = false;
        } finally {
          button.disabled = false;
          button.classList.remove('busy');
        }
      } }, action.label);
      if (action.ref) action.ref(button);
      if (action.kind === 'primary' || action.kind === 'danger' || (!primary && action.run)) primary = button;
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
 * `description` adds a second line; `checked` (true/false) makes the item a choice with a check mark;
 * `keepOpen` keeps the menu open after selecting (multi-select filters), flipping the check in place.
 */
export function openMenu(anchor, items, { align = 'start' } = {}) {
  closeMenu();
  const button = anchor instanceof HTMLElement && anchor.hasAttribute('aria-haspopup') ? anchor : null;
  if (button) button.setAttribute('aria-expanded', 'true');
  const menu = h('div', { class: 'menu', role: 'menu' });
  for (const item of items) {
    if (!item) continue;
    if (item.separator) { menu.append(h('div', { class: 'menu-sep', role: 'separator' })); continue; }
    if (item.header) { menu.append(h('div', { class: 'menu-header' }, item.header)); continue; }
    const choice = typeof item.checked === 'boolean';
    // A disabled item says why on its second line, not only in a mouse tooltip.
    const description = item.description || (item.disabled && item.title ? item.title : null);
    const label = description
      ? h('span', { class: 'menu-text' }, h('span', { class: 'menu-label' }, item.label), h('span', { class: 'menu-desc' }, description))
      : h('span', { class: 'menu-label' }, item.label);
    const check = choice ? h('span', { class: 'menu-check' }, item.checked ? icon('check', { size: 14 }) : null) : null;
    const select = () => {
      if (item.disabled) return;
      if (!item.keepOpen) {
        if (anchor instanceof HTMLElement && anchor.isConnected) anchor.focus({ preventScroll: true });
        closeMenu();
        if (item.onSelect) item.onSelect();
        return;
      }
      if (choice) {
        item.checked = !item.checked;
        el.setAttribute('aria-checked', String(item.checked));
        setChildren(check, item.checked ? icon('check', { size: 14 }) : null);
      }
      if (item.onSelect) item.onSelect();
    };
    const el = h('button', { type: 'button', role: choice ? (item.keepOpen ? 'menuitemcheckbox' : 'menuitemradio') : 'menuitem', 'aria-checked': choice ? String(item.checked) : null, class: ['menu-item', item.danger && 'danger', description && 'rich'], 'aria-disabled': item.disabled ? 'true' : null, title: item.title || null, onClick: select },
      check,
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
    // Focus goes back to the button before the menu closes (a button shown only while its menu is
    // open would hide first and could not take focus).
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); if (anchor instanceof HTMLElement) anchor.focus(); closeMenu(); return; }
    // APG menu: arrows wrap, Home/End jump, a letter moves to the next item starting with it,
    // Tab leaves (closing the menu). Disabled items stay focusable so their reason can be read.
    const buttons = [...menu.querySelectorAll('.menu-item')];
    const index = buttons.indexOf(document.activeElement);
    let next = -1;
    if (event.key === 'ArrowDown') next = (index + 1) % buttons.length;
    else if (event.key === 'ArrowUp') next = (index - 1 + buttons.length) % buttons.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = buttons.length - 1;
    else if (event.key === 'Tab') { event.preventDefault(); if (anchor instanceof HTMLElement) anchor.focus(); closeMenu(); return; }
    else if (event.key.length === 1 && /\S/.test(event.key) && !event.metaKey && !event.ctrlKey && !event.altKey) {
      const letter = event.key.toLowerCase();
      for (let step = 1; step <= buttons.length; step += 1) {
        const candidate = buttons[(index + step) % buttons.length];
        if ((candidate.querySelector('.menu-label')?.textContent || '').trim().toLowerCase().startsWith(letter)) { next = buttons.indexOf(candidate); break; }
      }
    }
    if (next >= 0 && buttons[next]) { event.preventDefault(); buttons[next].focus(); }
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
      if (button) button.setAttribute('aria-expanded', 'false');
    },
  };
  const first = menu.querySelector('.menu-item:not([aria-disabled="true"])') || menu.querySelector('.menu-item');
  if (first) first.focus({ preventScroll: true });
  return menu;
}

// ---------------------------------------------------------------- forms

/** Labelled form field wrapper. */
export function field(label, control, hint, error) {
  return h('label', { class: 'field' }, h('span', { class: 'field-label' }, label), control, hint ? h('span', { class: 'field-hint' }, hint) : null, error || null);
}

let errorIds = 0;
/**
 * Shows a validation message under its field (not in a toast): marks the control aria-invalid, ties
 * the message to it with aria-describedby, focuses it, and clears both on the next edit (WCAG 3.3.1).
 */
export function invalid(control, message) {
  const wrapper = control.closest('.field') || control.parentElement;
  let note = wrapper.querySelector(':scope > .field-error');
  if (!note) {
    errorIds += 1;
    note = h('span', { class: 'field-error', id: `field-error-${errorIds}`, role: 'alert' });
    wrapper.append(note);
  }
  note.textContent = message;
  control.setAttribute('aria-invalid', 'true');
  control.setAttribute('aria-describedby', note.id);
  control.focus();
  const clear = () => { note.remove(); control.removeAttribute('aria-invalid'); control.removeAttribute('aria-describedby'); };
  control.addEventListener('input', clear, { once: true });
  control.addEventListener('change', clear, { once: true });
  return false;
}

/** Toggle switch (checkbox styled as a switch). */
/**
 * An on/off switch. `label` names what is switched ("quartio", "Access for anna-q"), never the
 * action: the state is conveyed by role="switch" and checked, so the name stays true either way.
 */
export function toggle({ checked, onChange, label, disabled }) {
  const input = h('input', { type: 'checkbox', role: 'switch', class: 'switch-input', checked, disabled, 'aria-label': label, onChange: (e) => onChange(e.target.checked, input) });
  return h('span', { class: 'switch' }, input, h('span', { class: 'switch-track', 'aria-hidden': 'true' }));
}

/** Focus the main view's heading (made focusable for this) when focus would otherwise be lost. */
export function focusMainHeading() {
  const heading = document.querySelector('#main h1');
  if (!heading) return;
  heading.tabIndex = -1;
  heading.focus({ preventScroll: true });
}

/**
 * Popover focus (bell, Status): focus moves into the panel when it opens, the panel closes when
 * focus leaves it for somewhere other than its trigger, and Esc returns focus to the trigger.
 * Returns a cleanup function.
 */
export function popoverFocus(panel, trigger, close) {
  panel.tabIndex = -1;
  const first = panel.querySelector(FOCUSABLE);
  (first || panel).focus({ preventScroll: true });
  // Decide after the move: a re-render (keepFocus) puts focus back inside within the same task.
  const onOut = () => setTimeout(() => {
    const now = document.activeElement;
    if (!panel.isConnected) return;
    if (!panel.contains(now) && !trigger.contains(now)) close();
  }, 0);
  // Tab past either end returns to the button that opened the panel and closes it.
  const onKey = (event) => {
    if (event.key !== 'Tab') return;
    const items = [...panel.querySelectorAll(FOCUSABLE)].filter((el) => el.getClientRects().length);
    const atEnd = event.shiftKey ? document.activeElement === items[0] || document.activeElement === panel : document.activeElement === items[items.length - 1];
    if (atEnd) { event.preventDefault(); trigger.focus(); close(); }
  };
  panel.addEventListener('focusout', onOut);
  panel.addEventListener('keydown', onKey);
  return () => { panel.removeEventListener('focusout', onOut); panel.removeEventListener('keydown', onKey); };
}

/** Rebuild `container` with `rebuild()` and put focus back on the same control, if it had it. */
export function keepFocus(container, rebuild) {
  const before = [...container.querySelectorAll(FOCUSABLE)];
  const index = before.indexOf(document.activeElement);
  rebuild();
  if (index < 0) return;
  const after = [...container.querySelectorAll(FOCUSABLE)];
  const target = after[Math.min(index, after.length - 1)] || container;
  target.focus({ preventScroll: true });
}

const REGIONS = ['.tabrow', '.sidebar', '#main', '.panel:not([hidden])', '.rail:not([hidden])'];
const FOCUSABLE = 'a[href], summary, button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * Moves keyboard focus to the next (or previous) page region — tab row, sidebar, main, tool
 * panel, rail — skipping regions that are hidden or collapsed. F6 / Shift+F6 call it, so the
 * terminal and the remote browser view, which keep Tab and Esc for themselves, never trap focus.
 */
export function focusNextRegion(backwards = false) {
  const regions = REGIONS.map((sel) => document.querySelector(sel)).filter((el) => el && el.getBoundingClientRect().width > 0);
  if (!regions.length) return;
  const current = regions.findIndex((el) => el.contains(document.activeElement));
  for (let step = 1; step <= regions.length; step += 1) {
    const region = regions[(current + (backwards ? -step : step) + regions.length * 2) % regions.length];
    const target = [...region.querySelectorAll(FOCUSABLE)].find((el) => el.getClientRects().length && getComputedStyle(el).visibility !== 'hidden');
    if (target) { target.focus(); return; }
  }
}

/** A badge with an icon in front. */
export function iconBadge(iconName, text, tone, title) {
  const el = badge(text, tone, title);
  el.prepend(icon(iconName, { size: 11 }));
  return el;
}

/**
 * A card for one item in a list (workflows, skills…): an icon tile, the title with badges, a
 * description and optional extra content, and controls on the right. With `onOpen` the title is
 * a button stretched over the whole card (click or Enter anywhere opens it) and a chevron shows;
 * `controls` (toggles, icon buttons) keep their own clicks above it.
 */
export function itemCard({ iconName, iconEl, title, badges = [], description, extra, controls = [], onOpen, openLabel, dim = false }) {
  const name = onOpen
    ? h('button', { type: 'button', class: 'item-name item-open', 'aria-label': openLabel || null, onClick: onOpen }, title)
    : h('span', { class: 'item-name' }, title);
  return h('div', { class: ['item-card', onOpen && 'clickable', dim && 'off'] },
    h('span', { class: 'item-icon', 'aria-hidden': 'true' }, iconEl || icon(iconName, { size: 18 })),
    h('div', { class: 'item-main' },
      h('div', { class: 'item-title' }, name, badges),
      description ? h('div', { class: 'item-desc' }, description) : null,
      extra || null),
    h('div', { class: 'item-controls' }, controls, onOpen ? h('span', { class: 'item-chevron', 'aria-hidden': 'true' }, icon('right', { size: 16 })) : null));
}

/** True while polling would be wasted: the tab is hidden or the browser is offline (PERF-12). */
export function pollPaused() {
  return document.hidden || navigator.onLine === false;
}

/**
 * Coalesces repaints driven by live events (PERF-02): any number of calls before the next frame
 * run `fn` once, and runs are at least `minInterval` ms apart, so a burst of task events repaints a
 * long list a few times a second instead of once per event. A hidden tab (no frames) still runs it
 * after a short timer. `cancel()` drops a pending run (call it when the view goes away).
 */
export function coalesce(fn, { minInterval = 0 } = {}) {
  let pending = false;
  let last = -Infinity;
  let timer = null;
  const run = () => {
    if (!pending) return;
    pending = false;
    last = performance.now();
    fn();
  };
  const schedule = () => {
    if (pending) return;
    pending = true;
    const wait = Math.max(0, last + minInterval - performance.now());
    timer = setTimeout(() => { requestAnimationFrame(run); timer = setTimeout(run, 100); }, wait);
  };
  schedule.cancel = () => { pending = false; clearTimeout(timer); };
  return schedule;
}

/**
 * Wraps an async action so a repeated press (Enter twice, a double click) does not run it again
 * while the first is in flight: costly side effects such as starting a task happen once.
 */
export function singleFlight(fn) {
  let busy = false;
  return async (...args) => {
    if (busy) return undefined;
    busy = true;
    try {
      return await fn(...args);
    } finally {
      busy = false;
    }
  };
}

/**
 * The one empty state (Primer blankslate): an icon, a title, one line of what this is for, and the
 * action that fills it. Used wherever a list is empty, so every empty page looks and reads alike.
 */
export function emptyState({ iconName = 'tasks', title, text, action }) {
  return h('div', { class: 'empty-state' },
    h('span', { class: 'empty-icon', 'aria-hidden': 'true' }, icon(iconName, { size: 20 })),
    h('p', { class: 'empty-title' }, title),
    text ? h('p', { class: 'empty-text' }, text) : null,
    action || null);
}

/** A load that failed, said inline with a way out (not a spinner forever or a vanished toast). */
export function loadError(text, retry) {
  return h('div', { class: 'banner banner-danger load-error', role: 'alert' }, icon('warn', { size: 16 }), h('span', { class: 'banner-text' }, text),
    retry ? h('button', { type: 'button', class: 'btn btn-sm', onClick: retry }, 'Try again') : null);
}

/** A task state mark: the state's SVG icon in its tone (statusInfo from status.js). */
export function statusMark(info, size = 14) {
  return h('span', { class: ['glyph', `tone-${info.tone}`], role: 'img', 'aria-label': info.label, title: info.label }, icon(info.icon, { size }));
}

const PR_CHECKS = {
  success: ['check', 'ok', 'checks passed'],
  failure: ['x', 'danger', 'checks failed'],
  pending: ['clock', 'warn', 'checks running'],
};

/**
 * A pull request chip: branch icon, number and checks. A link to GitHub, or (`link: false`) a plain
 * chip that can sit inside another link, such as a sidebar row.
 */
export function prChip(pr, { link = true } = {}) {
  const merged = pr.state === 'merged';
  const checks = merged || pr.state === 'closed' ? null : PR_CHECKS[pr.checks];
  const label = `Pull request #${pr.number} · ${pr.state}${checks ? ` · ${checks[2]}` : ''}`;
  const parts = [icon('branch', { size: 12 }), `#${pr.number}`, checks ? h('span', { class: `tone-${checks[1]}` }, icon(checks[0], { size: 12 })) : null];
  const cls = ['tl-pr-chip', merged && 'merged', pr.state === 'closed' && 'closed'];
  if (link) return h('a', { class: cls, href: pr.url, target: '_blank', rel: 'noopener noreferrer', title: label, 'aria-label': label }, parts);
  return h('span', { class: cls, title: label }, parts, h('span', { class: 'sr-only' }, ` pull request, ${pr.state}${checks ? `, ${checks[2]}` : ''}`));
}

/** Small colored badge. */
export function badge(text, tone = '', title) {
  return h('span', { class: ['badge', tone && `badge-${tone}`], title: title || null }, text);
}

/** Loading / empty / error placeholders. */
export function placeholder(text, kind = 'empty') {
  return h('div', { class: ['placeholder', `placeholder-${kind}`] }, text);
}

/**
 * A calm loading state (F 2026-10-10): shimmering bars where the content will appear, announced
 * as `label`. The composer, the split card and the task view share it.
 */
export function skeleton(label, lines = 3) {
  return h('div', { class: 'skeleton', role: 'status' },
    Array.from({ length: lines }, () => h('span', { class: 'skeleton-line', 'aria-hidden': 'true' })),
    h('span', { class: 'sr-only' }, label));
}

/** Usage/percentage meter. `percent` null renders "unknown" (never zero). */
export function meter(percent, { label, detail } = {}) {
  const known = typeof percent === 'number' && Number.isFinite(percent);
  const value = known ? Math.max(0, Math.min(100, percent)) : 0;
  const tone = !known ? 'unknown' : value >= 90 ? 'danger' : value >= 70 ? 'warn' : 'ok';
  return h('div', { class: ['meter', `meter-${tone}`] },
    h('div', { class: 'meter-row' }, h('span', { class: 'meter-label' }, label || ''), h('span', { class: 'meter-value' }, known ? `${value.toFixed(value < 10 ? 1 : 0)}%` : 'unknown')),
    // A meter needs a value: an unknown one reports 0 with the text 'unknown'.
    h('div', { class: 'meter-track', role: 'meter', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': known ? value : 0, 'aria-valuetext': known ? `${Math.round(value)}%` : 'unknown', 'aria-label': label || 'usage' },
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
