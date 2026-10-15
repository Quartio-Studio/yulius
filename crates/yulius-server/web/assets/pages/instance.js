// Instance (superadmin; SPEC-001 OPS-006..OPS-009, ADM-002): the version this server runs, the
// update offered from main (checked when the page opens, SPEC-149) with its new commits, "Update
// now" and the update's progress (building → pausing tasks → restarting → resuming, with a
// failure or rollback said plainly), and the Jev key.

import { h, icon, setChildren, errorToast, confirmDialog, badge, skeleton } from '../dom.js';
import { relLong, dateTime } from '../format.js';
import { BRAND } from '../brand.js';
import { pageFrame } from './common.js';
import { jevSettingsCard } from '../jevkey.js';
import { confirmAndStartUpdate, UPDATE_OUTCOMES as OUTCOMES } from '../updates.js';

const REPO_URL = 'https://github.com/Quartio-Studio/yulius';
const STEPS = [
  ['building', 'Build', 'The new version is built on this server while tasks keep working.'],
  ['pausing', 'Pause tasks', 'Tasks pause at once, mid-step. The agents don’t notice.'],
  ['restarting', 'Restart', 'Installs and starts the new version, after a database backup.'],
  ['resuming', 'Resume', 'Tasks continue one by one.'],
];

function short(commit) {
  return commit ? commit.slice(0, 7) : 'unknown';
}

function commitLink(sha, text) {
  return h('a', { class: 'mono', href: `${REPO_URL}/commit/${sha}`, target: '_blank', rel: 'noopener noreferrer' }, text || short(sha));
}

function progressBar(progress, label) {
  const total = progress && progress.total;
  const done = progress ? progress.done : 0;
  const pct = total ? Math.min(100, Math.floor((done / total) * 100)) : null;
  return h('div', { class: 'update-progress' },
    h('div', { class: ['update-track', pct === null && 'indeterminate'], role: 'progressbar', 'aria-label': label, 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': pct === null ? undefined : pct },
      h('div', { class: 'update-fill', style: pct === null ? null : { width: `${pct}%` } })),
    h('span', { class: 'muted xs update-count' }, total ? `${done} of ${total}` : ''));
}

/** The stepper of a running update. */
function stepper(run) {
  const index = STEPS.findIndex(([state]) => state === run.state);
  return h('ol', { class: 'update-steps' }, STEPS.map(([state, title, text], i) => {
    const status = i < index ? 'done' : i === index ? 'current' : 'next';
    let extra = null;
    if (status === 'current' && state === 'building') extra = progressBar(run.progress, 'Build progress');
    if (status === 'current' && state === 'resuming') extra = progressBar(run.progress, 'Tasks resumed');
    if (status === 'current' && state === 'pausing' && run.tasks) {
      const t = run.tasks;
      extra = h('div', { class: 'stack-sm' },
        progressBar({ done: t.paused, total: Math.max(t.total, 1) }, 'Tasks paused'),
        t.waiting.length
          ? h('div', { class: 'muted xs' }, 'Still in a turn: ', t.waiting.map((w, n) => h('span', null, n ? ', ' : '',
            h('a', { href: `/s/${encodeURIComponent(w.sandbox)}/t/${w.number}`, dataset: { route: `/s/${encodeURIComponent(w.sandbox)}/t/${w.number}` } }, `#${w.number} ${w.title}`))))
          : null,
        // Only a task that could not be frozen waits for its turn to end (SPEC-160 OPS-011).
        t.deadline && t.waiting.length ? h('div', { class: 'muted xs' }, `If a task is still in a turn at ${dateTime(t.deadline)}, the update is cancelled and nothing changes.`) : null);
    }
    return h('li', { class: ['update-step', `update-step-${status}`], 'aria-current': status === 'current' ? 'step' : null },
      h('span', { class: 'update-step-mark', 'aria-hidden': 'true' }, status === 'done' ? icon('check', { size: 12 }) : status === 'current' ? h('span', { class: 'spinner' }) : String(i + 1)),
      h('div', { class: 'update-step-body' },
        h('div', { class: 'update-step-title' }, title, status === 'current' ? h('span', { class: 'sr-only' }, ' (in progress)') : null),
        h('div', { class: 'muted xs' }, status === 'current' && run.message ? run.message : text),
        extra));
  }));
}

/** The outcome of the last update. */
function outcome(run) {
  const tone = { succeeded: 'ok', failed: 'danger', rolled_back: 'danger', cancelled: '' }[run.state];
  const title = {
    succeeded: `Updated to ${short(run.target.commit)}`,
    failed: 'The last update failed',
    rolled_back: 'The last update was rolled back',
    cancelled: 'The last update was cancelled',
  }[run.state];
  const when = run.finished_at ? relLong(run.finished_at) : '';
  return h('div', { class: ['update-outcome', tone && `update-outcome-${tone}`] },
    h('div', { class: 'row' }, icon(run.state === 'succeeded' ? 'check' : run.state === 'cancelled' ? 'clock' : 'warn', { size: 16 }),
      h('strong', null, title), h('span', { class: 'muted small' }, [when, run.requested_by ? `started by ${run.requested_by}` : null].filter(Boolean).join(' · '))),
    run.state === 'rolled_back' ? h('p', { class: 'small' }, 'The previous version and its database were restored and run again. Tasks continued.') : null,
    run.error ? h('p', { class: 'small' }, run.error) : null,
    run.log_tail && run.log_tail.length
      ? h('details', null, h('summary', { class: 'muted small' }, 'Last lines of the helper’s output'), h('pre', { class: 'update-log mono' }, run.log_tail.join('\n')))
      : null);
}

/** Superadmin: version, updates and the Jev key. */
export function mountInstancePage(ctx) {
  if (!ctx.isSuperadmin()) {
    const frame = pageFrame({ title: 'Instance' });
    frame.body.append(h('div', { class: 'card placeholder' }, 'Only the superadmin manages updates and the Jev key.'));
    return { el: frame.el };
  }
  const frame = pageFrame({ title: 'Instance', sub: 'version, updates and the Jev key' });
  const updatesBody = h('div', { class: 'settings-body instance-body' }, skeleton('Loading updates…', 3, { block: true }));
  const checkButton = h('button', { type: 'button', class: 'btn btn-sm', onClick: () => checkNow() }, icon('refresh', { size: 14 }), 'Check now');
  const updatesCard = h('section', { class: 'settings-section', 'aria-labelledby': 'updates-title' },
    h('div', { class: 'settings-head' }, icon('download', { size: 16 }), h('h2', { id: 'updates-title' }, 'Updates'), h('span', { class: 'muted small' }, 'from main of Quartio-Studio/yulius'), h('span', { class: 'spacer' }), checkButton),
    updatesBody);
  frame.body.append(updatesCard, jevSettingsCard(ctx));

  function render(view) {
    if (!view) return;
    const run = view.run;
    const running = run && !OUTCOMES.has(run.state);
    const check = view.check || {};
    const available = view.available;
    const parts = [];
    parts.push(h('div', { class: 'instance-version' },
      h('span', { class: 'muted small' }, 'This server runs'),
      view.current && view.current.commit ? commitLink(view.current.commit) : h('span', { class: 'mono' }, 'an unknown build'),
      running ? badge('updating', 'warn') : available ? badge('update available', 'accent') : check.relation === 'up_to_date' ? badge('up to date', 'ok') : null));
    if (check.error) {
      parts.push(h('div', { class: 'banner banner-warn', role: 'status' }, icon('warn', { size: 16 }), h('span', { class: 'banner-text' }, check.error)));
    } else if (check.checked_at) {
      const via = { app: 'with the GitHub App', token: 'with the configured read token', anonymous: 'without credentials' }[check.via] || '';
      const relation = check.relation === 'ahead' ? ' This build is newer than main.' : '';
      parts.push(h('p', { class: 'muted xs' }, `Checked ${relLong(check.checked_at)} ${via}.${relation}`));
    } else {
      parts.push(h('p', { class: 'muted xs' }, `${BRAND.name} checks main when this page opens and every hour.`));
    }
    if (view.helper_installed === false) {
      parts.push(h('div', { class: 'banner banner-info', role: 'note' }, icon('shield', { size: 16 }),
        h('span', { class: 'banner-text' }, 'Updates need the update helper on this server (yulius-update.path). The operations guide has the two unit files to install.')));
    }
    if (running) {
      parts.push(h('div', { class: 'update-run' },
        h('div', { class: 'row' }, h('strong', null, `Updating to ${short(run.target.commit)}`), h('span', { class: 'muted small' }, run.summary)),
        stepper(run),
        h('div', { class: 'row' },
          run.can_proceed && run.tasks && run.tasks.waiting.length
            ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => proceed() }, 'Continue without waiting')
            : null,
          run.can_cancel ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => cancel() }, 'Cancel update') : null)));
    } else if (available) {
      const shown = available.commits.slice(0, 12);
      parts.push(h('div', { class: 'update-offer' },
        h('div', { class: 'row' }, h('strong', null, `Update to ${available.short}`), commitLink(available.commit, 'on GitHub')),
        available.relation === 'unknown' ? h('p', { class: 'muted xs' }, 'This server’s build is unknown, so every recent commit of main is listed.') : null,
        available.relation === 'diverged' ? h('p', { class: 'muted xs' }, 'This server runs commits that are not on main; updating replaces them with main.') : null,
        h('ul', { class: 'update-commits' }, shown.map((c) => h('li', null,
          commitLink(c.sha), h('span', null, c.title), c.author ? h('span', { class: 'muted xs' }, c.author) : null))),
        available.count > shown.length ? h('p', { class: 'muted xs' }, `and ${available.count - shown.length} more`) : null,
        h('div', { class: 'row' },
          h('button', { type: 'button', class: 'btn btn-primary', disabled: view.helper_installed === false, onClick: () => confirmAndStartUpdate(ctx, available) }, icon('download', { size: 14 }), 'Update now…'))));
    }
    if (run && OUTCOMES.has(run.state)) parts.push(outcome(run));
    setChildren(updatesBody, ...parts);
  }

  /** Reads main now; `quiet` (the check on opening) keeps the last result if it fails (SPEC-149). */
  async function checkNow({ quiet = false } = {}) {
    checkButton.disabled = true;
    checkButton.classList.add('busy');
    try {
      await ctx.api.post('/api/admin/update/check', {});
      await ctx.updates.refresh();
    } catch (error) {
      if (!quiet) errorToast(error, 'Couldn’t check for updates');
    } finally {
      checkButton.disabled = false;
      checkButton.classList.remove('busy');
    }
  }

  async function cancel() {
    if (!(await confirmDialog({ title: 'Cancel the update?', message: 'Nothing is installed, and paused tasks continue at once.', confirmLabel: 'Cancel update', danger: true }))) return;
    try {
      await ctx.api.post('/api/admin/update/cancel', {});
      await ctx.updates.refresh();
    } catch (error) {
      errorToast(error, 'Couldn’t cancel the update');
    }
  }

  async function proceed() {
    const ok = await confirmDialog({
      title: 'Restart without waiting?',
      message: `Tasks still in a turn may see a short interruption of their ${BRAND.name} tools while the server restarts. The others stay paused as usual.`,
      confirmLabel: 'Restart now',
      danger: true,
    });
    if (!ok) return;
    try {
      await ctx.api.post('/api/admin/update/proceed', {});
      await ctx.updates.refresh();
    } catch (error) {
      errorToast(error, 'Couldn’t continue the update');
    }
  }

  const off = ctx.updates ? ctx.updates.onChange(render) : () => {};
  if (ctx.updates) {
    ctx.updates.setQuiet(true);
    if (ctx.updates.view()) render(ctx.updates.view());
    ctx.updates.refresh();
    // Opening the page checks main at once; the server reuses a check younger than 30 s.
    checkNow({ quiet: true });
  }
  return {
    el: frame.el,
    destroy() {
      off();
      if (ctx.updates) ctx.updates.setQuiet(false);
    },
  };
}
