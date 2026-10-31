// Connection troubles (SPEC-361, ADR-0246): a warning pill in the tab row while the egress proxy
// refuses a sandbox at its share, a sandbox's relay toward Yulius is full, or the proxy itself is
// full, for the people who can view that sandbox (the proxy's own for everyone). Read from
// GET /api/connections at start, on `connections.updated`, on `resync` (events were lost) and when
// the event stream reconnects. Its title gives the reason, since when, and whose the connections
// are; with one trouble whose busiest holder is a task, it leads there. Each trouble lands once in
// the notification center: the ids already notified are kept apart from its list, so clearing the
// list does not notify the open ones again.

import { h, icon, setChildren } from './dom.js';
import { BRAND } from './brand.js';
import { dateTime } from './format.js';
import { readJson, writeJson } from './store.js';

/** How a holder is named in a sentence: "task #12's run", "a CI job", "app". */
function holderName(holder) {
  return holder.kind === 'task' ? `${holder.label}’s run` : holder.label;
}

/** The pill's text, title and route for one trouble. Its counts are those of the refusal that
 * began it or of the last read; the server publishes no refusal within a trouble. */
export function connectionAlertText(trouble) {
  const alert = describe(trouble);
  const since = trouble.since_ms ? ` Since ${dateTime(new Date(trouble.since_ms).toISOString())}.` : '';
  return { ...alert, title: `${alert.title}${since}` };
}

function describe(trouble) {
  const sandbox = trouble.sandbox_name || 'a sandbox';
  const top = (trouble.holders || [])[0];
  const route = top && top.kind === 'task' && trouble.sandbox_name
    ? `/s/${encodeURIComponent(trouble.sandbox_name)}/t/${top.task_number}`
    : null;
  if (trouble.limit === 'share') {
    const whose = top ? ` ${holderName(top)} holds ${top.held} of them${top.task_title ? ` (“${top.task_title}”)` : ''}.` : '';
    return {
      text: `Proxy share full: ${sandbox}`,
      title: `${sandbox} is at its egress proxy share: its processes hold all of its ${trouble.max} connections, and new downloads of its runs are refused (503).${whose} End that run’s leftover processes (browsers, test jobs), or stop the task.`,
      route,
    };
  }
  if (trouble.limit === 'relay') {
    return {
      text: `Relay full: ${sandbox}`,
      title: `${sandbox}’s relay toward ${BRAND.name} is full (${trouble.held} of ${trouble.max} connections): its runs’ calls to ${BRAND.name} (tools, hooks) and downloads fail. End leftover processes in the sandbox.`,
      route: null,
    };
  }
  const busiest = (trouble.holders || []).map((holder) => `${holderName(holder)} (${holder.held})`).join(', ');
  return {
    text: 'Egress proxy full',
    title: `The egress proxy is full (${trouble.held} of ${trouble.max} connections): downloads of every sandbox may be refused.${busiest ? ` The busiest: ${busiest}.` : ''}`,
    route: null,
  };
}

/** The pill: `{ el, refresh, destroy }`. */
export function createConnectionAlert(ctx) {
  const el = h('div', { class: 'update-banner update-warn connection-alert', role: 'status', 'aria-live': 'polite', hidden: true });
  const user = (ctx.store.state.me && ctx.store.state.me.user) || {};
  // The troubles already notified, pruned to the open ones (CHR-006).
  const notifiedKey = `yulius.connections.notified.${user.id || user.login || 'anonymous'}`;
  let destroyed = false;
  let reads = 0;
  let shown = 0;

  function notify(troubles, alerts) {
    const notified = new Set(readJson(notifiedKey, []));
    troubles.forEach((trouble, at) => {
      if (notified.has(trouble.id) || !ctx.notifications) return;
      const alert = alerts[at];
      ctx.notifications.add({ kind: 'warn', title: alert.text, detail: alert.title, route: alert.route, key: `connections:${trouble.id}`, once: true });
    });
    writeJson(notifiedKey, troubles.map((trouble) => trouble.id));
  }

  function render(view) {
    const troubles = (view && view.troubles) || [];
    const alerts = troubles.map((trouble) => connectionAlertText(trouble));
    notify(troubles, alerts);
    if (!troubles.length) { el.hidden = true; setChildren(el); return; }
    const first = alerts[0];
    const more = alerts.length > 1 ? ` +${alerts.length - 1}` : '';
    const title = alerts.map((alert) => alert.title).join('\n');
    const label = first.route && alerts.length === 1
      ? h('a', { class: 'update-text', href: first.route, dataset: { route: first.route }, title }, `${first.text}${more}`)
      : h('span', { class: 'update-text', title }, `${first.text}${more}`);
    setChildren(el, icon('warn', { size: 14 }), label, h('span', { class: 'sr-only' }, title));
    el.hidden = false;
  }

  async function refresh() {
    const asked = ++reads;
    try {
      const view = await ctx.api.get('/api/connections');
      if (destroyed || asked < shown) return;
      shown = asked;
      render(view);
    } catch {
      // Unreachable for a moment: the next event or reconnect reads it again.
    }
  }

  const offs = [
    ctx.store.on('connections', () => refresh()),
    ctx.store.on('resync', () => refresh()),
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
