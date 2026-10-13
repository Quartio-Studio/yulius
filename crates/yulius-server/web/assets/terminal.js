// Terminal views, loaded on demand (PERF-04): xterm is about 340 KB, 43 % of the app's start-up
// JavaScript, and most pages (sign-in included) never show a terminal. createTerminalView()
// returns at once with the same API as the core's view (terminal-core.js); the core, xterm's
// stylesheet and the terminal typeface arrive with the first terminal, or earlier through
// preloadTerminal(). Until the view is ready it shows a calm placeholder of the terminal's
// colour (never text at another size or font, F 2026-10-09).

import { h, setChildren } from './dom.js';
import { token, px, isPhone } from './theme.js';

const XTERM_CSS = '/assets/vendor/xterm/xterm.css';
/** Longest wait for the terminal typeface before the terminal opens with a fallback font. */
const FONT_WAIT_MS = 1500;
let core = null;

/**
 * Loads the code typeface (--font-mono, regular and bold) before the first terminal opens:
 * xterm measures its cells once, so a web font that arrives later would change the glyphs and
 * then the grid in front of the user. Bounded: after FONT_WAIT_MS the terminal opens anyway (and
 * re-measures when the font arrives, terminal-core.js).
 */
export function loadTerminalFont() {
  if (!document.fonts || !document.fonts.load) return Promise.resolve();
  const family = token('--font-mono');
  const size = px(isPhone() ? '--fs-sm' : '--fs-term') || 13;
  if (!family) return Promise.resolve();
  const loads = Promise.all(['400', '600'].map((weight) => document.fonts.load(`${weight} ${size}px ${family}`).catch(() => [])));
  return Promise.race([loads, new Promise((resolve) => { setTimeout(resolve, FONT_WAIT_MS); })]);
}

function loadStylesheet(href) {
  const existing = document.querySelector(`link[rel="stylesheet"][href="${href}"]`);
  if (existing) return existing.sheet ? Promise.resolve() : new Promise((resolve, reject) => { existing.addEventListener('load', resolve, { once: true }); existing.addEventListener('error', reject, { once: true }); });
  return new Promise((resolve, reject) => {
    const link = h('link', { rel: 'stylesheet', href, onLoad: resolve, onError: () => { link.remove(); reject(new Error('Couldn’t load the terminal styles.')); } });
    document.head.append(link);
  });
}

/** Loads the terminal core and xterm's stylesheet once; a failed load can be tried again. */
function loadCore() {
  if (!core) {
    core = Promise.all([
      import('./terminal-core.js'),
      // The core's own imports, fetched at the same time instead of after it (one round trip).
      import('./mousefilter.js'),
      import('./termkeys.js'),
      import('./termlinks.js'),
      import('./vendor/xterm/xterm.mjs'),
      import('./vendor/xterm/addon-fit.mjs'),
      import('./vendor/xterm/addon-web-links.mjs'),
      loadStylesheet(XTERM_CSS),
      loadTerminalFont(),
    ]).then(([module]) => module);
    core.catch(() => { core = null; });
  }
  return core;
}

/** Starts loading the terminal in the background (after sign-in, when the browser is idle). */
export function preloadTerminal() {
  loadCore().catch(() => {});
}

/** A terminal view (options as terminal-core.js); usable before the core has loaded. */
export function createTerminalView(options) {
  // The wrapper takes no box of its own, so the view lays out as a direct child of its host.
  const el = h('div', { class: 'term-lazy' });
  let view = null;
  let destroyed = false;
  let started = false;
  let focusWanted = false;

  function mount() {
    // The same calm placeholder the core shows until its first screen (terminal-core.js).
    setChildren(el, h('div', { class: 'term term-placeholder' }, h('div', { class: 'term-skeleton', 'aria-hidden': 'true' }, h('span'), h('span'), h('span'))));
    loadCore().then((module) => {
      if (destroyed) return;
      view = module.createTerminalView(options);
      setChildren(el, view.el);
      if (started) view.start();
      // The core focuses itself once shown, if the focus is still free then (terminal-core.js).
      if (focusWanted) view.focus();
    }, () => {
      if (destroyed) return;
      setChildren(el, h('div', { class: 'term term-placeholder', role: 'alert' },
        h('p', { class: 'term-load-error' }, 'Couldn’t load the terminal. Check the connection.',
          h('button', { type: 'button', class: 'btn btn-sm', onClick: mount }, 'Try again'))));
    });
  }
  mount();

  return {
    el,
    get term() { return view ? view.term : null; },
    /** Before the core loads the socket is not open yet; start() connects it. */
    get socket() { return view ? view.socket : { status: 'connecting' }; },
    start() {
      started = true;
      if (view) view.start();
    },
    reconnect() {
      if (view) view.reconnect();
    },
    sendKeys(data) {
      return view ? view.sendKeys(data) : false;
    },
    get canWrite() {
      return view ? view.canWrite : false;
    },
    focus() {
      if (view) view.focus();
      else focusWanted = true;
    },
    refit() {
      if (view) view.refit();
    },
    refreshEmpty() {
      if (view) view.refreshEmpty();
    },
    get ready() {
      return view ? view.ready : false;
    },
    destroy() {
      destroyed = true;
      if (view) view.destroy();
    },
  };
}
