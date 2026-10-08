// Yulius web UI entry point: boot (setup → sign-in → workspace), route dispatch and
// composition of the global layout. Plain ES modules, no build step (SPEC-002 ADR-01).
//
// Module map: api.js (HTTP client), router.js (routes), store.js (state + event stream),
// termsocket.js (WebSocket protocols), terminal.js (xterm view), layout.js (tab row,
// sidebar, status), panels.js + browser.js (right rail), taskview.js (task + composer),
// actions.js (task actions, dialogs), pages/*.js (pages), mock.js (?mock=1 backend).

import { createApi, realTransport, enc } from './api.js';
import { initRouter, onRoute, navigate, currentLocation, refreshRoute, parseAppLink, href } from './router.js';
import { createStore, readJson, writeJson } from './store.js';
import { h, setChildren, toast, errorToast, placeholder, icon, isLocalUrl } from './dom.js';
import { createTabRow, createSidebar } from './layout.js';
import { createPanelHost } from './panels.js';
import { mountTaskView, mountComposer } from './taskview.js';
import { openSandboxDialog } from './actions.js';
import { mountSetup, mountSignIn } from './pages/auth.js';
import { mountTasksPage, mountArchivedPage, mountDevicePage } from './pages/tasks.js';
import { mountGitPage } from './pages/git.js';
import { mountWorkflowsPage } from './pages/workflows.js';
import { mountSkillsPage } from './pages/skills.js';
import { mountAutomationsPage } from './pages/automations.js';
import { mountMcpPage } from './pages/mcp.js';
import { mountDashboardPage } from './pages/dashboard.js';
import { mountUsersPage, mountAccountsPage, mountGithubPage } from './pages/admin.js';
import { mountSettingsPage } from './pages/settings.js';

const params = new URLSearchParams(window.location.search);
const MOCK = params.get('mock') === '1';
const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '');
const LAST_SANDBOX_KEY = 'yulius.lastSandbox';

const SANDBOX_PAGES = {
  tasks: (ctx, sandbox) => mountTasksPage(ctx, sandbox),
  git: (ctx, sandbox) => mountGitPage(ctx, sandbox),
  workflows: (ctx, sandbox) => mountWorkflowsPage(ctx, sandbox),
  skills: (ctx, sandbox, route) => mountSkillsPage(ctx, sandbox, route),
  automations: (ctx, sandbox) => mountAutomationsPage(ctx, sandbox),
  mcp: (ctx, sandbox, route) => mountMcpPage(ctx, sandbox, route),
  dashboard: (ctx, sandbox) => mountDashboardPage(ctx, sandbox),
};

const ADMIN_PAGES = {
  users: mountUsersPage,
  accounts: mountAccountsPage,
  github: mountGithubPage,
};

function createApp(root, transport) {
  let expiredHandled = false;
  const api = createApi(transport, { onUnauthenticated: () => sessionExpired() });
  const store = createStore(api);
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
    onTaskClosed,
    signOut,
    isSuperadmin: () => Boolean(store.state.me && store.state.me.user && store.state.me.user.is_superadmin),
    isInstanceAdmin: () => Boolean(store.state.me && store.state.me.user && (store.state.me.user.is_superadmin || store.state.me.user.is_instance_admin)),
  };

  // ------------------------------------------------------------ boot

  async function start() {
    setChildren(root, h('div', { class: 'auth' }, placeholder('Loading Yulius…', 'loading')));
    let setup;
    try {
      setup = await api.get('/api/setup/status', { allow401: true });
    } catch (error) {
      fatal(error);
      return;
    }
    if (setup && setup.setup_required) {
      renderSetup();
      return;
    }
    try {
      const me = await api.get('/api/me', { allow401: true });
      await signedIn(me);
    } catch (error) {
      if (error.status === 401) renderSignIn();
      else fatal(error);
    }
  }

  function fatal(error) {
    setChildren(root, h('div', { class: 'auth' }, h('div', { class: 'auth-card' },
      h('div', { class: 'auth-logo' }, 'Yulius'),
      h('div', { class: 'banner banner-danger' }, icon('warn'), h('span', { class: 'banner-text' }, `The server is not reachable: ${error.message}`)),
      h('button', { type: 'button', class: 'btn btn-primary', onClick: start }, 'Try again'))));
  }

  function renderSetup() {
    if (currentLocation().path !== '/setup') navigate('/setup', { replace: true });
    setChildren(root, mountSetup(ctx, { onDone: (me) => signedIn(me, '/') }).el);
  }

  function renderSignIn() {
    const loc = currentLocation();
    const next = loc.path === '/signin' ? loc.query.get('next') || '/' : loc.full;
    const error = loc.query.get('error');
    navigate(`/signin?next=${encodeURIComponent(next)}${error ? `&error=${encodeURIComponent(error)}` : ''}`, { replace: true });
    setChildren(root, mountSignIn(ctx, { next, error, onDone: (me, target) => signedIn(me, target) }).el);
  }

  async function signedIn(me, next) {
    expiredHandled = false;
    store.state.me = me;
    api.setCsrf(me && me.csrf_token);
    try {
      await store.loadSandboxes();
    } catch (error) {
      errorToast(error, 'Could not load sandboxes');
    }
    buildShell();
    store.startEvents();
    const user = (me && me.user) || {};
    if (user.is_superadmin && !readJson('yulius.passwordAck', false)) {
      ctx.notifications.add({ kind: 'warn', title: 'Your superadmin password cannot be recovered', detail: 'Store it in several safe places.', key: 'notice:password', once: true });
      toast('Your superadmin password cannot be recovered. Store it in several safe places.', {
        kind: 'warn',
        timeout: 0,
        action: { label: 'I have stored it', run: () => writeJson('yulius.passwordAck', true) },
      });
    }
    if (user.is_superadmin && me.instance && me.instance.github_configured === false) {
      ctx.notifications.add({ kind: 'info', title: 'Connect this instance to GitHub', detail: 'Needed to create and import projects.', route: '/admin/github', key: 'notice:github', once: true });
      toast('Connect this instance to GitHub to create and import projects.', { kind: 'info', timeout: 12000, action: { label: 'Connect GitHub', run: () => navigate('/admin/github') } });
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
    toast('Your session ended. Please sign in again.', { kind: 'warn' });
    renderSignIn();
  }

  async function signOut() {
    try {
      await api.post('/api/auth/logout');
    } catch (error) {
      if (error.status !== 401) { errorToast(error, 'Sign out failed'); return; }
    }
    teardown();
    navigate('/signin', { replace: true });
    renderSignIn();
  }

  function teardown() {
    store.stopEvents();
    if (centerView && centerView.destroy) centerView.destroy();
    centerView = null;
    if (shell) shell.panels.setContext(null, null);
    if (ctx.notifications) ctx.notifications.destroy();
    ctx.notifications = null;
    shell = null;
    store.state.me = null;
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
    setChildren(root, appEl, backdrop);
    ctx.toggleRail = () => panels.setRailVisible(!panels.railVisible);
    ctx.railVisible = () => panels.railVisible;
    ctx.setRailVisible = (visible) => panels.setRailVisible(visible);
    panels.mounted();
    shell = { el: appEl, tabRow, sidebar, center, panels };
    tabRow.renderMe();

    unsubscribers.push(
      store.on('task', (task) => {
        if (centerView && centerView.key === `task:${task.id}` && centerView.update) {
          centerView.update(task);
          currentTask = { ...currentTask, ...task };
        }
        updateTitle();
      }),
      store.on('task-removed', (taskId) => {
        if (centerView && centerView.key === `task:${taskId}` && currentSandbox) {
          toast('This task was archived or removed.', { kind: 'info' });
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
          errorToast(error, 'Could not refresh');
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

  function updateTitle() {
    let attention = 0;
    for (const sandbox of store.state.sandboxes) attention += (sandbox.counts && sandbox.counts.needs_attention) || 0;
    const parts = [];
    if (currentTask) parts.push(`#${currentTask.number} ${currentTask.title}`);
    if (currentSandbox) parts.push(currentSandbox.name);
    parts.push('Yulius');
    document.title = `${attention ? `(${attention}) ` : ''}${parts.join(' · ')}`;
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

  function message(key, text, extra) {
    setCenter(key, () => ({ el: h('div', { class: 'page' }, h('div', { class: 'page-inner' }, h('div', { class: 'card placeholder' }, text, extra || null))) }));
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

  /** Links from the terminal, messages and pages (UI-011, BRW-004). */
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

  async function ensureTasks(sandbox) {
    if (store.tasksOf(sandbox.id) !== null) return;
    try {
      await store.loadTasks(sandbox.id);
    } catch (error) {
      errorToast(error, 'Could not load tasks');
    }
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

    if (route.name === 'home') {
      const sandboxes = store.state.sandboxes;
      const last = store.sandboxById(readJson(LAST_SANDBOX_KEY, null));
      const target = last || sandboxes[0];
      if (target) { navigate(`/s/${encodeURIComponent(target.slug)}`, { replace: true }); return; }
      currentSandbox = null;
      currentTask = null;
      shell.el.classList.add('no-sandbox');
      shell.tabRow.update(null);
      shell.sidebar.update(route, null);
      shell.panels.setContext(null, null);
      setCenter('home', () => homeView());
      updateTitle();
      return;
    }
    shell.el.classList.remove('no-sandbox');

    let sandbox = currentSandbox;
    if (route.slug) {
      sandbox = store.sandboxBySlug(route.slug);
      if (!sandbox) {
        try { await store.loadSandboxes(); } catch { /* reported below */ }
        sandbox = store.sandboxBySlug(route.slug);
      }
      if (currentRoute !== route) return;
      if (!sandbox) {
        currentSandbox = null;
        shell.tabRow.update(null);
        shell.sidebar.update(route, null);
        shell.panels.setContext(null, null);
        message(`missing:${route.slug}`, 'This sandbox does not exist or you do not have access to its GitHub repository.');
        return;
      }
      writeJson(LAST_SANDBOX_KEY, sandbox.id);
    } else if (sandbox) {
      sandbox = store.sandboxById(sandbox.id) || store.state.sandboxes[0] || null;
    } else {
      sandbox = store.sandboxById(readJson(LAST_SANDBOX_KEY, null)) || store.state.sandboxes[0] || null;
    }
    currentSandbox = sandbox;
    shell.tabRow.update(route.slug && sandbox ? sandbox.slug : null);
    shell.sidebar.update(route, sandbox);
    if (sandbox) ensureTasks(sandbox).then(() => { if (currentRoute === route && shell) shell.sidebar.refresh(); });

    let task = null;
    switch (route.name) {
      case 'task': {
        await ensureTasks(sandbox);
        if (currentRoute !== route) return;
        task = await findTaskByNumber(sandbox, route.number);
        if (currentRoute !== route) return;
        if (!task) { message(`missing-task:${sandbox.id}:${route.number}`, `Task #${route.number} was not found in ${sandbox.name}.`); break; }
        setCenter(`task:${task.id}`, () => mountTaskView(ctx, sandbox, task), { paper: false });
        break;
      }
      case 'composer':
        setCenter(`composer:${sandbox.id}`, () => mountComposer(ctx, sandbox));
        break;
      case 'page': {
        const factory = SANDBOX_PAGES[route.page];
        setCenter(`${route.page}:${sandbox.id}:${route.query.toString()}`, () => factory(ctx, sandbox, route));
        break;
      }
      case 'archived':
        setCenter('archived', () => mountArchivedPage(ctx));
        break;
      case 'settings':
        setCenter('settings', () => mountSettingsPage(ctx));
        break;
      case 'device':
        setCenter(`device:${route.query.toString()}`, () => mountDevicePage(ctx, route));
        break;
      case 'admin':
        setCenter(`admin:${route.page}`, () => ADMIN_PAGES[route.page](ctx, route));
        break;
      default:
        message('notfound', 'Page not found.', h('div', { style: { marginTop: '10px' } }, h('a', { href: href('/'), dataset: { route: '/' } }, 'Go to your workspace')));
    }
    currentTask = task;
    shell.panels.setContext(sandbox, task);
    if (route.name === 'composer' && route.browserTab !== undefined && sandbox) {
      shell.panels.open('browser', { tabId: route.browserTab });
      navigate(`/s/${encodeURIComponent(sandbox.slug)}`, { replace: true });
    }
    updateTitle();
  }

  function homeView() {
    const me = store.state.me || {};
    const user = me.user || {};
    const instance = me.instance || {};
    const notes = [];
    if (user.is_superadmin && instance.github_configured === false) {
      notes.push(h('div', { class: 'banner banner-info' }, icon('globe'), h('span', { class: 'banner-text' }, 'First connect this instance to a GitHub organization or account.'), h('a', { class: 'btn btn-sm', href: href('/admin/github'), dataset: { route: '/admin/github' } }, 'Connect GitHub')));
    }
    if (user.github_linked === false) {
      notes.push(h('div', { class: 'banner' }, icon('lock'), h('span', { class: 'banner-text' }, 'Project content needs a GitHub-linked account. Sign in with GitHub to see repositories you can access.'), h('a', { class: 'btn btn-sm', href: '/api/auth/github/start?next=/' }, 'Sign in with GitHub')));
    }
    return {
      el: h('div', { class: 'composer' }, h('div', { class: 'composer-card' },
        h('h1', { class: 'composer-title' }, 'No sandboxes yet'),
        h('p', { class: 'text-2' }, 'A sandbox is one GitHub repository with its own tasks, browser and editor. Import an existing repository or create a new project from a template.'),
        notes,
        h('div', { class: 'row-wrap' },
          h('button', { type: 'button', class: 'btn btn-primary', onClick: () => openSandboxDialog(ctx, 'import') }, 'Import repository'),
          h('button', { type: 'button', class: 'btn', onClick: () => openSandboxDialog(ctx, 'create') }, 'Create project')))),
    };
  }

  onRoute((route) => {
    handleRoute(route).catch((error) => {
      console.error(error);
      errorToast(error, 'Navigation failed');
    });
  });

  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') document.body.classList.remove('drawer-open');
  });

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

  /** Opens the current sandbox's composer and puts the cursor in the prompt. */
  function openNewTask() {
    document.body.classList.remove('drawer-open');
    const sandbox = currentSandbox || store.state.sandboxes[0] || null;
    const path = sandbox ? `/s/${encodeURIComponent(sandbox.slug)}` : '/';
    if (currentLocation().path !== path) navigate(path);
    setTimeout(() => {
      const prompt = document.querySelector('.composer-prompt');
      if (prompt && !prompt.disabled) prompt.focus();
    }, 0);
  }

  return { start };
}

async function boot() {
  let transport = realTransport();
  if (MOCK) {
    const mock = await import('./mock.js');
    transport = mock.createMockTransport(params);
  }
  initRouter({ hash: MOCK });
  const root = document.getElementById('app');
  createApp(root, transport).start();
}

boot().catch((error) => {
  console.error(error);
  const root = document.getElementById('app');
  if (root) root.textContent = `Yulius failed to start: ${error.message}`;
});
