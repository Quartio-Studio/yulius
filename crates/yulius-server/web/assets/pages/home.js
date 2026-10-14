// Home (SPEC-001 UI-045, F 2026-10-10): the default landing view at `/`. Every sandbox the user can
// see, in the tab order, as a card with its counts, last activity and two quick actions; with no
// sandbox, the first steps (import a repository or create one from a template).

import { h, icon, badge, setChildren, keepFocus, coalesce, skeleton } from '../dom.js';
import { relLong, dateTime } from '../format.js';
import { allows } from '../status.js';
import { href } from '../router.js';
import { openSandboxDialog } from '../actions.js';

/** At most this many task lists load at once for the cards' last activity. */
const LOAD_CONCURRENCY = 3;

/** Mount the home screen. */
export function mountHomePage(ctx) {
  const store = ctx.store;
  const el = h('div', { class: 'home' });
  let alive = true;

  /** The newest change among a sandbox's tasks (its last activity), or null with none. */
  function lastActivity(tasks) {
    let newest = null;
    for (const task of tasks) if (task.updated_at && (!newest || String(task.updated_at) > newest)) newest = String(task.updated_at);
    return newest;
  }

  function count(iconName, value, one, many, tone) {
    return h('li', { class: ['home-count', value > 0 && tone && `home-count-${tone}`] },
      icon(iconName, { size: 14 }),
      h('strong', null, String(value)),
      h('span', null, value === 1 ? one : many));
  }

  function card(sandbox) {
    const counts = sandbox.counts || {};
    const path = `/s/${encodeURIComponent(sandbox.slug)}`;
    const repo = sandbox.repo ? `${sandbox.repo.owner}/${sandbox.repo.name}` : null;
    const nameId = `home-card-${sandbox.id}`;
    const tasks = store.tasksOf(sandbox.id);
    const newest = tasks ? lastActivity(tasks) : null;
    let activity;
    // One bar per card; the grid says it is busy, not each card (SPEC-092).
    if (tasks === null) activity = store.tasksError(sandbox.id) ? 'Last activity unknown' : skeleton('Loading activity…', 1, { announce: false });
    else if (!newest) activity = 'No tasks yet';
    else activity = `Last activity ${relLong(newest)}`;
    const write = allows(sandbox.access, 'write');
    const preparing = sandbox.onboarding === 'pending' || sandbox.onboarding === 'preparing';
    const article = h('article', { class: 'home-card', 'aria-labelledby': nameId, dataset: { id: sandbox.id } },
      h('div', { class: 'home-card-head' },
        h('span', { class: 'item-icon', 'aria-hidden': 'true' }, icon('folder', { size: 18 })),
        h('div', { class: 'home-card-titles' },
          h('h2', { class: 'home-card-name', id: nameId }, h('a', { href: href(path), dataset: { route: path } }, sandbox.name)),
          repo ? h('span', { class: 'home-card-repo', title: repo }, icon('github', { size: 12 }), h('span', { class: 'ellipsis' }, repo)) : null),
        h('div', { class: 'home-card-badges' },
          sandbox.access === 'read' ? badge('Read-only', '', 'GitHub gives you read access: you can follow tasks but not start them') : null,
          preparing ? badge(sandbox.onboarding === 'preparing' ? 'Preparing' : 'Needs onboarding', 'warn', sandbox.onboarding === 'preparing' ? 'The repository is being prepared for agents' : 'Start onboarding on the sandbox page') : null)),
      h('ul', { class: 'home-counts', 'aria-label': `Tasks in ${sandbox.name}` },
        count('bell', counts.needs_attention || 0, 'needs you', 'need you', 'attention'),
        count('pulse', counts.running || 0, 'working', 'working', 'running'),
        count('clock', counts.queued || 0, 'queued', 'queued', '')),
      h('div', { class: 'home-activity', title: newest ? dateTime(newest) : null }, activity),
      h('div', { class: 'home-card-actions' },
        h('a', { class: 'btn btn-sm', href: href(path), dataset: { route: path }, 'aria-label': `Open ${sandbox.name}` }, 'Open'),
        h('button', {
          type: 'button',
          class: 'btn btn-sm btn-primary',
          disabled: !write,
          title: write ? null : 'Requires write access',
          'aria-label': `New task in ${sandbox.name}`,
          onClick: () => ctx.openNewTask && ctx.openNewTask(sandbox),
        }, icon('plus', { size: 14 }), 'New task')));
    // A click anywhere on the card opens the sandbox (its links and buttons keep their own action).
    article.addEventListener('click', (event) => {
      if (event.defaultPrevented || event.target.closest('a, button')) return;
      ctx.navigate(path);
    });
    return article;
  }

  /**
   * Import and Create; without GitHub, Connect GitHub first (superadmin) and both disabled. With
   * sandboxes listed they are secondary: the cards' actions come first.
   */
  function addActions(needsGithub, user, { primary = true } = {}) {
    return [
      needsGithub && user.is_superadmin ? h('a', { class: 'btn btn-primary', href: href('/admin/github'), dataset: { route: '/admin/github' } }, 'Connect GitHub') : null,
      h('button', { type: 'button', class: ['btn', primary && !needsGithub && 'btn-primary'], disabled: needsGithub, title: needsGithub ? 'Connect GitHub first' : null, onClick: () => openSandboxDialog(ctx, 'import') }, 'Import repository'),
      h('button', { type: 'button', class: 'btn', disabled: needsGithub, title: needsGithub ? 'Connect GitHub first' : null, onClick: () => openSandboxDialog(ctx, 'create') }, 'Create from template'),
    ];
  }

  /** No sandbox yet: what a sandbox is, and the way to the first one (UX2-17). */
  function emptyView(needsGithub, user) {
    const notes = [];
    if (needsGithub && !user.is_superadmin) {
      notes.push(h('div', { class: 'banner banner-info' }, icon('globe'), h('span', { class: 'banner-text' }, 'This instance is not connected to GitHub yet. Ask a superadmin to connect it.')));
    }
    // The superadmin owns the instance and sees every sandbox without GitHub (ACC-012).
    if (user.github_linked === false && !user.is_superadmin) {
      notes.push(h('div', { class: 'banner' }, icon('lock'), h('span', { class: 'banner-text' }, 'Sandbox content needs a GitHub-linked account. Sign in with GitHub to see the repositories you can access.'), h('a', { class: 'btn btn-sm', href: '/api/auth/github/start?next=/' }, 'Sign in with GitHub')));
    }
    return h('div', { class: 'composer' }, h('div', { class: 'composer-card' },
      h('h1', { class: 'composer-title' }, 'No sandboxes yet'),
      h('p', { class: 'text-2' }, 'A sandbox is one GitHub repository with its own tasks, browser and editor. Import an existing repository or create a new one from a template.'),
      notes,
      h('div', { class: 'row-wrap' }, addActions(needsGithub, user)),
      needsGithub && user.is_superadmin ? h('p', { class: 'muted small' }, 'Connect this instance to a GitHub organization or account first; then import or create repositories.') : null));
  }

  function render() {
    if (!alive) return;
    const me = store.state.me || {};
    const user = me.user || {};
    // Without GitHub nothing can be imported or created: that step is the primary action (UX2-17).
    const needsGithub = Boolean(me.instance && me.instance.github_configured === false);
    const sandboxes = store.state.sandboxes;
    if (!sandboxes.length) {
      keepFocus(el, () => setChildren(el, emptyView(needsGithub, user)));
      return;
    }
    // The subtitle says what the page is for, as on every page (#147); each card has its counts.
    keepFocus(el, () => setChildren(el, h('div', { class: 'page' }, h('div', { class: 'page-inner' },
      h('div', { class: 'page-head home-head' },
        h('h1', { class: 'page-title' }, 'Sandboxes'),
        h('span', { class: 'page-sub' }, 'one GitHub repository each, with its tasks, browser and editor'),
        h('div', { class: 'page-actions' }, addActions(needsGithub, user, { primary: false }))),
      h('div', { class: 'home-grid', 'aria-busy': String(sandboxes.some((s) => store.tasksOf(s.id) === null && !store.tasksError(s.id))) }, sandboxes.map(card))))));
  }

  // Last activity comes from each sandbox's task list: load the ones not loaded yet, a few at a time
  // (the store keeps them current from the event stream afterwards).
  const loading = new Set(); // sandbox ids whose list is on its way
  async function loadActivity() {
    const queue = store.state.sandboxes.filter((s) => store.tasksOf(s.id) === null && !store.tasksError(s.id) && !loading.has(s.id)).map((s) => s.id);
    queue.forEach((id) => loading.add(id));
    const worker = async () => {
      while (queue.length) {
        const id = queue.shift();
        try { if (alive) await store.loadTasks(id); } catch { /* the card says "unknown" */ } finally { loading.delete(id); }
      }
    };
    await Promise.all(Array.from({ length: Math.min(LOAD_CONCURRENCY, queue.length) }, worker));
  }

  const repaint = coalesce(render, { minInterval: 250 });
  const offs = [
    store.on('sandboxes', () => { repaint(); loadActivity(); }),
    store.on('tasks', repaint),
  ];
  // Relative times stay true while the page is open (one bounded timer).
  const timer = setInterval(repaint, 30000);
  render();

  return {
    el,
    start() { loadActivity(); },
    destroy() {
      alive = false;
      offs.forEach((off) => off());
      repaint.cancel();
      clearInterval(timer);
    },
  };
}
