// Sandbox Docker (superadmin, Settings → Instance; SPEC-203 HST-014, HST-015): whether isolated
// sandboxes get Docker on this server, why not and the fix in `yulius doctor`'s words, the base
// being rebuilt to pick up `dockerd` and when it will be ready, sandboxes running Docker installed
// in them instead of their base's, the last update's re-check, and the smoke test run from here
// with each check as it ends.

import { h, icon, setChildren, errorToast, badge, skeleton } from './dom.js';
import { relLong, dateTime } from './format.js';

const API = '/api/admin/sandbox-docker';
/** While a smoke test runs, the card asks again this often (ms); while a rebuild runs or waits,
 * less often; after a failed read, it tries again. */
const POLL_MS = 2000;
const REBUILD_POLL_MS = 30000;
const RETRY_MS = 10000;

function short(commit) {
  return commit ? commit.slice(0, 7) : 'unknown';
}

/** The base's rebuild: running or waiting for the host to settle, and its estimate. */
function rebuildBanner(rebuild) {
  if (!rebuild) return null;
  const parts = [];
  const ready = rebuild.ready_by
    ? `ready around ${dateTime(rebuild.ready_by)} (the last build took ${Math.max(1, Math.round((rebuild.last_build_secs || 0) / 60))} min)`
    : 'a build usually takes a few minutes';
  if (rebuild.building_since) {
    parts.push(`Rebuilding the sandbox base to pick up the host’s system, started ${relLong(rebuild.building_since)}: ${ready}. Sandboxes move to it when they next start.`);
  } else if (rebuild.pending_since) {
    parts.push(`The host changed ${relLong(rebuild.pending_since)}. The base is rebuilt once it has been unchanged for 10 minutes, at ${dateTime(rebuild.starts_at)}; then ${ready}.`);
  }
  if (rebuild.error) parts.push(`The last build failed: ${rebuild.error}`);
  if (!parts.length) return null;
  return h('div', { class: ['banner', rebuild.error ? 'banner-warn' : 'banner-info'], role: 'status' },
    icon(rebuild.error ? 'warn' : 'clock', { size: 16 }), h('span', { class: 'banner-text' }, parts.join(' ')));
}

/** The last root check of the host: an update's re-check, or `yulius host setup`/`check` since. */
function hostCheck(report) {
  if (!report) return null;
  const when = report.checked_at ? relLong(new Date(report.checked_at).toISOString()) : '';
  const head = `The last host check (yulius ${short(report.commit)}${when ? `, ${when}` : ''})`;
  if (report.ok) {
    return h('p', { class: 'muted small sandbox-docker-recheck' }, icon('check', { size: 14 }), ` ${head}: the host gives sandboxes Docker.`);
  }
  const problems = [];
  if (report.error) problems.push(report.error);
  for (const line of report.lines || []) {
    if (line.outcome === 'failed') problems.push(`${line.name}: ${line.detail}`);
  }
  return h('div', { class: 'banner banner-warn sandbox-docker-recheck', role: 'status' }, icon('warn', { size: 16 }),
    h('div', { class: 'banner-text' },
      h('strong', null, `${head} found that sandboxes may have lost Docker.`),
      h('ul', null, problems.map((p) => h('li', null, p)))));
}

/** Sandboxes on Docker installed in them. */
function ownEngines(view) {
  if (!view.own_engines || !view.own_engines.length) return null;
  return h('div', { class: 'stack-sm sandbox-docker-own' },
    h('p', { class: 'small' }, 'These sandboxes run Docker installed in them instead of their base’s:'),
    h('ul', { class: 'small' }, view.own_engines.map((o) => h('li', null,
      h('strong', null, o.name || o.sandbox), ' ', h('span', { class: 'mono muted' }, `/${o.dockerd}`)))),
    h('p', { class: 'muted xs' }, view.own_engine_fix.charAt(0).toUpperCase() + view.own_engine_fix.slice(1), '.'));
}

/** The smoke test: its state and each check as it ended. */
function smokeResult(smoke) {
  if (!smoke) return null;
  const running = smoke.state === 'running';
  const title = { running: 'Smoke check running', passed: 'Smoke check passed', failed: 'Smoke check failed' }[smoke.state] || smoke.state;
  const when = running ? `started ${relLong(smoke.started_at)} by ${smoke.started_by}` : `${relLong(smoke.finished_at)} · started by ${smoke.started_by}`;
  return h('div', { class: ['sandbox-docker-smoke', `sandbox-docker-smoke-${smoke.state}`] },
    h('div', { class: 'row' },
      running ? h('span', { class: 'spinner', 'aria-hidden': 'true' }) : icon(smoke.state === 'passed' ? 'check' : 'warn', { size: 16 }),
      h('strong', null, title), h('span', { class: 'muted small' }, when)),
    running ? h('p', { class: 'muted xs' }, 'A throwaway sandbox runs hello-world, a build and a two-service compose; it pulls images, so it can take a few minutes. Each check appears as it ends.') : null,
    h('ol', { class: 'sandbox-docker-checks', 'aria-label': 'Smoke checks', 'aria-live': 'polite' }, smoke.checks.map((c) => h('li', { class: c.ok ? 'ok' : 'failed' },
      icon(c.ok ? 'check' : 'x', { size: 14, label: c.ok ? 'passed' : 'failed' }),
      h('span', { class: 'sandbox-docker-check-name' }, c.name),
      c.detail ? h('span', { class: 'muted xs mono' }, c.detail) : null))),
    smoke.error ? h('p', { class: 'small' }, smoke.error) : null);
}

/** The card. */
export function sandboxDockerCard(ctx) {
  const body = h('div', { class: 'settings-body sandbox-docker-body' }, skeleton('Checking sandboxes’ Docker…', 2, { block: true }));
  const smokeButton = h('button', { type: 'button', class: 'btn btn-sm', onClick: () => runSmoke() }, icon('pulse', { size: 14 }), 'Run smoke check');
  const card = h('section', { class: 'settings-section sandbox-docker', 'aria-labelledby': 'sandbox-docker-title' },
    h('div', { class: 'settings-head' }, icon('cpu', { size: 16 }), h('h2', { id: 'sandbox-docker-title' }, 'Sandbox Docker'),
      h('span', { class: 'muted small' }, 'whether sandboxes get Docker'), h('span', { class: 'spacer' }), smokeButton),
    body);
  let timer = null;
  let gone = false;
  let polling = false;
  let failed = false;

  function render(view) {
    const running = view.smoke && view.smoke.state === 'running';
    smokeButton.disabled = running || !view.isolated;
    smokeButton.title = view.isolated ? '' : 'Sandboxes are not isolated on this server';
    const parts = [
      h('div', { class: 'row sandbox-docker-verdict' },
        view.ok ? badge('sandboxes get Docker', 'ok') : badge('no Docker in sandboxes', 'warn'),
        view.base ? h('span', { class: 'muted small' }, `base ${view.base.id}${view.base.dockerd ? '' : ' (no dockerd)'}`) : null),
      h('p', { class: 'small sandbox-docker-detail' }, view.detail),
      rebuildBanner(view.rebuild),
      ownEngines(view),
      hostCheck(view.host_check),
      smokeResult(view.smoke),
    ];
    setChildren(body, ...parts.filter(Boolean));
    clearTimeout(timer);
    const rebuilding = view.rebuild && (view.rebuild.building_since || view.rebuild.pending_since);
    polling = running || Boolean(rebuilding);
    if (!gone && polling) timer = setTimeout(refresh, running ? POLL_MS : REBUILD_POLL_MS);
  }

  async function refresh() {
    try {
      render(await ctx.api.get(API));
      failed = false;
    } catch (error) {
      setChildren(body, h('p', { class: 'muted small' }, 'Couldn’t read whether sandboxes get Docker.'));
      // One toast per outage; a smoke check or a rebuild in progress is asked again.
      if (!failed) errorToast(error, 'Couldn’t read sandboxes’ Docker');
      failed = true;
      clearTimeout(timer);
      if (!gone && polling) timer = setTimeout(refresh, RETRY_MS);
    }
  }

  async function runSmoke() {
    smokeButton.disabled = true;
    try {
      render(await ctx.api.post(`${API}/smoke`, {}));
    } catch (error) {
      smokeButton.disabled = false;
      errorToast(error, 'Couldn’t start the smoke check');
    }
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
