// Client-side router. History API paths (/s/<slug>/t/<number>) against the real server,
// which serves index.html for every UI path. Hash mode (#/s/<slug>/…) is used with
// ?mock=1 so a plain static file server can reload deep links.

const listeners = new Set();
const quietListeners = new Set();
let hashMode = false;
let lastKey = null;

const SANDBOX_PAGES = new Set(['tasks', 'sessions', 'git', 'ci', 'cd', 'workflows', 'skills', 'automations', 'mcp', 'secrets', 'dashboard']);
const ADMIN_PAGES = new Set(['users', 'accounts', 'github', 'instance']);

// Where each sandbox and each sandbox page was left (SPEC-113 KST-001): a link marked
// `data-restore` goes back there instead of to its plain path. Kept for this browser tab
// (sessionStorage), forgotten on sign-out; the oldest go first past MAX_PLACES.
const PLACES_KEY = 'yulius.places';
const MAX_PLACES = 200;
let places = loadPlaces();

function loadPlaces() {
  try {
    const saved = JSON.parse(window.sessionStorage.getItem(PLACES_KEY) || '[]');
    return new Map((Array.isArray(saved) ? saved : []).filter((entry) => Array.isArray(entry) && typeof entry[0] === 'string' && typeof entry[1] === 'string' && entry[1].startsWith('/s/')));
  } catch {
    return new Map();
  }
}

function savePlaces() {
  try { window.sessionStorage.setItem(PLACES_KEY, JSON.stringify([...places])); } catch { /* kept in memory only */ }
}

/** `/s/<slug>` of a path inside a sandbox. */
const sandboxOf = (path) => path.split('/').slice(0, 3).join('/');

function remember(loc) {
  const route = matchRoute(loc.path, loc.query);
  if (!route.slug) return;
  const entries = [[`sandbox:${sandboxOf(loc.path)}`, loc.full]];
  if (route.name === 'page') entries.push([`page:${loc.path}`, loc.full]);
  for (const [key, full] of entries) {
    places.delete(key); // re-added last: the map's order is the age
    places.set(key, full);
  }
  while (places.size > MAX_PLACES) places.delete(places.keys().next().value);
  savePlaces();
}

/**
 * Where a link to `path` goes: with `restore` 'sandbox' (a sandbox's tab or card) the address last
 * shown inside that sandbox, with 'page' (a sandbox page's link) that page's last address. A link
 * to where the person already is, or to a place not visited in this tab, goes to `path` itself.
 */
export function placeFor(path, restore) {
  const here = currentLocation().path;
  if (restore === 'sandbox' && here !== path && !here.startsWith(`${path}/`)) return places.get(`sandbox:${path}`) || path;
  if (restore === 'page' && here !== path) return places.get(`page:${path}`) || path;
  return path;
}

/** Where an `a[data-route]` link goes: its route, or the place it restores (`data-restore`). */
export function linkTarget(anchor) {
  return anchor.dataset.restore ? placeFor(anchor.dataset.route, anchor.dataset.restore) : anchor.dataset.route;
}

/** The address `full` is not a place to return to (a task that is not there, KST-004). */
export function forgetPlace(full) {
  const key = `sandbox:${sandboxOf(full.split('?')[0])}`;
  if (places.get(key) !== full) return;
  places.delete(key);
  savePlaces();
}

/** Sign-out: no place of this person stays in the tab. */
export function forgetPlaces() {
  places = new Map();
  try {
    window.sessionStorage.removeItem(PLACES_KEY);
    window.sessionStorage.removeItem(`${PLACES_KEY}.owner`);
  } catch { /* nothing kept */ }
}

/** Signed in as `userId`: places another person left in this tab (signed in elsewhere) go. */
export function bindPlaces(userId) {
  const owner = String(userId || '');
  try {
    if (window.sessionStorage.getItem(`${PLACES_KEY}.owner`) === owner) return;
    forgetPlaces();
    window.sessionStorage.setItem(`${PLACES_KEY}.owner`, owner);
  } catch {
    places = new Map();
  }
}

/** Start listening to navigation. */
export function initRouter({ hash = false } = {}) {
  hashMode = hash;
  window.addEventListener('popstate', emit);
  if (hash) window.addEventListener('hashchange', emit);
  document.addEventListener('click', onDocumentClick);
}

/** Subscribe to route changes; returns an unsubscribe function. */
export function onRoute(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/**
 * Subscribe to quiet address changes (`replaceQuietly`) with the route they now name; returns an
 * unsubscribe function. The view in place already shows it: nothing is mounted again.
 */
export function onQuietReplace(fn) {
  quietListeners.add(fn);
  return () => quietListeners.delete(fn);
}

/** Current {path, query} independent of the routing mode. */
export function currentLocation() {
  if (hashMode) {
    const raw = window.location.hash.slice(1) || '/';
    const url = new URL(raw.startsWith('/') ? raw : `/${raw}`, window.location.origin);
    return { path: url.pathname, query: url.searchParams, full: url.pathname + url.search };
  }
  return { path: window.location.pathname, query: new URLSearchParams(window.location.search), full: window.location.pathname + window.location.search };
}

/** href attribute for an app path (keeps middle-click / open-in-new-tab working). */
export function href(path) {
  if (!hashMode) return path;
  return `${window.location.pathname}${window.location.search}#${path}`;
}

/**
 * Replace the current URL without a route change (a page keeping its own state in the query, such as
 * Tasks filters). The router records it, so a later link to the plain path is a real change.
 */
export function replaceQuietly(path) {
  const url = hashMode ? `${window.location.pathname}${window.location.search}#${path}` : path;
  window.history.replaceState(window.history.state, '', url);
  const loc = currentLocation();
  lastKey = loc.full;
  remember(loc);
  const route = matchRoute(loc.path, loc.query);
  for (const fn of quietListeners) fn(route);
}

/** Navigate to an app path, e.g. navigate('/s/studio/t/12'). */
export function navigate(path, { replace = false } = {}) {
  const url = hashMode ? `${window.location.pathname}${window.location.search}#${path}` : path;
  if (replace) window.history.replaceState(null, '', url);
  else window.history.pushState(null, '', url);
  emit();
}

function emit() {
  const loc = currentLocation();
  if (loc.full === lastKey) return;
  lastKey = loc.full;
  remember(loc);
  const route = matchRoute(loc.path, loc.query);
  for (const fn of listeners) fn(route);
}

/** Force listeners to re-run for the current location (after sign-in, resync). */
export function refreshRoute() {
  lastKey = null;
  emit();
}

function onDocumentClick(event) {
  if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  const anchor = event.target.closest && event.target.closest('a[data-route]');
  if (!anchor) return;
  event.preventDefault();
  navigate(linkTarget(anchor));
}

/** Map a path to a route descriptor {name, slug, number, page, query}. */
export function matchRoute(path, query = new URLSearchParams()) {
  const parts = path.split('/').filter(Boolean).map((p) => {
    try { return decodeURIComponent(p); } catch { return p; }
  });
  const base = { query, path };
  if (parts.length === 0) return { ...base, name: 'home' };
  const [first, second, third, fourth] = parts;
  if (parts.length === 1 && ['setup', 'signin', 'device', 'archived', 'settings', 'design', 'secrets'].includes(first)) return { ...base, name: first };
  if (first === 'admin' && parts.length === 2 && ADMIN_PAGES.has(second)) return { ...base, name: 'admin', page: second };
  if (first === 's' && second) {
    if (parts.length === 2) return { ...base, name: 'composer', slug: second };
    if (parts.length === 4 && third === 't' && /^\d+$/.test(fourth)) return { ...base, name: 'task', slug: second, number: Number(fourth) };
    // Tasks → a task's overview, and Git → a task's changes, before opening its session.
    if (parts.length === 4 && third === 'tasks' && /^\d+$/.test(fourth)) return { ...base, name: 'taskInfo', slug: second, page: 'tasks', number: Number(fourth) };
    if (parts.length === 4 && third === 'skills') return { ...base, name: 'skill', slug: second, page: 'skills', skill: fourth };
    if (parts.length === 4 && third === 'workflows') return { ...base, name: 'workflow', slug: second, page: 'workflows', workflow: fourth };
    if (parts.length === 4 && third === 'mcp') return { ...base, name: 'mcpServer', slug: second, page: 'mcp', server: fourth };
    if (parts.length === 4 && third === 'automations') return { ...base, name: 'automation', slug: second, page: 'automations', automation: fourth };
    // Sessions → one session's read-only transcript (SES-003).
    if (parts.length === 4 && third === 'sessions') return { ...base, name: 'session', slug: second, page: 'sessions', session: fourth };
    if (parts.length === 5 && third === 'git' && fourth === 't' && /^\d+$/.test(parts[4])) return { ...base, name: 'gitTask', slug: second, page: 'git', number: Number(parts[4]) };
    if (parts.length === 3 && third === 'browser') return { ...base, name: 'composer', slug: second, browserTab: query.get('tab') || '' };
    // The Superagent (SPEC-035 AGT-006, SPEC-074 SUP-007) is a right panel; its earlier addresses
    // (the Agent's, the Cockpit's) open it too.
    if (parts.length === 3 && ['superagent', 'agent', 'cockpit'].includes(third)) return { ...base, name: 'composer', slug: second, panel: 'agent' };
    if (parts.length === 3 && SANDBOX_PAGES.has(third)) return { ...base, name: 'page', slug: second, page: third };
  }
  return { ...base, name: 'notfound' };
}

/**
 * Recognize links that belong to this Yulius instance. Returns
 * {kind: 'browser', slug, tabId} | {kind: 'route', path} | null.
 */
export function parseAppLink(uri, publicUrl) {
  let url;
  try { url = new URL(uri, window.location.origin); } catch { return null; }
  const origins = new Set([window.location.origin]);
  if (publicUrl) {
    try { origins.add(new URL(publicUrl).origin); } catch { /* ignore malformed public URL */ }
  }
  if (!origins.has(url.origin)) return null;
  let path = url.pathname;
  let query = url.searchParams;
  if (url.hash.startsWith('#/')) {
    const inner = new URL(url.hash.slice(1), window.location.origin);
    path = inner.pathname;
    query = inner.searchParams;
  }
  const browser = path.match(/^\/s\/([^/]+)\/browser\/?$/);
  if (browser) return { kind: 'browser', slug: decodeURIComponent(browser[1]), tabId: query.get('tab') || '' };
  const route = matchRoute(path, query);
  if (route.name !== 'notfound' && !path.includes('/editor')) return { kind: 'route', path: path + (query.toString() ? `?${query}` : '') };
  return null;
}
