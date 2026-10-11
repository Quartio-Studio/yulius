// Client-side router. History API paths (/s/<slug>/t/<number>) against the real server,
// which serves index.html for every UI path. Hash mode (#/s/<slug>/…) is used with
// ?mock=1 so a plain static file server can reload deep links.

const listeners = new Set();
let hashMode = false;
let lastKey = null;

const SANDBOX_PAGES = new Set(['tasks', 'sessions', 'git', 'workflows', 'skills', 'automations', 'mcp', 'dashboard']);
const ADMIN_PAGES = new Set(['users', 'accounts', 'github', 'instance']);

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
  lastKey = currentLocation().full;
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
  navigate(anchor.dataset.route);
}

/** Map a path to a route descriptor {name, slug, number, page, query}. */
export function matchRoute(path, query = new URLSearchParams()) {
  const parts = path.split('/').filter(Boolean).map((p) => {
    try { return decodeURIComponent(p); } catch { return p; }
  });
  const base = { query, path };
  if (parts.length === 0) return { ...base, name: 'home' };
  const [first, second, third, fourth] = parts;
  if (parts.length === 1 && ['setup', 'signin', 'device', 'archived', 'settings', 'design'].includes(first)) return { ...base, name: first };
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
