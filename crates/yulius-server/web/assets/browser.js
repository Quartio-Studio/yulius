// Sandbox browser panel (BRW-001..008; SPEC-002 ADR-14). Two kinds of tabs share one strip:
//
// - Live tabs (yours): the sandbox's dev servers rendered by your own browser in an iframe. Each
//   sandbox port has its own preview origin (`https://<port>-<key>.<yulius host>`), served by
//   Yulius's preview proxy from the VM; the page runs here, so scrolling, selection, typing,
//   WebSockets and hot reload are native. A small bridge script in the page reports its address
//   and title and takes back, forward and reload (postMessage between the preview origin and
//   Yulius only). Live tabs belong to this browser (kept in localStorage per sandbox).
// - Sandbox Chrome tabs (shared): the headless Chrome on the server that agents drive. They are
//   watched remotely (browserremote.js) with the agent's labelled cursor (BRW-007); one click
//   opens the same address live. Other sites (not this sandbox's ports) open there too.
//
// A new tab shows the start page: the sandbox's listening ports and how to use the panel
// (BRW-006). Sockets and polling run only while the panel is visible.

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, toast, focusNextRegion, closableTabKeys } from './dom.js';
import { readJson, writeJson } from './store.js';
import { allows } from './status.js';
import { createRemoteView } from './browserremote.js';

export { needsProxy } from './browserremote.js';

const WELCOME_PATH = /\/browser\/welcome\?sandbox=/;
/** Live tabs per sandbox and browser. */
const MAX_LIVE_TABS = 8;
/** Port list refresh while the start page shows. */
const INFO_REFRESH_MS = 5000;
/** A preview origin that does not answer its ping in this time is reported unreachable (its
 *  first visit may wait for the TLS proxy to obtain a certificate). */
const PING_TIMEOUT_MS = 30000;
/** How long an agent's notice stays over a live tab. */
const AGENT_NOTICE_MS = 8000;
/** A sandbox Chrome tab is marked as the agent's this long after its last action. */
const AGENT_MARK_MS = 60000;
/** Everything a page may do in a live tab except navigating Yulius itself away. */
const LIVE_SANDBOX = 'allow-scripts allow-same-origin allow-forms allow-popups allow-popups-to-escape-sandbox allow-modals allow-downloads allow-pointer-lock allow-presentation allow-storage-access-by-user-activation';

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

/** The loopback address (`http://localhost:<port>/…`) of a preview URL, or null. */
export function loopbackUrlOf(template, href) {
  if (!template || typeof href !== 'string') return null;
  const [before, after] = template.split('{port}');
  if (after === undefined || !href.startsWith(before)) return null;
  const match = href.slice(before.length).match(/^(\d{1,5})/);
  if (!match) return null;
  const rest = href.slice(before.length + match[1].length);
  if (!rest.startsWith(after)) return null;
  const path = rest.slice(after.length);
  if (path && !/^[/?#]/.test(path)) return null;
  return `http://localhost:${match[1]}${path || '/'}`;
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

  const storeKey = `yulius.browser.live.${sandbox.id}`;
  let info = null; // PreviewInfo
  let infoLoad = null;
  let infoTimer = 0;
  let visible = false;
  let seq = 0;
  let selected = null; // {kind: 'live' | 'remote', id}
  let remoteTabs = [];
  let pendingRemote = []; // addresses to open in the sandbox Chrome once connected
  let pendingSelect = null; // a sandbox Chrome tab to select once known (links)
  let noticeTimer = 0;
  const agentMarks = new Map(); // sandbox Chrome tab id → time of the agent's last action
  const pings = new Map(); // preview origin → true once it answered

  const liveTabs = (readJson(storeKey, []) || [])
    .filter((saved) => saved && (saved.url === null || typeof saved.url === 'string'))
    .slice(0, MAX_LIVE_TABS)
    .map((saved) => newLiveTabState(saved.url, saved.title));

  function newLiveTabState(url, title) {
    seq += 1;
    return { id: `live-${seq}`, url: url || null, title: title || '', frameBox: null, iframe: null, origin: null, back: null, forward: null, bridged: false };
  }

  const remote = createRemoteView(ctx, sandbox, {
    onTabs: onRemoteTabs,
    onAgent,
    onActionRequested: (message) => ctx.store.handleEvent({ ...message, type: 'browser.action_requested', sandbox_id: sandbox.id }),
    onCreated: (tabId) => selectRemote(tabId),
    onConnected: () => {
      const urls = pendingRemote;
      pendingRemote = [];
      for (const url of urls) remote.open(url);
    },
  });

  // ---------------------------------------------------------------- elements
  const tabList = h('div', { class: 'bp-tablist', role: 'tablist', 'aria-label': 'Browser tabs' });
  // The new tab shows the start page; it follows the tablist, which holds only tabs.
  const tabStrip = h('div', { class: 'bp-tabs' }, tabList, iconButton('plus', 'New tab', () => selectLive(addLiveTab(null))));
  const backButton = iconButton('back', 'Back', () => history('back'));
  const forwardButton = iconButton('forward', 'Forward', () => history('forward'));
  const reloadButton = iconButton('refresh', 'Reload', reload);
  const address = h('input', { class: 'input', type: 'text', placeholder: 'Type an address, e.g. localhost:20000 or aws.amazon.com', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'Address' });
  const openLiveButton = h('button', { type: 'button', class: 'btn btn-sm', hidden: true, title: 'Open this address live in your browser', onClick: openSelectedLive }, 'Open live');
  const popOutButton = iconButton('external', 'Open in a new window', popOut);
  const bar = h('div', { class: 'bp-bar' }, backButton, forwardButton, reloadButton, address, openLiveButton, remote.viewBadge, remote.pixelsButton, popOutButton);
  const attention = h('div', { class: 'banner banner-warn bp-attn small', hidden: true });
  const startPage = h('div', { class: 'bp-start', role: 'region', 'aria-label': 'New tab' });
  const notice = h('div', { class: 'bp-notice', role: 'status', hidden: true });
  const liveStage = h('div', { class: 'bp-view bp-livestage' }, startPage, notice);
  remote.el.hidden = true;
  const stages = h('div', { class: 'bp-stages' }, liveStage, remote.el);
  setChildren(el, tabStrip, bar, attention, stages);

  // ---------------------------------------------------------------- state helpers
  function persist() {
    writeJson(storeKey, liveTabs.map((tab) => ({ url: tab.url, title: tab.title })));
  }

  function liveTab(id) {
    return liveTabs.find((tab) => tab.id === id) || null;
  }

  function current() {
    if (!selected) return null;
    if (selected.kind === 'live') return liveTab(selected.id);
    return remoteTabs.find((tab) => tab.id === selected.id) || null;
  }

  function isWelcome(tab) {
    return Boolean(tab && WELCOME_PATH.test(tab.url || ''));
  }

  /** Sandbox Chrome tabs worth showing: not its blank or welcome tabs, unless chosen or asking. */
  function shownRemoteTabs() {
    return remoteTabs.filter((tab) => (selected && selected.kind === 'remote' && selected.id === tab.id)
      || tab.action_requested || !(isWelcome(tab) || !tab.url || tab.url === 'about:blank'));
  }

  /** Whether `url` opens live (a listening-or-not port of this sandbox) or in the sandbox Chrome. */
  function opensLive(url) {
    const port = loopbackPort(url);
    return Boolean(info && info.enabled && info.origin_template && port
      && port >= info.first_port && port <= info.last_port && !(info.excluded || []).includes(port));
  }

  function loadInfo() {
    if (!infoLoad) {
      infoLoad = ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/previews`)
        .then((value) => { info = value; })
        .catch((error) => { console.warn('previews', error); })
        .finally(() => {
          infoLoad = null;
          if (selected && selected.kind === 'live' && !current()?.url) renderStart();
          render();
        });
    }
    return infoLoad;
  }

  // ---------------------------------------------------------------- live tabs
  function addLiveTab(url) {
    if (liveTabs.length >= MAX_LIVE_TABS) {
      const oldest = liveTabs.find((tab) => !selected || tab.id !== selected.id);
      if (oldest) removeLiveTab(oldest);
    }
    const tab = newLiveTabState(url, '');
    liveTabs.push(tab);
    persist();
    return tab;
  }

  function removeLiveTab(tab) {
    const index = liveTabs.indexOf(tab);
    if (index < 0) return;
    liveTabs.splice(index, 1);
    if (tab.frameBox) tab.frameBox.remove();
    persist();
    if (selected && selected.kind === 'live' && selected.id === tab.id) {
      const next = liveTabs[Math.min(index, liveTabs.length - 1)];
      if (next) selectLive(next);
      else if (shownRemoteTabs().length) selectRemote(shownRemoteTabs()[0].id);
      else selectLive(addLiveTab(null));
    } else {
      render();
    }
  }

  function frameBox(tab) {
    if (!tab.frameBox) {
      tab.frameBox = h('div', { class: 'bp-frame', hidden: true });
      liveStage.insertBefore(tab.frameBox, notice);
    }
    return tab.frameBox;
  }

  /** Checks that the preview origin answers (wildcard DNS and TLS), once per origin. */
  async function reachable(origin) {
    if (origin === 'null' || pings.get(origin)) return true;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), PING_TIMEOUT_MS);
    try {
      await window.fetch(`${origin}/__yulius/ping`, { mode: 'no-cors', cache: 'no-store', credentials: 'omit', signal: controller.signal });
      pings.set(origin, true);
      return true;
    } catch {
      return false;
    } finally {
      clearTimeout(timer);
    }
  }

  function showUnreachable(tab, url) {
    const box = frameBox(tab);
    tab.iframe = null;
    setChildren(box, h('div', { class: 'bp-start' },
      h('h2', { class: 'bp-start-title' }, `${shortUrl(url)} cannot open live`),
      h('p', { class: 'muted' }, 'Your browser could not reach this sandbox port\'s preview address. The server needs wildcard DNS and TLS for its preview subdomains (see the operations guide).'),
      h('div', { class: 'row' },
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { pings.delete(tab.origin); load(tab, url); } }, 'Try again'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => openRemote(url) }, 'Open in the sandbox Chrome'))));
  }

  /** Shows loopback `url` live in `tab`. A restored tab whose address no longer opens live
   *  (previews turned off, the port left the range) becomes a new tab instead of opening a
   *  sandbox Chrome tab by itself. */
  async function load(tab, url, { restored = false } = {}) {
    if (!info) await loadInfo();
    if (!opensLive(url)) {
      if (restored) {
        tab.url = null;
        persist();
        if (selected && selected.id === tab.id) selectLive(tab);
        return;
      }
      openRemote(url);
      return;
    }
    tab.url = url;
    tab.title = '';
    tab.back = null;
    tab.forward = null;
    tab.bridged = false;
    persist();
    const target = previewUrl(info.origin_template, url);
    let origin = 'null';
    try { origin = new URL(target).origin; } catch { /* opaque */ }
    tab.origin = origin;
    const box = frameBox(tab);
    if (selected && selected.id === tab.id) { box.hidden = false; startPage.hidden = true; }
    if (!tab.iframe) setChildren(box, h('div', { class: 'bp-overlay' }, `Opening ${shortUrl(url)}…`));
    render();
    if (!(await reachable(origin))) {
      if (liveTab(tab.id) && tab.url === url) showUnreachable(tab, url);
      return;
    }
    if (!liveTab(tab.id) || tab.url !== url) return;
    if (!tab.iframe) {
      tab.iframe = h('iframe', { class: 'bp-live', title: `Live page ${shortUrl(url)}`, sandbox: LIVE_SANDBOX, allow: 'clipboard-read; clipboard-write; fullscreen', referrerpolicy: 'no-referrer' });
      setChildren(box, tab.iframe);
    }
    tab.iframe.setAttribute('title', `Live page ${shortUrl(url)}`);
    tab.iframe.src = target;
  }

  function selectLive(tab) {
    selected = { kind: 'live', id: tab.id };
    remote.watch(null);
    remote.el.hidden = true;
    liveStage.hidden = false;
    notice.hidden = true;
    for (const other of liveTabs) if (other.frameBox) other.frameBox.hidden = other !== tab || !tab.url;
    startPage.hidden = Boolean(tab.url);
    if (!tab.url) renderStart();
    else if (!tab.frameBox || (!tab.iframe && !tab.frameBox.firstChild)) load(tab, tab.url, { restored: true });
    render();
  }

  function selectRemote(tabId) {
    selected = { kind: 'remote', id: tabId };
    liveStage.hidden = true;
    remote.el.hidden = false;
    remote.watch(tabId);
    if (agentMarks.has(tabId)) agentMarks.delete(tabId);
    render();
    setTimeout(() => remote.layout(), 0);
  }

  function openRemote(url) {
    if (remote.isOpen) remote.open(url);
    else pendingRemote.push(url);
  }

  /** Opens `url` where it belongs: this sandbox's ports live (in the current live tab or a new
   *  one), everything else in a new sandbox Chrome tab. */
  async function openUrl(url, { newTab = false } = {}) {
    if (!info) await loadInfo();
    const port = loopbackPort(url);
    if (!opensLive(url)) {
      if (port && info && info.enabled && (port < info.first_port || port > info.last_port)) {
        toast(`Port ${port} is outside this sandbox's ports ${info.first_port}–${info.last_port}: it opens in the sandbox Chrome.`, { kind: 'info' });
      }
      openRemote(url);
      return;
    }
    const tab = current();
    const reuse = selected && selected.kind === 'live' && tab && (!newTab || !tab.url);
    const target = reuse ? tab : addLiveTab(null);
    selectLive(target);
    load(target, url);
  }

  // ---------------------------------------------------------------- start page (BRW-006)
  function renderStart() {
    const listening = info ? info.listening : [];
    const ports = info
      ? (listening.length
        ? h('div', { class: 'bp-ports' }, listening.map((port) => h('button', { type: 'button', class: 'bp-port', onClick: () => openUrl(`http://localhost:${port}/`) },
          h('b', null, `localhost:${port}`), h('span', { class: 'muted small' }, info.enabled ? 'Open live' : 'Open'))))
        : h('p', { class: 'muted' }, `No dev server is listening on ports ${info.first_port}–${info.last_port} yet. Start one in a shell or ask an agent, for example `, h('code', null, `PORT=${info.first_port} npm run dev`), '.'))
      : h('p', { class: 'muted' }, 'Looking for dev servers…');
    setChildren(startPage,
      h('h2', { class: 'bp-start-title' }, `Browser of ${sandbox.name || sandbox.slug || 'this sandbox'}`),
      info && !info.enabled ? h('div', { class: 'banner banner-info small' }, icon('warn', { size: 14 }), h('span', { class: 'banner-text' }, `Live pages are off on this server (${info.reason || 'not configured'}); pages open in the sandbox Chrome.`)) : null,
      h('section', { class: 'bp-start-section' }, h('h3', null, 'Running in this sandbox'), ports),
      h('section', { class: 'bp-start-section' }, h('h3', null, 'Tips'), h('ul', { class: 'small' },
        h('li', null, 'Type an address above. This sandbox\'s ports open live: the page runs in your browser, with real scrolling, selection and devtools.'),
        h('li', null, 'Other sites open in the sandbox Chrome on the server, which agents use too. Its tabs are marked; you watch them with the agent\'s cursor.'),
        h('li', null, 'Open a live page in its own window with the arrow button.'))));
  }

  // ---------------------------------------------------------------- toolbar
  function history(direction) {
    const tab = current();
    if (!tab || !selected || selected.kind !== 'live' || !tab.iframe || !tab.iframe.contentWindow || !tab.origin || tab.origin === 'null') return;
    tab.iframe.contentWindow.postMessage({ source: 'yulius', type: direction }, tab.origin);
  }

  function reload() {
    const tab = current();
    if (!tab || !selected) return;
    if (selected.kind === 'remote') {
      remote.navigate(tab.id, tab.url);
    } else if (tab.url) {
      if (tab.iframe && tab.iframe.contentWindow && tab.bridged && tab.origin && tab.origin !== 'null') tab.iframe.contentWindow.postMessage({ source: 'yulius', type: 'reload' }, tab.origin);
      else { tab.iframe = null; if (tab.frameBox) tab.frameBox.replaceChildren(); load(tab, tab.url); }
    }
  }

  function popOut() {
    const tab = current();
    if (!tab || !selected || selected.kind !== 'live' || !tab.url || !info) return;
    const target = previewUrl(info.origin_template, tab.url);
    if (target) window.open(target, '_blank', 'noopener');
  }

  function openSelectedLive() {
    const tab = current();
    if (tab && selected && selected.kind === 'remote' && opensLive(tab.url)) openUrl(tab.url, { newTab: true });
  }

  address.addEventListener('keydown', (event) => {
    const tab = current();
    if (event.key === 'Escape') { address.value = tab && tab.url && !isWelcome(tab) ? shortUrl(tab.url) : ''; return; }
    if (event.key !== 'Enter') return;
    const url = normalizeUrl(address.value);
    if (!url) { toast('Enter a web address, e.g. example.com or localhost:20000.', { kind: 'warn' }); return; }
    address.blur();
    // In a sandbox Chrome tab the address drives that tab, as in any browser.
    if (selected && selected.kind === 'remote' && tab) remote.navigate(tab.id, url);
    else openUrl(url);
  });

  // ---------------------------------------------------------------- rendering
  function render() {
    const focused = tabList.contains(document.activeElement) ? document.activeElement.dataset.tabId : null;
    const item = (id, kind, title, tip, extras) => {
      const active = Boolean(selected && selected.id === id);
      const close = (event) => {
        if (event) event.stopPropagation();
        if (kind === 'live') { const tab = liveTab(id); if (tab) removeLiveTab(tab); } else remote.close(id);
      };
      const select = () => {
        if (kind === 'live') { const tab = liveTab(id); if (tab && !active) selectLive(tab); } else if (!active) selectRemote(id);
        if (kind === 'remote') remote.focus();
      };
      const node = h('div', { class: ['bp-tab', active && 'active', kind === 'remote' && 'remote', extras.attention && 'attention', extras.agent && 'agent'], role: 'tab', tabindex: active ? '0' : '-1', dataset: { tabId: id }, 'aria-selected': String(active), 'aria-keyshortcuts': 'Delete Backspace', title: tip },
        extras.attention ? icon('warn', { size: 12 }) : kind === 'remote' ? icon('cpu', { size: 12, label: 'Sandbox Chrome' }) : null,
        h('span', { class: 'bp-tab-title' }, title),
        h('span', { class: 'bp-tab-close', 'aria-hidden': 'true', title: `Close ${title}`, onClick: close }, icon('x')));
      node.addEventListener('click', select);
      node.addEventListener('keydown', (event) => closableTabKeys(event, { select, close: () => close() }));
      return node;
    };
    const now = Date.now();
    const items = [
      ...liveTabs.map((tab) => item(tab.id, 'live', tab.title || (tab.url ? shortUrl(tab.url) : 'New tab'), tab.url ? `${shortUrl(tab.url)} · live in your browser` : 'New tab', {})),
      ...shownRemoteTabs().map((tab) => {
        const attn = Boolean(tab.action_requested);
        const agent = (agentMarks.get(tab.id) || 0) > now - AGENT_MARK_MS;
        const title = isWelcome(tab) ? 'Welcome' : (tab.title || shortUrl(tab.url) || 'New tab');
        const tip = attn ? `Action requested: ${tab.action_requested.reason}` : `${shortUrl(tab.url)} · sandbox Chrome, shared with agents`;
        return item(tab.id, 'remote', title, tip, { attention: attn, agent });
      }),
    ];
    if (items.length && !items.some((node) => node.tabIndex === 0)) items[0].tabIndex = 0;
    setChildren(tabList, items);
    // Focus stays in the strip; after closing the focused tab it moves to the selected one.
    if (focused) (items.find((node) => node.dataset.tabId === focused) || items.find((node) => node.tabIndex === 0))?.focus();

    const tab = current();
    const live = Boolean(selected && selected.kind === 'live');
    if (document.activeElement !== address) address.value = tab && tab.url && !isWelcome(tab) ? shortUrl(tab.url) : '';
    address.placeholder = live ? 'Type an address, e.g. localhost:20000 or aws.amazon.com' : 'Address in the sandbox Chrome';
    backButton.disabled = !(live && tab && tab.url && tab.back === true);
    forwardButton.disabled = !(live && tab && tab.url && tab.forward === true);
    backButton.title = live ? 'Back' : 'Back is available in live tabs';
    forwardButton.title = live ? 'Forward' : 'Forward is available in live tabs';
    reloadButton.disabled = !(tab && tab.url);
    popOutButton.hidden = !live;
    popOutButton.disabled = !(live && tab && tab.url);
    openLiveButton.hidden = !(selected && selected.kind === 'remote' && tab && opensLive(tab.url));
    remote.pixelsButton.hidden = live;
    if (live) remote.viewBadge.hidden = true;
    if (!live && tab && tab.action_requested) {
      attention.hidden = false;
      setChildren(attention, icon('warn', { size: 14 }), h('span', { class: 'banner-text' }, `An agent asks for your action here: ${tab.action_requested.reason}`));
    } else {
      attention.hidden = true;
    }
  }

  // ---------------------------------------------------------------- sandbox Chrome and agents
  function onRemoteTabs(tabs) {
    remoteTabs = tabs;
    if (pendingSelect && tabs.some((tab) => tab.id === pendingSelect)) {
      const id = pendingSelect;
      pendingSelect = null;
      selectRemote(id);
    } else if (pendingSelect && remote.isOpen) {
      toast('That browser tab is no longer open.', { kind: 'warn' });
      pendingSelect = null;
    }
    if (selected && selected.kind === 'remote' && !tabs.some((tab) => tab.id === selected.id)) {
      const fallback = liveTabs[liveTabs.length - 1] || addLiveTab(null);
      selectLive(fallback);
    }
    syncActionRequests();
    render();
  }

  function syncActionRequests() {
    const known = ctx.store.state.actionRequests.get(sandbox.id);
    const requested = new Set(remoteTabs.filter((t) => t.action_requested).map((t) => t.id));
    if (known) for (const tabId of [...known.keys()]) if (!requested.has(tabId)) ctx.store.clearActionRequest(sandbox.id, tabId);
    for (const tab of remoteTabs) if (tab.action_requested && !(known && known.has(tab.id))) {
      ctx.store.handleEvent({ type: 'browser.action_requested', sandbox_id: sandbox.id, tab_id: tab.id, reason: tab.action_requested.reason, task_id: tab.action_requested.task_id });
    }
  }

  /** An agent acted in a sandbox Chrome tab: mark it, and tell the user when they are looking at
   *  the same app live (the cursor itself shows in that tab). */
  function onAgent(message) {
    agentMarks.set(message.tab_id, Date.now());
    render();
    const tab = current();
    if (!tab || !selected || selected.kind !== 'live' || !tab.url) return;
    const agentTab = remoteTabs.find((t) => t.id === message.tab_id);
    if (!agentTab || loopbackPort(agentTab.url) !== loopbackPort(tab.url)) return;
    const verbs = { click: 'clicked', type: 'is typing', navigate: 'navigated', open: 'opened this app' };
    setChildren(notice,
      h('span', { class: 'bp-agent-dot', 'aria-hidden': 'true' }),
      h('span', null, `${message.label || 'An agent'} ${verbs[message.action] || 'acted'} in its own tab of this app`),
      h('button', { type: 'button', class: 'btn btn-sm', onClick: () => selectRemote(message.tab_id) }, 'Watch'));
    notice.hidden = false;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => { notice.hidden = true; }, AGENT_NOTICE_MS);
  }

  /** The bridge script in live pages reports their address and title (and F6). */
  function onBridge(event) {
    const data = event.data;
    if (!data || typeof data !== 'object' || data.source !== 'yulius-preview') return;
    const tab = liveTabs.find((t) => t.iframe && t.iframe.contentWindow === event.source);
    if (!tab || !info || !info.origin_template) return;
    if (data.type === 'f6') { focusNextRegion(Boolean(data.shift)); return; }
    if (data.type !== 'location' || typeof data.href !== 'string') return;
    let origin;
    try { origin = new URL(data.href).origin; } catch { return; }
    const url = loopbackUrlOf(info.origin_template, data.href);
    if (!url || origin !== event.origin) return;
    tab.url = url;
    tab.origin = origin;
    tab.title = String(data.title || '').slice(0, 300);
    tab.back = typeof data.back === 'boolean' ? data.back : null;
    tab.forward = typeof data.forward === 'boolean' ? data.forward : null;
    tab.bridged = true;
    if (tab.iframe) tab.iframe.setAttribute('title', `Live page ${shortUrl(url)}`);
    persist();
    render();
  }
  window.addEventListener('message', onBridge);

  // Initial selection: the last live tab, or a new tab.
  selectLive(liveTabs[liveTabs.length - 1] || addLiveTab(null));

  return {
    el,
    headExtra: () => [h('span', { class: 'muted xs' }, 'live pages run in your browser')],
    /** opts.tabId: select that sandbox Chrome tab once known (Yulius links, UI-011);
     *  opts.url: open it (localhost links from terminals and messages). */
    show(opts = {}) {
      visible = true;
      remote.start();
      loadInfo();
      clearInterval(infoTimer);
      infoTimer = setInterval(() => {
        if (visible && selected && selected.kind === 'live' && !current()?.url) loadInfo();
      }, INFO_REFRESH_MS);
      if (opts.url) openUrl(opts.url, { newTab: true });
      if (opts.tabId) {
        if (remoteTabs.some((tab) => tab.id === opts.tabId)) selectRemote(opts.tabId);
        else pendingSelect = opts.tabId;
      }
      setTimeout(() => remote.layout(), 0);
    },
    hide() {
      visible = false;
      clearInterval(infoTimer);
      remote.stop();
    },
    destroy() {
      clearInterval(infoTimer);
      clearTimeout(noticeTimer);
      window.removeEventListener('message', onBridge);
      remote.destroy();
      for (const tab of liveTabs) if (tab.frameBox) tab.frameBox.remove();
    },
    get visible() { return visible; },
  };
}
