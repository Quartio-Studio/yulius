// Git page (GIT-001, DLV-001..005): task branches with diff stats, PRs with checks,
// recent commits, environment runs, delivery actions per profile and (admin) delivery
// profile editing. Reads degrade: Git/GitHub problems arrive as `notes`, not errors.

import { enc } from '../api.js';
import { h, setChildren, badge, openModal, field, toast, iconButton, placeholder } from '../dom.js';
import { relShort, relLong, dateTime } from '../format.js';
import { allows } from '../status.js';
import { href } from '../router.js';
import { openDeliveryDialog, prLine, runEnvironment, confirmEnvironment, promote, notesBanner, openRunViewer, runStateBadge } from '../actions.js';
import { pageFrame, dataTable, loader } from './common.js';

/** Mount the Git page for a sandbox. */
export function mountGitPage(ctx, sandbox) {
  const write = allows(sandbox.access, 'write');
  const admin = allows(sandbox.access, 'admin');
  const frame = pageFrame({ title: 'Git', sub: sandbox.repo ? `${sandbox.repo.owner}/${sandbox.repo.name}` : sandbox.name, actions: [iconButton('refresh', 'Refresh', () => { reload(); reloadRuns(); })] });
  const deliveryHost = h('div');
  const overviewHost = h('div', { class: 'stack' });
  const runsHost = h('div');
  frame.body.append(deliveryHost, overviewHost,
    h('div', { class: 'card' }, h('div', { class: 'card-head' }, h('span', { class: 'card-title' }, 'Environment runs'), h('span', { class: 'muted xs' }, 'newest 20')), runsHost));

  const afterAction = () => { reload(); reloadRuns(); };

  function envButtons(sb, env) {
    const why = write ? null : 'Requires write access';
    const buttons = [];
    if (env.command) {
      buttons.push(h('button', { type: 'button', class: 'btn btn-sm', disabled: !write, title: why || `Runs: ${env.command}`, onClick: async () => { await runEnvironment(ctx, sb, env.name); afterAction(); } }, `Run ${env.name}`));
    }
    if (!env.command || env.verify === 'manual') {
      buttons.push(h('button', { type: 'button', class: 'btn btn-sm', disabled: !write, title: why || 'Record that you verified this environment', onClick: async () => { await confirmEnvironment(ctx, sb, env.name); afterAction(); } }, `Confirm ${env.name}`));
    }
    return buttons;
  }

  function renderDelivery() {
    const sb = ctx.store.sandboxById(sandbox.id) || sandbox;
    const profile = sb.profile || {};
    const envs = Array.isArray(profile.environments) ? profile.environments : [];
    const envBadge = (e) => badge(`${e.name}${e.command ? '' : ' (manual)'}`, '', e.command ? `Command: ${e.command} · verify: ${e.verify}` : 'Manual checkpoint');
    setChildren(deliveryHost, h('div', { class: 'card' },
      h('div', { class: 'card-head' },
        h('span', { class: 'card-title' }, 'Delivery profile'),
        badge(profile.preset || 'custom', 'accent'),
        profile.require_checks === false ? badge('checks not required', 'warn') : profile.require_checks === true ? badge('checks required') : null,
        h('span', { class: 'spacer' }),
        admin ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => openProfileEditor(ctx, sb) }, 'Edit profile') : null),
      h('div', { class: 'card-body stack' },
        h('div', { class: 'row-wrap small' },
          h('span', null, 'feature branch'), '→', h('span', null, 'PR into ', h('code', null, profile.integration_branch || '?'), ` (${profile.merge || '?'})`),
          envs.filter((e) => e.after === 'integration').map((e) => ['→', envBadge(e)]),
          '→', h('span', null, 'promote to ', h('code', null, profile.production_branch || '?'), ` (${profile.promote || '?'}${profile.auto_promote ? ', automatic' : ''})`),
          envs.filter((e) => e.after === 'production').map((e) => ['→', envBadge(e)])),
        h('div', { class: 'row-wrap' },
          envs.map((env) => envButtons(sb, env)),
          h('button', { type: 'button', class: 'btn btn-sm', disabled: !write, title: write ? null : 'Requires write access', onClick: async () => { await promote(ctx, sb); afterAction(); } }, `Promote ${profile.integration_branch || ''} → ${profile.production_branch || ''}`)),
        h('p', { class: 'muted xs' }, 'Environment runs, merges and promotions happen only when you click and confirm (GIT-002). Environment commands run on the server outside sandbox containment.'))));
  }

  const reload = loader(overviewHost, () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/git`), (git) => {
    const tasks = Array.isArray(git.tasks) ? git.tasks : [];
    const commits = Array.isArray(git.recent_commits) ? git.recent_commits : [];
    const pulls = Array.isArray(git.pulls) ? git.pulls : [];
    const commitUrl = (sha) => (sandbox.repo && sandbox.repo.url ? `${sandbox.repo.url}/commit/${sha}` : null);
    return [
      notesBanner(git.notes),
      h('div', { class: 'row-wrap small muted' },
        'Default ', h('code', null, git.default_branch || '?'), ' · integration ', h('code', null, git.integration_branch || '?'), ' · production ', h('code', null, git.production_branch || '?')),
      h('h2', { class: 'small muted' }, 'Task branches'),
      dataTable([
        { label: 'Task', class: 'title-cell', render: (t) => {
          const summary = ctx.store.findTask(t.task_id);
          if (!summary) return t.title;
          const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${summary.number}`;
          return h('a', { href: href(path), dataset: { route: path } }, `#${summary.number} ${t.title}`);
        } },
        { label: 'Branch', render: (t) => h('code', null, t.branch), phoneHide: true },
        { label: 'Ahead / behind', class: 'num', render: (t) => `${t.ahead ?? '—'} / ${t.behind ?? '—'}`, phoneHide: true },
        { label: 'Changes', class: 'num', render: (t) => h('span', { class: 'diffstat' }, `${t.files_changed ?? '—'} files `, h('span', { class: 'ins' }, `+${t.insertions ?? '—'}`), ' ', h('span', { class: 'del' }, `−${t.deletions ?? '—'}`)) },
        { label: 'Pull request', render: (t) => prLine(t.pr) },
        { label: '', render: (t) => {
          const summary = ctx.store.findTask(t.task_id);
          return h('button', { type: 'button', class: 'btn btn-sm', disabled: !write || !summary, title: !write ? 'Requires write access' : summary ? null : 'Archived task', onClick: () => openDeliveryDialog(ctx, summary) }, 'Deliver');
        } },
      ], tasks, { empty: 'No task branches yet.' }),
      h('div', { class: 'grid-2' },
        h('div', { class: 'card' },
          h('div', { class: 'card-head' }, h('span', { class: 'card-title' }, 'Open pull requests')),
          pulls.length ? h('div', { class: 'list' }, pulls.map((pr) => h('div', { class: 'list-item' }, prLine(pr)))) : h('div', { class: 'placeholder' }, 'No open pull requests.')),
        h('div', { class: 'card' },
          h('div', { class: 'card-head' }, h('span', { class: 'card-title' }, 'Recent commits')),
          commits.length ? h('div', { class: 'list' }, commits.map((c) => h('div', { class: 'list-item' },
            commitUrl(c.sha) ? h('a', { class: 'mono', href: commitUrl(c.sha), target: '_blank', rel: 'noopener noreferrer' }, String(c.sha).slice(0, 7)) : h('code', null, String(c.sha).slice(0, 7)),
            h('div', { class: 'list-main' }, h('div', { class: 'ellipsis' }, c.subject), h('div', { class: 'list-sub' }, `${c.author} · `, h('span', { title: dateTime(c.at) }, relShort(c.at))))))) : h('div', { class: 'placeholder' }, 'No commits.'))),
    ];
  });

  const reloadRuns = loader(runsHost, () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/delivery/runs`), (list) => {
    const runs = Array.isArray(list) ? list : [];
    if (runs.length === 0) return placeholder('No environment runs yet.');
    return dataTable([
      { label: 'Environment', render: (r) => h('strong', null, r.environment || '—') },
      { label: 'State', render: (r) => runStateBadge(r.state) },
      { label: 'Kind', render: (r) => r.kind || '—', phoneHide: true },
      { label: 'Commit', render: (r) => (r.commit ? h('code', null, String(r.commit).slice(0, 8)) : '—'), phoneHide: true },
      { label: 'By', render: (r) => r.requested_by || '—', phoneHide: true },
      { label: 'Started', class: 'num', render: (r) => h('span', { title: dateTime(r.started_at) }, relLong(r.started_at)) },
      { label: 'Exit', class: 'num', render: (r) => (r.exit_code === null || r.exit_code === undefined ? '—' : String(r.exit_code)), phoneHide: true },
    ], runs, { onRowClick: (r) => openRunViewer(ctx, sandbox, r.run_id, r.environment || 'run') });
  });

  const off = ctx.store.on('sandboxes', renderDelivery);
  renderDelivery();
  return { el: frame.el, key: `git:${sandbox.id}`, destroy: off };
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
  try {
    presets = await loadPresets(ctx);
  } catch (error) {
    toast(`Could not load the delivery presets: ${error.message}`, { kind: 'error' });
    presets = new Map();
  }
  const profile = JSON.parse(JSON.stringify(sandbox.profile || presets.get('standard') || {}));
  const preset = h('select', { class: 'select' }, [...presets.keys(), 'custom'].map((p) => h('option', { value: p }, p)));
  preset.value = profile.preset && (presets.has(profile.preset) || profile.preset === 'custom') ? profile.preset : 'custom';
  const integration = h('input', { class: 'input mono', value: profile.integration_branch || '' });
  const production = h('input', { class: 'input mono', value: profile.production_branch || '' });
  const merge = h('select', { class: 'select' }, ['squash', 'merge', 'rebase'].map((m) => h('option', { value: m }, m)));
  merge.value = profile.merge || 'squash';
  const promoteSel = h('select', { class: 'select' }, ['merge_commit', 'fast_forward'].map((m) => h('option', { value: m }, m)));
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
      return h('div', { class: 'card card-pad', style: { display: 'grid', gridTemplateColumns: '1fr 1fr 2fr 1fr auto', gap: '6px', alignItems: 'center' } },
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
      h('div', { class: 'form-grid' }, field('Preset', preset), field('Integration branch', integration), field('Production branch', production), field('Merge method', merge), field('Promotion', promoteSel)),
      h('label', { class: 'check' }, autoPromote, h('span', null, 'Promote automatically after a merge through Yulius (continuous, e.g. Studio)')),
      h('label', { class: 'check' }, requireChecks, h('span', null, 'Merges wait for green checks')),
      h('div', { class: 'field' }, h('span', { class: 'field-label' }, 'Environments'), envHost),
      h('p', { class: 'muted xs' }, 'Environment commands are shell text run on the server (/bin/sh -c in the sandbox clone) when someone confirms a run. Every reader of this sandbox can see them: keep secrets out. Changing any setting of a named preset makes the profile “custom”.')),
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
