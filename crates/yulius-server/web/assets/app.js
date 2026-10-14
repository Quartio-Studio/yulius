// Yulius web UI entry point: boot (setup → sign-in → workspace), route dispatch and
// composition of the global layout. Plain ES modules, no build step (SPEC-002 ADR-01).
//
// Module map: api.js (HTTP client), router.js (routes), store.js (state + event stream),
// termsocket.js (WebSocket protocols), terminal.js (xterm view, loaded on demand), layout.js (tab row,
// sidebar, status), panels.js + browser.js (right rail), taskview.js (task + composer),
// actions.js (task actions, dialogs), pages/*.js (pages), mock.js (?mock=1 backend).

import { createApi, realTransport, enc } from './api.js';
import { initRouter, onRoute, navigate, currentLocation, refreshRoute, parseAppLink, href, forgetPlaces, bindPlaces, onQuietReplace } from './router.js';
import { createStore, readJson, writeJson } from './store.js';
import { applyTheme, PHONE } from './theme.js';
import { BRAND } from './brand.js';
import { h, setChildren, toast, errorToast, placeholder, icon, isLocalUrl, focusNextRegion, loadError, closeMenu } from './dom.js';
import { notFound } from './pages/common.js';
import { createTabRow, createSidebar } from './layout.js';
import { createPanelHost } from './panels.js';
import { mountTaskView, mountComposer } from './taskview.js';
import { mountSetup, mountSignIn, signInProblem } from './pages/auth.js';
import { mountTasksPage, mountArchivedPage, mountDevicePage } from './pages/tasks.js';
import { mountGitPage } from './pages/git.js';
import { mountCiPage } from './pages/ci.js';
import { mountGitTaskView } from './pages/gittask.js';
import { mountTaskInfoView } from './pages/taskinfo.js';
import { mountSkillEditor } from './pages/skilledit.js';
import { mountSkillCreator } from './pages/skillnew.js';
import { mountWorkflowEditor } from './pages/workflowedit.js';
import { mountMcpEditor } from './pages/mcpedit.js';
import { mountAutomationEditor } from './pages/automationedit.js';
import { mountWorkflowsPage } from './pages/workflows.js';
import { mountSkillsPage } from './pages/skills.js';
import { mountAutomationsPage } from './pages/automations.js';
import { mountSessionsPage, mountSessionView } from './pages/sessions.js';
import { mountMcpPage } from './pages/mcp.js';
import { mountDashboardPage } from './pages/dashboard.js';
import { mountUsersPage, mountAccountsPage, mountGithubPage } from './pages/admin.js';
import { mountSettingsPage } from './pages/settings.js';
import { mountInstancePage } from './pages/instance.js';
import { createUpdateBanner } from './updates.js';
import { mountSecretsPage } from './pages/secrets.js';
import { mountDesignPage } from './pages/design.js';
import { mountHomePage } from './pages/home.js';
import { IS_MAC, showShortcuts } from './shortcuts.js';
import { preloadTerminal } from './terminal.js';
import { forgetDrafts } from './drafts.js';
import { forgetUnsavedNotes } from './notes.js';

const params = new URLSearchParams(window.location.search);
const NOT_FOUND = { title: 'Page not found', text: 'There is nothing at this address. The link may be old or mistyped.', back: ['All sandboxes', '/'] };
const MOCK = params.get('mock') === '1';
const LAST_SANDBOX_KEY = 'yulius.lastSandbox';

const SANDBOX_PAGES = {
  tasks: (ctx, sandbox) => mountTasksPage(ctx, sandbox),
  sessions: (ctx, sandbox) => mountSessionsPage(ctx, sandbox),
  git: (ctx, sandbox) => mountGitPage(ctx, sandbox),
  workflows: (ctx, sandbox) => mountWorkflowsPage(ctx, sandbox),
  skills: (ctx, sandbox, route) => mountSkillsPage(ctx, sandbox, route),
  automations: (ctx, sandbox) => mountAutomationsPage(ctx, sandbox),
  mcp: (ctx, sandbox, route) => mountMcpPage(ctx, sandbox, route),
  secrets: (ctx, sandbox, route) => mountSecretsPage(ctx, sandbox, route),
  ci: (ctx, sandbox, route) => mountCiPage(ctx, sandbox, route),
  dashboard: (ctx, sandbox, route) => mountDashboardPage(ctx, sandbox, route),
};

const ADMIN_PAGES = {
  users: mountUsersPage,
  accounts: mountAccountsPage,
  github: mountGithubPage,
  instance: mountInstancePage,
};

function createApp(root, transport) {
  let expiredHandled = false;
  const api = createApi(transport, { onUnauthenticated: () => sessionExpired() });
  const store = createStore(api, { pullGapMs: transport.pullGapMs });
  let shell = null;
  let centerView = null;
  let currentRoute = null;
  let currentSandbox = null;
  let currentTask = null;
  const unsubscribers = [];

  const ctx = {
    api,
    store,
    transport,
    mock: MOCK,
    navigate,
    href,
    route: () => currentRoute,
    currentSandbox: () => currentSandbox,
    currentTask: () => currentTask,
    openLink,
    openFile,
    onTaskClosed,
    signOut,
    isSuperadmin: () => Boolean(store.state.me && store.state.me.user && store.state.me.user.is_superadmin),
    isInstanceAdmin: () => Boolean(store.state.me && store.state.me.user && (store.state.me.user.is_superadmin || store.state.me.user.is_instance_admin)),
  };

  // ------------------------------------------------------------ boot

  async function start() {
    setChildren(root, h('main', { class: 'auth' }, placeholder(`Loading ${BRAND.name}…`, 'loading')));
    // Both asked at once: one round trip before the app shows instead of two (PERF-11).
    const setupRead = api.get('/api/setup/status', { allow401: true });
    const meRead = api.get('/api/me', { allow401: true });
    meRead.catch(() => {}); // answered below, or not needed when setup is required
    let setup;
    try {
      setup = await setupRead;
    } catch (error) {
      fatal(error);
      return;
    }
    if (setup && setup.setup_required) {
      renderSetup();
      return;
    }
    try {
      const me = await meRead;
      await signedIn(me);
    } catch (error) {
      if (error.status === 401) {
        // A session found expired at load: what it kept for the next page load goes (SPEC-104), and
        // where it left each sandbox (SPEC-113).
        store.forgetPulls();
        forgetPlaces();
        renderSignIn();
      } else fatal(error);
    }
  }

  function fatal(error) {
    setChildren(root, h('main', { class: 'auth' }, h('div', { class: 'auth-card' },
      h('div', { class: 'auth-logo' }, BRAND.name),
      h('div', { class: 'banner banner-danger' }, icon('warn'), h('span', { class: 'banner-text' }, error.status >= 500 ? `The server had a problem (${error.message}). Try again in a moment.` : `The server is not reachable: ${error.message}`)),
      h('button', { type: 'button', class: 'btn btn-primary', onClick: start }, 'Try again'))));
  }

  function renderSetup() {
    if (currentLocation().path !== '/setup') navigate('/setup', { replace: true });
    setChildren(root, mountSetup(ctx, { onDone: (me) => signedIn(me, '/') }).el);
  }

  function renderSignIn() {
    const loc = currentLocation();
    const next = loc.path === '/signin' ? loc.query.get('next') || '/' : loc.full;
    // A refused GitHub sign-in lands here with ?error=<reason> (ACC-004). The page says the reason
    // in fixed sentences (signInProblem), and the address loses it, so a reload starts clean.
    const reason = loc.path === '/signin' ? loc.query.get('error') : null;
    const problem = reason ? signInProblem(reason, loc.query.get('login')) : null;
    navigate(`/signin?next=${encodeURIComponent(next)}`, { replace: true });
    setChildren(root, mountSignIn(ctx, { next, problem, onDone: (me, target) => signedIn(me, target) }).el);
  }

  async function signedIn(me, next) {
    expiredHandled = false;
    store.state.me = me;
    bindPlaces(me && me.user && me.user.id);
    api.setCsrf(me && me.csrf_token);
    // A failed load is said in the view (Home, a sandbox's pages, the sidebar), not in a toast.
    try { await store.loadSandboxes(); } catch { /* store.state.sandboxesError */ }
    buildShell();
    store.startEvents();
    // The terminal (xterm) is not part of the first paint; fetch it once the browser is idle so the
    // first task opens without waiting for it (PERF-04).
    (window.requestIdleCallback || ((fn) => setTimeout(fn, 1500)))(() => preloadTerminal());
    const user = (me && me.user) || {};
    // A forgotten superadmin password can be reset with a recovery code (ACC-011), so the
    // warning appears only while no code exists (UX-12); the composer shows it too.
    if (user.is_superadmin) {
      api.get('/api/auth/recovery').then((state) => {
        if (!state || state.available) return;
        ctx.notifications.add({ kind: 'warn', title: 'No recovery code yet', detail: 'A forgotten superadmin password can’t be reset without one.', route: '/admin/users', key: 'notice:recovery', once: true });
      }).catch(() => {});
    }
    // GitHub is optional (SPEC-099): playgrounds work without it, so it is a quiet notification for
    // later, never a toast on every sign-in.
    if (user.is_superadmin && me.instance && me.instance.github_configured === false) {
      ctx.notifications.add({ kind: 'info', title: 'GitHub is optional', detail: 'Playgrounds work without it. Connect GitHub to import or create your organisation’s repositories.', route: '/admin/github', key: 'notice:github', once: true });
    }
    const loc = currentLocation();
    if (next) navigate(next, { replace: true });
    else if (loc.path === '/signin' || loc.path === '/setup') navigate(loc.query.get('next') || '/', { replace: true });
    else refreshRoute();
  }

  function sessionExpired() {
    if (expiredHandled || !store.state.me) return;
    expiredHandled = true;
    teardown();
    toast('You were signed out. Please sign in again.', { kind: 'warn' });
    renderSignIn();
  }

  async function signOut() {
    try {
      await api.post('/api/auth/logout');
    } catch (error) {
      if (error.status !== 401) { errorToast(error, 'Sign out failed'); return; }
    }
    teardown();
    // Signed-out data does not stay in this browser: the notification history names tasks and
    // agent requests (OWASP ASVS V8.2.3). Preferences such as the theme stay.
    try {
      for (const key of Object.keys(window.localStorage)) if (key.startsWith('yulius.notifications.')) window.localStorage.removeItem(key);
    } catch { /* storage unavailable: nothing kept */ }
    forgetDrafts(); // unfinished prompts too (SPEC-026)
    forgetUnsavedNotes(); // and notes not yet saved (SPEC-041)
    navigate('/signin', { replace: true });
    renderSignIn();
  }

  function teardown() {
    store.stopEvents();
    if (centerView && centerView.destroy) centerView.destroy();
    centerView = null;
    // Close open menus and dialogs, then tear the shell down (no timers or listeners survive).
    closeMenu();
    document.querySelectorAll('dialog[open]').forEach((d) => { d.close(); d.remove(); });
    if (shell) {
      shell.panels.setContext(null, null);
      for (const part of [shell.tabRow, shell.sidebar, shell.panels]) if (part && part.destroy) part.destroy();
    }
    if (ctx.notifications) ctx.notifications.destroy();
    ctx.notifications = null;
    if (ctx.updates) ctx.updates.destroy();
    ctx.updates = null;
    shell = null;
    store.state.me = null;
    // The pull requests kept for the next page load go with the session, signed out or expired
    // (SPEC-104); a read answering later finds no user and writes nothing to storage.
    store.forgetPulls();
    forgetPlaces(); // and where each sandbox was left (SPEC-113)
    unsubscribers.splice(0).forEach((off) => off());
    document.body.classList.remove('drawer-open');
  }

  // ------------------------------------------------------------ shell

  function buildShell() {
    const appEl = h('div', { class: 'app' });
    const tabRow = createTabRow(ctx);
    const sidebar = createSidebar(ctx);
    const center = h('main', { class: 'center', id: 'main' });
    const panels = createPanelHost(ctx);
    const backdrop = h('div', { class: 'drawer-backdrop', onClick: () => document.body.classList.remove('drawer-open') });
    appEl.append(tabRow.el, sidebar.el, center, panels.panel, panels.rail);
    // First Tab stop: skip the tab row and sidebar (WCAG 2.4.1).
    const skip = h('a', { class: 'skip-link', href: '#main', onClick: (event) => { event.preventDefault(); const target = center.querySelector('h1, input, textarea, button, a[href]'); if (target) { if (target.tagName === 'H1') target.tabIndex = -1; target.focus(); } } }, 'Skip to main content');
    // Live-updates outage (PERF-08): said after 5 s without the event stream (at once when it gave
    // up), with Retry; the network coming back or the tab being shown again retries at once.
    const connBanner = h('div', { class: 'conn-banner', role: 'status', hidden: true });
    // Instance updates (SPEC-002 ADR-18): a calm banner while one runs and "Update available".
    const updates = createUpdateBanner(ctx);
    ctx.updates = updates;
    setChildren(root, skip, appEl, backdrop, connBanner);
    const tabRight = tabRow.el.querySelector('.tabrow-right');
    if (tabRight) tabRight.prepend(updates.el);
    else root.append(updates.el);
    new MutationObserver(syncDrawer).observe(appEl, { attributes: true, attributeFilter: ['class'] });
    syncDrawer();
    ctx.openNewTask = (sandbox) => openNewTask(sandbox);
    ctx.defaultSandbox = () => defaultSandbox();
    ctx.openBrowserTab = (sandbox, tabId) => openBrowserTab(sandbox, tabId);
    ctx.toggleRail = () => panels.setRailVisible(!panels.railVisible);
    ctx.closePanel = () => panels.close();
    ctx.railVisible = () => panels.railVisible;
    ctx.setRailVisible = (visible) => panels.setRailVisible(visible);
    panels.mounted();
    shell = { el: appEl, tabRow, sidebar, center, panels };
    tabRow.renderMe();

    let connTimer = null;
    const showConn = () => {
      const status = store.state.eventsStatus;
      const offline = navigator.onLine === false;
      const restarting = !offline && ctx.updates && ctx.updates.restarting();
      const text = offline
        ? `You’re offline. ${BRAND.name} reconnects when the network is back.`
        : restarting ? `${BRAND.name} is restarting for an update. Reconnecting…`
          : status === 'failed' ? `Can’t reach ${BRAND.name}. Live updates stopped.` : `Can’t reach ${BRAND.name}. Reconnecting…`;
      setChildren(connBanner, icon('warn', { size: 14 }), h('span', null, text),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => store.retryEvents() }, 'Retry now'));
      connBanner.hidden = false;
    };
    const onConn = (status) => {
      if (status === 'open' || status === 'closed') { clearTimeout(connTimer); connTimer = null; connBanner.hidden = true; return; }
      // The server ends the stream for good when the session ended (#66): a 401 here signs out.
      if (status === 'failed') api.get('/api/me').catch(() => {});
      if (status === 'failed' || !connBanner.hidden) { clearTimeout(connTimer); connTimer = null; showConn(); return; }
      // Each reconnect attempt reports again: the 5 s count starts at the first one only.
      if (!connTimer) connTimer = setTimeout(() => { connTimer = null; showConn(); }, 5000);
    };
    const retryIfDown = () => { if (store.state.eventsStatus !== 'open' && store.state.eventsStatus !== 'closed') store.retryEvents(); };
    const onVisible = () => { if (!document.hidden) retryIfDown(); };
    const onOffline = () => { if (!connBanner.hidden) showConn(); };
    window.addEventListener('online', retryIfDown);
    window.addEventListener('offline', onOffline);
    document.addEventListener('visibilitychange', onVisible);

    unsubscribers.push(
      store.on('events-status', onConn),
      () => { clearTimeout(connTimer); window.removeEventListener('online', retryIfDown); window.removeEventListener('offline', onOffline); document.removeEventListener('visibilitychange', onVisible); },
      store.on('task', (task) => {
        if (centerView && centerView.key === `task:${task.id}` && centerView.update) {
          centerView.update(task);
          currentTask = { ...currentTask, ...task };
        }
        updateTitle();
      }),
      store.on('task-removed', ({ taskId, byUser }) => {
        if (centerView && centerView.key === `task:${taskId}` && currentSandbox) {
          // Someone else (or another tab) removed it; your own Archive or Delete has its own toast.
          if (!byUser) toast('This task was archived or removed.', { kind: 'info' });
          navigate(`/s/${encodeURIComponent(currentSandbox.slug)}`);
        }
      }),
      store.on('sandboxes', () => {
        if (currentSandbox) {
          const fresh = store.sandboxById(currentSandbox.id);
          if (fresh) currentSandbox = fresh;
        }
        updateTitle();
      }),
      store.on('tasks', () => updateTitle()),
      // `resync` follows lost events and every sandbox change (import, onboarding, removal).
      store.on('resync', async () => {
        try {
          await store.loadSandboxes();
          if (currentSandbox && !store.sandboxById(currentSandbox.id)) {
            toast(`${currentSandbox.name} was removed or is no longer visible to you.`, { kind: 'info' });
            ctx.notifications.add({ kind: 'info', title: `${currentSandbox.name} was removed or is no longer visible to you` });
            navigate('/', { replace: true });
            return;
          }
          if (currentSandbox) await store.loadTasks(currentSandbox.id);
        } catch (error) {
          errorToast(error, 'Couldn’t refresh');
        }
      }),
      store.on('browser-action', (event) => {
        if (event.cleared) return;
        const sandbox = store.sandboxById(event.sandbox_id);
        if (!sandbox) return;
        const task = event.task_id ? store.findTask(event.task_id) : null;
        toast(`${task ? `#${task.number} ${task.title}` : 'An agent'} needs your action in the ${sandbox.name} browser`, {
          kind: 'warn',
          detail: event.reason || null,
          timeout: 15000,
          action: { label: 'Open tab', run: () => openBrowserTab(sandbox, event.tab_id) },
        });
      }),
    );
  }

  // Each view has its own title (WCAG 2.4.2): the task, or the page's heading, then the sandbox.
  // A changed view is announced to screen readers (route changes are otherwise silent).
  let announcedTitle = '';
  function updateTitle() {
    let attention = 0;
    for (const sandbox of store.state.sandboxes) attention += (sandbox.counts && sandbox.counts.needs_attention) || 0;
    const parts = [];
    const heading = document.querySelector('#main h1');
    if (currentTask) parts.push(`#${currentTask.number} ${currentTask.title}`);
    else if (heading && heading.textContent.trim()) parts.push(heading.textContent.trim());
    if (currentSandbox) parts.push(currentSandbox.name);
    parts.push(BRAND.name);
    const title = parts.join(' · ');
    document.title = `${attention ? `(${attention}) ` : ''}${title}`;
    if (title !== announcedTitle) {
      announcedTitle = title;
      let live = document.getElementById('route-announcer');
      if (!live) {
        live = h('div', { id: 'route-announcer', class: 'sr-only', 'aria-live': 'polite', 'aria-atomic': 'true' });
        document.body.append(live);
      }
      live.textContent = parts.slice(0, -1).join(', ');
    }
  }

  function setCenter(key, factory, { paper = true } = {}) {
    shell.center.classList.toggle('paper', paper);
    if (centerView && centerView.key === key) return centerView;
    if (centerView && centerView.destroy) centerView.destroy();
    centerView = factory();
    centerView.key = key;
    setChildren(shell.center, centerView.el);
    if (centerView.start) centerView.start();
    return centerView;
  }

  /** A page that cannot be shown (#147): a heading, why, and the way back (pages/common.js). */
  function missing(key, view) {
    setCenter(key, () => ({ el: h('div', { class: 'page' }, notFound(view)) }));
  }

  /** A load that failed, in the view with a retry (not a toast). */
  function failed(key, text, retry) {
    setCenter(key, () => ({ el: h('div', { class: 'page' }, h('div', { class: 'page-inner' }, loadError(text, retry))) }));
  }

  // ------------------------------------------------------------ links

  function openBrowserTab(sandbox, tabId) {
    if (currentSandbox && currentSandbox.id === sandbox.id && shell) {
      shell.panels.open('browser', { tabId });
    } else {
      navigate(`/s/${encodeURIComponent(sandbox.slug)}/browser?tab=${encodeURIComponent(tabId || '')}`);
    }
  }

  /** The viewed task closed because the user left the session: show the task list. */
  function onTaskClosed(task) {
    const sandbox = store.sandboxById(task.sandbox_id);
    if (!sandbox || !currentSandbox || currentSandbox.id !== sandbox.id) return;
    navigate(`/s/${encodeURIComponent(sandbox.slug)}/tasks`);
    toast(`Task #${task.number} closed. Continue it any time from Tasks.`, { kind: 'info' });
  }

  /**
   * File links from terminals (UI-018, F 2026-10-10): the Editor panel at the file and line.
   * `file` is {root ('sandbox' or 'task:<id>'), path, line, col}.
   */
  function openFile(sandbox, file) {
    if (!shell || !currentSandbox || currentSandbox.id !== sandbox.id) return;
    shell.panels.open('editor', { file });
  }

  /** Links from the terminal and pages (UI-011, BRW-004). */
  function openLink(uri) {
    const me = store.state.me;
    const link = parseAppLink(uri, me && me.instance ? me.instance.public_url : null);
    if (link && link.kind === 'browser') {
      const sandbox = store.sandboxBySlug(link.slug);
      if (!sandbox) { toast('That sandbox is not available to you.', { kind: 'warn' }); return; }
      openBrowserTab(sandbox, link.tabId);
      return;
    }
    if (link && link.kind === 'route') { navigate(link.path); return; }
    if (!/^https?:\/\//i.test(uri)) return;
    // Pages served on the instance (localhost) open in the sandbox's browser; everything
    // else in a normal new tab of the user's browser (F 2026-10-07).
    if (isLocalUrl(uri) && currentSandbox && shell) {
      shell.panels.open('browser', { url: uri });
      return;
    }
    window.open(uri, '_blank', 'noopener,noreferrer');
  }

  // ------------------------------------------------------------ routing

  // A failed load is said where the tasks show (the sidebar, Tasks, the task route), not in a toast.
  async function ensureTasks(sandbox) {
    if (store.tasksOf(sandbox.id) !== null) return;
    try { await store.loadTasks(sandbox.id); } catch { /* store.tasksError(sandbox.id) */ }
  }

  async function findTaskByNumber(sandbox, number) {
    const active = (store.tasksOf(sandbox.id) || []).find((t) => t.number === number);
    if (active) return active;
    try {
      const archived = await api.get(`/api/sandboxes/${enc(sandbox.id)}/tasks?archived=true`);
      return (Array.isArray(archived) ? archived : []).find((t) => t.number === number) || null;
    } catch {
      return null;
    }
  }

  async function handleRoute(route) {
    if (!store.state.me || !shell) return;
    currentRoute = route;
    document.body.classList.remove('drawer-open');
    if (route.name === 'setup' || route.name === 'signin') { navigate('/', { replace: true }); return; }

    // Home (UI-045) and the other pages outside a sandbox (leaveSandbox).
    if (!route.slug) {
      leaveSandbox(route, { home: route.name === 'home' });
      showOutside(route);
      updateTitle();
      return;
    }
    let sandbox = store.sandboxBySlug(route.slug);
    if (!sandbox) {
      try { await store.loadSandboxes(); } catch { /* reported below */ }
      sandbox = store.sandboxBySlug(route.slug);
    }
    if (currentRoute !== route) return;
    if (!sandbox) {
      leaveSandbox(route);
      if (store.state.sandboxesError) sandboxesFailed(`sandboxes-error:${route.slug}`);
      else missing(`missing:${route.slug}`, { title: 'Sandbox not found', text: `There is no sandbox “${route.slug}”, or you don’t have access to its GitHub repository.`, back: ['All sandboxes', '/'] });
      updateTitle();
      return;
    }
    writeJson(LAST_SANDBOX_KEY, sandbox.id);
    const wasOutside = shell.el.classList.contains('no-sandbox');
    shell.el.classList.remove('no-sandbox');
    if (wasOutside && ctx.fitSidebar) ctx.fitSidebar();
    currentSandbox = sandbox;
    shell.tabRow.update(sandbox.slug);
    shell.sidebar.update(route, sandbox);
    ensureTasks(sandbox).then(() => { if (currentRoute === route && shell) shell.sidebar.refresh(); });

    let task = null;
    switch (route.name) {
      case 'task': {
        await ensureTasks(sandbox);
        if (currentRoute !== route) return;
        if (store.tasksOf(sandbox.id) === null && store.tasksError(sandbox.id)) {
          // The list failed to load: say so, rather than "Task #12 was not found".
          failed(`tasks-error:${sandbox.id}:${route.number}`, `Couldn’t load the tasks of ${sandbox.name}: ${store.tasksError(sandbox.id).message}`,
            () => store.loadTasks(sandbox.id).then(() => refreshRoute(), () => {}));
          break;
        }
        task = await findTaskByNumber(sandbox, route.number);
        if (currentRoute !== route) return;
        if (!task) { missingTask(sandbox, route.number); break; }
        setCenter(`task:${task.id}`, () => mountTaskView(ctx, sandbox, task), { paper: false });
        break;
      }
      case 'skill':
        // Skills → New skill; "new" is reserved, so no skill can hide the page.
        if (route.skill === 'new') setCenter(`skill-new:${sandbox.id}`, () => mountSkillCreator(ctx, sandbox));
        else setCenter(`skill:${sandbox.id}:${route.skill}`, () => mountSkillEditor(ctx, sandbox, route.skill));
        break;
      case 'workflow':
        setCenter(`workflow:${sandbox.id}:${route.workflow}`, () => mountWorkflowEditor(ctx, sandbox, route.workflow));
        break;
      case 'mcpServer':
        setCenter(`mcp:${sandbox.id}:${route.server}`, () => mountMcpEditor(ctx, sandbox, route.server));
        break;
      case 'automation':
        setCenter(`automation:${sandbox.id}:${route.automation}`, () => mountAutomationEditor(ctx, sandbox, route.automation));
        break;
      case 'session':
        setCenter(`session:${sandbox.id}:${route.session}:${route.query.toString()}`, () => mountSessionView(ctx, sandbox, route.session, route));
        break;
      case 'taskInfo': {
        await ensureTasks(sandbox);
        if (currentRoute !== route) return;
        const shown = await findTaskByNumber(sandbox, route.number);
        if (currentRoute !== route) return;
        if (!shown) { missingTask(sandbox, route.number); break; }
        setCenter(`task-info:${shown.id}`, () => mountTaskInfoView(ctx, sandbox, shown));
        break;
      }
      case 'gitTask': {
        // Git → a task's changes: found like a task (archived and closed ones included).
        await ensureTasks(sandbox);
        if (currentRoute !== route) return;
        const reviewed = await findTaskByNumber(sandbox, route.number);
        if (currentRoute !== route) return;
        if (!reviewed) { missingTask(sandbox, route.number); break; }
        const view = setCenter(`git-task:${reviewed.id}`, () => mountGitTaskView(ctx, sandbox, reviewed, route));
        if (view.update) view.update(route);
        break;
      }
      case 'composer':
        setCenter(`composer:${sandbox.id}`, () => mountComposer(ctx, sandbox));
        break;
      case 'page': {
        const factory = SANDBOX_PAGES[route.page];
        const base = `${route.page}:${sandbox.id}`;
        // A page that keeps state in its query (Tasks filters) updates in place; others remount.
        if (centerView && centerView.update && String(centerView.key).startsWith(`${base}:`)) {
          centerView.key = `${base}:${route.query.toString()}`;
          centerView.update(route);
          break;
        }
        setCenter(`${base}:${route.query.toString()}`, () => factory(ctx, sandbox, route));
        break;
      }
      default:
        missing('notfound', NOT_FOUND);
    }
    currentTask = task;
    shell.panels.setContext(sandbox, task);
    if (route.name === 'composer' && route.browserTab !== undefined && sandbox) {
      shell.panels.open('browser', { tabId: route.browserTab });
      navigate(`/s/${encodeURIComponent(sandbox.slug)}`, { replace: true });
    }
    if (route.name === 'composer' && route.panel && sandbox) {
      shell.panels.open(route.panel, { persistRail: false });
      navigate(`/s/${encodeURIComponent(sandbox.slug)}`, { replace: true });
    }
    updateTitle();
  }

  /**
   * Pages outside a sandbox — Home (UI-045), Settings, Archived, Device, Team secrets, the
   * administration pages, Design, unknown addresses and a sandbox that is not there — show no
   * sandbox, as the tab row does (no sandbox tab is current): the sidebar drops the sandbox's
   * navigation and tasks, and the tools go (#147). New task still starts in the sandbox used last.
   */
  function leaveSandbox(route, { home = false } = {}) {
    currentSandbox = null;
    currentTask = null;
    const wasInside = !shell.el.classList.contains('no-sandbox');
    shell.el.classList.add('no-sandbox');
    shell.tabRow.update(null, { home });
    shell.sidebar.update(route, null);
    shell.panels.setContext(null, null);
    if (wasInside && ctx.fitSidebar) ctx.fitSidebar();
  }

  /** Task #n of a sandbox does not exist (deleted, or a wrong link). */
  function missingTask(sandbox, number) {
    const tasks = `/s/${encodeURIComponent(sandbox.slug)}/tasks`;
    missing(`missing-task:${sandbox.id}:${number}`, { title: 'Task not found', text: `There is no task #${number} in ${sandbox.name}. It may have been deleted.`, back: ['Tasks', tasks] });
  }

  /** A page outside any sandbox (the caller cleared the sandbox). */
  function showOutside(route) {
    if (route.name === 'home') {
      if (store.state.sandboxesError) sandboxesFailed('sandboxes-error'); else setCenter('home', () => mountHomePage(ctx));
      return;
    }
    switch (route.name) {
      case 'archived': setCenter('archived', () => mountArchivedPage(ctx)); break;
      case 'settings': setCenter('settings', () => mountSettingsPage(ctx)); break;
      // Team secrets without a sandbox (the user menu); agent secrets need one.
      case 'secrets': setCenter('secrets', () => mountSecretsPage(ctx, null, route)); break;
      case 'design': setCenter('design', () => mountDesignPage(ctx)); break;
      case 'device': setCenter(`device:${route.query.toString()}`, () => mountDevicePage(ctx, route)); break;
      case 'admin': setCenter(`admin:${route.page}`, () => ADMIN_PAGES[route.page](ctx, route)); break;
      default: missing('notfound', NOT_FOUND);
    }
  }

  /** The sandbox list could not be loaded: say so with a retry (not "no sandboxes" or "no access"). */
  function sandboxesFailed(key) {
    const error = store.state.sandboxesError;
    setCenter(key, () => ({ el: h('div', { class: 'page' }, h('div', { class: 'page-inner' },
      loadError(`Couldn’t load your sandboxes: ${error.message}`, () => store.loadSandboxes().then(() => refreshRoute(), () => {})))) }));
  }

  // A page that kept its state in the address (a filter, an open run) is that address's view now: a
  // link to the page's plain address mounts it again instead of keeping the filtered view (SPEC-113).
  onQuietReplace((route) => {
    if (route.name !== 'page' || !centerView || !currentSandbox) return;
    const base = `${route.page}:${currentSandbox.id}`;
    if (String(centerView.key).startsWith(`${base}:`)) centerView.key = `${base}:${route.query.toString()}`;
  });

  onRoute((route) => {
    handleRoute(route).catch((error) => {
      console.error(error);
      errorToast(error, 'Navigation failed');
    });
  });

  document.addEventListener('keydown', (event) => {
    // Escape closes the innermost layer only: a menu, popover or dialog handles it first.
    if (event.key === 'Escape' && !event.defaultPrevented && !document.querySelector('dialog[open]')) document.body.classList.remove('drawer-open');
  });

  // Phone drawer (WCAG 2.4.3, 4.1.2): while closed the off-screen sidebar is inert (not tabbable);
  // opening it moves focus in, closing it from inside returns focus to the menu button.
  function syncDrawer() {
    const phone = window.matchMedia(PHONE);
    const open = document.body.classList.contains('drawer-open');
    const sidebarEl = document.getElementById('sidebar');
    const menuButton = document.querySelector('.hamburger');
    if (menuButton) menuButton.setAttribute('aria-expanded', String(open));
    if (!sidebarEl) return;
    const wasInside = sidebarEl.contains(document.activeElement);
    // The closed phone drawer is inert (its links would be invisible Tab stops).
    sidebarEl.inert = phone.matches && !open;
    // While the phone drawer is open, the page under its scrim cannot take focus (Esc or the scrim closes it).
    for (const region of document.querySelectorAll('#main, .panel, .rail')) region.inert = phone.matches && open;
    if (phone.matches && open && !wasInside) sidebarEl.querySelector('a[href], button:not([disabled])')?.focus();
    if (phone.matches && !open && wasInside && menuButton) menuButton.focus();
  }
  new MutationObserver(syncDrawer).observe(document.body, { attributes: true, attributeFilter: ['class'] });
  window.matchMedia(PHONE).addEventListener('change', syncDrawer);

  // F6 / Shift+F6 move between regions; captured before the terminal and the browser view, which
  // keep every other key (WCAG 2.1.2: no keyboard trap).
  window.addEventListener('keydown', (event) => {
    if (event.key !== 'F6' || event.altKey || event.metaKey || event.ctrlKey) return;
    event.preventDefault();
    event.stopPropagation();
    focusNextRegion(event.shiftKey);
  }, true);

  // New task (UI-023): ⌘⇧O / Ctrl+Shift+O anywhere — browsers keep ⌘N for a new window — and ⌘N
  // on macOS where the browser passes it (installed app, full screen). Ctrl+N stays with the
  // terminal. Captured before the terminal so xterm never sees it.
  window.addEventListener('keydown', (event) => {
    if (!shell || event.altKey || event.repeat) return;
    const key = (event.key || '').toLowerCase();
    const mod = IS_MAC ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
    if (!mod || !((event.shiftKey && key === 'o') || (IS_MAC && !event.shiftKey && key === 'n'))) return;
    event.preventDefault();
    event.stopPropagation();
    openNewTask();
  }, true);

  // All sandboxes (UI-045): ⌘⇧H / Ctrl+Shift+H anywhere, like the logo and the All sandboxes tab.
  // Captured before the terminal and the editor, as the new-task shortcut is.
  window.addEventListener('keydown', (event) => {
    if (!shell || event.altKey || event.repeat || !event.shiftKey) return;
    const mod = IS_MAC ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
    if (!mod || (event.key || '').toLowerCase() !== 'h') return;
    event.preventDefault();
    event.stopPropagation();
    document.body.classList.remove('drawer-open');
    if (currentLocation().path !== '/') navigate('/');
  }, true);

  // The shortcut sheet (UI-029): ⌘/ on macOS, Ctrl+/ elsewhere — except in the terminal, where
  // Ctrl+/ belongs to the shell (undo in readline and emacs).
  window.addEventListener('keydown', (event) => {
    if (!shell || event.altKey || event.shiftKey || event.repeat || event.key !== '/') return;
    if (IS_MAC ? !(event.metaKey && !event.ctrlKey) : !(event.ctrlKey && !event.metaKey)) return;
    if (!IS_MAC && event.target instanceof Element && event.target.closest('.xterm')) return;
    event.preventDefault();
    event.stopPropagation();
    showShortcuts();
  }, true);

  /** The sandbox in view, else the one used last, else the first. */
  function defaultSandbox() {
    return currentSandbox || store.sandboxById(readJson(LAST_SANDBOX_KEY, null)) || store.state.sandboxes[0] || null;
  }

  /** Opens a sandbox's composer (the one in view, or used last) and puts the cursor in the prompt. */
  function openNewTask(target) {
    document.body.classList.remove('drawer-open');
    const sandbox = target || defaultSandbox();
    // While onboarding runs, the sandbox's page is its onboarding task (SPEC-111): go there
    // directly, not through the page that would open it again.
    const onboarding = sandbox && sandbox.onboarding === 'preparing' && !store.tasksError(sandbox.id)
      ? (store.tasksOf(sandbox.id) || []).find((t) => t.origin === 'onboarding') : null;
    if (onboarding) {
      const taskPath = `/s/${encodeURIComponent(sandbox.slug)}/t/${onboarding.number}`;
      if (currentLocation().path !== taskPath) navigate(taskPath);
      return;
    }
    const path = sandbox ? `/s/${encodeURIComponent(sandbox.slug)}` : '/';
    if (currentLocation().path !== path) navigate(path);
    setTimeout(() => {
      // A sandbox that awaits onboarding shows no prompt: its action takes the keyboard (SPEC-096).
      const gated = document.querySelector('.onboarding-gate:not([hidden]) .btn');
      if (gated) { gated.focus(); return; }
      const prompt = document.querySelector('.composer-prompt');
      if (prompt && !prompt.disabled) prompt.focus();
    }, 0);
  }

  return { start };
}

/** Yulius inside a frame (its own browser panel, or a page that frames it): show a notice and
 *  run nothing — no session, no sockets (BRW-008). The server forbids framing too. */
function refuseFramed(root) {
  setChildren(root, h('main', { class: 'framed-notice' },
    h('h1', null, `${BRAND.name} cannot run inside a frame`),
    h('p', null, `${BRAND.name} does not open inside its own browser or any other page. Open it in a tab of your own browser.`)));
}

async function boot() {
  if (window.__yuliusFramed) {
    refuseFramed(document.getElementById('app'));
    return;
  }
  applyTheme();
  let transport = realTransport();
  if (MOCK) {
    const mock = await import('./mock.js');
    transport = mock.createMockTransport(params);
  }
  initRouter({ hash: MOCK });
  const root = document.getElementById('app');
  createApp(root, transport).start();
  // Brand lab (lab.js): a development aid, loaded only when the URL asks for it.
  if (params.has('lab')) import('./lab.js').then((lab) => lab.startLab(params.get('lab'))).catch((error) => console.error('brand lab', error));
}

boot().catch((error) => {
  console.error(error);
  const root = document.getElementById('app');
  if (root) root.textContent = `${BRAND.name} failed to start: ${error.message}`;
});
