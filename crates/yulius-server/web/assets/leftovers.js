// Leftovers (superadmin, Settings → Instance; SPEC-215 LFT-011, ADR-0170): what is on the server's
// disk that no record owns or that its owner no longer needs. Each item has its path, kind, size,
// owner and why it is a leftover, and whether Yulius deletes it by itself. The card shows the
// disk's usage and the reclaimable total. Refresh rescans. Delete takes the selected items, and
// Delete all safe takes every item Yulius deletes by itself, each after a confirmation that names
// the size. Deletions run in the background, and the card follows them until the list updates.

import { h, icon, setChildren, errorToast, confirmDialog, badge, skeleton } from './dom.js';
import { relLong, bytes, plural } from './format.js';

const API = '/api/admin/leftovers';
/** While a scan or a deletion runs, the card asks again this often (ms). */
const POLL_MS = 1500;

const KINDS = {
  'removed-worktrees': 'Worktrees of a removed sandbox',
  'removed-ci': 'CI checkouts of a removed sandbox',
  'removed-browser': 'Browser profile of a removed sandbox',
  'removed-cd': 'Deploy files of a removed sandbox',
  'removed-display': 'Display of a removed sandbox',
  'removed-system': 'System layer of a removed sandbox',
  'removed-runtime': 'Holder state of a removed sandbox',
  'removed-sandbox': 'Everything else of a removed sandbox',
  'task-cache': 'Build cache',
  'orphan-worktree': 'Worktree no task owns',
  'git-metadata': 'Git record of a gone worktree',
  'stale-ci-checkout': 'Unused CI checkout',
  'old-draft': 'Old draft attachment',
  'ended-run': 'Run directory',
  'orphan-transcripts': 'Transcripts of a deleted task',
  'stale-smoke': 'Smoke test sandbox',
  unowned: 'Directory no sandbox owns',
  'unused-base': 'Unused sandbox base',
  'host-docker': 'Host Docker data',
};

function size(item) {
  return item.bytes === null || item.bytes === undefined ? 'unknown' : bytes(item.bytes);
}

function owner(item) {
  if (!item.sandbox) return h('span', { class: 'muted' }, 'instance');
  return h('span', null, item.sandbox, item.task ? h('span', { class: 'muted' }, ` · #${item.task}`) : null);
}

/** The disk's usage and what can be reclaimed. */
function diskLine(view) {
  const parts = [];
  if (view.disk_total) {
    const pct = Math.round((view.disk_used / view.disk_total) * 100);
    parts.push(h('span', null, h('strong', null, `${pct}%`), ` of the disk used (${bytes(view.disk_used)} of ${bytes(view.disk_total)})`));
  }
  parts.push(h('span', null, h('strong', null, bytes(view.total_bytes)), ' reclaimable, ', h('strong', null, bytes(view.safe_bytes)), ' of it safe to delete automatically'));
  return h('div', { class: 'leftovers-disk small' }, parts);
}

/** The running or last scan or deletion. */
function jobLine(job) {
  if (!job) return null;
  if (job.running) {
    const what = job.action === 'delete' ? `Deleting ${job.done} of ${job.total}` : 'Scanning the disk';
    return h('div', { class: 'leftovers-job row', role: 'status' }, h('span', { class: 'spinner', 'aria-hidden': 'true' }),
      h('span', { class: 'small' }, what, job.action === 'delete' && job.freed ? ` · ${bytes(job.freed)} freed` : '', ` · started by ${job.actor}`));
  }
  if (job.action !== 'delete') return null;
  const notes = [];
  for (const kept of job.kept || []) notes.push(h('li', null, `Kept: ${kept}`));
  for (const failed of job.failed || []) notes.push(h('li', null, `Failed: ${failed}`));
  return h('div', { class: ['banner', job.failed && job.failed.length ? 'banner-warn' : 'banner-info', 'leftovers-result'], role: 'status' },
    icon(job.failed && job.failed.length ? 'warn' : 'check', { size: 16 }),
    h('div', { class: 'banner-text' },
      h('span', null, `${plural(job.deleted, 'item')} deleted, ${bytes(job.freed)} freed ${job.ended_at ? relLong(job.ended_at) : ''} by ${job.actor}.`),
      notes.length ? h('ul', null, notes) : null));
}

/** The card. */
export function leftoversCard(ctx) {
  const body = h('div', { class: 'settings-body leftovers-body' }, skeleton('Reading the leftovers…', 3, { block: true }));
  const refreshButton = h('button', { type: 'button', class: 'btn btn-sm', onClick: () => rescan() }, icon('refresh', { size: 14 }), 'Refresh');
  const card = h('section', { class: 'settings-section leftovers', 'aria-labelledby': 'leftovers-title' },
    h('div', { class: 'settings-head' }, icon('trash', { size: 16 }), h('h2', { id: 'leftovers-title' }, 'Leftovers'),
      h('span', { class: 'muted small' }, 'files on this server nothing needs any more'), h('span', { class: 'spacer' }), refreshButton),
    body);
  const selected = new Set();
  let view = null;
  let timer = null;
  let gone = false;

  function deleteButtons() {
    const items = view ? view.items : [];
    const chosen = items.filter((i) => selected.has(i.id) && i.deletable);
    const chosenBytes = chosen.reduce((sum, i) => sum + (i.bytes || 0), 0);
    const safe = items.filter((i) => i.auto);
    const busy = Boolean(view && view.job && view.job.running);
    return h('div', { class: 'row leftovers-actions' },
      h('button', { type: 'button', class: 'btn btn-sm btn-danger', disabled: busy || !chosen.length, onClick: () => removeSelected(chosen, chosenBytes) },
        icon('trash', { size: 14 }), chosen.length ? `Delete ${plural(chosen.length, 'item')} (${bytes(chosenBytes)})` : 'Delete'),
      h('button', { type: 'button', class: 'btn btn-sm', disabled: busy || !safe.length, onClick: () => removeAllSafe(safe) },
        'Delete all safe', safe.length ? ` (${bytes(view.safe_bytes)})` : ''));
  }

  function table(items) {
    if (!items.length) {
      return h('p', { class: 'muted small' }, view.scanned_at ? 'Nothing left over: every file on this server has an owner that needs it.' : 'Not scanned yet. Refresh scans the disk.');
    }
    const rows = items.map((item) => {
      const check = h('input', {
        type: 'checkbox', checked: selected.has(item.id), disabled: !item.deletable,
        'aria-label': `Select ${item.path}`,
        onChange: (event) => { if (event.target.checked) selected.add(item.id); else selected.delete(item.id); render(); },
      });
      return h('tr', { dataset: { kind: item.kind } },
        h('td', { class: 'leftovers-check' }, check),
        h('td', { class: 'leftovers-what' },
          h('div', null, KINDS[item.kind] || item.kind, ' ', item.auto ? badge('safe', 'ok') : item.deletable ? badge('needs you', 'warn') : badge('report only', '')),
          h('div', { class: 'mono xs leftovers-path' }, item.path),
          h('div', { class: 'muted xs' }, item.reason)),
        h('td', { class: 'hide-phone leftovers-owner' }, owner(item)),
        h('td', { class: 'num' }, size(item)));
    });
    return h('div', { class: 'table-wrap' },
      h('table', { class: 'table leftovers-table' },
        h('thead', null, h('tr', null,
          h('th', null, h('span', { class: 'sr-only' }, 'Select')), h('th', null, 'Leftover'), h('th', { class: 'hide-phone' }, 'Owner'), h('th', { class: 'num' }, 'Size'))),
        h('tbody', null, rows)));
  }

  function render() {
    if (!view) return;
    for (const id of [...selected]) if (!view.items.some((i) => i.id === id)) selected.delete(id);
    const running = Boolean(view.job && view.job.running);
    refreshButton.disabled = running;
    setChildren(body,
      diskLine(view),
      h('p', { class: 'muted xs' }, [
        view.scanned_at ? `Scanned ${relLong(view.scanned_at)}.` : null,
        view.next_auto_at ? `Yulius deletes the safe ones by itself, next ${relLong(view.next_auto_at)}.` : 'Yulius deletes the safe ones by itself after every start and every hour.',
      ].filter(Boolean).join(' ')),
      jobLine(view.job),
      deleteButtons(),
      table(view.items));
    clearTimeout(timer);
    if (!gone && running) timer = setTimeout(refresh, POLL_MS);
  }

  async function refresh() {
    try {
      view = await ctx.api.get(API);
      render();
    } catch (error) {
      setChildren(body, h('p', { class: 'muted small' }, 'Couldn’t read the leftovers.'));
      errorToast(error, 'Couldn’t read the leftovers');
    }
  }

  async function rescan() {
    refreshButton.disabled = true;
    try {
      view = await ctx.api.post(`${API}/scan`, {});
      render();
    } catch (error) {
      refreshButton.disabled = false;
      errorToast(error, 'Couldn’t scan for leftovers');
    }
  }

  async function start(request) {
    try {
      view = await ctx.api.post(`${API}/delete`, request);
      selected.clear();
      render();
    } catch (error) {
      errorToast(error, 'Couldn’t delete the leftovers');
    }
  }

  async function removeSelected(chosen, chosenBytes) {
    const manual = chosen.filter((i) => !i.auto);
    const ok = await confirmDialog({
      title: `Delete ${plural(chosen.length, 'leftover')} (${bytes(chosenBytes)})?`,
      message: manual.length
        ? `${plural(manual.length, 'of them needs', 'of them need')} a person: ${manual.map((i) => KINDS[i.kind] || i.kind).join(', ')}. Deleted files cannot be restored. Anything in use by then is kept.`
        : 'Deleted files cannot be restored. Anything in use by then is kept.',
      confirmLabel: `Delete ${bytes(chosenBytes)}`,
      danger: true,
    });
    if (ok) await start({ ids: chosen.map((i) => i.id) });
  }

  async function removeAllSafe(safe) {
    const ok = await confirmDialog({
      title: `Delete every safe leftover (${bytes(view.safe_bytes)})?`,
      message: `${plural(safe.length, 'item')} that Yulius would delete by itself. Items that need you stay. Anything in use by then is kept.`,
      confirmLabel: `Delete ${bytes(view.safe_bytes)}`,
      danger: true,
    });
    if (ok) await start({ all_safe: true });
  }

  refresh();
  return {
    el: card,
    destroy() {
      gone = true;
      clearTimeout(timer);
    },
  };
}
