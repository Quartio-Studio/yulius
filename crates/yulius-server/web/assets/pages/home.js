// Home (SPEC-001 UI-045, F 2026-10-10): the default landing view at `/`. Every sandbox the user can
// see, in the tab order, as a card with its counts, last activity and two quick actions; with no
// sandbox, the first steps: a playground from a project folder or a template, which needs no
// GitHub (SPEC-099), then importing or creating the organisation's repositories.

import { h, icon, badge, setChildren, keepFocus, coalesce, skeleton } from '../dom.js';
import { relLong, dateTime } from '../format.js';
import { allows } from '../status.js';
import { href } from '../router.js';
import { BRAND } from '../brand.js';
import { openSandboxDialog } from '../actions.js';
import { folderDropZone } from '../playground.js';

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
          sandbox.playground
            ? h('span', { class: 'home-card-repo', title: 'Its Git stays on this server; nothing is pushed to GitHub' }, icon('folder', { size: 12 }), h('span', { class: 'ellipsis' }, 'Playground · local Git on this server'))
            : repo ? h('span', { class: 'home-card-repo', title: repo }, icon('github', { size: 12 }), h('span', { class: 'ellipsis' }, repo)) : null),
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
   * The header's ways to add a sandbox, secondary to the cards: a playground needs no GitHub, so it
   * is always offered (SPEC-099); Import and Create need it, with Connect GitHub (superadmin) first.
   */
  function addActions(needsGithub, user) {
    return [
      h('button', { type: 'button', class: 'btn', onClick: () => openSandboxDialog(ctx, 'playground') }, 'New playground'),
      needsGithub && user.is_superadmin ? h('a', { class: 'btn', href: href('/admin/github'), dataset: { route: '/admin/github' } }, 'Connect GitHub') : null,
      h('button', { type: 'button', class: 'btn', disabled: needsGithub, title: needsGithub ? 'Connect GitHub first' : null, onClick: () => openSandboxDialog(ctx, 'import') }, 'Import repository'),
      h('button', { type: 'button', class: 'btn', disabled: needsGithub, title: needsGithub ? 'Connect GitHub first' : null, onClick: () => openSandboxDialog(ctx, 'create') }, 'Create from template'),
    ];
  }

  const opened = (sandbox) => {
    store.upsertSandbox(sandbox);
    ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}`);
  };

  /**
   * No sandbox yet (UX2-17, SPEC-099): try Yulius on your own code at once, in a playground from a
   * dropped folder or a template; the organisation's GitHub comes second and can wait.
   */
  function emptyView(needsGithub, user) {
    const github = [];
    if (needsGithub) {
      github.push(h('p', { class: 'text-2' }, user.is_superadmin
        ? 'When your team is ready, connect GitHub to import or create your organisation’s repositories. Playgrounds keep working.'
        : 'When your team is ready, a superadmin connects GitHub to bring in your organisation’s repositories. Playgrounds keep working.'));
      if (user.is_superadmin) github.push(h('div', { class: 'row-wrap' }, h('a', { class: 'btn', href: href('/admin/github'), dataset: { route: '/admin/github' } }, icon('github', { size: 14 }), 'Connect GitHub')));
    } else {
      // The superadmin owns the instance and sees every sandbox without GitHub (ACC-012).
      if (user.github_linked === false && !user.is_superadmin) {
        github.push(h('div', { class: 'banner' }, icon('lock'), h('span', { class: 'banner-text' }, 'Sandbox content needs a GitHub-linked account. Sign in with GitHub to see the repositories you can access.'), h('a', { class: 'btn btn-sm', href: '/api/auth/github/start?next=/' }, 'Sign in with GitHub')));
      }
      github.push(h('p', { class: 'text-2' }, 'Or work on your organisation’s repositories: each becomes a sandbox with its own tasks, browser and editor.'));
      github.push(h('div', { class: 'row-wrap' },
        h('button', { type: 'button', class: 'btn', onClick: () => openSandboxDialog(ctx, 'import') }, 'Import repository'),
        h('button', { type: 'button', class: 'btn', onClick: () => openSandboxDialog(ctx, 'create') }, 'Create from template')));
    }
    return h('div', { class: 'composer' }, h('div', { class: 'composer-card' },
      h('h1', { class: 'composer-title' }, `Try ${BRAND.name} on your own code`),
      h('p', { class: 'text-2' }, 'Drop a project folder or a repository here, and start tasks in it a minute later. It becomes a playground: its Git stays on this server and nothing is pushed anywhere, so nothing needs to be connected first.'),
      folderDropZone(ctx, { onCreated: opened }),
      h('div', { class: 'row-wrap' },
        h('span', { class: 'muted small' }, 'No code at hand?'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => openSandboxDialog(ctx, 'playground') }, 'Start from a template')),
      h('section', { class: 'stack-sm home-github', 'aria-label': 'GitHub' },
        h('h2', { class: 'home-github-title' }, 'Your organisation on GitHub'),
        github)));
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
        h('span', { class: 'page-sub' }, 'one repository each, with its tasks, browser and editor'),
        h('div', { class: 'page-actions' }, addActions(needsGithub, user))),
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
