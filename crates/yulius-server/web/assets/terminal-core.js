// The terminal's core, loaded by terminal.js with the first terminal (PERF-04): an
// xterm.js terminal bound to a terminal WebSocket (task terminal, sandbox shell,
// account login). Handles fit/resize, read-only state, control hand-over, links,
// reconnect notices and exit banners. Text behaves like page text (F 2026-10-07): xterm's DOM
// renderer, mouse reporting requested by TUIs filtered out unless the viewer hands the mouse
// to the app, and selections copied to the clipboard as soon as they are made.
//
// It appears once, at its final size and font (F 2026-10-09): the grid is fitted before the
// socket opens and the size travels with the connection (`?cols=&rows=`), so the server's first
// state and screen already have it; until both are drawn a calm placeholder of the terminal's
// colour covers it. Connection notices float over the terminal instead of pushing it down, so
// nothing around it changes its size.

import { Terminal } from './vendor/xterm/xterm.mjs';
import { FitAddon } from './vendor/xterm/addon-fit.mjs';
import { WebLinksAddon } from './vendor/xterm/addon-web-links.mjs';
import { TerminalSocket, isEndedStatus, CLOSE_REVOKED } from './termsocket.js';
import { createMouseFilter } from './mousefilter.js';
import { h, icon, setChildren } from './dom.js';
import { terminalColors, onThemeChange, token, isPhone, px } from './theme.js';
import { readJson } from './store.js';

/** Font sizes a view may take to fit a terminal whose size someone else sets (#30). */
const FIT_MIN_FONT = 8;
const FIT_MAX_SCALE = 1.6;
/** Longest time the placeholder covers a terminal whose server has not drawn it yet. */
const REVEAL_LIMIT_MS = 5000;
/** Wheel travel (px) of one notch sent to an app that scrolls itself, and the most notches per event. */
const WHEEL_NOTCH_PX = 40;
const WHEEL_MAX_NOTCHES = 10;

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
 *   emptyMessage(state)        optional {title, text} shown over a blank screen (e.g. a task
 *                              that has not started yet); hidden as soon as there is output
 *   onReady()                  the terminal is shown (its first screen is drawn)
 *   onConnection(status)       the socket's status (connecting|open|reconnecting|failed|closed)
 * Call start() after the element is in the document; destroy() when done.
 */
export function createTerminalView({ transport, path, onLink, readOnlyLabel, onState, onExit, onNotice, exitActions, exitMessage, emptyMessage, onReady, onConnection, fontSize, followServerSize = false }) {
  const banner = h('div', { class: 'term-banner', hidden: true, role: 'status' });
  const noticeEl = h('div', { class: 'term-notice', hidden: true, role: 'status', 'aria-live': 'polite' });
  // Connection trouble floats over the terminal: it never changes the terminal's size.
  const connEl = h('div', { class: 'term-conn', hidden: true, role: 'status', 'aria-live': 'polite' });
  const host = h('div', { class: 'term-host term-pending', style: followServerSize ? { overflowX: 'auto' } : null });
  // Until the first screen is drawn: the terminal's colour and a few quiet lines, no text.
  const skeleton = h('div', { class: 'term-skeleton', 'aria-hidden': 'true' }, h('span'), h('span'), h('span'));
  const emptyEl = h('div', { class: 'term-empty', hidden: true, role: 'status' });
  const mouseToggle = h('button', { type: 'button', class: 'btn btn-sm term-mouse-toggle', hidden: true });
  const el = h('div', { class: 'term', 'aria-busy': 'true' }, banner, noticeEl, connEl, mouseToggle, host, skeleton, emptyEl);
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
  // Shown once: after the first state and the first screen are drawn (or the session ended, or
  // the connection failed, or REVEAL_LIMIT_MS passed).
  let ready = false;
  let gotState = false;
  let drawn = false;
  let focusWanted = false;
  let revealTimer = 0;
  let emptyShown = false;

  const socket = new TerminalSocket({
    transport,
    path,
    // The server applies the size before its first state and screen (not for login terminals,
    // whose size is the server's).
    sizeInPath: !followServerSize,
    handlers: {
      // The size this view connects with (in the address and the hello): the server applies it
      // when this viewer may control the size, so it is not sent again as a resize.
      getSize: () => {
        const size = proposed() || { cols: term.cols, rows: term.rows };
        if (!followServerSize) lastSent = size;
        return size;
      },
      onOutput: (bytes) => {
        const had = mouse.requested();
        term.write(mouse.feed(bytes));
        if (had !== mouse.requested()) renderMouseToggle();
        if (emptyShown) renderEmpty();
      },
      onSnapshot: (bytes) => {
        mouse.reset();
        term.reset();
        term.write(mouse.feed(bytes), () => { drawn = true; maybeReveal(); });
        if (mouse.appMouse()) term.write(mouse.setAppMouse(true));
        renderMouseToggle();
      },
      onState: (message) => {
        gotState = true;
        state = { ...state, ...message };
        if (message.status && !isEndedStatus(message.status)) exited = null;
        // A task that ended before this page opened still shows how it ended (and Continue).
        else if (message.status && isEndedStatus(message.status) && !exited) exited = { status: message.status, code: null };
        term.options.disableStdin = !state.can_write;
        applySize();
        render();
        maybeReveal();
        if (onState) onState(state);
      },
      onNotice: (text) => { showNotice(text); if (onNotice) onNotice(text); },
      onExit: (message) => { exited = message; applySize(); render(); reveal(); if (onExit) onExit(message); },
      onError: (message) => {
        if (['unauthenticated', 'not_found'].includes(message.code)) { fatal = message; reveal(); }
        else showNotice(message.message || message.code || 'Terminal error');
        render();
      },
      onStatus: (status, detail) => {
        conn = status;
        connDetail = detail || {};
        if (status === 'failed') reveal();
        render();
        if (onConnection) onConnection(status, connDetail);
      },
    },
  });

  /**
   * Shows the terminal once its first state and screen are drawn, two frames later: a banner the
   * first state brought (read-only) has been laid out and the grid fitted to it by then.
   */
  function maybeReveal() {
    if (ready || !gotState || !drawn) return;
    requestAnimationFrame(() => requestAnimationFrame(() => reveal()));
  }

  function reveal() {
    if (ready || destroyed) return;
    ready = true;
    clearTimeout(revealTimer);
    host.classList.remove('term-pending');
    skeleton.classList.add('term-skeleton-done');
    el.removeAttribute('aria-busy');
    renderEmpty();
    render();
    if (focusWanted) term.focus();
    if (onReady) onReady();
  }

  /** Whether the visible screen has no text (a session that has not drawn anything yet). */
  function screenEmpty() {
    const buffer = term.buffer.active;
    for (let y = 0; y < term.rows; y += 1) {
      const line = buffer.getLine(buffer.viewportY + y);
      if (line && line.translateToString(true).trim()) return false;
    }
    return true;
  }

  // A helpful word over a blank screen (a task that has not started, waits for a slot or for an
  // account), from the caller; gone with the first output.
  function renderEmpty() {
    const message = ready && !fatal && emptyMessage && screenEmpty() ? emptyMessage(state) : null;
    emptyShown = Boolean(message);
    emptyEl.hidden = !message;
    setChildren(emptyEl, message ? [h('p', { class: 'term-empty-title' }, message.title), message.text ? h('p', { class: 'term-empty-text' }, message.text) : null] : null);
  }

  // A typeface that arrives after the terminal opened (slower than terminal.js waits): xterm
  // measured its cells with the fallback, so it measures again and fits, once.
  const fontLoaded = () => {
    try {
      return !document.fonts || !document.fonts.check || document.fonts.check(`${term.options.fontSize}px ${term.options.fontFamily}`);
    } catch {
      return true; // an unparsable font list: nothing to wait for
    }
  };
  let fontPending = false;
  const onFontsLoaded = () => {
    if (!opened || destroyed || !fontPending || !fontLoaded()) return;
    fontPending = false;
    const family = term.options.fontFamily || '';
    term.options.fontFamily = family.endsWith(' ') ? family.trimEnd() : `${family} `;
    applySize();
  };
  if (document.fonts && document.fonts.addEventListener) document.fonts.addEventListener('loadingdone', onFontsLoaded);

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
  // Claude Code draws full screen (the alternate buffer has no scrollback) and scrolls its own
  // transcript with the mouse wheel. Text selection keeps the mouse from the app (mousefilter.js),
  // so there only wheel turns go to the app, in its encoding; clicks still select. In the normal
  // buffer xterm scrolls its own scrollback (#34).
  let wheelRest = 0;
  term.attachCustomWheelEventHandler((event) => {
    if (!mouse.requested() || mouse.appMouse() || term.buffer.active.type !== 'alternate' || !state.can_write) return true;
    // Pinch-zoom (ctrl+wheel) and horizontal swipes stay with the browser (#78).
    if (event.ctrlKey || !event.deltaY) return true;
    event.preventDefault();
    const pixels = event.deltaMode === 1 ? event.deltaY * WHEEL_NOTCH_PX : event.deltaMode === 2 ? event.deltaY * term.rows * WHEEL_NOTCH_PX : event.deltaY;
    wheelRest += pixels;
    const notches = Math.trunc(wheelRest / WHEEL_NOTCH_PX);
    if (!notches) return false;
    wheelRest -= notches * WHEEL_NOTCH_PX;
    const screen = host.querySelector('.xterm-screen') || host;
    const box = screen.getBoundingClientRect();
    const col = Math.min(term.cols, Math.max(1, 1 + Math.floor(((event.clientX - box.left) / Math.max(1, box.width)) * term.cols)));
    const row = Math.min(term.rows, Math.max(1, 1 + Math.floor(((event.clientY - box.top) / Math.max(1, box.height)) * term.rows)));
    // Scrolling is not typing: it takes neither control nor the terminal size (#78).
    socket.sendInput(mouse.wheelReport(notches < 0, col, row).repeat(Math.min(Math.abs(notches), WHEEL_MAX_NOTCHES)), { scroll: true });
    return false;
  });
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
    const following = !(state.controller || exited || !state.cols || !state.rows);
    // Back to this screen's own font before measuring what fits it.
    if (!following && term.options.fontSize !== termSize()) term.options.fontSize = termSize();
    const fitted = proposed();
    // An ended session has no PTY to follow: fit it to this screen (a desktop-sized screen would
    // otherwise overflow a phone).
    if (!following) {
      if (!fitted) return;
      if (term.cols !== fitted.cols || term.rows !== fitted.rows) term.resize(fitted.cols, fitted.rows);
      if (state.controller && !exited && (!lastSent || lastSent.cols !== fitted.cols || lastSent.rows !== fitted.rows)) {
        if (socket.resize(fitted.cols, fitted.rows)) lastSent = fitted;
      }
    } else {
      if (term.cols !== state.cols || term.rows !== state.rows) term.resize(Math.max(2, state.cols), Math.max(1, state.rows));
      fitFontToWidth();
    }
  }

  // Someone else sets the terminal's size (#30): show their columns across this view's whole
  // width instead of a narrower block with an empty band — by font size, not a CSS transform, so
  // text selection and links keep their positions.
  function fitFontToWidth() {
    // How many columns this view holds at the current font, as the fit addon measures them.
    const room = proposed();
    if (!room || !room.cols || !state.cols) return;
    const current = term.options.fontSize;
    // Rows too: growing the font to the width alone could push the bottom rows (the agent's
    // input box) out of view when the controller's window is taller (#78).
    const scale = Math.min(room.cols / state.cols, room.rows && state.rows ? room.rows / state.rows : Infinity);
    const next = Math.min(termSize() * FIT_MAX_SCALE, Math.max(FIT_MIN_FONT, Math.floor(current * scale * 4) / 4));
    if (Math.abs(next - current) >= 0.25) term.options.fontSize = next;
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

  // Connection trouble, floating over the top of the terminal (it never resizes the terminal).
  function renderConn() {
    let content = null;
    if (fatal) {
      content = null;
    } else if (conn === 'reconnecting') {
      const secs = connDetail.delay ? Math.max(1, Math.round(connDetail.delay / 1000)) : null;
      content = [icon('warn', { size: 14 }), h('span', null, connDetail.code === 4008 ? 'Catching up with the session…' : `Connection lost — reconnecting${secs ? ` in ${secs}s` : ''}…`),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => socket.retry() }, 'Retry now')];
    } else if (conn === 'failed') {
      content = [icon('st-failed', { size: 14 }), h('span', null, connDetail.code === CLOSE_REVOKED ? 'Your access to this terminal was revoked.' : 'Disconnected from the terminal.'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => socket.retry() }, 'Reconnect')];
    } else if (conn === 'connecting' && ready) {
      content = [h('span', { class: 'spinner', 'aria-hidden': 'true' }), h('span', null, 'Connecting…')];
    }
    connEl.hidden = !content;
    connEl.classList.toggle('term-conn-error', conn === 'failed');
    setChildren(connEl, content);
  }

  function render() {
    renderConn();
    const rows = [];
    if (fatal) {
      rows.push(bannerRow('error', fatal.code === 'not_found' ? 'This terminal is not available.' : fatal.message || 'Access to this terminal was revoked.'));
    }
    if (exited) {
      const message = exitMessage ? exitMessage(exited) : { tone: 'info', text: 'The session ended.' };
      const details = [exited.status, exited.code === null || exited.code === undefined ? null : `exit ${exited.code}`].filter(Boolean).join(' · ');
      rows.push(bannerRow(message.tone, message.text, exitActions ? exitActions(exited) : [], details));
    } else if (conn === 'open' && gotState && !state.can_write) {
      // Only once the server said so: a read-only row shown between the socket opening and the
      // first state would resize the terminal twice (and make the agent redraw twice).
      rows.push(bannerRow('readonly', readOnlyLabel ? readOnlyLabel() : 'Read-only'));
    } else if (conn === 'open' && state.can_write && !state.controller && state.cols && !followServerSize) {
      rows.push(bannerRow('info', 'Someone else controls this terminal’s size; it is fitted to your width.', [{ label: 'Take control', run: () => { if (socket.takeControl()) { state = { ...state, controller: true }; lastSent = null; applySize(); render(); term.focus(); } } }]));
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
    /** Open xterm in the (attached) host, fit it, and connect with that size. */
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
      fontPending = !fontLoaded();
      observer.observe(host);
      applySize();
      render();
      socket.start();
      // Never stuck behind the placeholder: a silent server still gets the terminal shown.
      revealTimer = setTimeout(() => reveal(), REVEAL_LIMIT_MS);
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
    /** Focus the terminal now, or as soon as it is shown. */
    focus() {
      focusWanted = true;
      if (opened && ready) term.focus();
    },
    get ready() {
      return ready;
    },
    /** The caller's empty-screen message may have changed (e.g. the task's status). */
    refreshEmpty() {
      renderEmpty();
    },
    refit() {
      applySize();
    },
    destroy() {
      destroyed = true;
      clearTimeout(noticeTimer);
      clearTimeout(revealTimer);
      cancelAnimationFrame(resizeFrame);
      observer.disconnect();
      if (document.fonts && document.fonts.removeEventListener) document.fonts.removeEventListener('loadingdone', onFontsLoaded);
      socket.close();
      offTheme();
      term.dispose();
    },
  };
}
