// Terminal views, loaded on demand (PERF-04): xterm is about 340 KB, 43 % of the app's start-up
// JavaScript, and most pages (sign-in included) never show a terminal. createTerminalView()
// returns at once with the same API as the core's view (terminal-core.js); the core and xterm's
// stylesheet arrive with the first terminal, or earlier through preloadTerminal().

import { h, setChildren } from './dom.js';

const XTERM_CSS = '/assets/vendor/xterm/xterm.css';
let core = null;

function loadStylesheet(href) {
  const existing = document.querySelector(`link[rel="stylesheet"][href="${href}"]`);
  if (existing) return existing.sheet ? Promise.resolve() : new Promise((resolve, reject) => { existing.addEventListener('load', resolve, { once: true }); existing.addEventListener('error', reject, { once: true }); });
  return new Promise((resolve, reject) => {
    const link = h('link', { rel: 'stylesheet', href, onLoad: resolve, onError: () => { link.remove(); reject(new Error('Could not load the terminal styles.')); } });
    document.head.append(link);
  });
}

/** Loads the terminal core and xterm's stylesheet once; a failed load can be tried again. */
function loadCore() {
  if (!core) {
    core = Promise.all([import('./terminal-core.js'), loadStylesheet(XTERM_CSS)]).then(([module]) => module);
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
    setChildren(el, h('div', { class: 'term term-placeholder' }));
    loadCore().then((module) => {
      if (destroyed) return;
      view = module.createTerminalView(options);
      setChildren(el, view.el);
      if (started) view.start();
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
    destroy() {
      destroyed = true;
      if (view) view.destroy();
    },
  };
}
