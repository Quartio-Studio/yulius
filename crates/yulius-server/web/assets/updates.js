// Instance updates (SPEC-001 OPS-006..OPS-009, SPEC-002 ADR-18): one calm banner — a pill in the
// tab row, so it never covers a page — and the superadmin's notification. Everyone sees an update while it pauses tasks and restarts, and a
// "Reload" once this page runs older code than the server; superadmins also see "Update
// available" (dismissible per commit) and a failed or rolled-back update. Built from
// GET /api/update, refreshed on the `update.updated` event and, while an update runs, every few
// seconds (the event stream drops during the restart). The superadmin's Update button sits beside
// the account button in the sidebar foot, whatever the pill shows (SPEC-133).

import { h, icon, setChildren, toast, errorToast, confirmDialog } from './dom.js';
import { readJson, writeJson } from './store.js';
import { BRAND } from './brand.js';

const DISMISSED_KEY = 'yulius.update.dismissed';
/** While an update runs: how often the view is read again. */
const POLL_MS = 3000;

/** The states of a run that has ended (the instance page shows its outcome). */
export const UPDATE_OUTCOMES = new Set(['succeeded', 'failed', 'rolled_back', 'cancelled']);

/** The route of the instance page (updates and the Jev key). */
export const INSTANCE_ROUTE = '/admin/instance';

/** A short sentence for an update's phase, for everyone. */
export function phaseText(phase, run) {
  switch (phase) {
    case 'building': {
      const p = run && run.progress;
      const pct = p && p.total ? ` ${Math.floor((p.done / p.total) * 100)}%` : '';
      return `Building the update${pct}. Tasks keep working.`;
    }
    case 'pausing': return `Updating ${BRAND.name}: tasks pause at their next step.`;
    case 'restarting': return `${BRAND.name} is restarting for an update. Tasks continue right after.`;
    case 'resuming': return 'Updated. Resuming tasks…';
    default: return '';
  }
}

/**
 * Asks the superadmin to confirm the offered update, then starts it (the instance page's "Update
 * now…" and the sidebar's Update button). Resolves once it started, was refused or was cancelled.
 */
export async function confirmAndStartUpdate(ctx, available) {
  const ok = await confirmDialog({
    title: `Update ${BRAND.name} to ${available.short}?`,
    message: h('div', { class: 'stack-sm' },
      h('p', null, available.summary),
      h('ol', { class: 'update-confirm-steps' },
        h('li', null, 'The new version is built on this server. Tasks keep working; this takes several minutes.'),
        h('li', null, 'Each task then pauses at its next step, and the server restarts (about a minute). The agents don’t notice; terminals stay open.'),
        h('li', null, 'Tasks continue one by one. If the new version fails its check, the previous version and its database come back.'))),
    confirmLabel: `Update to ${available.short}`,
  });
  if (!ok) return;
  try {
    await ctx.api.post('/api/admin/update', { commit: available.commit, confirm: true });
    toast('Update started. Tasks keep working while it builds.', { kind: 'ok' });
    await ctx.updates.refresh();
  } catch (error) {
    errorToast(error, 'Couldn’t start the update');
  }
}

/**
 * The superadmin's Update button for the sidebar foot (SPEC-133 UPB-001..UPB-006): "Update" when
 * main moved on (one click to the confirmation), "Updating" while one runs, otherwise a quiet
 * "Version and updates". Everyone else gets a hidden element.
 */
export function createUpdateButton(ctx, updates) {
  const el = h('button', { type: 'button', class: 'update-btn', hidden: true, onClick: () => activate() });
  let view = updates.view();

  function state() {
    if (!view) return 'idle';
    if (view.phase || (view.run && !UPDATE_OUTCOMES.has(view.run.state))) return 'running';
    return view.available ? 'offer' : 'idle';
  }

  function render() {
    if (!ctx.isSuperadmin()) { el.hidden = true; setChildren(el); return; }
    const now = state();
    el.hidden = false;
    el.className = `update-btn update-btn-${now}`;
    if (now === 'offer') {
      const available = view.available;
      el.setAttribute('aria-label', `Update ${BRAND.name} to ${available.short}`);
      el.title = view.helper_installed === false
        ? 'Update available, but it needs the update helper on this server: see the instance page'
        : `Update available: ${available.summary}`;
      setChildren(el, icon('download', { size: 14 }), h('span', { class: 'update-btn-text' }, 'Update'));
    } else if (now === 'running') {
      const text = phaseText(view.phase, view.run) || `Updating ${BRAND.name}`;
      el.setAttribute('aria-label', `Updating: ${text}`);
      el.title = text;
      setChildren(el, h('span', { class: 'spinner', 'aria-hidden': 'true' }), h('span', { class: 'update-btn-text' }, 'Updating'));
    } else {
      el.setAttribute('aria-label', 'Version and updates');
      el.title = 'Version and updates';
      setChildren(el, icon('download', { size: 16 }));
    }
  }

  function activate() {
    if (state() === 'offer' && view.helper_installed !== false) confirmAndStartUpdate(ctx, view.available);
    else ctx.navigate(INSTANCE_ROUTE);
  }

  const off = updates.onChange((fresh) => { view = fresh; render(); });
  render();
  return { el, render, destroy: off };
}

/** The banner and the view, shared with the instance page (ctx.updates). */
export function createUpdateBanner(ctx) {
  const el = h('div', { class: 'update-banner', role: 'status', 'aria-live': 'polite', hidden: true });
  const listeners = new Set();
  let view = null;
  let loadedCommit; // the build this page's code came from (the first answer)
  let timer = null;
  let destroyed = false;
  let quiet = false; // the instance page shows the same, in full
  const superadmin = () => ctx.isSuperadmin();

  function dismissed() {
    const value = readJson(DISMISSED_KEY, '');
    return typeof value === 'string' ? value : '';
  }

  function hide() {
    el.hidden = true;
    setChildren(el);
  }

  /** Shows the pill: a mark, the text (a link to the instance page when `route`), actions. */
  function show(tone, mark, text, { route = null, title = null, actions = [] } = {}) {
    el.className = `update-banner update-${tone}`;
    const label = route
      ? h('a', { class: 'update-text', href: route, dataset: { route }, title: title || null }, text)
      : h('span', { class: 'update-text', title: title || null }, text);
    setChildren(el, mark, label, ...actions.filter(Boolean));
    el.hidden = false;
  }

  const close = (key, label) => h('button', {
    type: 'button',
    class: 'update-close',
    'aria-label': label,
    title: label,
    onClick: () => { writeJson(DISMISSED_KEY, key); render(); },
  }, icon('x', { size: 12 }));

  function render() {
    // While the event stream is down the connection banner speaks (it knows about a restart).
    if (!view || quiet || ctx.store.state.eventsStatus !== 'open') { hide(); return; }
    const run = view.run;
    const phase = view.phase;
    const current = view.current && view.current.commit;
    if (phase && (phase !== 'building' || superadmin())) {
      const text = phaseText(phase, run);
      show('busy', h('span', { class: 'spinner', 'aria-hidden': 'true' }), text, { route: superadmin() ? INSTANCE_ROUTE : null, title: text });
      return;
    }
    if (loadedCommit && current && loadedCommit !== current) {
      show('info', icon('refresh', { size: 14 }), `${BRAND.name} was updated. Reload to use the new version.`, {
        actions: [h('button', { type: 'button', class: 'update-action', onClick: () => window.location.reload() }, 'Reload')],
      });
      return;
    }
    if (!superadmin()) { hide(); return; }
    if (run && (run.state === 'failed' || run.state === 'rolled_back') && dismissed() !== `run:${run.id}`) {
      const text = run.state === 'rolled_back' ? 'The update was rolled back; the previous version runs.' : 'The update failed; nothing changed.';
      show('warn', icon('warn', { size: 14 }), text, { route: INSTANCE_ROUTE, title: run.error || text, actions: [close(`run:${run.id}`, 'Dismiss')] });
      return;
    }
    const available = view.available;
    if (available && dismissed() !== available.commit) {
      const text = `Update available: ${available.summary}`;
      show('info', icon('download', { size: 14 }), text, { route: INSTANCE_ROUTE, title: `${text} (review and update)`, actions: [close(available.commit, 'Dismiss until the next update')] });
      return;
    }
    hide();
  }

  function notify() {
    if (!view || !superadmin() || !ctx.notifications) return;
    const available = view.available;
    if (available) {
      ctx.notifications.add({ kind: 'info', title: 'Update available', detail: available.summary, route: INSTANCE_ROUTE, key: `update:${available.commit}`, once: true });
    }
    const run = view.run;
    if (run && (run.state === 'failed' || run.state === 'rolled_back')) {
      ctx.notifications.add({ kind: 'error', title: run.state === 'rolled_back' ? 'The update was rolled back' : 'The update failed', detail: run.error || null, route: INSTANCE_ROUTE, key: `update-run:${run.id}`, once: true });
    }
  }

  function schedule() {
    clearTimeout(timer);
    timer = null;
    if (!destroyed && view && view.phase) timer = setTimeout(refresh, POLL_MS);
  }

  async function refresh() {
    try {
      const fresh = await ctx.api.get('/api/update');
      if (destroyed || !fresh) return;
      view = fresh;
      if (loadedCommit === undefined) loadedCommit = (view.current && view.current.commit) || null;
      render();
      notify();
      for (const fn of [...listeners]) {
        try { fn(view); } catch (error) { console.error('update listener failed', error); }
      }
    } catch {
      // Unreachable while restarting: try again shortly.
    }
    schedule();
  }

  const offs = [
    ctx.store.on('update', () => refresh()),
    // The stream reconnects after a restart: read the outcome at once.
    ctx.store.on('events-status', (status) => { if (status === 'open' && view) refresh(); else render(); }),
  ];
  refresh();

  return {
    el,
    refresh,
    /** The last view (or null). */
    view: () => view,
    /** Whether the server is restarting for an update (the connection banner says so). */
    restarting: () => Boolean(view && (view.phase === 'restarting' || view.phase === 'pausing')),
    /** Hides the banner while the instance page (which says it all) is open. */
    setQuiet(on) { quiet = Boolean(on); render(); },
    /** Calls fn(view) on every new view; returns the unsubscribe. */
    onChange(fn) { listeners.add(fn); return () => listeners.delete(fn); },
    destroy() {
      destroyed = true;
      clearTimeout(timer);
      offs.forEach((off) => off());
      listeners.clear();
    },
  };
}
