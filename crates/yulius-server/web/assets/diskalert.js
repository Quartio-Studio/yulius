// The server's disk running low (SPEC-276 RDA-007, ADR-0205): a warning pill in the tab row, for
// everyone, while GET /api/disk says `low` (from 85 % used) or `critical` (from 95 %). Read at
// start, on the `disk.updated` event and when the event stream reconnects. Its title says what
// Yulius does about it; the superadmin's pill links to Settings → Instance, where the Leftovers
// card lists what can still go.

import { h, icon, setChildren } from './dom.js';
import { bytes } from './format.js';
import { BRAND } from './brand.js';
import { INSTANCE_ROUTE } from './updates.js';

/** The pill's text and title for a disk view, or null when the disk is fine. */
export function diskAlertText(view) {
  if (!view || (view.level !== 'low' && view.level !== 'critical')) return null;
  const known = view.total > 0 && view.used !== null && view.used !== undefined;
  const pct = known ? Math.min(100, Math.round((view.used / view.total) * 100)) : null;
  const free = known ? bytes(Math.max(0, view.total - view.used)) : null;
  const text = view.level === 'critical'
    ? `Disk almost full${pct === null ? '' : `: ${pct}%`}`
    : `Disk ${pct === null ? 'low' : `${pct}% full`}`;
  const doing = view.level === 'critical'
    ? `${BRAND.name} is deleting old build caches and the caches of idle tasks. Builds may fail until space is back.`
    : `${BRAND.name} is deleting build caches nobody used recently.`;
  const title = `The server’s disk is ${pct === null ? 'nearly full' : `${pct}% used`}${free ? ` (${free} free)` : ''}. ${doing} Removed sandboxes whose files were kept can be deleted in Settings → Instance → Leftovers.`;
  return { text, title, critical: view.level === 'critical' };
}

/** The pill: `{ el, refresh, destroy }`. */
export function createDiskAlert(ctx) {
  const el = h('div', { class: 'update-banner disk-alert', role: 'status', 'aria-live': 'polite', hidden: true });
  let destroyed = false;
  let reads = 0;
  let shown = 0;

  function render(view) {
    const alert = diskAlertText(view);
    if (!alert) { el.hidden = true; setChildren(el); return; }
    el.className = `update-banner update-warn disk-alert${alert.critical ? ' disk-critical' : ''}`;
    const label = ctx.isSuperadmin()
      ? h('a', { class: 'update-text', href: INSTANCE_ROUTE, dataset: { route: INSTANCE_ROUTE }, title: alert.title }, alert.text)
      : h('span', { class: 'update-text', title: alert.title, tabindex: '0' }, alert.text);
    setChildren(el, icon('warn', { size: 14 }), label, h('span', { class: 'sr-only' }, alert.title));
    el.hidden = false;
  }

  async function refresh() {
    const asked = ++reads;
    try {
      const view = await ctx.api.get('/api/disk');
      if (destroyed || asked < shown) return;
      shown = asked;
      render(view);
    } catch {
      // Unreachable for a moment: the next event or reconnect reads it again.
    }
  }

  const offs = [
    ctx.store.on('disk', () => refresh()),
    ctx.store.on('events-status', (status) => { if (status === 'open') refresh(); }),
  ];
  refresh();

  return {
    el,
    refresh,
    destroy() {
      destroyed = true;
      offs.forEach((off) => off());
    },
  };
}
