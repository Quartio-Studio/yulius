// The sandbox's Chrome on the server, watched remotely (BRW-004, BRW-007; SPEC-002 ADR-09, as
// narrowed by ADR-14). Agents browse with it through their MCP tools; people watch its tabs —
// the agent's cursor included — and can act in them (an AWS sign-in an agent asked for, a page
// only the sandbox's Chrome can reach). The watched tab is shown as live DOM (rrweb mirror) or,
// when a page cannot be mirrored or the user asks for it, as JPEG frames. The mirror is a real
// page in the user's browser: text selects and copies natively, hover and cursors work, and
// clicks, drags, keys and wheel are forwarded to the real browser, whose changes stream back.
// Page resources served on the VM (localhost, plain http) load through Yulius.
//
// The panel (browser.js) owns the tab strip and the address bar; this view owns the socket,
// the sandbox Chrome's tab list and the stage of the watched tab. Each viewer watches the tab
// it chose (`watch`), independently of the other viewers.
//
// rrweb's replayer is vendored under a neutral name and loaded on first use: content blockers
// (Brave Shields, uBlock Origin with EasyPrivacy) block any `/rrweb.js`, and a blocked or failed
// load must cost only the DOM view — the panel then shows pixels — never the whole UI.

import { enc } from './api.js';
import { h, toast, focusNextRegion } from './dom.js';
import { ReconnectingSocket, parseBrowserFrame, CLOSE_REVOKED, CLOSE_SERVER_ERROR } from './termsocket.js';

const MOUSE_BUTTONS = ['left', 'middle', 'right'];
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

/**
 * The remote view of one sandbox's Chrome.
 * hooks: onTabs(tabs), onAgent(message), onActionRequested(message), onCreated(tabId),
 * onConnected().
 */
export function createRemoteView(ctx, sandbox, hooks) {
  ensureReplayerStyles();

  let tabs = [];
  let watched = null; // the tab this viewer watches
  let socket = null;
  let expectNew = 0; // tabs this viewer opened: the next new ones are selected
  let knownIds = new Set();
  let lastViewport = null;
  let decoding = false;
  let queuedFrame = null;
  let moveFrame = 0;
  let pendingMove = null;
  let viewportTimer = null;
  let lastError = null; // last {code, message} error frame; explains a following close
  let wantPixels = false; // the user's choice
  let pixelsReason = null; // the server could not mirror the watched tab
  let replayer = null;
  let replayerTab = null;
  let replayerClock = 0; // events are applied in arrival order, all before this baseline
  let domTimer = 0;
  let watchedUrl = null; // the watched tab's URL when the view mode was last chosen
  let mirrorFailures = 0; // on the current page
  let missedDom = false; // DOM arrived before the replayer loaded: resynchronize once it has
  let serverViewport = null; // {width, height} the server last applied to every tab
  const canvasSeq = new Map(); // mirror node id → newest painted frame
  let pageSize = { width: 0, height: 0 };
  const tags = new Map(); // mirror node id → tag name (for attribute rewriting)
  const frames = new Map(); // tab id → last ImageBitmap
  let lastAgent = null; // {x, y}
  let agentTimer = 0;

  const mirrorHost = h('div', { class: 'bp-mirror', role: 'region', 'aria-label': 'Sandbox Chrome page', tabindex: '-1' });
  const canvas = h('canvas', { class: 'bp-canvas', hidden: true });
  // Pixel view only: the canvas cannot take input itself.
  const inputLayer = h('div', { class: 'bp-input', role: 'application', tabindex: '0', hidden: true, 'aria-label': 'Sandbox Chrome page (click to interact)', 'aria-description': 'Keys go to the page. Press F6 to move to the next area, Shift+F6 to go back.', title: 'F6 moves to the next area' });
  const agentCursor = h('div', { class: 'bp-agent', hidden: true }, h('span', { class: 'bp-agent-arrow' }), h('span', { class: 'bp-agent-label' }));
  const agentPill = h('div', { class: 'bp-agent-pill', hidden: true });
  const copied = h('div', { class: 'bp-copied', hidden: true }, 'Copied');
  let copiedTimer = 0;
  const overlay = h('div', { class: 'bp-overlay' }, 'Connecting to the sandbox Chrome…');
  const el = h('div', { class: 'bp-view bp-remote' }, mirrorHost, canvas, inputLayer, agentCursor, agentPill, copied, overlay);
  const viewBadge = h('span', { class: 'chip bp-viewchip', hidden: true });
  const g = canvas.getContext('2d');

  const pixelsButton = h('button', { type: 'button', class: 'btn btn-sm', 'aria-pressed': 'false', title: 'Show the sandbox Chrome as pixels instead of live DOM', onClick: () => setPixels(!wantPixels) }, 'Pixels');

  function watchedTab() {
    return tabs.find((t) => t.id === watched) || null;
  }

  function send(message) {
    return socket ? socket.send(message) : false;
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

  /** Falls back to pixels when the watched tab sends no DOM in time. */
  function expectDom() {
    clearTimeout(domTimer);
    if (showingPixels() || !watched) return;
    domTimer = setTimeout(() => {
      if (showingPixels() || replayer || !watched) return;
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
    inputLayer.hidden = !pixels;
    canvas.hidden = !pixels;
    mirrorHost.hidden = pixels;
    viewBadge.hidden = !pixels || !watched;
    viewBadge.textContent = pixelsReason ? 'Pixel view' : 'Pixels';
    viewBadge.title = pixelsReason ? `This page cannot be mirrored as DOM: ${pixelsReason}` : 'Pixel view (you chose it)';
    if (pixels) {
      const tab = watchedTab();
      if (tab && frames.has(tab.id)) draw(frames.get(tab.id));
    }
    layout();
    updateOverlay();
  }

  /** Focus for keyboard input: the mirror page or the pixel overlay. */
  function focus() {
    if (showingPixels()) inputLayer.focus();
    else if (replayer && replayer.iframe && replayer.iframe.contentWindow) replayer.iframe.contentWindow.focus();
    // Still loading: leave the tab strip anyway, so a key meant for the page (Backspace) never
    // closes the tab that was just clicked (#80).
    else mirrorHost.focus();
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

  function updateOverlay() {
    const tab = watchedTab();
    if (!watched) showOverlay('');
    else if (!tab) showOverlay('This tab of the sandbox Chrome is closed.');
    else if (showingPixels() && frames.has(tab.id)) overlay.hidden = true;
    else if (!showingPixels() && replayer && replayerTab === tab.id) overlay.hidden = true;
    else showOverlay('Loading page…');
  }

  function showOverlay(content) {
    overlay.hidden = false;
    overlay.replaceChildren();
    if (content instanceof Node) overlay.append(content); else overlay.textContent = content;
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
      if (showingPixels() && watched === frame.tabId) { overlay.hidden = true; draw(bitmap); }
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
    const drawCommand = data.commands.find((command) => command && command.property === 'drawImage');
    const bitmap = drawCommand && Array.isArray(drawCommand.args) ? drawCommand.args[0] : null;
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
    if (!watched || message.tab_id !== watched) return;
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
        inputLayer.hidden = !showingPixels();
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
    const rect = el.getBoundingClientRect();
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
    if (hooks.onAgent) hooks.onAgent(message);
    if (!watched || message.tab_id !== watched) return;
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
  function onTabs(list) {
    const before = watchedTab();
    tabs = list;
    for (const id of [...frames.keys()]) if (!tabs.some((t) => t.id === id)) frames.delete(id);
    const fresh = tabs.filter((t) => !knownIds.has(t.id));
    knownIds = new Set(tabs.map((t) => t.id));
    const now = watchedTab();
    if (!now || (before && before.id !== now.id) || (replayerTab && replayerTab !== now.id)) {
      if (!now) destroyReplayer();
    } else if (now.url !== watchedUrl) {
      // A navigation: a page that could not be mirrored may be followed by one that can.
      watchedUrl = now.url;
      mirrorFailures = 0;
      if (pixelsReason) retryDom();
    }
    if (hooks.onTabs) hooks.onTabs(tabs);
    if (expectNew > 0 && fresh.length) {
      expectNew -= 1;
      const created = fresh[fresh.length - 1];
      if (hooks.onCreated) hooks.onCreated(created.id);
    }
    updateOverlay();
  }

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
      onTabs(message.tabs);
    } else if (message.type === 'dom_unavailable') {
      if (watched && watched === message.tab_id) {
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
      if (hooks.onActionRequested) hooks.onActionRequested(message);
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
        send({ type: 'watch', tab_id: watched });
        if (watched) send({ type: 'activate', tab_id: watched });
        if (wantPixels) send({ type: 'view', mode: 'pixels' });
        else if (replayerFailed) autoPixels(MIRROR_UNAVAILABLE);
        else expectDom();
        applyMode();
        sendViewport();
        if (hooks.onConnected) hooks.onConnected();
      },
      onMessage,
      onStatus: (status, detail = {}) => {
        if (status === 'reconnecting' && detail.code === CLOSE_SERVER_ERROR) {
          // The browser failed to start or stopped (e.g. Chromium missing): do not hammer it.
          const reason = lastError && lastError.message ? lastError.message : 'The sandbox Chrome stopped.';
          if (socket) socket.stop();
          showFailure(`The sandbox Chrome is not available: ${reason}`);
        } else if (status === 'reconnecting' || status === 'connecting') {
          showOverlay(status === 'connecting' ? 'Connecting to the sandbox Chrome…' : 'Reconnecting to the sandbox Chrome…');
        } else if (status === 'failed') {
          showFailure(detail.code === CLOSE_REVOKED ? 'Your access to this sandbox browser was revoked.' : 'The sandbox Chrome is unreachable.');
        }
      },
    });
    socket.start();
  }

  function sendViewport() {
    const rect = el.getBoundingClientRect();
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
  observer.observe(el);

  // ---------------------------------------------------------------- input forwarding
  function sendMouse(kind, x, y, button, extra = {}) {
    if (!watched) return;
    send({ type: 'mouse', tab_id: watched, event: kind, x, y, button, dx: 0, dy: 0, ...extra });
  }

  /** Keys go to the real page; copy and paste stay with the user's browser (paste is sent as
   *  text), and the user's own browser shortcuts are left alone. */
  function forwardKey(kind, event) {
    if (!watched || event.key === 'F6') return; // F6 moves between regions (app.js), never to the page
    const key = event.key.toLowerCase();
    const command = event.metaKey || event.ctrlKey;
    if (command && ['r', 'w', 't', 'q', 'l', 'n'].includes(key)) return;
    if (command && ['c', 'x', 'v'].includes(key)) return;
    if (!(command && key === 'a')) event.preventDefault(); // select-all: local and remote
    const text = kind === 'down' ? (event.key.length === 1 ? event.key : event.key === 'Enter' ? '\r' : '') : '';
    send({ type: 'key', tab_id: watched, event: kind, key: event.key, code: event.code, text, modifiers: modifiers(event) });
  }

  function forwardPaste(event) {
    const text = event.clipboardData ? event.clipboardData.getData('text/plain') : '';
    if (!watched || !text) return;
    event.preventDefault();
    send({ type: 'key', tab_id: watched, event: 'char', key: '', code: '', text: text.slice(0, 10000), modifiers: 0 });
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

  loadReplayer().then(() => {
    if (replayerFailed) {
      if (!wantPixels && !pixelsReason && watched) autoPixels(MIRROR_UNAVAILABLE);
    } else if (missedDom) {
      missedDom = false;
      send({ type: 'dom_resync' });
      expectDom();
    }
  });

  return {
    el,
    viewBadge,
    pixelsButton,
    get tabs() { return tabs; },
    get watched() { return watched; },
    get isOpen() { return Boolean(socket && socket.isOpen); },
    /** Watch `tabId` (null: none — the user looks at a live tab, nothing streams). */
    watch(tabId) {
      if (watched === tabId) return;
      watched = tabId || null;
      destroyReplayer();
      clearTimeout(domTimer);
      agentCursor.hidden = true;
      agentPill.hidden = true;
      lastAgent = null;
      mirrorFailures = 0;
      const tab = watchedTab();
      watchedUrl = tab ? tab.url : null;
      send({ type: 'watch', tab_id: watched });
      if (watched) {
        send({ type: 'activate', tab_id: watched });
        retryDom();
        setTimeout(sendViewport, 0);
      }
      applyMode();
    },
    /** Opens a tab in the sandbox Chrome; `onCreated` names it once it exists. */
    open(url) {
      expectNew += 1;
      if (!send({ type: 'open', url })) expectNew -= 1;
    },
    navigate(tabId, url) { send({ type: 'navigate', tab_id: tabId, url }); },
    close(tabId) { send({ type: 'close', tab_id: tabId }); },
    focus,
    layout,
    start() { connect(); setTimeout(() => { sendViewport(); layout(); }, 0); },
    stop() {
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
  };
}
