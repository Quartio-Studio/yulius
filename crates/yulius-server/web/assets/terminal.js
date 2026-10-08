// xterm.js terminal bound to a terminal WebSocket (task terminal, sandbox shell,
// account login). Handles fit/resize, read-only state, control hand-over, links,
// reconnect banners and exit notices. Text behaves like page text (F 2026-10-07): xterm's DOM
// renderer, mouse reporting requested by TUIs filtered out unless the viewer hands the mouse
// to the app, and selections copied to the clipboard as soon as they are made.

import { Terminal } from './vendor/xterm/xterm.mjs';
import { FitAddon } from './vendor/xterm/addon-fit.mjs';
import { WebLinksAddon } from './vendor/xterm/addon-web-links.mjs';
import { TerminalSocket, isEndedStatus, CLOSE_REVOKED } from './termsocket.js';
import { createMouseFilter } from './mousefilter.js';
import { h, icon, setChildren } from './dom.js';
import { terminalColors, onThemeChange } from './theme.js';

const MONO = 'ui-monospace, "SF Mono", SFMono-Regular, Menlo, Consolas, "Liberation Mono", "DejaVu Sans Mono", monospace';

/**
 * Create a terminal view. Options:
 *   transport, path            WebSocket transport and path (e.g. /api/tasks/{tid}/terminal)
 *   onLink(uri, event)         link activation (web links and OSC 8 hyperlinks)
 *   readOnlyLabel()            banner text when the server says can_write=false
 *   onState(state), onExit(exit), onNotice(text)
 *   exitActions(exit)          optional [{label, run}] shown in the exit banner
 *   followServerSize           render at the PTY size from `state` and never ask the server
 *                              to resize (account login terminals: fixed 500 columns so
 *                              tokens and URLs never wrap; the view scrolls horizontally)
 * Call start() after the element is in the document; destroy() when done.
 */
export function createTerminalView({ transport, path, onLink, readOnlyLabel, onState, onExit, onNotice, exitActions, fontSize, followServerSize = false }) {
  const banner = h('div', { class: 'term-banner', hidden: true, role: 'status' });
  const noticeEl = h('div', { class: 'term-notice', hidden: true, role: 'status', 'aria-live': 'polite' });
  const host = h('div', { class: 'term-host', style: followServerSize ? { overflowX: 'auto' } : null });
  const mouseToggle = h('button', { type: 'button', class: 'btn btn-sm term-mouse-toggle', hidden: true });
  const el = h('div', { class: 'term' }, banner, noticeEl, mouseToggle, host);
  const mouse = createMouseFilter();
  // The app asked for mouse reporting: offer to hand the mouse over (text selection otherwise).
  function renderMouseToggle() {
    mouseToggle.hidden = !mouse.requested();
    mouseToggle.textContent = mouse.appMouse() ? 'Mouse: app · select text instead' : 'Mouse: select text · send to app';
    mouseToggle.title = mouse.appMouse()
      ? 'Clicks go to the program; Option-drag still selects. Click to select text with the mouse again.'
      : 'The program asked for the mouse. Clicks select text now; click to send them to the program.';
  }
  mouseToggle.addEventListener('click', () => {
    term.write(mouse.setAppMouse(!mouse.appMouse()));
    renderMouseToggle();
  });

  const size = fontSize || (window.matchMedia('(max-width: 767px)').matches ? 12 : 13);
  const term = new Terminal({
    fontFamily: MONO,
    fontSize: size,
    lineHeight: 1.15,
    cursorBlink: !window.matchMedia('(prefers-reduced-motion: reduce)').matches,
    scrollback: 5000,
    theme: terminalColors(),
    macOptionIsMeta: true,
    // While the app has the mouse, Option-drag (macOS) / Shift-drag still selects text.
    macOptionClickForcesSelection: true,
    disableStdin: true,
    allowProposedApi: false,
    linkHandler: { activate: (event, text) => onLink && onLink(text, event) },
  });
  // Colours come from tokens.css and follow theme changes (theme.js).
  const offTheme = onThemeChange(() => { term.options.theme = terminalColors(); });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.loadAddon(new WebLinksAddon((event, uri) => onLink && onLink(uri, event)));

  let state = { can_write: false, controller: false, cols: null, rows: null, status: null };
  let conn = 'connecting';
  let connDetail = {};
  let exited = null;
  let fatal = null;
  let lastSent = null;
  let noticeTimer = null;
  let resizeFrame = 0;
  let opened = false;
  let destroyed = false;

  const socket = new TerminalSocket({
    transport,
    path,
    handlers: {
      getSize: () => proposed() || { cols: term.cols, rows: term.rows },
      onOutput: (bytes) => {
        const had = mouse.requested();
        term.write(mouse.feed(bytes));
        if (had !== mouse.requested()) renderMouseToggle();
      },
      onSnapshot: (bytes) => {
        mouse.reset();
        term.reset();
        term.write(mouse.feed(bytes));
        if (mouse.appMouse()) term.write(mouse.setAppMouse(true));
        renderMouseToggle();
      },
      onState: (message) => {
        state = { ...state, ...message };
        if (message.status && !isEndedStatus(message.status)) exited = null;
        // A task that ended before this page opened still shows how it ended (and Continue).
        else if (message.status && isEndedStatus(message.status) && !exited) exited = { status: message.status, code: null };
        term.options.disableStdin = !state.can_write;
        applySize();
        render();
        if (onState) onState(state);
      },
      onNotice: (text) => { showNotice(text); if (onNotice) onNotice(text); },
      onExit: (message) => { exited = message; render(); if (onExit) onExit(message); },
      onError: (message) => {
        if (['unauthenticated', 'not_found'].includes(message.code)) fatal = message;
        else showNotice(message.message || message.code || 'Terminal error');
        render();
      },
      onStatus: (status, detail) => { conn = status; connDetail = detail || {}; render(); },
    },
  });

  // Copy on select, like many terminals: the selection is in the clipboard right away.
  let copyTimer = 0;
  term.onSelectionChange(() => {
    clearTimeout(copyTimer);
    copyTimer = setTimeout(() => {
      const text = term.getSelection();
      if (!text || !navigator.clipboard) return;
      navigator.clipboard.writeText(text).then(() => showNotice('Copied'), () => {});
    }, 120);
  });
  term.onData((data) => send(data));
  term.onBinary((data) => send(data));

  function proposed() {
    if (!opened) return null;
    try {
      const d = fit.proposeDimensions();
      if (d && Number.isFinite(d.cols) && Number.isFinite(d.rows) && d.cols > 2 && d.rows > 1) {
        return { cols: Math.min(d.cols, 400), rows: Math.min(d.rows, 200) };
      }
    } catch { /* not measurable while hidden */ }
    return null;
  }

  // The controlling viewer's size drives the PTY (TSK-006: the last to type controls
  // the size). Other viewers render at the PTY size reported in state frames.
  function applySize() {
    if (followServerSize) {
      if (state.cols && state.rows && (term.cols !== state.cols || term.rows !== state.rows)) term.resize(Math.max(2, state.cols), Math.max(1, state.rows));
      return;
    }
    const fitted = proposed();
    if (state.controller || !state.cols || !state.rows) {
      if (!fitted) return;
      if (term.cols !== fitted.cols || term.rows !== fitted.rows) term.resize(fitted.cols, fitted.rows);
      if (state.controller && (!lastSent || lastSent.cols !== fitted.cols || lastSent.rows !== fitted.rows)) {
        if (socket.resize(fitted.cols, fitted.rows)) lastSent = fitted;
      }
    } else if (term.cols !== state.cols || term.rows !== state.rows) {
      term.resize(Math.max(2, state.cols), Math.max(1, state.rows));
    }
  }

  function send(data) {
    if (!state.can_write) {
      showNotice(readOnlyLabel ? readOnlyLabel() : 'Read-only');
      return false;
    }
    if (!socket.sendInput(data)) {
      showNotice('Not connected — input was not sent');
      return false;
    }
    if (!state.controller && !followServerSize) {
      // The server makes the last typist the controller; mirror it until the next state frame.
      state = { ...state, controller: true };
      lastSent = null;
      applySize();
      render();
    }
    return true;
  }

  function showNotice(text) {
    noticeEl.textContent = text;
    noticeEl.hidden = false;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => { noticeEl.hidden = true; }, 6000);
  }

  function bannerRow(kind, text, actions = []) {
    return h('div', { class: ['term-banner-row', `term-banner-${kind}`] },
      kind === 'readonly' ? icon('lock', { size: 14 }) : null,
      h('span', { class: 'term-banner-text' }, text),
      ...actions.map((a) => h('button', { type: 'button', class: 'btn btn-sm', onClick: a.run }, a.label)));
  }

  function render() {
    const rows = [];
    if (fatal) {
      rows.push(bannerRow('error', fatal.code === 'not_found' ? 'This terminal is not available.' : fatal.message || 'Access to this terminal was revoked.'));
    } else if (conn === 'reconnecting') {
      const secs = connDetail.delay ? Math.max(1, Math.round(connDetail.delay / 1000)) : null;
      rows.push(bannerRow('warn', connDetail.code === 4008 ? 'Catching up with the session…' : `Connection lost — reconnecting${secs ? ` in ${secs}s` : ''}…`, [{ label: 'Retry now', run: () => socket.retry() }]));
    } else if (conn === 'failed' && connDetail.code === CLOSE_REVOKED) {
      rows.push(bannerRow('error', 'Your access to this terminal was revoked.', [{ label: 'Reconnect', run: () => socket.retry() }]));
    } else if (conn === 'failed') {
      rows.push(bannerRow('error', 'Disconnected from the terminal.', [{ label: 'Reconnect', run: () => socket.retry() }]));
    } else if (conn === 'connecting') {
      rows.push(bannerRow('info', 'Connecting…'));
    }
    if (exited) {
      const code = exited.code === null || exited.code === undefined ? '' : ` (exit ${exited.code})`;
      rows.push(bannerRow('info', `Session ended: ${exited.status || 'closed'}${code}.`, exitActions ? exitActions(exited) : []));
    } else if (conn === 'open' && !state.can_write) {
      rows.push(bannerRow('readonly', readOnlyLabel ? readOnlyLabel() : 'Read-only'));
    } else if (conn === 'open' && state.can_write && !state.controller && state.cols && !followServerSize) {
      rows.push(bannerRow('info', 'Someone else controls this terminal; you see it at their size.', [{ label: 'Take control', run: () => { if (socket.takeControl()) { state = { ...state, controller: true }; lastSent = null; applySize(); render(); term.focus(); } } }]));
    }
    setChildren(banner, rows);
    banner.hidden = rows.length === 0;
  }

  const observer = new ResizeObserver(() => {
    cancelAnimationFrame(resizeFrame);
    resizeFrame = requestAnimationFrame(() => { if (!destroyed) applySize(); });
  });

  return {
    el,
    term,
    socket,
    /** Open xterm in the (attached) host and connect. */
    start() {
      if (opened || destroyed) return;
      term.open(host);
      opened = true;
      observer.observe(host);
      applySize();
      render();
      socket.start();
    },
    /** Reconnect after the task was continued. */
    reconnect() {
      exited = null;
      fatal = null;
      socket.retry();
    },
    /** Send input from the phone quick-input bar. */
    sendKeys(data) {
      return send(data);
    },
    get canWrite() {
      return state.can_write;
    },
    focus() {
      if (opened) term.focus();
    },
    refit() {
      applySize();
    },
    destroy() {
      destroyed = true;
      clearTimeout(noticeTimer);
      cancelAnimationFrame(resizeFrame);
      observer.disconnect();
      socket.close();
      offTheme();
      term.dispose();
    },
  };
}
