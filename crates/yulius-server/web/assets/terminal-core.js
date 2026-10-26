// The terminal's core, loaded by terminal.js with the first terminal (PERF-04): an
// xterm.js terminal bound to a terminal WebSocket (task terminal, sandbox shell,
// account login). Handles fit/resize, read-only state, who sets the size, links,
// reconnect notices and exit banners. Text is page text (F 2026-10-07): xterm's DOM renderer, and
// selections copied to the clipboard as soon as they are made. The mouse is the program's when it
// asks for it, as in a native terminal (SPEC-098): Shift-drag (Option-drag on macOS) still selects;
// a read-only viewer, who cannot send anything, keeps the mouse for selection (mousefilter.js).
//
// Each viewer sees the terminal full size on its side (F 2026-10-10). A PTY has one size: the
// active viewer's. Every writer reports the size its own window fits and whether it is active
// (visible, focused, used in the last minute); typing or clicking into the terminal takes the size
// at once, an active viewer takes it from an idle one, and a viewer alone always has it (the
// server's policy: crates/yulius-app/src/hub/control.rs). A viewer who does not set the size sees
// the other viewer's grid scaled to fill its box — font size, not a CSS transform, so the text
// stays selectable HTML — with a small chip instead of a banner.
//
// Copy and paste work as in a desktop terminal (termkeys.js): ⌘C/⌘V on macOS, Ctrl+C copies a
// selection (and sends ^C without one), Ctrl+V pastes (bracketed when the program asked for it),
// Ctrl+Shift+C/V and a right-click menu with Copy and Paste. A program that has the mouse selects
// a plain drag itself and copies it with OSC 52 (Claude Code does): the write reaches the
// clipboard for a viewer who just used this terminal, and when the browser refuses it the copy key
// copies it (SPEC-158, ADR-0130). Paths of existing files are links to the built-in editor
// (termlinks.js).
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
import { clipboardKeyAction, osc52Text } from './termkeys.js';
import { isTyping } from './typing.js';
import { createFileChecker, createFileLinkProvider, relativeCwd, trackOscFileLinks } from './termlinks.js';
import { h, icon, setChildren, openMenu, keepFocus } from './dom.js';
import { terminalColors, onThemeChange, token, isPhone, px } from './theme.js';
import { readJson } from './store.js';
import { IS_MAC } from './shortcuts.js';

/** Font sizes a view may take to show a terminal whose size another viewer sets, and the step. */
const FIT_MIN_FONT = 6;
const FIT_MAX_SCALE = 3;
const FIT_STEP = 0.25;
/** Line spacing, and the tightest a shown grid may take to grow wider in a short box. */
const LINE_HEIGHT = 1.15;
const FIT_MIN_LINE_HEIGHT = 1;
/** A viewer that has not used the terminal for this long is idle: an active viewer may take the size. */
const IDLE_MS = 60000;
/** A click shorter than this (and without a drag or a selection) is a click, not the start of a double-click. */
const CLICK_SETTLE_MS = 450;
/** Input sent in pieces of at most this many UTF-16 units (the server takes 64 KiB per frame). */
const INPUT_CHUNK = 8192;
/** Longest time the placeholder covers a terminal whose server has not drawn it yet. */
const REVEAL_LIMIT_MS = 5000;
/** A program's clipboard write (OSC 52) counts only this soon after this viewer's key or click. */
const PROGRAM_COPY_MS = 5000;
/** A wheel turn up that left the screen's upper half unchanged this long reached the program's top. */
const TOP_SETTLE_MS = 400;
/** How long the way on past the top shows (SPEC-326 HIS-002). */
const TOP_OFFER_MS = 8000;

/**
 * Whether the person is typing in a text field outside `root` that is not a terminal's own input
 * (xterm's hidden textarea), so a terminal shown late leaves the keyboard there (SPEC-052). Monaco
 * on Chromium types through an EditContext on a plain div (SPEC-056 WUI-101).
 */
function typingElsewhere(root) {
  const active = document.activeElement;
  if (!active || root.contains(active) || active.closest('.xterm')) return false;
  if (active.isContentEditable || active.tagName === 'TEXTAREA' || active.editContext || active.closest('.monaco-editor')) return true;
  return active.tagName === 'INPUT' && !['button', 'checkbox', 'radio', 'submit', 'reset', 'range', 'color', 'file', 'image'].includes(active.type);
}

/**
 * Order of the focus requests of every terminal on the page: a request that waits for its
 * terminal to show lands only if no later one was made (SPEC-056 WUI-102).
 */
let focusRequests = 0;

/** Whether the keyboard is on nothing in particular (the page itself). */
function keyboardOnPage() {
  const active = document.activeElement;
  return !active || active === document.body || active === document.documentElement;
}

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
 *   emptyMessage(state)        optional {title, text, busy} shown over a blank screen (e.g. a task
 *                              that has not started yet); hidden as soon as there is output
 *   onReady()                  the terminal is shown (its first screen is drawn)
 *   onConnection(status)       the socket's status (connecting|open|reconnecting|failed|closed)
 *   onFatal(message)           the server refused this terminal for good (not_found, unauthenticated)
 *   topOffer                   optional {label, open()}: a button with `label` over the top of the
 *                              terminal when a wheel turn up reached it (the program's screen did not
 *                              move, or xterm's own scrollback is at its first line); `open()` on a
 *                              click (SPEC-326)
 *   fileLinks                  optional {list(dir) → Promise<[{name, kind}]>, roots() → [absolute
 *                              folder paths], open({path, line, col}, event)}: paths of existing
 *                              files in the terminal's folder become links (termlinks.js)
 * Call start() after the element is in the document; destroy() when done.
 */
export function createTerminalView({ transport, path, onLink, readOnlyLabel, onState, onExit, onNotice, exitActions, exitMessage, emptyMessage, onReady, onConnection, onFatal, topOffer = null, fontSize, fileLinks = null, followServerSize = false }) {
  const banner = h('div', { class: 'term-banner', hidden: true, role: 'status' });
  const noticeEl = h('div', { class: 'term-notice', hidden: true, role: 'status', 'aria-live': 'polite' });
  // Connection trouble floats over the terminal: it never changes the terminal's size.
  const connEl = h('div', { class: 'term-conn', hidden: true, role: 'status', 'aria-live': 'polite' });
  // xterm opens in `fitBox`, which has no padding, so the fit measures exactly the room it has.
  const fitBox = h('div', { class: 'term-fit' });
  const host = h('div', { class: 'term-host term-pending', style: followServerSize ? { overflowX: 'auto' } : null }, fitBox);
  // Until the first screen is drawn: the terminal's colour and a few quiet lines, no text.
  const skeleton = h('div', { class: 'term-skeleton', 'aria-hidden': 'true' }, h('span'), h('span'), h('span'));
  const emptyEl = h('div', { class: 'term-empty', hidden: true, role: 'status' });
  // Another viewer sets the size: a small, opaque chip in the corner (never a row that resizes the terminal).
  const sizeChip = h('span', { class: 'term-size-chip', hidden: true, role: 'status', title: 'Another viewer is using this terminal, so it shows their size, scaled to your window. Click in the terminal or type to use your own size.' }, 'Sized by another viewer');
  const corner = h('div', { class: 'term-corner' }, noticeEl, sizeChip);
  // The way on past the terminal's top (SPEC-326): over the top, below the banner, out of the way of
  // the keyboard and the size.
  const topOfferEl = topOffer ? h('button', { type: 'button', class: 'btn btn-sm term-history-offer', hidden: true, onClick: () => { hideTopOffer(); topOffer.open(); } }, icon('up', { size: 14 }), topOffer.label) : null;
  const el = h('div', { class: 'term', 'aria-busy': 'true' }, banner, connEl, topOfferEl, corner, host, skeleton, emptyEl);
  // The program's mouse reporting reaches xterm only for a viewer who can write (SPEC-098 MSE-004).
  const mouse = createMouseFilter();

  const termSize = () => fontSize || px(isPhone() ? '--fs-sm' : '--fs-term'); // tokens.css
  const size = termSize();
  const term = new Terminal({
    fontFamily: token('--font-mono'), // the same typeface as the UI's code (tokens.css)
    fontWeight: 400,
    fontWeightBold: 600, // a weight the UI scale (and a brand font's files) provides
    fontSize: size,
    lineHeight: LINE_HEIGHT,
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
    // While the program has the mouse, Option-drag (macOS) / Shift-drag still selects text.
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
  // The order of this terminal's pending focus request (0: none).
  let focusWanted = 0;
  let revealTimer = 0;
  let emptyShown = false;
  // What this viewer last told the server about itself (null: nothing yet on this connection).
  let present = null;
  let lastUse = Date.now();
  let presenceTimer = 0;
  // The working directory a shell reports (OSC 7), absolute.
  let cwd = '';

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
        const size = ownSize() || { cols: term.cols, rows: term.rows };
        if (!followServerSize) lastSent = size;
        return size;
      },
      onOutput: (bytes) => {
        term.write(mouse.feed(bytes));
        if (emptyShown) renderEmpty();
      },
      onSnapshot: (bytes) => {
        mouse.reset();
        if (oscFiles) oscFiles.reset();
        term.reset();
        term.write(mouse.feed(bytes), () => { drawn = true; maybeReveal(); });
      },
      onState: (message) => {
        const handedOver = gotState && !state.controller && message.controller;
        gotState = true;
        state = { ...state, ...message };
        // Control just came to this viewer (another one went idle or left): the server applied the
        // size this viewer reported; say it again in case that report was lost.
        if (handedOver) lastSent = null;
        if (message.status && !isEndedStatus(message.status)) exited = null;
        // A task that ended before this page opened still shows how it ended (and Continue).
        else if (message.status && isEndedStatus(message.status) && !exited) exited = { status: message.status, code: null };
        term.options.disableStdin = !state.can_write;
        if (mouse.appMouse() !== state.can_write) term.write(mouse.setAppMouse(state.can_write));
        applySize();
        render();
        maybeReveal();
        updatePresence();
        if (onState) onState(state);
      },
      onNotice: (text) => { showNotice(text); if (onNotice) onNotice(text); },
      onExit: (message) => { exited = message; applySize(); render(); reveal(); if (onExit) onExit(message); },
      onError: (message) => {
        if (['unauthenticated', 'not_found'].includes(message.code)) { fatal = message; reveal(); if (onFatal) onFatal(message); }
        else showNotice(message.message || message.code || 'Terminal error');
        render();
      },
      onStatus: (status, detail) => {
        conn = status;
        connDetail = detail || {};
        // A new connection is a new viewer for the server: it learns this viewer's presence again.
        if (status !== 'open') present = null;
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
    // Focus asked for before the terminal showed lands now, unless the person is typing in a
    // text field meanwhile (the queued messages box beside a waiting task's terminal), or another
    // terminal was asked for since (a Shell opened meanwhile): the terminal asked for last wins.
    // Buttons and tabs do not hold it back.
    if (focusWanted && (focusWanted === focusRequests || keyboardOnPage()) && !typingElsewhere(el)) term.focus();
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
    setChildren(emptyEl, message ? [h('p', { class: 'term-empty-title' }, message.title), message.text ? h('p', { class: 'term-empty-text' }, message.text) : null, message.busy ? h('span', { class: 'term-empty-bar', 'aria-hidden': 'true' }) : null] : null);
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

  // The text of the program's last copy (OSC 52) the browser refused, for the copy keys.
  let programCopy = '';
  // Copy on select, like many terminals: the selection is in the clipboard right away.
  let copyTimer = 0;
  term.onSelectionChange(() => {
    // A selection of xterm's own is what the copy keys take from now on, not an older program copy.
    if (term.hasSelection()) programCopy = '';
    clearTimeout(copyTimer);
    copyTimer = setTimeout(() => {
      const text = term.getSelection();
      if (!text || !navigator.clipboard) return;
      navigator.clipboard.writeText(text).then(() => showNotice('Copied'), () => {});
    }, 120);
  });
  term.onData((data) => send(data));

  // ---- copy and paste (termkeys.js)
  const readOnlyText = () => (readOnlyLabel ? readOnlyLabel() : 'Read-only');
  function writeClipboard(text) {
    if (!text) return;
    if (!navigator.clipboard || !navigator.clipboard.writeText) { showNotice('Copying is not available in this browser'); return; }
    navigator.clipboard.writeText(text).then(() => showNotice('Copied'), () => showNotice('The browser blocked the clipboard'));
  }
  /**
   * Copies the selection from a key press: the browser's copy command (xterm fills it), else the
   * clipboard API. Without a selection of xterm's own, the program's copy the browser refused.
   */
  function copySelection(clear) {
    const text = term.getSelection() || programCopy;
    if (!text) return;
    const fromProgram = !term.hasSelection();
    // xterm fills the copy event only for its own selection; the program's text is filled here.
    let filled = false;
    const fill = (event) => {
      if (!event.clipboardData) return;
      event.clipboardData.setData('text/plain', text);
      event.preventDefault();
      filled = true;
    };
    if (fromProgram) document.addEventListener('copy', fill, true);
    let copied = false;
    try { copied = document.execCommand('copy'); } catch { copied = false; }
    if (fromProgram) {
      document.removeEventListener('copy', fill, true);
      copied = copied && filled;
    }
    if (copied) showNotice('Copied');
    else writeClipboard(text);
    if (fromProgram) programCopy = '';
    // After Ctrl+C the selection goes, so the next Ctrl+C interrupts the program (as in Windows Terminal).
    if (clear) term.clearSelection();
  }

  // The program's copy (OSC 52): Claude Code copies the selection it draws (a plain drag is its
  // own) this way, as a native terminal expects. One PTY shows to several viewers and runs an
  // agent's code, so only a viewer who can write and pressed a key or the mouse here moments ago
  // takes it; a query is never answered. A write the browser refuses (Safari, no user gesture;
  // plain HTTP, no clipboard API) waits for the copy key, whose copy command needs no permission.
  let handsOnAt = 0;
  const handsOn = () => { handsOnAt = Date.now(); };
  el.addEventListener('keydown', handsOn, true);
  // A drag may end anywhere (the program still gets that release): the release counts too.
  el.addEventListener('mousedown', () => {
    handsOn();
    window.addEventListener('mouseup', handsOn, { capture: true, once: true });
  }, true);
  term.parser.registerOscHandler(52, (data) => {
    const text = osc52Text(data);
    if (!text || !state.can_write || Date.now() - handsOnAt > PROGRAM_COPY_MS) return true;
    const refused = () => {
      programCopy = text;
      showNotice(`Press ${IS_MAC ? '⌘C' : 'Ctrl+Shift+C'} to copy`);
    };
    if (!navigator.clipboard || !navigator.clipboard.writeText) { refused(); return true; }
    navigator.clipboard.writeText(text).then(() => { programCopy = ''; showNotice('Copied'); }, refused);
    return true;
  });
  /** Pastes the clipboard's text where no paste key exists (Ctrl+V on macOS, the menu). */
  async function pasteFromClipboard() {
    if (!state.can_write) { showNotice(readOnlyText()); return; }
    let text = '';
    try {
      text = await navigator.clipboard.readText();
    } catch {
      showNotice(`The browser did not share the clipboard; paste with ${IS_MAC ? '⌘V' : 'Ctrl+V'}`);
      return;
    }
    // xterm wraps it in bracketed-paste markers when the program asked for them, then sends it.
    if (text && !destroyed) term.paste(text);
  }
  term.attachCustomKeyEventHandler((event) => {
    const action = clipboardKeyAction(event, { mac: IS_MAC, selection: term.hasSelection() });
    if (!action) return true;
    if (event.type === 'keydown') {
      if (action.copy) {
        event.preventDefault();
        copySelection(action.clear);
      } else if (action.paste === 'read') {
        event.preventDefault();
        pasteFromClipboard();
      } else if (!state.can_write) {
        event.preventDefault();
        showNotice(readOnlyText());
      }
      // `native`: the browser's paste command follows the key and delivers a paste event, which
      // xterm turns into input (no clipboard permission needed).
    }
    return false; // xterm never turns these keys into ^C/^V
  });
  // Read-only viewers cannot paste (any paste: keys, the browser's menu, drag and drop of text).
  el.addEventListener('paste', (event) => {
    if (state.can_write) return;
    event.preventDefault();
    event.stopPropagation();
    showNotice(readOnlyText());
  }, true);
  // Right-click: Copy, Paste and Select all; Shift+right-click keeps the browser's own menu. While
  // the program has the mouse, a right-click is the program's alone (no menu over it) and the key
  // that selects text (Shift; Option on macOS) brings this menu instead (SPEC-098 MSE-003).
  el.addEventListener('contextmenu', (event) => {
    if (!host.contains(event.target)) return;
    if (programHasMouse()) {
      if (!pageKey(event)) { event.preventDefault(); return; }
    } else if (event.shiftKey) return;
    event.preventDefault();
    const own = term.hasSelection();
    const selected = term.getSelection() || programCopy;
    openMenu({ x: event.clientX, y: event.clientY }, [
      { label: 'Copy', hint: IS_MAC ? '⌘C' : own ? 'Ctrl+C' : 'Ctrl+Shift+C', disabled: !selected, title: selected ? null : 'Select text first', onSelect: () => { writeClipboard(selected); if (!own) programCopy = ''; term.focus(); } },
      { label: 'Paste', hint: IS_MAC ? '⌘V' : 'Ctrl+V', disabled: !state.can_write, title: state.can_write ? null : readOnlyText(), onSelect: () => { term.focus(); pasteFromClipboard(); } },
      { separator: true },
      { label: 'Select all', hint: IS_MAC ? '⌘A' : null, onSelect: () => { term.selectAll(); term.focus(); } },
    ]);
  });

  // ---- file links (termlinks.js) and the shell's working directory (OSC 7, sent by bash's prompt)
  const oscFiles = fileLinks ? trackOscFileLinks(term) : null;
  if (fileLinks) {
    term.parser.registerOscHandler(7, (data) => {
      try {
        const url = new URL(data);
        if (url.protocol === 'file:') cwd = decodeURIComponent(url.pathname);
      } catch { /* not a file URL */ }
      return true;
    });
    term.registerLinkProvider(createFileLinkProvider(term, {
      context: () => {
        const roots = (fileLinks.roots && fileLinks.roots()) || [];
        return { roots, cwd: cwd && roots.length ? relativeCwd(cwd, roots) : '' };
      },
      checker: createFileChecker((dir) => fileLinks.list(dir)),
      osc: oscFiles,
      open: (file, event) => fileLinks.open(file, event),
    }));
  }
  // Claude Code draws full screen (the alternate buffer has no scrollback) and asks for the mouse:
  // xterm reports its clicks, drags and wheel turns in the program's encoding, as a native terminal
  // does, and they take neither control nor the size (typing.js). Pinch-zoom (ctrl+wheel) and
  // horizontal swipes stay with the browser (#78): xterm would cancel them even when it does not
  // report them, so they never reach it.
  const programHasMouse = () => term.modes.mouseTrackingMode !== 'none';
  /** The key that takes the mouse back from the program: Option on macOS, Shift elsewhere (xterm). */
  const pageKey = (event) => (IS_MAC ? event.altKey : event.shiftKey);
  // A plain wheel turn reaches the program as one report per row it covers, as Alacritty, kitty
  // and iTerm2 send (SPEC-280, ADR-0210): xterm sends one per event and damps small (trackpad)
  // deltas, and Claude Code moves one row per report, so a notch moved one row of a long
  // conversation (#863).
  // The turn becomes line-mode wheel events of one row each, which xterm reports in the program's
  // encoding at the pointer's cell; the fraction of a row left carries on in the same direction.
  const rowEvents = new WeakSet();
  let wheelRows = 0;
  // Claude Code shows a session only from its last compaction, and a screen without scrollback
  // has nothing above it: a turn up that moves nothing reached the top (SPEC-326 HIS-001). xterm
  // scrolls a normal buffer itself, so its first line is the top. Elsewhere the program moves the
  // screen (wheel reports, or arrow keys on the alternate screen): a turn after which the upper half
  // stays as it was did not move it. The lower half holds the spinner and the prompt, which change
  // while the agent works. Turns closer than TOP_SETTLE_MS are one gesture, compared with the
  // screen at its start and at its last turn: the program's answer to a turn arrives a round trip
  // later, so the last turn's screen may still be moving towards the top.
  let topCheck = null;
  let gestureStart = null;
  let topOfferTimer = null;
  // The upper half when the screen check showed the offer: a program that answers later than the
  // check (a slow link) moves it after all, and that turn was no top (HIS-001).
  let topOfferScreen = null;
  const upperHalf = () => {
    const buffer = term.buffer.active;
    const lines = [];
    for (let y = 0; y < Math.ceil(term.rows / 2); y++) {
      const line = buffer.getLine(buffer.viewportY + y);
      lines.push(line ? line.translateToString(true) : '');
    }
    return lines.join('\n');
  };
  function showTopOffer(screen) {
    topOfferEl.hidden = false;
    topOfferScreen = screen;
    clearTimeout(topOfferTimer);
    topOfferTimer = setTimeout(hideTopOffer, TOP_OFFER_MS);
  }
  function hideTopOffer() {
    if (!topOfferEl) return;
    clearTimeout(topOfferTimer);
    topOfferTimer = null;
    topOfferScreen = null;
    topOfferEl.hidden = true;
  }
  if (topOfferEl) {
    term.onWriteParsed(() => {
      if (topOfferScreen !== null && upperHalf() !== topOfferScreen) hideTopOffer();
    });
    // A key means the person stays in the terminal; the button's own keys open it.
    el.addEventListener('keydown', (event) => { if (!topOfferEl.contains(event.target)) hideTopOffer(); }, true);
  }
  const noticeTop = (event) => {
    if (!topOfferEl || event.ctrlKey || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
    if (!(event.deltaY < 0)) {
      // A turn back down ends the gesture: what it shows is not the top.
      clearTimeout(topCheck);
      topCheck = null;
      hideTopOffer();
      return;
    }
    const buffer = term.buffer.active;
    if (buffer.type === 'normal' && !programHasMouse()) {
      if (buffer.viewportY === 0) showTopOffer(null);
      return;
    }
    const before = upperHalf();
    if (topCheck === null) gestureStart = before;
    clearTimeout(topCheck);
    topCheck = setTimeout(() => {
      topCheck = null;
      const now = upperHalf();
      if (!destroyed && (now === before || now === gestureStart)) showTopOffer(now);
    }, TOP_SETTLE_MS);
  };
  host.addEventListener('wheel', (event) => {
    if (rowEvents.has(event)) return;
    noticeTop(event);
    if (!programHasMouse()) return;
    if (event.ctrlKey || !event.deltaY) {
      event.stopPropagation();
      return;
    }
    if (event.shiftKey || event.altKey || event.metaKey) return; // xterm's, as before
    const screen = host.querySelector('.xterm-screen');
    const rowHeight = screen ? screen.getBoundingClientRect().height / term.rows : 0;
    if (!(rowHeight > 0)) return;
    event.stopPropagation();
    event.preventDefault();
    const rows = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? event.deltaY
      : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? event.deltaY * term.rows
        : event.deltaY / rowHeight;
    if (Math.sign(rows) !== Math.sign(wheelRows)) wheelRows = 0;
    wheelRows += rows;
    const whole = Math.trunc(wheelRows);
    wheelRows -= whole;
    const { clientX, clientY, screenX, screenY } = event;
    for (let i = Math.min(Math.abs(whole), term.rows); i > 0; i--) {
      const row = new WheelEvent('wheel', { bubbles: true, cancelable: true, composed: true, clientX, clientY, screenX, screenY, deltaMode: WheelEvent.DOM_DELTA_LINE, deltaY: Math.sign(whole) });
      rowEvents.add(row);
      event.target.dispatchEvent(row);
    }
  }, { capture: true, passive: false });
  // Another viewer's grid is centred in this box, and xterm would report a press in the margin
  // beside it at the grid's edge: that press only takes the size (SPEC-098 MSE-005). A press with
  // the selection key still starts a selection there.
  host.addEventListener('mousedown', (event) => {
    if (!programHasMouse() || pageKey(event)) return;
    const grid = (host.querySelector('.xterm-screen') || host).getBoundingClientRect();
    if (event.clientX >= grid.left && event.clientX < grid.right && event.clientY >= grid.top && event.clientY < grid.bottom) return;
    event.preventDefault();
    event.stopPropagation();
    term.focus();
  }, true);
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

  /** The grid this view's box holds at the font given (the fit addon's measure), unbounded. */
  function room() {
    try {
      const d = fit.proposeDimensions();
      return d && Number.isFinite(d.cols) && Number.isFinite(d.rows) && d.cols > 0 && d.rows > 0 ? d : null;
    } catch {
      return null;
    }
  }

  // The grid this view's own window fits at its own font: what it asks for as the PTY size. While
  // it shows another viewer's grid (another font), the font is switched for the measure and back —
  // xterm re-measures synchronously and draws once, on the next frame.
  let ownCache = null;
  function ownSize() {
    if (!opened) return null;
    const base = termSize();
    const key = `${fitBox.clientWidth}x${fitBox.clientHeight}|${base}|${term.options.fontFamily}`;
    if (ownCache && ownCache.key === key) return ownCache.size;
    const current = term.options.fontSize;
    const spacing = term.options.lineHeight;
    if (current !== base) term.options.fontSize = base;
    if (spacing !== LINE_HEIGHT) term.options.lineHeight = LINE_HEIGHT;
    const size = proposed();
    if (current !== base) term.options.fontSize = current;
    if (spacing !== LINE_HEIGHT) term.options.lineHeight = spacing;
    if (size) ownCache = { key, size };
    return size;
  }

  const sameSize = (a, b) => Boolean(a && b && a.cols === b.cols && a.rows === b.rows);

  // The PTY follows the viewer that controls the size (TSK-006, F 2026-10-10); every writer says
  // which size its window fits, and the server applies it while that viewer controls the size.
  // A viewer that does not control it shows the PTY's grid, scaled to fill its box.
  function applySize() {
    if (followServerSize) {
      if (state.cols && state.rows && (term.cols !== state.cols || term.rows !== state.rows)) term.resize(Math.max(2, state.cols), Math.max(1, state.rows));
      return;
    }
    const own = ownSize();
    if (own && gotState && state.can_write && !exited && !sameSize(lastSent, own)) {
      if (socket.resize(own.cols, own.rows)) lastSent = own;
    }
    // An ended session has no PTY to follow: fit it to this screen (a desktop-sized screen would
    // otherwise overflow a phone).
    const mirroring = Boolean(state.cols && state.rows && !state.controller && !exited);
    host.classList.toggle('term-mirror', mirroring);
    if (!mirroring) {
      if (term.options.fontSize !== termSize()) term.options.fontSize = termSize();
      if (term.options.lineHeight !== LINE_HEIGHT) term.options.lineHeight = LINE_HEIGHT;
      if (own && (term.cols !== own.cols || term.rows !== own.rows)) term.resize(own.cols, own.rows);
      return;
    }
    if (term.cols !== state.cols || term.rows !== state.rows) term.resize(Math.max(2, state.cols), Math.max(1, state.rows));
    fitMirror(state.cols, state.rows);
  }

  // Another viewer sets the size: its grid fills this box — the font grows or shrinks until the
  // grid is as large as fits, whole (both directions, the aspect ratio kept), and the grid is
  // centred across the width that is left. By font size, not a CSS transform, so selection and
  // links keep their positions (#30, F 2026-10-10: a fit capped at 1.6× and never checked used
  // half of a wide window, and could cut off the bottom rows).
  function fitMirror(cols, rows) {
    const max = Math.max(FIT_MIN_FONT, termSize() * FIT_MAX_SCALE);
    const clamp = (font) => Math.min(max, Math.max(FIT_MIN_FONT, font));
    const setFont = (font) => { if (term.options.fontSize !== font) term.options.fontSize = font; };
    const setSpacing = (lineHeight) => { if (term.options.lineHeight !== lineHeight) term.options.lineHeight = lineHeight; };
    const fits = () => { const d = room(); return Boolean(d) && d.cols >= cols && d.rows >= rows; };
    setSpacing(LINE_HEIGHT);
    let font = term.options.fontSize;
    // Estimate from what fits now (cells scale with the font), twice: rounding settles quickly.
    for (let i = 0; i < 2; i += 1) {
      const d = room();
      if (!d) return;
      const next = clamp(Math.floor((font * Math.min(d.cols / cols, d.rows / rows)) / FIT_STEP) * FIT_STEP);
      if (next === font) break;
      font = next;
      setFont(font);
    }
    // Then exactly: the largest step at which the whole grid fits.
    for (let i = 0; i < 24 && font > FIT_MIN_FONT && !fits(); i += 1) { font = clamp(font - FIT_STEP); setFont(font); }
    for (let i = 0; i < 24 && font + FIT_STEP <= max; i += 1) {
      setFont(font + FIT_STEP);
      if (!fits()) { setFont(font); break; }
      font += FIT_STEP;
    }
    // A box shorter than the grid's shape: tighter lines let the text grow wider (row heights also
    // come in whole pixels, which leaves a band below otherwise). Glyphs keep their shape.
    let wider = font;
    setSpacing(FIT_MIN_LINE_HEIGHT);
    for (let i = 0; i < 24 && wider + FIT_STEP <= max; i += 1) {
      setFont(wider + FIT_STEP);
      if (!fits()) break;
      wider += FIT_STEP;
    }
    setFont(wider);
    // The loosest spacing at that size that still fits.
    for (let spacing = LINE_HEIGHT; spacing > FIT_MIN_LINE_HEIGHT; spacing = Math.round((spacing - 0.025) * 1000) / 1000) {
      setSpacing(spacing);
      if (fits()) return;
    }
    setSpacing(FIT_MIN_LINE_HEIGHT);
    if (!fits()) { setFont(font); setSpacing(LINE_HEIGHT); }
  }

  /** Input in pieces the server accepts (a long paste is several frames, in order). */
  function pieces(data) {
    if (data.length <= INPUT_CHUNK) return [data];
    const out = [];
    for (let i = 0; i < data.length;) {
      let end = Math.min(data.length, i + INPUT_CHUNK);
      const unit = data.charCodeAt(end - 1);
      if (end < data.length && unit >= 0xd800 && unit <= 0xdbff) end -= 1; // never split a surrogate pair
      out.push(data.slice(i, end));
      i = end;
    }
    return out;
  }

  function send(data) {
    if (!state.can_write) {
      showNotice(readOnlyText());
      return false;
    }
    // Focus and mouse reports and xterm's answers to queries are not typing: the server neither
    // counts them nor moves control for them, so this view does not take the size (SPEC-068), and
    // one lost while disconnected is not worth a notice.
    const typing = isTyping(data);
    for (const piece of pieces(data)) {
      if (!socket.sendInput(piece)) {
        if (typing) showNotice('Not connected — input was not sent');
        return false;
      }
    }
    if (!typing) return true;
    used();
    // The server makes the viewer who types the controller; mirror it until the next state frame.
    if (!state.controller && !followServerSize) takeSize();
    return true;
  }

  /** This viewer controls the size from now on (it typed or clicked into the terminal). */
  function takeSize() {
    state = { ...state, controller: true };
    applySize();
    render();
  }

  /** A click into the terminal (not a drag, not a double-click that selects, not a link) takes the size. */
  function claim() {
    if (followServerSize || destroyed || !state.can_write || state.controller || exited || conn !== 'open') return;
    if (socket.takeControl()) takeSize();
  }

  // ---- presence: active (visible, focused, used recently) or idle; the server hands the size
  // from idle viewers to active ones.
  function presentNow() {
    if (!opened || destroyed || exited || !state.can_write) return false;
    if (document.visibilityState !== 'visible' || !document.hasFocus()) return false;
    if (!el.isConnected || !host.getClientRects().length) return false; // a hidden tab or panel
    return Date.now() - lastUse < IDLE_MS;
  }
  function updatePresence() {
    clearTimeout(presenceTimer);
    if (followServerSize || destroyed) return;
    const now = presentNow();
    if (now) presenceTimer = setTimeout(updatePresence, Math.max(1000, IDLE_MS - (Date.now() - lastUse) + 100));
    if (!gotState || conn !== 'open' || now === present) return;
    if (socket.presence(now)) present = now;
  }
  function used() {
    lastUse = Date.now();
    updatePresence();
  }
  let movedAt = 0;
  const onPointerMove = () => { const now = Date.now(); if (now - movedAt > 5000) { movedAt = now; used(); } };
  const onWindowFocus = () => { if (host.contains(document.activeElement)) used(); else updatePresence(); };
  el.addEventListener('pointerdown', used, true);
  el.addEventListener('keydown', used, true);
  el.addEventListener('focusin', used);
  // A turn counts once, not again for each row it sends (ADR-0210).
  el.addEventListener('wheel', (event) => { if (!rowEvents.has(event)) used(); }, { capture: true, passive: true });
  el.addEventListener('pointermove', onPointerMove, { passive: true });
  window.addEventListener('focus', onWindowFocus);
  window.addEventListener('blur', updatePresence);
  document.addEventListener('visibilitychange', updatePresence);

  // A click takes the size once it is clearly a click: no drag, no selection (a double- or
  // triple-click selects), the pointer not on a link. A program that asked for the mouse already
  // got the click from xterm.
  let press = null;
  let clickTimer = 0;
  host.addEventListener('pointerdown', (event) => {
    clearTimeout(clickTimer);
    press = event.button === 0 ? { x: event.clientX, y: event.clientY } : null;
  }, true);
  host.addEventListener('pointerup', (event) => {
    const start = press;
    press = null;
    if (!start || event.button !== 0 || Math.abs(event.clientX - start.x) > 4 || Math.abs(event.clientY - start.y) > 4) return;
    if (host.querySelector('.xterm-cursor-pointer')) return; // a link click opens the link
    clearTimeout(clickTimer);
    clickTimer = setTimeout(() => { if (!term.hasSelection()) claim(); }, CLICK_SETTLE_MS);
  }, true);

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
    }
    setChildren(banner, rows);
    banner.hidden = rows.length === 0;
    sizeChip.hidden = !(conn === 'open' && gotState && state.can_write && !state.controller && state.cols && !exited && !fatal && !followServerSize);
  }

  const observer = new ResizeObserver(() => {
    cancelAnimationFrame(resizeFrame);
    resizeFrame = requestAnimationFrame(() => { if (!destroyed) applySize(); });
  });
  // The floating corner (notices, the size chip, the mouse switch) sits below the banner, never over
  // its buttons (Continue, Try again, Check subscriptions; #147).
  const bannerObserver = new ResizeObserver(() => el.style.setProperty('--term-banner-h', `${banner.offsetHeight}px`));

  return {
    el,
    term,
    socket,
    /** Open xterm in the (attached) host, fit it, and connect with that size. */
    start() {
      if (opened || destroyed) return;
      term.open(fitBox);
      term.options.theme = terminalColors(host); // attached now: colours as seen where it sits
      // The terminal keeps Tab and Esc; say how to leave it (WCAG 2.1.2 asks that the way out be told).
      const input = host.querySelector('.xterm-helper-textarea');
      if (input) {
        input.setAttribute('aria-description', 'Terminal. Press F6 to move to the next area, Shift+F6 to go back.');
        input.title = 'F6 moves to the next area';
      }
      opened = true;
      fontPending = !fontLoaded();
      observer.observe(fitBox);
      bannerObserver.observe(banner);
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
      focusRequests += 1;
      focusWanted = focusRequests;
      if (opened && ready) term.focus();
    },
    get ready() {
      return ready;
    },
    /** The caller's empty-screen message may have changed (e.g. the task's status). */
    refreshEmpty() {
      renderEmpty();
    },
    /**
     * The caller's exit message may have changed (e.g. the task's details arrived, SPEC-268). Every
     * detail read redraws it: a keyboard user on Continue or Try again stays there (SPEC-272).
     */
    refreshBanner() {
      if (opened && !destroyed) keepFocus(banner, render);
    },
    refit() {
      applySize();
    },
    destroy() {
      destroyed = true;
      window.removeEventListener('mouseup', handsOn, true);
      clearTimeout(noticeTimer);
      clearTimeout(revealTimer);
      clearTimeout(presenceTimer);
      clearTimeout(clickTimer);
      clearTimeout(topCheck);
      clearTimeout(topOfferTimer);
      window.removeEventListener('focus', onWindowFocus);
      window.removeEventListener('blur', updatePresence);
      document.removeEventListener('visibilitychange', updatePresence);
      cancelAnimationFrame(resizeFrame);
      observer.disconnect();
      bannerObserver.disconnect();
      if (document.fonts && document.fonts.removeEventListener) document.fonts.removeEventListener('loadingdone', onFontsLoaded);
      socket.close();
      offTheme();
      term.dispose();
    },
  };
}
