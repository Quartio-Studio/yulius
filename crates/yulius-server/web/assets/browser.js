// Sandbox browser panel (BRW-001..008; SPEC-002 ADR-15). The panel is the sandbox's Chrome — the
// same Chrome agents drive — streamed from the server (browserstream.js): its tabs in the strip,
// an address bar, back, forward and reload. What the user does in the page is real input, so
// sign-ins and captchas an agent asks for (BRW-004) are done right here, and the agent's cursor
// shows when it works (BRW-007). A new tab opens the sandbox's welcome page: its listening dev
// servers (BRW-006). A dev server can also open live in a window of its own through its preview
// origin (ADR-14), when the server has previews.
//
// The socket runs only while the panel is visible.

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, toast, closableTabKeys } from './dom.js';
import { allows } from './status.js';
import { createStreamView } from './browserstream.js';

const WELCOME_PATH = /\/browser\/welcome\?sandbox=/;
/** A tab is marked as the agent's this long after its last action. */
const AGENT_MARK_MS = 60000;
/** How long the notice about an agent acting in another tab stays. */
const AGENT_NOTICE_MS = 8000;
/** After the user picks a tab, tab lists sent before the server saw it do not move the panel. */
const SELECT_SETTLE_MS = 1500;

/** Normalize an address bar entry to a URL; null when it is not URL-like. A bare port
 *  (`20001`, `:20001/x`) means this sandbox's localhost. */
export function normalizeUrl(input) {
  const value = String(input || '').trim();
  if (!value || /\s/.test(value)) return null;
  const port = value.match(/^:?(\d{2,5})([/?#].*)?$/);
  if (port) return `http://localhost:${port[1]}${port[2] || '/'}`;
  if (/^[a-z][a-z0-9+.-]*:/i.test(value) && !/^[^:/]+:\d+/.test(value)) return value;
  if (/^(localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\])(:\d+)?([/?#]|$)/i.test(value)) return `http://${value}`;
  if (/^[^/]+\.[^/]+/.test(value) || /^[^/:]+:\d+/.test(value)) return `https://${value}`;
  return null;
}

/** The port of a URL on the VM's loopback (`http://localhost:20001/…`), or null. */
export function loopbackPort(url) {
  let parsed;
  try { parsed = new URL(url); } catch { return null; }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return null;
  const host = parsed.hostname.toLowerCase().replace(/^\[|\]$/g, '');
  const loopback = host === 'localhost' || host.endsWith('.localhost') || host === '::1' || host === '0.0.0.0' || /^127\.\d+\.\d+\.\d+$/.test(host);
  const port = Number(parsed.port);
  return loopback && port > 0 ? port : null;
}

/** The preview URL showing loopback `url` (template: an origin with `{port}`). */
export function previewUrl(template, url) {
  const port = loopbackPort(url);
  if (!template || !port) return null;
  const parsed = new URL(url);
  return `${template.replace('{port}', String(port))}${parsed.pathname}${parsed.search}${parsed.hash}`;
}

/** `localhost:20001/path` for display. */
function shortUrl(url) {
  return String(url || '').replace(/^https?:\/\//, '');
}

/** Create the browser panel view for a sandbox. */
export function createBrowserPanel(ctx, sandbox) {
  const el = h('div', { class: 'bp' });
  if (!allows(sandbox.access, 'write')) {
    setChildren(el, h('div', { class: 'panel-msg' }, icon('lock', { size: 20 }), h('p', null, 'The sandbox browser requires write access on GitHub.')));
    return { el };
  }

  let visible = false;
  let selected = null; // the shown tab's id
  let tabs = [];
  let pendingOpen = []; // addresses to open once connected
  let pendingSelect = null; // a tab to select once known (links)
  let previews = null; // PreviewInfo, for "Open live"
  let noticeTimer = 0;
  let selectedAt = 0;
  let knownIds = new Set();
  const agentMarks = new Map(); // tab id → time of the agent's last action

  const stream = createStreamView(ctx, sandbox, {
    onTabs,
    onAgent,
    onActionRequested: (message) => ctx.store.handleEvent({ ...message, type: 'browser.action_requested', sandbox_id: sandbox.id }),
    onCreated: (tabId) => select(tabId),
    onConnected: () => {
      const urls = pendingOpen;
      pendingOpen = [];
      for (const url of urls) stream.open(url);
    },
  });

  // ---------------------------------------------------------------- elements
  const tabList = h('div', { class: 'bp-tablist', role: 'tablist', 'aria-label': 'Browser tabs' });
  const tabStrip = h('div', { class: 'bp-tabs' }, tabList, iconButton('plus', 'New tab', () => openUrl('')));
  const backButton = iconButton('back', 'Back', () => current() && stream.history(current().id, -1));
  const forwardButton = iconButton('forward', 'Forward', () => current() && stream.history(current().id, 1));
  const reloadButton = iconButton('refresh', 'Reload', () => current() && stream.reload(current().id));
  const address = h('input', { class: 'input', type: 'text', placeholder: 'Type an address, e.g. localhost:20000 or aws.amazon.com', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'Address' });
  const liveButton = iconButton('external', 'Open live in a new window', openLive);
  liveButton.hidden = true;
  const bar = h('div', { class: 'bp-bar' }, backButton, forwardButton, reloadButton, address, stream.viewBadge, liveButton, stream.keyboardButton);
  const attention = h('div', { class: 'banner banner-warn bp-attn small', hidden: true });
  const notice = h('div', { class: 'bp-notice', role: 'status', hidden: true });
  const stage = h('div', { class: 'bp-stages' }, stream.el, notice);
  setChildren(el, tabStrip, bar, attention, stage);

  function current() {
    return tabs.find((tab) => tab.id === selected) || null;
  }

  function isWelcome(tab) {
    return Boolean(tab && WELCOME_PATH.test(tab.url || ''));
  }

  function tabTitle(tab) {
    if (isWelcome(tab) || !tab.url || tab.url === 'about:blank') return 'New tab';
    return tab.title || shortUrl(tab.url) || 'New tab';
  }

  function select(tabId, { byUser = true } = {}) {
    if (byUser) selectedAt = Date.now();
    selected = tabId;
    stream.watch(tabId);
    agentMarks.delete(tabId);
    notice.hidden = true;
    render();
  }

  /** Opens `url` in a new tab ('' opens the welcome page). */
  function openUrl(url) {
    if (stream.isOpen) stream.open(url);
    else pendingOpen.push(url);
  }

  function loadPreviews() {
    ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/previews`)
      .then((value) => { previews = value; render(); })
      .catch(() => { previews = null; });
  }

  /** The address opens live in its own window when it is one of this sandbox's dev servers and
   *  the server has preview origins (ADR-14). */
  function liveTarget(url) {
    const port = loopbackPort(url);
    if (!previews || !previews.enabled || !previews.origin_template || !port) return null;
    if (port < previews.first_port || port > previews.last_port || (previews.excluded || []).includes(port)) return null;
    return previewUrl(previews.origin_template, url);
  }

  function openLive() {
    const tab = current();
    const target = tab && liveTarget(tab.url);
    if (target) window.open(target, '_blank', 'noopener');
  }

  address.addEventListener('keydown', (event) => {
    const tab = current();
    if (event.key === 'Escape') { address.value = tab && tab.url && !isWelcome(tab) ? shortUrl(tab.url) : ''; stream.focus(); return; }
    if (event.key !== 'Enter') return;
    const url = normalizeUrl(address.value);
    if (!url) { toast('Enter a web address, e.g. example.com or localhost:20000.', { kind: 'warn' }); return; }
    if (tab) stream.navigate(tab.id, url); else openUrl(url);
    stream.focus();
  });

  // ---------------------------------------------------------------- rendering
  function render() {
    const focused = tabList.contains(document.activeElement) ? document.activeElement.dataset.tabId : null;
    const now = Date.now();
    const items = tabs.map((tab) => {
      const active = tab.id === selected;
      const attn = Boolean(tab.action_requested);
      const agent = (agentMarks.get(tab.id) || 0) > now - AGENT_MARK_MS;
      const title = tabTitle(tab);
      const tip = attn ? `Action requested: ${tab.action_requested.reason}` : (tab.url && !isWelcome(tab) ? shortUrl(tab.url) : 'New tab');
      const close = (event) => { if (event) event.stopPropagation(); stream.close(tab.id); };
      const choose = () => { if (!active) select(tab.id); stream.focus(); };
      const node = h('div', { class: ['bp-tab', active && 'active', attn && 'attention', agent && 'agent'], role: 'tab', tabindex: active ? '0' : '-1', dataset: { tabId: tab.id }, 'aria-selected': String(active), 'aria-keyshortcuts': 'Delete Backspace', title: tip },
        attn ? icon('warn', { size: 12 }) : null,
        h('span', { class: 'bp-tab-title' }, title),
        h('span', { class: 'bp-tab-close', 'aria-hidden': 'true', title: `Close ${title}`, onClick: close }, icon('x')));
      node.addEventListener('click', choose);
      node.addEventListener('keydown', (event) => closableTabKeys(event, { select: choose, close: () => close() }));
      return node;
    });
    if (items.length && !items.some((node) => node.tabIndex === 0)) items[0].tabIndex = 0;
    setChildren(tabList, items);
    // Focus stays in the strip; after closing the focused tab it moves to the selected one.
    if (focused) (items.find((node) => node.dataset.tabId === focused) || items.find((node) => node.tabIndex === 0))?.focus();

    const tab = current();
    if (document.activeElement !== address) address.value = tab && tab.url && !isWelcome(tab) ? shortUrl(tab.url) : '';
    backButton.disabled = !tab;
    forwardButton.disabled = !tab;
    reloadButton.disabled = !tab;
    liveButton.hidden = !(tab && liveTarget(tab.url));
    if (tab && tab.action_requested) {
      attention.hidden = false;
      setChildren(attention, icon('warn', { size: 14 }), h('span', { class: 'banner-text' }, `An agent asks for your action here: ${tab.action_requested.reason}`));
    } else {
      attention.hidden = true;
    }
  }

  // ---------------------------------------------------------------- tabs and agents
  function onTabs(list) {
    tabs = list;
    if (pendingSelect && tabs.some((tab) => tab.id === pendingSelect)) {
      const id = pendingSelect;
      pendingSelect = null;
      select(id);
    } else if (pendingSelect && stream.isOpen) {
      toast('That browser tab is no longer open.', { kind: 'warn' });
      pendingSelect = null;
    }
    // The display shows the active tab (a page, an agent or another viewer may bring a tab or a
    // popup to the front); a list sent before the server saw this user's own choice is stale.
    const active = tabs.find((tab) => tab.active);
    const fresh = Boolean(active && !knownIds.has(active.id));
    knownIds = new Set(tabs.map((tab) => tab.id));
    const settled = Date.now() - selectedAt > SELECT_SETTLE_MS;
    if (active && active.id !== selected && (settled || fresh || !tabs.some((tab) => tab.id === selected))) select(active.id, { byUser: false });
    else if (!tabs.some((tab) => tab.id === selected)) select(tabs[0] ? tabs[0].id : null, { byUser: false });
    syncActionRequests();
    render();
  }

  function syncActionRequests() {
    const known = ctx.store.state.actionRequests.get(sandbox.id);
    const requested = new Set(tabs.filter((t) => t.action_requested).map((t) => t.id));
    if (known) for (const tabId of [...known.keys()]) if (!requested.has(tabId)) ctx.store.clearActionRequest(sandbox.id, tabId);
    for (const tab of tabs) if (tab.action_requested && !(known && known.has(tab.id))) {
      ctx.store.handleEvent({ type: 'browser.action_requested', sandbox_id: sandbox.id, tab_id: tab.id, reason: tab.action_requested.reason, task_id: tab.action_requested.task_id });
    }
  }

  /** An agent acted: its cursor shows on the shown tab; a tab in the background is marked and a
   *  notice offers to show it. */
  function onAgent(message) {
    agentMarks.set(message.tab_id, Date.now());
    render();
    if (message.tab_id === selected) return;
    const tab = tabs.find((t) => t.id === message.tab_id);
    if (!tab) return;
    const verbs = { click: 'clicked', type: 'is typing', navigate: 'navigated', open: 'opened a tab' };
    setChildren(notice,
      h('span', { class: 'bp-agent-dot', 'aria-hidden': 'true' }),
      h('span', null, `${message.label || 'An agent'} ${verbs[message.action] || 'acted'} in ${tabTitle(tab)}`),
      h('button', { type: 'button', class: 'btn btn-sm', onClick: () => select(message.tab_id) }, 'Show'));
    notice.hidden = false;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => { notice.hidden = true; }, AGENT_NOTICE_MS);
  }

  render();

  return {
    el,
    headExtra: () => [h('span', { class: 'muted xs' }, 'the sandbox Chrome, shared with agents')],
    /** opts.tabId: select that tab once known (Yulius links, UI-011); opts.url: open it
     *  (localhost links from terminals and messages). */
    show(opts = {}) {
      visible = true;
      stream.start();
      loadPreviews();
      if (opts.url) openUrl(opts.url);
      if (opts.tabId) {
        if (tabs.some((tab) => tab.id === opts.tabId)) select(opts.tabId);
        else pendingSelect = opts.tabId;
      }
      setTimeout(() => stream.layout(), 0);
    },
    hide() {
      visible = false;
      stream.stop();
    },
    destroy() {
      clearTimeout(noticeTimer);
      stream.destroy();
    },
    get visible() { return visible; },
  };
}
