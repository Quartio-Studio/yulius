// Git page (GIT-001/002, DLV-001..005, UI-025): delivery at a glance, as a presentation layer.
// Agents merge, deploy and release as part of their workflow, governed by the delivery profile;
// this page only shows where things are. Top to bottom: how this project ships (one sentence from
// the profile), what is live and what is waiting to go live, what is stuck, where every task is on
// the way to production (a board with columns from the profile), and recent activity. Its only
// controls are harmless: refresh, links to GitHub, deploy output, a task's delivery details and,
// for administrators, the delivery profile. Reads degrade: Git/GitHub problems arrive as `notes`,
// not errors; check results the GitHub App may not read arrive as `checks_access`, one quiet line
// for instance administrators (#111).

import { enc } from '../api.js';
import { h, icon, setChildren, toast, iconButton, placeholder, statusMark, field, openModal, keepFocus, coalesce, loadError } from '../dom.js';
import { relShort, dateTime, plural } from '../format.js';
import { allows, taskInfo } from '../status.js';
import { href } from '../router.js';
import { openDeliveryDialog, notesBanner, checksAccessNote, openRunViewer, UNKNOWN_CHECKS } from '../actions.js';
import { pageFrame } from './common.js';
import { BRAND } from '../brand.js';

const LIVE_LIMIT = 5;
const ACTIVITY_LIMIT = 8;
const CHECK_TEXT = { success: 'checks passed', failure: 'checks failing', pending: 'checks running', unknown: 'checks unknown' };
const CHECK_ICON = { success: ['check', 'ok'], failure: ['x', 'danger'], pending: ['clock', 'warn'] };

/** Environments of a profile, split by where they run. */
function environments(profile) {
  const all = Array.isArray(profile.environments) ? profile.environments : [];
  return { integration: all.filter((e) => e.after === 'integration'), production: all.filter((e) => e.after === 'production') };
}

const ago = (iso) => { const rel = relShort(iso); return rel === 'now' ? 'just now' : `${rel} ago`; };
const listNames = (names) => (names.length < 2 ? names.join('') : `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`);

/** "How this project ships", in one or two plain sentences generated from the profile. */
export function profileSentence(profile) {
  const int = profile.integration_branch || 'the integration branch';
  const prod = profile.production_branch || 'production';
  const envs = environments(profile);
  const checks = profile.require_checks === false ? 'Checks don’t block merging.' : 'Merges wait for green checks.';
  const agents = 'Agents merge and release as part of their workflow; this page shows where things are.';
  if (profile.auto_promote) return `Merging a pull request into ${int} puts it live on ${prod} right away. ${checks} ${agents}`;
  let text = `Merged work collects on ${int}`;
  if (envs.integration.length) text += `, is checked on ${listNames(envs.integration.map((e) => e.name))}`;
  text += `, then is released to ${prod}`;
  if (envs.production.length) text += ` and verified on ${listNames(envs.production.map((e) => e.name))}`;
  return `${text}. ${checks} ${agents}`;
}

/** Board column of a delivery stage: built | review | waiting | live. */
function columnOf(delivery, profile) {
  const stage = delivery ? delivery.stage : 'none';
  if (stage === 'pr_open') return 'review';
  if (stage === 'merged_integration') return profile.auto_promote ? 'live' : 'waiting';
  if (stage === 'env_done') return environments(profile).integration.some((e) => e.name === delivery.environment) ? 'waiting' : 'live';
  if (stage === 'promoted' || stage === 'production_done') return 'live';
  return 'built';
}

/** Pull request chip: number and checks in Claude's colours (merged violet). */
function prChip(pr) {
  if (!pr) return null;
  const check = pr.state === 'open' ? CHECK_ICON[pr.checks] : null;
  const label = `Pull request #${pr.number} · ${pr.state}${pr.state === 'open' && CHECK_TEXT[pr.checks] ? ` · ${CHECK_TEXT[pr.checks]}` : ''}`;
  const title = pr.state === 'open' && pr.checks === 'unknown' ? `${label}. ${UNKNOWN_CHECKS}` : label;
  return h('a', { class: ['tl-pr-chip', pr.state === 'merged' && 'merged', pr.state === 'closed' && 'closed'], href: pr.url, target: '_blank', rel: 'noopener noreferrer', title, 'aria-label': label },
    icon('branch', { size: 12 }), `#${pr.number}`, check ? h('span', { class: `tone-${check[1]}` }, icon(check[0], { size: 12 })) : null);
}

// Each task's delivery, kept across visits to the page (PERF-09): read again only when its row in
// the overview or the sandbox's latest environment run changed, after 5 minutes, or after an action.
const DELIVERY_TTL_MS = 5 * 60_000;
const deliveryCache = new Map(); // task id → { sig, at, delivery }

/** Mount the Git page for a sandbox. */
export function mountGitPage(ctx, sandbox) {
  const admin = allows(sandbox.access, 'admin');
  const sb = () => ctx.store.sandboxById(sandbox.id) || sandbox;
  const repoUrl = sandbox.repo && sandbox.repo.url ? sandbox.repo.url : null;
  const frame = pageFrame({ title: 'Git', sub: 'where each task is on its way to production', actions: [
    iconButton('refresh', 'Refresh', () => load()),
    repoUrl ? h('a', { class: 'btn', href: `${repoUrl}/pulls`, target: '_blank', rel: 'noopener noreferrer' }, 'Pull requests', icon('external', { size: 12 })) : null,
    admin ? h('button', { type: 'button', class: 'btn', onClick: () => openProfileEditor(ctx, sb()) }, icon('gear', { size: 14 }), 'Delivery profile') : null,
  ] });
  const intro = h('div', { class: 'dl-intro' });
  const body = h('div', { class: 'stack dl' }, placeholder('Loading delivery…', 'loading'));
  frame.body.append(intro, body);
  let model = null;
  let showAllActivity = false;
  let showAllNeeds = false;
  const expanded = new Set();

  // Only the newest load renders, and none after the page is left; delivery reads run at most
  // four at a time and stop when the page goes away.
  let generation = 0;
  let alive = true;
  const reads = new AbortController(); // leaving the page cancels its reads in flight (PERF-13)
  async function mapLimited(list, limit, fn) {
    const out = new Array(list.length);
    let next = 0;
    const worker = async () => {
      while (next < list.length && alive) {
        const i = next;
        next += 1;
        out[i] = await fn(list[i]);
      }
    };
    await Promise.all(Array.from({ length: Math.min(limit, list.length) }, worker));
    return out;
  }
  async function load() {
    generation += 1;
    const mine = generation;
    const current = () => alive && mine === generation;
    try {
      const [git, runs] = await Promise.all([
        ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/git`, { signal: reads.signal }),
        ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/delivery/runs`, { signal: reads.signal }).catch(() => []),
      ]);
      if (!current()) return;
      const rows = Array.isArray(git.tasks) ? git.tasks : [];
      // The overview has no stages yet, so each task's delivery is read once (docs/api.md);
      // a server that adds `stage` to the overview rows skips these reads.
      const runsHead = JSON.stringify((Array.isArray(runs) ? runs : []).slice(0, 3).map((r) => [r.id, r.state, r.status]));
      const now = Date.now();
      const deliveries = await mapLimited(rows, 4, async (row) => {
        if (row.stage) return row;
        const sig = `${JSON.stringify(row)}|${runsHead}`;
        const hit = deliveryCache.get(row.task_id);
        if (hit && hit.sig === sig && now - hit.at < DELIVERY_TTL_MS) return hit.delivery;
        const delivery = await ctx.api.get(`/api/tasks/${enc(row.task_id)}/delivery`, { signal: reads.signal }).catch(() => null);
        if (delivery) deliveryCache.set(row.task_id, { sig, at: now, delivery });
        return delivery;
      });
      if (!current()) return;
      model = { git, runs: Array.isArray(runs) ? runs : [], items: rows.map((row, i) => ({ row, delivery: deliveries[i] })) };
      render();
    } catch (error) {
      if (!current()) return;
      setChildren(body, loadError(`Couldn’t load the delivery overview: ${error.message}`, load));
    }
  }

  function renderIntro() {
    const profile = sb().profile || {};
    const envs = environments(profile);
    const steps = [
      ['Being built', 'the agent works on its own branch'],
      ['In review', `pull request into ${profile.integration_branch || 'integration'}`],
      ...envs.integration.map((e) => [`Checked on ${e.name}`, e.command ? 'deployed, then verified' : 'verified']),
      [profile.auto_promote ? 'Live' : 'Released', profile.auto_promote ? `${profile.production_branch || 'production'} moves forward automatically` : `released to ${profile.production_branch || 'production'}`],
      ...envs.production.map((e) => [`Verified: ${e.name}`, 'checked that it works']),
    ];
    // Keep the explainer open and focus where it was across live re-renders.
    const wasOpen = Boolean(intro.querySelector('details')?.open);
    keepFocus(intro, () => setChildren(intro,
      h('p', { class: 'dl-sentence' }, profileSentence(profile)),
      h('details', { class: 'dl-how' }, h('summary', null, 'How this works'),
        h('ol', { class: 'dl-flow' }, steps.map(([name, what], i) => h('li', null, h('span', { class: 'step-num' }, String(i + 1)), h('span', null, h('strong', null, name), h('span', { class: 'muted' }, ` · ${what}`))))))));
    const how = intro.querySelector('details');
    if (how) how.open = wasOpen;
  }

  function render() {
    renderIntro();
    if (!model) return;
    const profile = sb().profile || {};
    const envs = environments(profile);
    const items = model.items.map((item) => {
      const task = ctx.store.findTask(item.row.task_id);
      return { ...item, task, info: task ? taskInfo(task, ctx.store.state.actionRequests, ctx.store.pullsOf(task.sandbox_id).get(task.id)) : null, column: columnOf(item.delivery, profile) };
    });
    const by = (column) => items.filter((item) => item.column === column);
    // Live updates rebuild the page; keyboard focus stays on the same control (keepFocus).
    keepFocus(body, () => setChildren(body,
      notesBanner(model.git.notes),
      checksAccessNote(ctx, model.git.checks_access),
      statusStrip(profile, envs, by('waiting'), by('live')),
      attention(items),
      board(profile, items, by),
      activity()));
  }

  // ---- Live / waiting
  function statusStrip(profile, envs, waiting, live) {
    const prod = profile.production_branch || 'production';
    const int = profile.integration_branch || 'integration';
    const titles = (list) => list.slice(0, 3).map((i) => `#${i.task ? i.task.number : '?'} ${i.row.title}`).join(', ') + (list.length > 3 ? `, +${list.length - 3}` : '');
    const lastRun = (env) => model.runs.find((r) => r.environment === env.name && r.state === 'succeeded');
    const liveCard = h('section', { class: 'dl-card', 'aria-label': 'Live in production' },
      h('h2', { class: 'card-title' }, icon('globe', { size: 14 }), `Live in production · ${prod}`),
      live.length ? h('p', { class: 'dl-big' }, plural(live.length, 'task'), h('span', { class: 'muted' }, ` went live through ${BRAND.name}`)) : h('p', { class: 'dl-big muted' }, `No task has gone live through ${BRAND.name} yet`),
      live.length ? h('p', { class: 'small text-2 ellipsis' }, titles(live)) : null,
      envs.production.map((env) => {
        const run = lastRun(env);
        const pending = live.filter((i) => i.delivery && (i.delivery.next_actions || []).some((a) => a.endsWith(`:${env.name}`)));
        return h('p', { class: 'small row nowrap' },
          pending.length ? h('span', { class: 'tone-warn' }, icon('clock', { size: 14 })) : h('span', { class: 'tone-ok' }, icon('check', { size: 14 })),
          pending.length ? `${env.name}: ${plural(pending.length, 'task')} waiting to be verified` : run ? `${env.name} verified by ${run.requested_by} ${ago(run.started_at)}` : `${env.name}: nothing to verify`);
      }));

    let waitingCard;
    if (profile.auto_promote) {
      waitingCard = h('section', { class: 'dl-card', 'aria-label': 'Waiting to go live' },
        h('h2', { class: 'card-title' }, icon('zap', { size: 14 }), 'Waiting to go live'),
        h('p', { class: 'dl-big' }, 'Goes live automatically after each merge'),
        h('p', { class: 'small muted' }, `${prod} moves forward to ${int} on every merge through ${BRAND.name}; nothing waits for a release.`));
    } else {
      const ready = waiting.filter((i) => i.delivery && (i.delivery.next_actions || []).includes('promote'));
      const blockedRelease = waiting.find((i) => i.delivery && (i.delivery.blocked || []).some((b) => b.action === 'promote'));
      const atEnv = (env) => waiting.filter((i) => i.delivery && (i.delivery.next_actions || []).some((a) => a === `run_env:${env.name}` || a === `confirm_env:${env.name}`));
      const pendingEnv = envs.integration.find((env) => atEnv(env).length);
      const release = !waiting.length ? null
        : ready.length ? ['ok', 'check', `Ready to release ${plural(ready.length, 'task')} to ${prod}`]
          : pendingEnv ? ['warn', 'clock', `Released once ${pendingEnv.name} is verified`]
            : blockedRelease ? ['warn', 'clock', `Not released yet: ${blockedRelease.delivery.blocked.find((b) => b.action === 'promote').reason}`] : ['warn', 'clock', 'Not ready to release yet'];
      waitingCard = h('section', { class: 'dl-card', 'aria-label': 'Waiting to go live' },
        h('h2', { class: 'card-title' }, icon('clock', { size: 14 }), `Waiting to go live · on ${int}`),
        waiting.length
          ? h('p', { class: 'dl-big' }, plural(waiting.length, 'task'), h('span', { class: 'muted' }, ` merged, not live yet`))
          : h('p', { class: 'dl-big muted' }, `${int} and ${prod} match: everything merged is live`),
        waiting.length ? h('p', { class: 'small text-2 ellipsis' }, titles(waiting)) : null,
        envs.integration.map((env) => {
          const at = atEnv(env);
          const run = lastRun(env);
          const action = at.length && at[0].delivery.next_actions.find((a) => a.endsWith(`:${env.name}`));
          // A deploy in progress says so, with its output.
          const running = model.runs.find((r) => r.environment === env.name && r.state === 'running');
          if (running) {
            return h('div', { class: 'dl-env' }, h('span', { class: 'tone-warn' }, icon('st-working', { size: 14 })),
              h('span', { class: 'dl-env-text' }, h('strong', null, env.name), ` · deploying, started ${ago(running.started_at)} by ${running.requested_by || 'an agent'}`),
              h('button', { type: 'button', class: 'btn btn-sm', onClick: () => openRunViewer(ctx, sb(), running.run_id, env.name) }, 'View output'));
          }
          return h('div', { class: 'dl-env' },
            at.length ? h('span', { class: 'tone-warn' }, icon('clock', { size: 14 })) : h('span', { class: 'tone-ok' }, icon('check', { size: 14 })),
            h('span', { class: 'dl-env-text' }, h('strong', null, env.name), ' · ', at.length ? `${plural(at.length, 'task')} ${action.startsWith('run_env:') ? 'waiting to be deployed' : 'waiting to be verified'}` : run ? `verified by ${run.requested_by} ${ago(run.started_at)}` : 'nothing to check'));
        }),
        release ? h('p', { class: 'small dl-release' }, h('span', { class: `tone-${release[0]}` }, icon(release[1], { size: 14 })), release[2]) : null);
    }
    return h('div', { class: 'dl-strip' }, liveCard, waitingCard);
  }

  // ---- Blocked: what is stuck, most urgent first (failing checks, then what blocks a pull request
  // or a merge). Status only: the agents fix it; a row links to the task, its checks on GitHub and
  // its delivery details. Named apart from the task list's "Needs you" (tasks waiting for input).
  function attention(items) {
    const rows = [];
    for (const item of items) {
      const d = item.delivery;
      if (!d || !item.task) continue;
      const blocked = d.blocked || [];
      const ended = item.info && item.info.group === 'ended';
      const pr = d.pr || item.row.pr;
      if (pr && pr.state === 'open' && pr.checks === 'failure') {
        rows.push({ rank: 0, item, tone: 'danger', icon: 'x', text: `Checks failing on pull request #${pr.number}`, link: pr.url ? { label: 'Checks', url: `${pr.url}/checks` } : null });
      } else if (ended && blocked.length && d.stage === 'none') {
        rows.push({ rank: 1, item, tone: 'warn', icon: 'warn', text: `No pull request yet: ${blocked[0].reason}` });
      } else if (d.stage === 'pr_open' && blocked.length && pr && pr.checks !== 'failure') {
        rows.push({ rank: 2, item, tone: 'muted', icon: 'clock', text: `Not merged yet: ${blocked[0].reason}`, link: pr.url ? { label: 'Pull request', url: pr.url } : null });
      }
    }
    rows.sort((a, b) => a.rank - b.rank);
    const shown = showAllNeeds ? rows : rows.slice(0, 5);
    return h('section', { class: 'dl-section', 'aria-labelledby': 'dl-needs' },
      h('h2', { class: 'dl-section-title', id: 'dl-needs' }, icon('bell', { size: 14 }), 'Blocked', h('span', { class: 'tl-group-count' }, String(rows.length))),
      shown.length ? h('ul', { class: 'dl-needs' }, shown.map((r) => h('li', { class: 'dl-need' },
        h('span', { class: `tone-${r.tone}` }, icon(r.icon, { size: 16 })),
        h('span', { class: 'dl-need-text' }, taskLink(r.item, true), h('span', { class: 'dl-need-why' }, r.text)),
        r.link ? h('a', { class: 'btn btn-sm', href: r.link.url, target: '_blank', rel: 'noopener noreferrer' }, r.link.label, icon('external', { size: 12 })) : null,
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => openDeliveryDialog(ctx, r.item.task) }, 'Details'))))
        : h('p', { class: 'dl-empty' }, icon('check', { size: 14 }), 'Nothing is blocked.'),
      rows.length > shown.length ? h('button', { type: 'button', class: 'link-btn small', onClick: () => { showAllNeeds = true; render(); } }, `Show all ${rows.length}`) : null);
  }

  function taskLink(item, bold) {
    if (!item.task) return h('span', { class: bold ? 'dl-task' : '' }, item.row.title);
    const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${item.task.number}`;
    return h('a', { class: ['dl-task', bold && 'strong'], href: href(path), dataset: { route: path } }, h('span', { class: 'muted' }, `#${item.task.number} `), item.row.title);
  }

  // ---- Board: where each task is on the way to production
  function board(profile, items, by) {
    const int = profile.integration_branch || 'integration';
    const prod = profile.production_branch || 'production';
    const columns = [
      { id: 'built', name: 'Being built', sub: 'no pull request yet' },
      { id: 'review', name: 'In review', sub: `pull request into ${int}` },
      profile.auto_promote ? null : { id: 'waiting', name: `On ${int}, not live`, sub: 'merged, waiting for release' },
      { id: 'live', name: 'Live', sub: `on ${prod}` },
    ].filter(Boolean);
    return h('section', { class: 'dl-section', 'aria-labelledby': 'dl-board' },
      h('h2', { class: 'dl-section-title', id: 'dl-board' }, icon('workflow', { size: 14 }), 'Where every task is'),
      h('div', { class: 'dl-board', style: { '--dl-cols': String(columns.length) } }, columns.map((column) => {
        const list = by(column.id).sort((a, b) => String(b.task ? b.task.updated_at : '').localeCompare(String(a.task ? a.task.updated_at : '')));
        const open = expanded.has(column.id);
        const shown = open ? list : list.slice(0, LIVE_LIMIT);
        return h('div', { class: 'dl-col' },
          h('div', { class: 'dl-col-head' }, h('span', { class: 'dl-col-name' }, column.name, h('span', { class: 'tl-group-count' }, ` ${list.length}`)), h('span', { class: 'xs muted' }, column.sub)),
          shown.length ? shown.map((item) => card(item, profile)) : h('p', { class: 'dl-col-empty' }, 'Nothing here'),
          list.length > shown.length ? h('button', { type: 'button', class: 'link-btn small', onClick: () => { expanded.add(column.id); render(); } }, `+${list.length - shown.length} more`) : null);
      })));
  }

  function card(item, profile) {
    const d = item.delivery || {};
    const pr = d.pr || item.row.pr;
    const next = d.next_actions || [];
    const envs = environments(profile);
    let line;
    if (item.column === 'built') {
      line = item.info ? [statusMark(item.info, 12), ` ${item.info.label}`] : 'Being built';
    } else if (item.column === 'review') {
      line = next.includes('merge') ? 'Ready to merge' : pr && CHECK_TEXT[pr.checks] ? CHECK_TEXT[pr.checks][0].toUpperCase() + CHECK_TEXT[pr.checks].slice(1) : 'In review';
    } else if (item.column === 'waiting') {
      const env = next.find((a) => a.startsWith('run_env:') || a.startsWith('confirm_env:'));
      line = env ? `${env.slice(env.indexOf(':') + 1)}: ${env.startsWith('run_env:') ? 'waiting to be deployed' : 'waiting to be verified'}` : next.includes('promote') ? 'Ready to release' : d.stage === 'env_done' ? `Verified on ${d.environment}` : 'Merged';
    } else {
      const env = next.find((a) => a.startsWith('confirm_env:') || a.startsWith('run_env:'));
      line = env && envs.production.length ? `Released · ${env.slice(env.indexOf(':') + 1)}: waiting to be verified` : 'Live';
    }
    const stuck = Boolean(pr && pr.state === 'open' && pr.checks === 'failure');
    const files = item.row.files_changed ? h('span', { class: 'dl-diffstat', title: item.row.branch || null },
      h('span', { class: 'muted' }, plural(item.row.files_changed, 'file')), ' ', h('span', { class: 'gt-add' }, `+${item.row.insertions ?? 0}`), ' ', h('span', { class: 'gt-del' }, `−${item.row.deletions ?? 0}`)) : null;
    // The card opens the task's changes (diff, commits, branch); "Go to task" is there.
    const review = item.task ? `/s/${encodeURIComponent(sandbox.slug)}/git/t/${item.task.number}` : null;
    const title = review
      ? h('a', { class: 'dl-task dl-card-link', href: href(review), dataset: { route: review } }, h('span', { class: 'muted' }, `#${item.task.number} `), item.row.title)
      : taskLink(item, false);
    return h('div', { class: ['dl-card-task', stuck && 'attention', review && 'clickable'] },
      h('div', { class: 'dl-card-title' }, title, stuck ? h('span', { class: 'dl-dot', title: 'Checks failing' }, h('span', { class: 'sr-only' }, 'Checks failing')) : null),
      h('div', { class: 'dl-card-line' }, line),
      h('div', { class: 'dl-card-meta' }, prChip(pr), files,
        item.task && (item.column !== 'built' || (item.info && item.info.group === 'ended')) ? h('button', { type: 'button', class: 'link-btn xs dl-deliver', onClick: () => openDeliveryDialog(ctx, item.task) }, 'Details') : null));
  }

  // ---- Activity: deploys, verifications and commits, newest first
  function activity() {
    const commitUrl = (sha) => (sandbox.repo && sandbox.repo.url ? `${sandbox.repo.url}/commit/${sha}` : null);
    const events = [
      ...model.runs.map((r) => ({ at: r.started_at, run: r })),
      ...(Array.isArray(model.git.recent_commits) ? model.git.recent_commits : []).map((c) => ({ at: c.at, commit: c })),
    ].sort((a, b) => String(b.at).localeCompare(String(a.at)));
    const shown = showAllActivity ? events : events.slice(0, ACTIVITY_LIMIT);
    const runText = (r) => (r.kind === 'manual' ? `Verified ${r.environment}` : `Deployed to ${r.environment} · ${r.state === 'running' ? 'running' : r.state === 'succeeded' ? 'succeeded' : 'failed'}`);
    return h('section', { class: 'dl-section', 'aria-labelledby': 'dl-activity' },
      h('h2', { class: 'dl-section-title', id: 'dl-activity' }, icon('pulse', { size: 14 }), 'Activity',
        sandbox.repo && sandbox.repo.url ? h('a', { class: 'small dl-head-link', href: `${sandbox.repo.url}/commits`, target: '_blank', rel: 'noopener noreferrer' }, 'All commits on GitHub', icon('external', { size: 12 })) : null),
      shown.length ? h('ul', { class: 'dl-activity' }, shown.map((e) => h('li', { class: 'dl-event' },
        h('span', { class: 'dl-event-time', title: dateTime(e.at) }, relShort(e.at)),
        e.run
          ? [h('span', { class: `tone-${e.run.state === 'succeeded' ? 'ok' : e.run.state === 'running' ? 'warn' : 'danger'}` }, icon(e.run.kind === 'manual' ? 'check' : 'zap', { size: 14 })),
            h('span', { class: 'dl-event-text' }, runText(e.run), h('span', { class: 'muted' }, ` · ${e.run.requested_by || 'an agent'}`)),
            e.run.kind === 'manual' ? null : h('button', { type: 'button', class: 'btn btn-sm', onClick: () => openRunViewer(ctx, sb(), e.run.run_id, e.run.environment) }, 'Output')]
          : [h('span', { class: 'muted' }, icon('branch', { size: 14 })),
            h('span', { class: 'dl-event-text' }, e.commit.subject, h('span', { class: 'muted' }, ` · ${e.commit.author}`)),
            commitUrl(e.commit.sha) ? h('a', { class: 'mono xs', href: commitUrl(e.commit.sha), target: '_blank', rel: 'noopener noreferrer' }, String(e.commit.sha).slice(0, 7)) : h('code', { class: 'xs' }, String(e.commit.sha).slice(0, 7))])))
        : h('p', { class: 'dl-empty' }, 'No deploys or commits yet.'),
      events.length > shown.length ? h('button', { type: 'button', class: 'link-btn small', onClick: () => { showAllActivity = true; render(); } }, `Show ${events.length - shown.length} more`) : null);
  }

  const repaint = coalesce(() => { if (alive) render(); }, { minInterval: 200 });
  const offs = [
    ctx.store.on('sandboxes', repaint),
    ctx.store.on('tasks', (sid) => { if (sid === sandbox.id) repaint(); }),
  ];
  renderIntro();
  load();
  return { el: frame.el, key: `git:${sandbox.id}`, destroy: () => { alive = false; reads.abort(); repaint.cancel(); offs.forEach((off) => off()); } };
}

let presetCache = null;

/** The server's named presets (`GET /api/delivery/presets`), fetched once per page load. */
async function loadPresets(ctx) {
  if (!presetCache) {
    const list = await ctx.api.get('/api/delivery/presets');
    presetCache = new Map((Array.isArray(list) ? list : []).map((p) => [p.preset, p]));
  }
  return presetCache;
}

/**
 * Edit the delivery profile (admin, PATCH /api/sandboxes/{sid}). The server relabels the
 * profile: it keeps a named preset only while every setting equals that preset's defaults.
 */
async function openProfileEditor(ctx, sandbox) {
  let presets;
  let presetsError = null;
  try {
    presets = await loadPresets(ctx);
  } catch (error) {
    // Said in the dialog, where the presets would be (not in a toast); the rest still works.
    presetsError = error;
    presets = new Map();
  }
  const profile = JSON.parse(JSON.stringify(sandbox.profile || presets.get('standard') || {}));
  const preset = h('select', { class: 'select' }, [...presets.keys(), 'custom'].map((p) => h('option', { value: p }, p)));
  preset.value = profile.preset && (presets.has(profile.preset) || profile.preset === 'custom') ? profile.preset : 'custom';
  const integration = h('input', { class: 'input mono', value: profile.integration_branch || '' });
  const production = h('input', { class: 'input mono', value: profile.production_branch || '' });
  const merge = h('select', { class: 'select' }, ['squash', 'merge', 'rebase'].map((m) => h('option', { value: m }, m)));
  merge.value = profile.merge || 'squash';
  const promoteSel = h('select', { class: 'select' }, [['merge_commit', 'Through a release pull request'], ['fast_forward', 'Move production forward to match']].map(([m, label]) => h('option', { value: m }, label)));
  promoteSel.value = profile.promote || 'merge_commit';
  const autoPromote = h('input', { type: 'checkbox', checked: Boolean(profile.auto_promote) });
  const requireChecks = h('input', { type: 'checkbox', checked: profile.require_checks !== false });
  let envs = Array.isArray(profile.environments) ? profile.environments.map((e) => ({ ...e })) : [];
  const envHost = h('div', { class: 'stack-sm' });

  const renderEnvs = () => setChildren(envHost,
    envs.length ? envs.map((env, i) => {
      const name = h('input', { class: 'input mono', value: env.name, placeholder: 'name (a-z, 0-9, - _)', 'aria-label': 'Environment name', onInput: (e) => { env.name = e.target.value; } });
      const after = h('select', { class: 'select', 'aria-label': 'Runs after', onChange: (e) => { env.after = e.target.value; } }, h('option', { value: 'integration' }, 'after integration'), h('option', { value: 'production' }, 'after production'));
      after.value = env.after || 'integration';
      const command = h('input', { class: 'input mono', value: env.command || '', placeholder: 'command (optional, single line)', 'aria-label': 'Command', maxlength: 2000, onInput: (e) => { env.command = e.target.value || null; } });
      const verify = h('select', { class: 'select', 'aria-label': 'Verification', onChange: (e) => { env.verify = e.target.value; } }, h('option', { value: 'manual' }, 'manual verify'), h('option', { value: 'command' }, 'command verifies'));
      verify.value = env.verify || 'manual';
      return h('div', { class: 'card card-pad env-row' },
        name, after, command, verify, iconButton('x', 'Remove environment', () => { envs.splice(i, 1); renderEnvs(); }));
    }) : h('div', { class: 'muted small' }, 'No environments.'),
    h('div', null, h('button', { type: 'button', class: 'btn btn-sm', disabled: envs.length >= 8, title: envs.length >= 8 ? 'At most 8 environments' : null, onClick: () => { envs.push({ name: '', after: 'integration', command: null, verify: 'manual' }); renderEnvs(); } }, 'Add environment')));

  preset.addEventListener('change', () => {
    const p = presets.get(preset.value);
    if (!p) return;
    integration.value = p.integration_branch;
    production.value = p.production_branch;
    merge.value = p.merge;
    promoteSel.value = p.promote;
    autoPromote.checked = Boolean(p.auto_promote);
    requireChecks.checked = p.require_checks !== false;
    envs = (p.environments || []).map((e) => ({ ...e }));
    renderEnvs();
  });
  renderEnvs();

  openModal({
    title: `Delivery profile · ${sandbox.name}`,
    wide: true,
    body: h('div', { class: 'stack' },
      presetsError ? loadError(`Couldn’t load the delivery presets: ${presetsError.message}. Edit the settings below, or close and open the profile again.`) : null,
      h('div', { class: 'form-grid' }, field('Preset', preset), field('Integration branch', integration), field('Production branch', production), field('Merge method', merge), field('Release to production', promoteSel)),
      h('label', { class: 'check' }, autoPromote, h('span', null, `Release automatically after each merge through ${BRAND.name} (continuous, like Studio)`)),
      h('label', { class: 'check' }, requireChecks, h('span', null, 'Merges wait for green checks')),
      h('div', { class: 'field' }, h('span', { class: 'field-label' }, 'Environments'), envHost),
      h('p', { class: 'muted xs' }, 'Agents follow this profile when they merge, deploy and release. Environment commands are shell text run on the server (/bin/sh -c in the sandbox clone). Every reader of this sandbox can see them: keep secrets out. Changing any setting of a named preset makes the profile “custom”.')),
    actions: [
      { label: 'Cancel', value: false },
      { label: 'Save profile', kind: 'primary', run: async () => {
        const next = {
          preset: preset.value,
          integration_branch: integration.value.trim(),
          production_branch: production.value.trim(),
          merge: merge.value,
          promote: promoteSel.value,
          auto_promote: autoPromote.checked,
          require_checks: requireChecks.checked,
          environments: envs.filter((e) => e.name.trim()).map((e) => ({ name: e.name.trim(), after: e.after, command: e.command && e.command.trim() ? e.command.trim() : null, verify: e.verify })),
        };
        const updated = await ctx.api.patch(`/api/sandboxes/${enc(sandbox.id)}`, { profile: next });
        if (updated && updated.id) ctx.store.upsertSandbox(updated);
        toast(updated && updated.profile && updated.profile.preset !== next.preset ? `Delivery profile saved (now “${updated.profile.preset}”).` : 'Delivery profile saved.', { kind: 'ok' });
      } },
    ],
  });
}
