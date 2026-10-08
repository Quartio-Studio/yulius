// The terminal's core, loaded by terminal.js with the first terminal (PERF-04): an
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
import { terminalColors, onThemeChange, token, isPhone, px } from './theme.js';
import { readJson } from './store.js';


/**
 * Create a terminal view. Options:
 *   transport, path            WebSocket transport and path (e.g. /api/tasks/{tid}/terminal)
 *   onLink(uri, event)         link activation (web links and OSC 8 hyperlinks)
 *   readOnlyLabel()            banner text when the server says can_write=false
 *   onState(state), onExit(exit), onNotice(text)
 *   exitActions(exit)          optional [{label, run}] shown in the exit banner
 *   exitMessage(exit)          optional {tone, text} for the exit banner (status.js endedMessage)
 *   followServerSize           render at the PTY size from `state` and never ask the server
 *                              to resize (account login terminals: fixed 500 columns so
 *                              tokens and URLs never wrap; the view scrolls horizontally)
 * Call start() after the element is in the document; destroy() when done.
 */
export function createTerminalView({ transport, path, onLink, readOnlyLabel, onState, onExit, onNotice, exitActions, exitMessage, fontSize, followServerSize = false }) {
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

  const termSize = () => fontSize || px(isPhone() ? '--fs-sm' : '--fs-term'); // tokens.css
  const size = termSize();
  const term = new Terminal({
    fontFamily: token('--font-mono'), // the same typeface as the UI's code (tokens.css)
    fontWeight: 400,
    fontWeightBold: 600, // a weight the UI scale (and a brand font's files) provides
    fontSize: size,
    lineHeight: 1.15,
    cursorBlink: !window.matchMedia('(prefers-reduced-motion: reduce)').matches,
    scrollback: 5000,
    theme: terminalColors(),
    // Agents print 256-colour and true-colour text that bypasses the theme; xterm lifts any cell
    // below WCAG AA against its background, in both themes.
    minimumContrastRatio: 4.5,
    // Settings → Preferences: an accessible text mirror of the screen for screen readers (it costs
    // some speed, so it is opt-in).
    screenReaderMode: readJson('yulius.terminalScreenReader', false) === true,
    macOptionIsMeta: true,
    // While the app has the mouse, Option-drag (macOS) / Shift-drag still selects text.
    macOptionClickForcesSelection: true,
    disableStdin: true,
    allowProposedApi: false,
    linkHandler: { activate: (event, text) => onLink && onLink(text, event) },
  });
  // Colours and the code typeface come from tokens.css and follow theme and brand changes (theme.js).
  const offTheme = onThemeChange(() => {
    term.options.theme = terminalColors(host.isConnected ? host : undefined); // a theme island counts
    const font = token('--font-mono');
    const nextSize = termSize();
    if ((font && term.options.fontFamily !== font) || (nextSize && term.options.fontSize !== nextSize)) {
      if (font) term.options.fontFamily = font;
      if (nextSize) term.options.fontSize = nextSize;
      if (opened) applySize(); // new glyph sizes: fit the grid again
    }
  });
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
      onExit: (message) => { exited = message; applySize(); render(); if (onExit) onExit(message); },
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
    // An ended session has no PTY to follow: fit it to this screen (a desktop-sized screen would
    // otherwise overflow a phone).
    if (state.controller || exited || !state.cols || !state.rows) {
      if (!fitted) return;
      if (term.cols !== fitted.cols || term.rows !== fitted.rows) term.resize(fitted.cols, fitted.rows);
      if (state.controller && !exited && (!lastSent || lastSent.cols !== fitted.cols || lastSent.rows !== fitted.rows)) {
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

  const BANNER_ICONS = { readonly: 'lock', warn: 'warn', error: 'st-failed' };
  function bannerRow(kind, text, actions = [], details = '') {
    return h('div', { class: ['term-banner-row', `term-banner-${kind}`] },
      BANNER_ICONS[kind] ? icon(BANNER_ICONS[kind], { size: 14 }) : null,
      // The status code stays out of the sentence; it is in the tooltip for support.
      h('span', { class: 'term-banner-text', title: details || null }, text),
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
      const message = exitMessage ? exitMessage(exited) : { tone: 'info', text: 'The session ended.' };
      const details = [exited.status, exited.code === null || exited.code === undefined ? null : `exit ${exited.code}`].filter(Boolean).join(' · ');
      rows.push(bannerRow(message.tone, message.text, exitActions ? exitActions(exited) : [], details));
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
      term.options.theme = terminalColors(host); // attached now: colours as seen where it sits
      // The terminal keeps Tab and Esc; say how to leave it (WCAG 2.1.2 asks that the way out be told).
      const input = host.querySelector('.xterm-helper-textarea');
      if (input) {
        input.setAttribute('aria-description', 'Terminal. Press F6 to move to the next area, Shift+F6 to go back.');
        input.title = 'F6 moves to the next area';
      }
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
