// Sandbox browser panel (BRW-001..004): tab strip, address bar and a canvas showing JPEG
// frames from WS /api/sandboxes/{sid}/browser, forwarding mouse and keyboard input.
// The socket is open only while the panel is visible (frames are not streamed when hidden).

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, toast } from './dom.js';
import { ReconnectingSocket, parseBrowserFrame, CLOSE_REVOKED, CLOSE_SERVER_ERROR } from './termsocket.js';
import { allows } from './status.js';

const MOUSE_BUTTONS = ['left', 'middle', 'right'];

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

/** Create the browser panel view for a sandbox. */
export function createBrowserPanel(ctx, sandbox) {
  const el = h('div', { class: 'bp' });
  if (!allows(sandbox.access, 'write')) {
    setChildren(el, h('div', { class: 'panel-msg' }, icon('lock', { size: 20 }), h('p', null, 'The sandbox browser requires write access on GitHub.')));
    return { el };
  }

  let tabs = [];
  let socket = null;
  let pendingActivate = null;
  let newTabMode = false;
  let lastViewport = null;
  let decoding = false;
  let queuedFrame = null;
  let moveFrame = 0;
  let pendingMove = null;
  let viewportTimer = null;
  let visible = false;
  let lastError = null; // last {code, message} error frame; explains a following close
  const frames = new Map(); // tab id → last ImageBitmap

  const tabStrip = h('div', { class: 'bp-tabs', role: 'tablist', 'aria-label': 'Browser tabs' });
  const address = h('input', { class: 'input', type: 'text', placeholder: 'Type an address, e.g. localhost:20000 or aws.amazon.com', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'Address' });
  const reloadButton = iconButton('refresh', 'Reload', () => {
    const tab = activeTab();
    if (tab) send({ type: 'navigate', tab_id: tab.id, url: tab.url });
  });
  const attention = h('div', { class: 'banner banner-warn bp-attn small', hidden: true });
  const canvas = h('canvas', { class: 'bp-canvas', tabindex: '0', 'aria-label': 'Remote browser page (click to interact)' });
  const overlay = h('div', { class: 'bp-overlay' }, 'Connecting to the sandbox browser…');
  const view = h('div', { class: 'bp-view' }, canvas, overlay);
  setChildren(el, tabStrip, h('div', { class: 'bp-bar' }, reloadButton, address), attention, view);
  const g = canvas.getContext('2d');

  function activeTab() {
    return tabs.find((t) => t.active) || null;
  }

  function send(message) {
    return socket ? socket.send(message) : false;
  }

  function renderTabs() {
    const items = tabs.map((tab) => {
      const attn = Boolean(tab.action_requested);
      const button = h('div', { class: ['bp-tab', tab.active && 'active', attn && 'attention'], role: 'tab', tabindex: '0', 'aria-selected': String(Boolean(tab.active)), title: attn ? `Action requested: ${tab.action_requested.reason}` : tab.url },
        attn ? icon('warn', { size: 12 }) : null,
        h('span', { class: 'bp-tab-title' }, tab.title || tab.url || 'New tab'),
        iconButton('x', `Close ${tab.title || 'tab'}`, (event) => { event.stopPropagation(); send({ type: 'close', tab_id: tab.id }); }));
      const activate = () => { if (!tab.active) send({ type: 'activate', tab_id: tab.id }); newTabMode = false; canvas.focus(); };
      button.addEventListener('click', activate);
      button.addEventListener('keydown', (event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); activate(); } });
      return button;
    });
    items.push(iconButton('plus', 'New tab', () => {
      newTabMode = true;
      address.value = '';
      address.placeholder = 'Open a new tab: type an address and press Enter';
      address.focus();
    }));
    setChildren(tabStrip, items);
    const tab = activeTab();
    if (document.activeElement !== address) address.value = tab ? tab.url : '';
    if (tab && tab.action_requested) {
      attention.hidden = false;
      setChildren(attention, icon('warn', { size: 14 }), h('span', { class: 'banner-text' }, `An agent asks for your action here: ${tab.action_requested.reason}`));
    } else {
      attention.hidden = true;
    }
    if (tabs.length === 0) showOverlay(emptyState());
    else if (tab && frames.has(tab.id)) { overlay.hidden = true; draw(frames.get(tab.id)); } else if (tab) showOverlay('Loading page…');
  }

  function emptyState() {
    const base = sandbox.port_base;
    return h('div', { class: 'stack', style: { alignItems: 'center' } },
      h('p', null, 'No open tabs. Type an address above.'),
      base ? h('p', { class: 'small' }, `Dev servers of this sandbox use ports ${base}–${base + (sandbox.port_count || 1) - 1}. `,
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => send({ type: 'open', url: `http://localhost:${base}/` }) }, `Open localhost:${base}`)) : null);
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

  function draw(bitmap) {
    if (!bitmap) return;
    if (canvas.width !== bitmap.width || canvas.height !== bitmap.height) {
      canvas.width = bitmap.width;
      canvas.height = bitmap.height;
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
      if (tab && tab.id === frame.tabId) { overlay.hidden = true; draw(bitmap); }
    } catch { /* corrupt frame: skip */ }
    decoding = false;
    if (queuedFrame) { const next = queuedFrame; queuedFrame = null; decode(next); }
  }

  function onMessage(data) {
    if (typeof data !== 'string') {
      const frame = parseBrowserFrame(data);
      if (frame) decode(frame);
      return;
    }
    let message;
    try { message = JSON.parse(data); } catch { return; }
    if (message.type === 'tabs' && Array.isArray(message.tabs)) {
      tabs = message.tabs;
      for (const id of [...frames.keys()]) if (!tabs.some((t) => t.id === id)) frames.delete(id);
      if (pendingActivate) {
        const target = tabs.find((t) => t.id === pendingActivate);
        if (target && !target.active) send({ type: 'activate', tab_id: target.id });
        else if (!target) toast('That browser tab is no longer open.', { kind: 'warn' });
        pendingActivate = null;
      }
      renderTabs();
      syncActionRequests();
    } else if (message.type === 'action_requested') {
      ctx.store.handleEvent({ ...message, type: 'browser.action_requested', sandbox_id: sandbox.id });
    } else if (message.type === 'error') {
      lastError = { code: message.code, message: message.message || '' };
      // `upstream` precedes a 1011 close when Chromium is missing or the browser stopped:
      // the overlay explains it, so no toast.
      if (message.code !== 'upstream') toast(message.message || 'Browser error', { kind: 'error', detail: message.code ? `code: ${message.code}` : null });
    }
  }

  function showFailure(text) {
    showOverlay(h('div', { class: 'stack', style: { alignItems: 'center', maxWidth: '420px', textAlign: 'center' } },
      h('p', null, text),
      h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { lastError = null; if (socket) socket.retry(); else connect(); } }, 'Retry')));
  }

  function connect() {
    if (socket) return;
    socket = new ReconnectingSocket({
      transport: ctx.transport,
      path: `/api/sandboxes/${enc(sandbox.id)}/browser`,
      onOpen: () => { lastViewport = null; sendViewport(); },
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
    const rect = view.getBoundingClientRect();
    const width = Math.round(rect.width);
    const height = Math.round(rect.height);
    if (width < 50 || height < 50) return;
    if (lastViewport && lastViewport.width === width && lastViewport.height === height) return;
    if (send({ type: 'viewport', width, height })) lastViewport = { width, height };
  }

  const observer = new ResizeObserver(() => {
    clearTimeout(viewportTimer);
    viewportTimer = setTimeout(sendViewport, 150);
  });
  observer.observe(view);

  // ---- input forwarding
  function point(event) {
    const rect = canvas.getBoundingClientRect();
    if (!canvas.width || !canvas.height) return null;
    const scale = Math.min(rect.width / canvas.width, rect.height / canvas.height);
    const x = (event.clientX - rect.left) / scale;
    const y = (event.clientY - rect.top) / scale;
    if (x < 0 || y < 0 || x > canvas.width || y > canvas.height) return null;
    return { x: Math.round(x), y: Math.round(y) };
  }

  function mouse(kind, event, extra = {}) {
    const tab = activeTab();
    const p = point(event);
    if (!tab || !p) return;
    send({ type: 'mouse', tab_id: tab.id, event: kind, x: p.x, y: p.y, button: MOUSE_BUTTONS[event.button] || 'left', dx: 0, dy: 0, ...extra });
  }

  canvas.addEventListener('pointerdown', (event) => { canvas.focus(); mouse('down', event); });
  canvas.addEventListener('pointerup', (event) => mouse('up', event));
  canvas.addEventListener('pointermove', (event) => {
    pendingMove = event;
    if (moveFrame) return;
    moveFrame = requestAnimationFrame(() => { moveFrame = 0; if (pendingMove) mouse('move', pendingMove, { button: 'none' }); pendingMove = null; });
  });
  canvas.addEventListener('wheel', (event) => {
    event.preventDefault();
    mouse('wheel', event, { button: 'none', dx: Math.round(event.deltaX), dy: Math.round(event.deltaY) });
  }, { passive: false });
  canvas.addEventListener('contextmenu', (event) => event.preventDefault());
  const keyHandler = (kind) => (event) => {
    const tab = activeTab();
    if (!tab) return;
    const browserShortcut = (event.metaKey || event.ctrlKey) && ['r', 'w', 't', 'q', 'l', 'n'].includes(event.key.toLowerCase());
    if (browserShortcut) return;
    event.preventDefault();
    const text = kind === 'down' ? (event.key.length === 1 ? event.key : event.key === 'Enter' ? '\r' : '') : '';
    send({ type: 'key', tab_id: tab.id, event: kind, key: event.key, code: event.code, text, modifiers: modifiers(event) });
  };
  canvas.addEventListener('keydown', keyHandler('down'));
  canvas.addEventListener('keyup', keyHandler('up'));
  canvas.addEventListener('paste', (event) => {
    const tab = activeTab();
    const text = event.clipboardData ? event.clipboardData.getData('text/plain') : '';
    if (!tab || !text) return;
    event.preventDefault();
    send({ type: 'key', tab_id: tab.id, event: 'char', key: '', code: '', text: text.slice(0, 10000), modifiers: 0 });
  });

  address.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') { newTabMode = false; address.placeholder = 'Type an address'; const tab = activeTab(); address.value = tab ? tab.url : ''; return; }
    if (event.key !== 'Enter') return;
    const url = normalizeUrl(address.value);
    if (!url) { toast('Enter a web address, e.g. example.com or localhost:3000.', { kind: 'warn' }); return; }
    const tab = activeTab();
    if (newTabMode || !tab) send({ type: 'open', url });
    else send({ type: 'navigate', tab_id: tab.id, url });
    newTabMode = false;
    address.blur();
    canvas.focus();
  });

  return {
    el,
    headExtra: () => [h('span', { class: 'muted xs' }, 'shared by this sandbox')],
    /** opts.tabId: activate that tab once tabs are known (Yulius links, UI-011). */
    show(opts = {}) {
      visible = true;
      if (opts.tabId) {
        const target = tabs.find((t) => t.id === opts.tabId);
        if (target && socket && socket.isOpen) { if (!target.active) send({ type: 'activate', tab_id: target.id }); } else pendingActivate = opts.tabId;
      }
      connect();
      setTimeout(sendViewport, 0);
    },
    hide() {
      visible = false;
      if (socket) socket.stop();
      socket = null;
    },
    destroy() {
      observer.disconnect();
      clearTimeout(viewportTimer);
      cancelAnimationFrame(moveFrame);
      if (socket) socket.stop();
      socket = null;
      for (const bitmap of frames.values()) if (bitmap && bitmap.close) bitmap.close();
      frames.clear();
    },
    get visible() { return visible; },
  };
}
