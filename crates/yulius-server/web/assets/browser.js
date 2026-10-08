// Sandbox browser panel (BRW-001..007): tab strip, address bar and the active tab of the
// server-side Chromium shown as live DOM (rrweb mirror, BRW-005) — or, when a page cannot be
// mirrored or the user asks for it, as JPEG frames. The mirror is a real page in the user's
// browser: text selects and copies natively, hover and cursors work, and clicks, drags, keys and
// wheel are forwarded to the real browser, whose changes stream back into the mirror.
// Page resources served on the VM (localhost, plain http) load through Yulius. Agents' actions
// show as a labelled cursor (BRW-007). New tabs open the Yulius welcome page (BRW-006).
// The socket is open only while the panel is visible.
//
// rrweb's replayer is vendored under a neutral name and loaded on first use: content blockers
// (Brave Shields, uBlock Origin with EasyPrivacy) block any `/rrweb.js`, and a blocked or failed
// load must cost only the DOM view — the panel then shows pixels — never the whole UI.

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, toast, focusNextRegion } from './dom.js';
import { ReconnectingSocket, parseBrowserFrame, CLOSE_REVOKED, CLOSE_SERVER_ERROR } from './termsocket.js';
import { allows } from './status.js';

const MOUSE_BUTTONS = ['left', 'middle', 'right'];
const WELCOME_PATH = /\/browser\/welcome\?sandbox=/;
const AGENT_CURSOR_IDLE_MS = 6000;
/** A tab that sends no DOM within this time is shown as pixels (BRW-005 fallback). */
const DOM_WAIT_MS = 6000;
/** Mirror failures on one page before it is shown as pixels instead of resynchronized. */
const MAX_MIRROR_FAILURES = 3;
/** Pixel view reason when the mirror's code cannot load in this browser. */
const MIRROR_UNAVAILABLE = 'the DOM mirror could not load in this browser';
/** rrweb event and source numbers used here. */
const EV_FULL_SNAPSHOT = 2;
const EV_INCREMENTAL = 3;
const EV_META = 4;
const SRC_MUTATION = 0;
const SRC_VIEWPORT = 4;
const SRC_STYLESHEET_RULE = 8;
const SRC_CANVAS = 9;
const SRC_SELECTION = 14;

/** CDP-style modifier bitmask: Alt=1, Ctrl=2, Meta=4, Shift=8. */
function modifiers(event) {
  return (event.altKey ? 1 : 0) | (event.ctrlKey ? 2 : 0) | (event.metaKey ? 4 : 0) | (event.shiftKey ? 8 : 0);
}

/** Normalize an address bar entry to a URL; null when it is not URL-like. */
export function normalizeUrl(input) {
  const value = String(input || '').trim();
  if (!value || /\s/.test(value)) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(value) && !/^[^:/]+:\d+/.test(value)) return value;
  if (/^(localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\])(:\d+)?([/?#]|$)/i.test(value)) return `http://${value}`;
  if (/^[^/]+\.[^/]+/.test(value) || /^[^/:]+:\d+/.test(value)) return `https://${value}`;
  return null;
}

/** Whether the user's browser cannot load `url` itself (VM loopback, private network, http). */
export function needsProxy(url) {
  let parsed;
  try { parsed = new URL(url); } catch { return false; }
  if (parsed.protocol === 'http:') return true;
  if (parsed.protocol !== 'https:') return false;
  const host = parsed.hostname.toLowerCase().replace(/^\[|\]$/g, '');
  return host === 'localhost' || host.endsWith('.localhost') || host === '::1' || host === '0.0.0.0'
    || /^127\./.test(host) || /^10\./.test(host) || /^192\.168\./.test(host) || /^172\.(1[6-9]|2\d|3[01])\./.test(host);
}

let rrwebStyles = false;
function ensureReplayerStyles() {
  if (rrwebStyles) return;
  rrwebStyles = true;
  document.head.append(h('link', { rel: 'stylesheet', href: '/assets/vendor/dom-mirror/replayer.css' }));
}

let Replayer = null; // rrweb's Replayer once loaded
let replayerLoad = null;
let replayerFailed = false;
/** Loads the replayer once; it settles either way (a failure sets `replayerFailed`). */
function loadReplayer() {
  if (!replayerLoad) {
    replayerLoad = import('./vendor/dom-mirror/replayer.js')
      .then((module) => { Replayer = module.Replayer; })
      .catch((error) => {
        replayerFailed = true;
        console.warn('DOM mirror unavailable; the browser panel shows pixels', error);
      });
  }
  return replayerLoad;
}

/** Create the browser panel view for a sandbox. */
export function createBrowserPanel(ctx, sandbox) {
  const el = h('div', { class: 'bp' });
  if (!allows(sandbox.access, 'write')) {
    setChildren(el, h('div', { class: 'panel-msg' }, icon('lock', { size: 20 }), h('p', null, 'The sandbox browser requires write access on GitHub.')));
    return { el };
  }
  ensureReplayerStyles();

  let tabs = [];
  let socket = null;
  let pendingActivate = null;
  let pendingOpen = null; // URL to open in a new tab once connected (localhost links)
  let lastViewport = null;
  let decoding = false;
  let queuedFrame = null;
  let moveFrame = 0;
  let pendingMove = null;
  let viewportTimer = null;
  let visible = false;
  let lastError = null; // last {code, message} error frame; explains a following close
  let wantPixels = false; // the user's choice
  let pixelsReason = null; // the server could not mirror the active tab
  let replayer = null;
  let replayerTab = null;
  let replayerClock = 0; // events are applied in arrival order, all before this baseline
  let domTimer = 0;
  let watchedUrl = null; // the active tab's URL when the view mode was last chosen
  let mirrorFailures = 0; // on the current page
  let missedDom = false; // DOM arrived before the replayer loaded: resynchronize once it has
  let serverViewport = null; // {width, height} the server last applied to every tab
  const canvasSeq = new Map(); // mirror node id → newest painted frame
  let pageSize = { width: 0, height: 0 };
  const tags = new Map(); // mirror node id → tag name (for attribute rewriting)
  const frames = new Map(); // tab id → last ImageBitmap
  let lastAgent = null; // {x, y}
  let agentTimer = 0;

  const tabStrip = h('div', { class: 'bp-tabs', role: 'tablist', 'aria-label': 'Browser tabs' });
  const address = h('input', { class: 'input', type: 'text', placeholder: 'Type an address, e.g. localhost:20000 or aws.amazon.com', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'Address' });
  const reloadButton = iconButton('refresh', 'Reload', () => {
    const tab = activeTab();
    if (tab) send({ type: 'navigate', tab_id: tab.id, url: tab.url });
  });
  const viewBadge = h('span', { class: 'chip bp-viewchip', hidden: true });
  const attention = h('div', { class: 'banner banner-warn bp-attn small', hidden: true });
  const mirrorHost = h('div', { class: 'bp-mirror', 'aria-label': 'Remote browser page' });
  const canvas = h('canvas', { class: 'bp-canvas', hidden: true });
  // Pixel view only: the canvas cannot take input itself.
  const inputLayer = h('div', { class: 'bp-input', tabindex: '0', hidden: true, 'aria-label': 'Remote browser page (click to interact)' });
  const agentCursor = h('div', { class: 'bp-agent', hidden: true }, h('span', { class: 'bp-agent-arrow' }), h('span', { class: 'bp-agent-label' }));
  const agentPill = h('div', { class: 'bp-agent-pill', hidden: true });
  const copied = h('div', { class: 'bp-copied', hidden: true }, 'Copied');
  let copiedTimer = 0;
  const overlay = h('div', { class: 'bp-overlay' }, 'Connecting to the sandbox browser…');
  const stage = h('div', { class: 'bp-view' }, mirrorHost, canvas, inputLayer, agentCursor, agentPill, copied, overlay);
  setChildren(el, tabStrip, h('div', { class: 'bp-bar' }, reloadButton, address, viewBadge), attention, stage);
  const g = canvas.getContext('2d');

  const pixelsButton = h('button', { type: 'button', class: 'btn btn-sm', 'aria-pressed': 'false', title: 'Show the page as pixels instead of live DOM', onClick: () => setPixels(!wantPixels) }, 'Pixels');

  function activeTab() {
    return tabs.find((t) => t.active) || null;
  }

  function send(message) {
    return socket ? socket.send(message) : false;
  }

  function isWelcome(tab) {
    return Boolean(tab && WELCOME_PATH.test(tab.url || ''));
  }

  function showingPixels() {
    return wantPixels || Boolean(pixelsReason);
  }

  function setPixels(on) {
    wantPixels = on;
    pixelsButton.setAttribute('aria-pressed', String(on));
    pixelsButton.classList.toggle('active', on);
    pixelsReason = null;
    send({ type: 'view', mode: on ? 'pixels' : 'dom' });
    if (on) clearTimeout(domTimer); else expectDom();
    applyMode();
  }

  /** Falls back to pixels when the active tab sends no DOM in time. */
  function expectDom() {
    clearTimeout(domTimer);
    if (showingPixels()) return;
    domTimer = setTimeout(() => {
      if (showingPixels() || replayer) return;
      autoPixels('this page did not send its DOM');
    }, DOM_WAIT_MS);
  }

  function autoPixels(reason) {
    clearTimeout(domTimer);
    pixelsReason = reason;
    destroyReplayer();
    send({ type: 'view', mode: 'pixels' });
    applyMode();
  }

  /** After a navigation or tab change, an automatic fallback tries the DOM view again. */
  function retryDom() {
    if (wantPixels || replayerFailed) return;
    if (pixelsReason) {
      pixelsReason = null;
      send({ type: 'view', mode: 'dom' });
      applyMode();
    }
    expectDom();
  }

  function applyMode() {
    const pixels = showingPixels();
    applyInputMode();
    canvas.hidden = !pixels;
    mirrorHost.hidden = pixels;
    viewBadge.hidden = !pixels;
    viewBadge.textContent = pixelsReason ? 'Pixel view' : 'Pixels';
    viewBadge.title = pixelsReason ? `This page cannot be mirrored as DOM: ${pixelsReason}` : 'Pixel view (you chose it)';
    if (pixels) {
      const tab = activeTab();
      if (tab && frames.has(tab.id)) draw(frames.get(tab.id));
    }
    layout();
  }

  /** The pixel view takes input through an overlay; the DOM mirror takes it directly. */
  function applyInputMode() {
    inputLayer.hidden = !showingPixels();
  }

  /** Focus for keyboard input: the mirror page or the pixel overlay. */
  function focusPage() {
    if (showingPixels()) inputLayer.focus();
    else if (replayer && replayer.iframe && replayer.iframe.contentWindow) replayer.iframe.contentWindow.focus();
  }

  /**
   * The mirror document behaves like the page: native selection, copy, hover and cursors stay
   * local; presses, moves, wheel and keys go to the real browser at the same CSS coordinates
   * (the mirror is laid out at the page's viewport). Its own navigation is suppressed.
   */
  function wireMirrorDocument() {
    const doc = replayer && replayer.iframe ? replayer.iframe.contentDocument : null;
    if (!doc) return;
    const at = (event) => ({ x: Math.round(event.clientX), y: Math.round(event.clientY) });
    let pressed = false;
    let pendingMoveEvent = null;
    let mirrorMoveFrame = 0;
    doc.addEventListener('mousedown', (event) => {
      if (event.button === 2) return; // the local context menu (Copy) stays
      pressed = true;
      const p = at(event);
      sendMouse('down', p.x, p.y, MOUSE_BUTTONS[event.button] || 'left');
    }, true);
    doc.addEventListener('mousemove', (event) => {
      pendingMoveEvent = event;
      if (mirrorMoveFrame) return;
      mirrorMoveFrame = requestAnimationFrame(() => {
        mirrorMoveFrame = 0;
        const move = pendingMoveEvent;
        pendingMoveEvent = null;
        if (!move) return;
        const p = at(move);
        sendMouse('move', p.x, p.y, pressed && (move.buttons & 1) ? 'left' : 'none');
      });
    }, true);
    doc.addEventListener('mouseup', (event) => {
      if (event.button === 2) return;
      pressed = false;
      const p = at(event);
      sendMouse('up', p.x, p.y, MOUSE_BUTTONS[event.button] || 'left');
      // Instant copy of a selection (also after a double-click).
      const text = String(doc.getSelection ? doc.getSelection() : '');
      if (!text.trim() || !navigator.clipboard) return;
      navigator.clipboard.writeText(text).then(() => {
        copied.hidden = false;
        clearTimeout(copiedTimer);
        copiedTimer = setTimeout(() => { copied.hidden = true; }, 1200);
      }, () => {});
    }, true);
    doc.addEventListener('wheel', (event) => {
      event.preventDefault();
      const p = at(event);
      sendMouse('wheel', p.x, p.y, 'none', { dx: Math.round(event.deltaX), dy: Math.round(event.deltaY) });
    }, { capture: true, passive: false });
    // The real page handles clicks; the mirror never navigates, submits or drags links away.
    doc.addEventListener('click', (event) => event.preventDefault(), true);
    doc.addEventListener('dragstart', (event) => event.preventDefault(), true);
    // The mirror is a same-origin iframe: its key events never reach the window's F6 handler, so
    // F6 / Shift+F6 leave it here (WCAG 2.1.2 No Keyboard Trap); every other key goes to the page.
    doc.addEventListener('keydown', (event) => {
      if (event.key === 'F6' && !event.altKey && !event.metaKey && !event.ctrlKey) {
        event.preventDefault();
        event.stopPropagation();
        focusNextRegion(event.shiftKey);
        return;
      }
      forwardKey('down', event);
    }, true);
    doc.addEventListener('keyup', (event) => forwardKey('up', event), true);
    doc.addEventListener('paste', (event) => forwardPaste(event), true);
  }

  function renderTabs() {
    const items = tabs.map((tab) => {
      const attn = Boolean(tab.action_requested);
      const title = isWelcome(tab) ? 'Welcome' : (tab.title || tab.url || 'New tab');
      const button = h('div', { class: ['bp-tab', tab.active && 'active', attn && 'attention'], role: 'tab', tabindex: '0', 'aria-selected': String(Boolean(tab.active)), title: attn ? `Action requested: ${tab.action_requested.reason}` : (isWelcome(tab) ? 'Welcome' : tab.url) },
        attn ? icon('warn', { size: 12 }) : null,
        h('span', { class: 'bp-tab-title' }, title),
        iconButton('x', `Close ${title}`, (event) => { event.stopPropagation(); send({ type: 'close', tab_id: tab.id }); }));
      const activate = () => { if (!tab.active) send({ type: 'activate', tab_id: tab.id }); focusPage(); };
      button.addEventListener('click', activate);
      button.addEventListener('keydown', (event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); activate(); } });
      return button;
    });
    // A new tab opens the welcome page; type an address there.
    items.push(iconButton('plus', 'New tab', () => send({ type: 'open', url: '' })));
    setChildren(tabStrip, items);
    const tab = activeTab();
    if (document.activeElement !== address) address.value = tab && !isWelcome(tab) ? tab.url : '';
    address.placeholder = isWelcome(tab) ? 'Welcome — type an address, e.g. localhost:20000' : 'Type an address, e.g. localhost:20000 or aws.amazon.com';
    if (tab && tab.action_requested) {
      attention.hidden = false;
      setChildren(attention, icon('warn', { size: 14 }), h('span', { class: 'banner-text' }, `An agent asks for your action here: ${tab.action_requested.reason}`));
    } else {
      attention.hidden = true;
    }
    if (tabs.length === 0) showOverlay(emptyState());
    else if (showingPixels() && tab && frames.has(tab.id)) { overlay.hidden = true; draw(frames.get(tab.id)); }
    else if (!showingPixels() && replayer && replayerTab === (tab && tab.id)) overlay.hidden = true;
    else if (tab) showOverlay('Loading page…');
  }

  function emptyState() {
    return h('div', { class: 'stack items-center' },
      h('p', null, 'No open tabs.'),
      h('button', { type: 'button', class: 'btn btn-sm', onClick: () => send({ type: 'open', url: '' }) }, 'Open a tab'));
  }

  function showOverlay(content) {
    overlay.hidden = false;
    setChildren(overlay, content);
  }

  function syncActionRequests() {
    const current = ctx.store.state.actionRequests.get(sandbox.id);
    const requested = new Set(tabs.filter((t) => t.action_requested).map((t) => t.id));
    if (current) for (const tabId of [...current.keys()]) if (!requested.has(tabId)) ctx.store.clearActionRequest(sandbox.id, tabId);
    for (const tab of tabs) if (tab.action_requested && !(current && current.has(tab.id))) {
      ctx.store.handleEvent({ type: 'browser.action_requested', sandbox_id: sandbox.id, tab_id: tab.id, reason: tab.action_requested.reason, task_id: tab.action_requested.task_id });
    }
  }

  // ---------------------------------------------------------------- pixels (fallback)
  function draw(bitmap) {
    if (!bitmap) return;
    if (canvas.width !== bitmap.width || canvas.height !== bitmap.height) {
      canvas.width = bitmap.width;
      canvas.height = bitmap.height;
      layout();
    }
    g.drawImage(bitmap, 0, 0);
  }

  async function decode(frame) {
    if (decoding) { queuedFrame = frame; return; } // keep only the newest pending frame
    decoding = true;
    try {
      const bitmap = await createImageBitmap(new Blob([frame.bytes], { type: 'image/jpeg' }));
      const old = frames.get(frame.tabId);
      if (old && old.close) old.close();
      frames.set(frame.tabId, bitmap);
      const tab = activeTab();
      if (showingPixels() && tab && tab.id === frame.tabId) { overlay.hidden = true; draw(bitmap); }
    } catch { /* corrupt frame: skip */ }
    decoding = false;
    if (queuedFrame) { const next = queuedFrame; queuedFrame = null; decode(next); }
  }

  // ---------------------------------------------------------------- DOM mirror
  const proxied = (url) => `/api/sandboxes/${enc(sandbox.id)}/browser/resource?tab=${encodeURIComponent(replayerTab || '')}&url=${encodeURIComponent(url)}`;
  const rewriteUrl = (url) => (typeof url === 'string' && needsProxy(url) ? proxied(url) : url);
  const rewriteCss = (css) => (typeof css === 'string' ? css.replace(/url\(\s*(['"]?)([^'")]+)\1\s*\)/g, (match, quote, url) => (needsProxy(url) ? `url(${quote}${proxied(url)}${quote})` : match)) : css);
  const rewriteSrcset = (value) => (typeof value === 'string' ? value.split(',').map((part) => {
    const [url, ...descriptor] = part.trim().split(/\s+/);
    return [rewriteUrl(url), ...descriptor].join(' ');
  }).join(', ') : value);

  function rewriteAttributes(tag, attributes) {
    if (!attributes) return;
    for (const name of Object.keys(attributes)) {
      const value = attributes[name];
      if (value == null) continue;
      if (name === 'src' || name === 'poster' || name === 'data' || name === 'background' || name === 'xlink:href') attributes[name] = rewriteUrl(value);
      else if (name === 'srcset') attributes[name] = rewriteSrcset(value);
      else if (name === 'href' && (tag === 'link' || tag === 'image' || tag === 'use')) attributes[name] = rewriteUrl(value);
      else if (name === 'style' || name === '_cssText') attributes[name] = rewriteCss(value);
    }
  }

  function rewriteNode(node) {
    if (!node) return;
    if (node.type === 2) {
      tags.set(node.id, String(node.tagName || '').toLowerCase());
      rewriteAttributes(String(node.tagName || '').toLowerCase(), node.attributes);
    } else if (node.type === 3 && node.isStyle) {
      node.textContent = rewriteCss(node.textContent);
    }
    for (const child of node.childNodes || []) rewriteNode(child);
  }

  /** Points VM-only resources of an rrweb event at the Yulius resource endpoint. */
  function rewriteEvent(event) {
    if (event.type === EV_META && event.data) pageSize = { width: event.data.width || 0, height: event.data.height || 0 };
    if (event.type === EV_FULL_SNAPSHOT && event.data) rewriteNode(event.data.node);
    if (event.type !== EV_INCREMENTAL || !event.data) return event;
    const data = event.data;
    if (data.source === SRC_MUTATION) {
      for (const add of data.adds || []) rewriteNode(add.node);
      for (const change of data.attributes || []) rewriteAttributes(tags.get(change.id) || '', change.attributes);
    } else if (data.source === SRC_STYLESHEET_RULE) {
      for (const add of data.adds || []) add.rule = rewriteCss(add.rule);
    } else if (data.source === SRC_VIEWPORT) {
      pageSize = { width: data.width || pageSize.width, height: data.height || pageSize.height };
    }
    return event;
  }

  /**
   * Paints a sampled canvas frame (rrweb's fps sampling: clearRect + drawImage of one encoded
   * image) from this page. rrweb's own canvas replay needs scripts in the mirror iframe, which
   * would let a hostile page run code as Yulius; the iframe stays script-free instead.
   */
  function paintCanvas(data) {
    if (!replayer || data.type !== 0 || !Array.isArray(data.commands)) return;
    const draw = data.commands.find((command) => command && command.property === 'drawImage');
    const bitmap = draw && Array.isArray(draw.args) ? draw.args[0] : null;
    const blob = bitmap && bitmap.rr_type === 'ImageBitmap' && Array.isArray(bitmap.args) ? bitmap.args[0] : null;
    const buffer = blob && blob.rr_type === 'Blob' && Array.isArray(blob.data) ? blob.data[0] : null;
    if (!buffer || buffer.rr_type !== 'ArrayBuffer' || typeof buffer.base64 !== 'string') return;
    const node = replayer.getMirror().getNode(data.id);
    if (!node || node.nodeName !== 'CANVAS') return;
    const type = /^image\/(png|jpeg|webp)$/.test(blob.type) ? blob.type : 'image/png';
    let bytes;
    try { bytes = Uint8Array.from(atob(buffer.base64), (c) => c.charCodeAt(0)); } catch { return; }
    const seq = (canvasSeq.get(data.id) || 0) + 1;
    canvasSeq.set(data.id, seq);
    createImageBitmap(new Blob([bytes], { type })).then((image) => {
      const context = canvasSeq.get(data.id) === seq && node.isConnected ? node.getContext('2d') : null;
      if (context) {
        context.clearRect(0, 0, node.width, node.height);
        context.drawImage(image, 0, 0);
      }
      image.close();
    }, () => {});
  }

  /**
   * Hands one event to the replayer, stamped before the live baseline so it is applied without
   * delay. rrweb applies added events in microtasks (in order); canvas frames are queued
   * behind them so the canvas they paint exists.
   */
  function apply(event) {
    // The viewer's own text selection belongs to the viewer: the page's is not replayed.
    if (event.type === EV_INCREMENTAL && event.data && event.data.source === SRC_SELECTION) return;
    if (event.type === EV_INCREMENTAL && event.data && event.data.source === SRC_CANVAS) {
      const data = event.data;
      Promise.resolve().then(() => paintCanvas(data));
      return;
    }
    event.timestamp = replayerClock - 1;
    replayer.addEvent(event);
  }

  function destroyReplayer() {
    if (replayer) {
      try { replayer.destroy(); } catch { /* already gone */ }
    }
    replayer = null;
    replayerTab = null;
    tags.clear();
    canvasSeq.clear();
    mirrorHost.replaceChildren();
  }

  function onDom(message) {
    const tab = activeTab();
    if (!tab || message.tab_id !== tab.id) return;
    if (!Replayer) {
      if (!replayerFailed) missedDom = true;
      else if (!pixelsReason) autoPixels(MIRROR_UNAVAILABLE);
      return;
    }
    const events = Array.isArray(message.events) ? message.events : [];
    if (message.reset) {
      destroyReplayer();
      replayerTab = message.tab_id;
      for (const event of events) rewriteEvent(event);
      if (!events.length) return;
      try {
        // Security: never UNSAFE_replayCanvas — it allows scripts in the same-origin mirror
        // iframe. Events are applied synchronously in arrival order (stamped before the
        // baseline), so a snapshot with minutes of buffered changes shows the current state
        // at once and clock differences with the server do not matter.
        replayer = new Replayer([], { root: mirrorHost, liveMode: true, useVirtualDom: false, mouseTail: false, triggerFocus: false, skipInactive: false, showWarning: false, showDebug: false, pauseAnimation: false });
        replayerClock = Date.now();
        replayer.startLive(replayerClock);
        for (const event of events) apply(event);
      } catch (error) {
        console.warn('mirror start failed', error);
        destroyReplayer();
        mirrorFailed();
        return;
      }
      clearTimeout(domTimer);
      pixelsReason = null;
      // Once rrweb's queued microtasks built the snapshot (its document.open() drops earlier
      // listeners, its Meta sets the snapshot's size): wire the document, and lay it out at the
      // newest viewport, which the snapshot may predate (its resize event follows later).
      const built = replayer;
      Promise.resolve().then(() => {
        if (replayer !== built) return;
        wireMirrorDocument();
        applyInputMode();
        if (serverViewport) resizeMirror(serverViewport.width, serverViewport.height);
      });
      applyMode();
      overlay.hidden = true;
      layout();
      return;
    }
    if (!replayer || replayerTab !== message.tab_id) return;
    try {
      for (const event of events) apply(rewriteEvent(event));
    } catch (error) {
      console.warn('mirror replay failed; resynchronizing', error);
      mirrorFailed();
    }
    layout();
  }

  /** Resynchronizes from a fresh snapshot; a page that keeps failing is shown as pixels. */
  function mirrorFailed() {
    mirrorFailures += 1;
    if (mirrorFailures >= MAX_MIRROR_FAILURES) {
      autoPixels('the mirror could not show this page');
      return;
    }
    send({ type: 'dom_resync' });
    expectDom();
  }

  /**
   * Lays the mirror out at the page's new viewport now. The mirror is real DOM, so it reflows
   * like the server page; waiting for the page's resize event (late under load) would map
   * clicks onto the old layout meanwhile.
   */
  function resizeMirror(width, height) {
    if (!replayer || !width || !height) return;
    pageSize = { width, height };
    try { replayer.handleResize({ width, height }); } catch { /* replayer API changed: wait for the event */ }
    layout();
  }

  /** Scales the page (mirror or pixels) to fit the panel; the input layer follows it. */
  function layout() {
    const rect = stage.getBoundingClientRect();
    const width = showingPixels() ? canvas.width : (pageSize.width || rect.width);
    const height = showingPixels() ? canvas.height : (pageSize.height || rect.height);
    if (!width || !height || !rect.width || !rect.height) return;
    const scale = Math.min(1, rect.width / width, rect.height / height);
    const wrapper = mirrorHost.querySelector('.replayer-wrapper');
    if (wrapper) {
      wrapper.style.transform = `scale(${scale})`;
      wrapper.style.transformOrigin = '0 0';
    }
    canvas.style.width = `${Math.round(width * scale)}px`;
    canvas.style.height = `${Math.round(height * scale)}px`;
    Object.assign(inputLayer.style, { width: `${Math.round(width * scale)}px`, height: `${Math.round(height * scale)}px` });
    inputLayer.dataset.scale = String(scale);
    inputLayer.dataset.width = String(width);
    inputLayer.dataset.height = String(height);
  }

  // ---------------------------------------------------------------- agent cursor (BRW-007)
  function onAgent(message) {
    const tab = activeTab();
    if (!tab || message.tab_id !== tab.id) return;
    const scale = Number(inputLayer.dataset.scale || 1);
    const label = `${message.label || 'Agent'}${message.action === 'type' ? ' · typing' : ''}`;
    if (message.x != null && message.y != null) lastAgent = { x: message.x, y: message.y };
    if (lastAgent && (message.action === 'click' || message.action === 'type')) {
      agentCursor.hidden = false;
      agentCursor.style.left = `${Math.round(lastAgent.x * scale)}px`;
      agentCursor.style.top = `${Math.round(lastAgent.y * scale)}px`;
      agentCursor.querySelector('.bp-agent-label').textContent = label;
      if (message.action === 'click') {
        const ripple = h('span', { class: 'bp-agent-ripple' });
        agentCursor.append(ripple);
        setTimeout(() => ripple.remove(), 700);
      }
    } else {
      agentPill.hidden = false;
      agentPill.textContent = `${message.label || 'Agent'} ${message.action === 'open' ? 'opened a tab' : 'navigated'}`;
    }
    clearTimeout(agentTimer);
    agentTimer = setTimeout(() => { agentCursor.hidden = true; agentPill.hidden = true; }, AGENT_CURSOR_IDLE_MS);
  }

  // ---------------------------------------------------------------- socket
  function onMessage(data) {
    if (typeof data !== 'string') {
      const frame = parseBrowserFrame(data);
      if (frame) decode(frame);
      return;
    }
    let message;
    try { message = JSON.parse(data); } catch { return; }
    if (message.type === 'dom') {
      onDom(message);
    } else if (message.type === 'tabs' && Array.isArray(message.tabs)) {
      const before = activeTab();
      tabs = message.tabs;
      for (const id of [...frames.keys()]) if (!tabs.some((t) => t.id === id)) frames.delete(id);
      const now = activeTab();
      if (!now || (before && before.id !== now.id) || (replayerTab && replayerTab !== now.id)) {
        destroyReplayer();
        watchedUrl = now ? now.url : null;
        mirrorFailures = 0;
        if (now) retryDom();
        applyMode();
      } else if (now.url !== watchedUrl) {
        // A navigation: a page that could not be mirrored may be followed by one that can.
        watchedUrl = now.url;
        mirrorFailures = 0;
        if (pixelsReason) retryDom();
      }
      if (pendingOpen) {
        send({ type: 'open', url: pendingOpen });
        pendingOpen = null;
      }
      if (pendingActivate) {
        const target = tabs.find((t) => t.id === pendingActivate);
        if (target && !target.active) send({ type: 'activate', tab_id: target.id });
        else if (!target) toast('That browser tab is no longer open.', { kind: 'warn' });
        pendingActivate = null;
      }
      renderTabs();
      syncActionRequests();
    } else if (message.type === 'dom_unavailable') {
      const tab = activeTab();
      if (tab && tab.id === message.tab_id) {
        clearTimeout(domTimer);
        pixelsReason = message.reason || 'unavailable';
        destroyReplayer();
        applyMode();
      }
    } else if (message.type === 'viewport') {
      serverViewport = { width: message.width, height: message.height };
      resizeMirror(message.width, message.height);
    } else if (message.type === 'agent_activity') {
      onAgent(message);
    } else if (message.type === 'action_requested') {
      ctx.store.handleEvent({ ...message, type: 'browser.action_requested', sandbox_id: sandbox.id });
    } else if (message.type === 'error') {
      lastError = { code: message.code, message: message.message || '' };
      // `upstream` precedes a 1011 close when Chromium is missing or the browser stopped (the
      // overlay explains it); a tab that vanished is replaced by the next tab list.
      if (message.code !== 'upstream' && message.code !== 'not_found') toast(message.message || 'Browser error', { kind: 'error', detail: message.code ? `code: ${message.code}` : null });
    }
  }

  function showFailure(text) {
    showOverlay(h('div', { class: 'stack overlay-message' },
      h('p', null, text),
      h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { lastError = null; if (socket) socket.retry(); else connect(); } }, 'Retry')));
  }

  function connect() {
    if (socket) return;
    socket = new ReconnectingSocket({
      transport: ctx.transport,
      path: `/api/sandboxes/${enc(sandbox.id)}/browser`,
      onOpen: () => {
        lastViewport = null;
        destroyReplayer();
        pixelsReason = null;
        if (wantPixels) send({ type: 'view', mode: 'pixels' });
        else if (replayerFailed) autoPixels(MIRROR_UNAVAILABLE);
        else expectDom();
        applyMode();
        sendViewport();
      },
      onMessage,
      onStatus: (status, detail = {}) => {
        if (status === 'reconnecting' && detail.code === CLOSE_SERVER_ERROR) {
          // The browser failed to start or stopped (e.g. Chromium missing): do not hammer it.
          const reason = lastError && lastError.message ? lastError.message : 'The sandbox browser stopped.';
          if (socket) socket.stop();
          showFailure(`The sandbox browser is not available: ${reason}`);
        } else if (status === 'reconnecting' || status === 'connecting') {
          showOverlay(status === 'connecting' ? 'Connecting to the sandbox browser…' : 'Reconnecting to the sandbox browser…');
        } else if (status === 'failed') {
          showFailure(detail.code === CLOSE_REVOKED ? 'Your access to this sandbox browser was revoked.' : 'The sandbox browser is unreachable.');
        } else if (status === 'open' && tabs.length === 0) {
          showOverlay('Waiting for tabs…');
        }
      },
    });
    socket.start();
  }

  function sendViewport() {
    const rect = stage.getBoundingClientRect();
    const width = Math.round(rect.width);
    const height = Math.round(rect.height);
    if (width < 50 || height < 50) return;
    if (lastViewport && lastViewport.width === width && lastViewport.height === height) return;
    if (send({ type: 'viewport', width, height })) {
      lastViewport = { width, height };
      serverViewport = { width, height };
      resizeMirror(width, height);
    }
  }

  const observer = new ResizeObserver(() => {
    layout();
    clearTimeout(viewportTimer);
    viewportTimer = setTimeout(sendViewport, 150);
  });
  observer.observe(stage);

  // ---------------------------------------------------------------- input forwarding
  function sendMouse(kind, x, y, button, extra = {}) {
    const tab = activeTab();
    if (!tab) return;
    send({ type: 'mouse', tab_id: tab.id, event: kind, x, y, button, dx: 0, dy: 0, ...extra });
  }

  /** Keys go to the real page; copy and paste stay with the user's browser (paste is sent as
   *  text), and the user's own browser shortcuts are left alone. */
  function forwardKey(kind, event) {
    const tab = activeTab();
    if (!tab || event.key === 'F6') return; // F6 moves between regions (app.js), never to the page
    const key = event.key.toLowerCase();
    const command = event.metaKey || event.ctrlKey;
    if (command && ['r', 'w', 't', 'q', 'l', 'n'].includes(key)) return;
    if (command && ['c', 'x', 'v'].includes(key)) return;
    if (!(command && key === 'a')) event.preventDefault(); // select-all: local and remote
    const text = kind === 'down' ? (event.key.length === 1 ? event.key : event.key === 'Enter' ? '\r' : '') : '';
    send({ type: 'key', tab_id: tab.id, event: kind, key: event.key, code: event.code, text, modifiers: modifiers(event) });
  }

  function forwardPaste(event) {
    const tab = activeTab();
    const text = event.clipboardData ? event.clipboardData.getData('text/plain') : '';
    if (!tab || !text) return;
    event.preventDefault();
    send({ type: 'key', tab_id: tab.id, event: 'char', key: '', code: '', text: text.slice(0, 10000), modifiers: 0 });
  }

  // Pixel view: map overlay coordinates back to the page.
  function point(event) {
    const rect = inputLayer.getBoundingClientRect();
    const scale = Number(inputLayer.dataset.scale || 1) || 1;
    const width = Number(inputLayer.dataset.width || 0);
    const height = Number(inputLayer.dataset.height || 0);
    const x = (event.clientX - rect.left) / scale;
    const y = (event.clientY - rect.top) / scale;
    if (x < 0 || y < 0 || (width && x > width) || (height && y > height)) return null;
    return { x: Math.round(x), y: Math.round(y) };
  }

  function overlayMouse(kind, event, button, extra = {}) {
    const p = point(event);
    if (p) sendMouse(kind, p.x, p.y, button, extra);
  }

  inputLayer.addEventListener('pointerdown', (event) => { inputLayer.focus(); overlayMouse('down', event, MOUSE_BUTTONS[event.button] || 'left'); });
  inputLayer.addEventListener('pointerup', (event) => overlayMouse('up', event, MOUSE_BUTTONS[event.button] || 'left'));
  inputLayer.addEventListener('pointermove', (event) => {
    pendingMove = event;
    if (moveFrame) return;
    moveFrame = requestAnimationFrame(() => {
      moveFrame = 0;
      if (pendingMove) overlayMouse('move', pendingMove, pendingMove.buttons & 1 ? 'left' : 'none');
      pendingMove = null;
    });
  });
  inputLayer.addEventListener('wheel', (event) => {
    event.preventDefault();
    overlayMouse('wheel', event, 'none', { dx: Math.round(event.deltaX), dy: Math.round(event.deltaY) });
  }, { passive: false });
  inputLayer.addEventListener('contextmenu', (event) => event.preventDefault());
  inputLayer.addEventListener('keydown', (event) => forwardKey('down', event));
  inputLayer.addEventListener('keyup', (event) => forwardKey('up', event));
  inputLayer.addEventListener('paste', forwardPaste);

  address.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') { const tab = activeTab(); address.value = tab && !isWelcome(tab) ? tab.url : ''; return; }
    if (event.key !== 'Enter') return;
    const url = normalizeUrl(address.value);
    if (!url) { toast('Enter a web address, e.g. example.com or localhost:3000.', { kind: 'warn' }); return; }
    const tab = activeTab();
    if (!tab) send({ type: 'open', url });
    else send({ type: 'navigate', tab_id: tab.id, url });
    address.blur();
    focusPage();
  });

  loadReplayer().then(() => {
    if (replayerFailed) {
      if (!wantPixels && !pixelsReason) autoPixels(MIRROR_UNAVAILABLE);
    } else if (missedDom) {
      missedDom = false;
      send({ type: 'dom_resync' });
      expectDom();
    }
  });

  return {
    el,
    headExtra: () => [pixelsButton, h('span', { class: 'muted xs' }, 'shared by this sandbox')],
    /** opts.tabId: activate that tab once tabs are known (Yulius links, UI-011);
     *  opts.url: open it in a new tab (localhost links from terminals and messages). */
    show(opts = {}) {
      visible = true;
      if (opts.url) {
        if (socket && socket.isOpen && tabs.length) send({ type: 'open', url: opts.url });
        else pendingOpen = opts.url;
      }
      if (opts.tabId) {
        const target = tabs.find((t) => t.id === opts.tabId);
        if (target && socket && socket.isOpen) { if (!target.active) send({ type: 'activate', tab_id: target.id }); } else pendingActivate = opts.tabId;
      }
      connect();
      setTimeout(() => { sendViewport(); layout(); }, 0);
    },
    hide() {
      visible = false;
      clearTimeout(domTimer);
      if (socket) socket.stop();
      socket = null;
      destroyReplayer();
    },
    destroy() {
      observer.disconnect();
      clearTimeout(viewportTimer);
      clearTimeout(agentTimer);
      clearTimeout(domTimer);
      clearTimeout(copiedTimer);
      cancelAnimationFrame(moveFrame);
      if (socket) socket.stop();
      socket = null;
      destroyReplayer();
      for (const bitmap of frames.values()) if (bitmap && bitmap.close) bitmap.close();
      frames.clear();
    },
    get visible() { return visible; },
  };
}
