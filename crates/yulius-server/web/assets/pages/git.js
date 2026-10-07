// Git page (GIT-001, DLV-001..005): task branches with diff stats, PRs with checks,
// recent commits, delivery actions per profile and (admin) delivery profile editing.

import { enc } from '../api.js';
import { h, setChildren, badge, openModal, field, toast, iconButton } from '../dom.js';
import { relShort, dateTime } from '../format.js';
import { allows } from '../status.js';
import { href } from '../router.js';
import { openDeliveryDialog, prLine, runDeploy, promote } from '../actions.js';
import { pageFrame, dataTable, loader } from './common.js';

const PRESETS = {
  standard: { integration_branch: 'dev', production_branch: 'main', merge: 'squash', promote: 'merge_commit', auto_promote: false, environments: [{ name: 'dev', after: 'integration', command: null, verify: 'manual' }] },
  product: { integration_branch: 'quartio', production_branch: 'main', merge: 'squash', promote: 'fast_forward', auto_promote: false, environments: [{ name: 'dev-vm', after: 'integration', command: null, verify: 'manual' }, { name: 'production', after: 'production', command: null, verify: 'manual' }] },
  studio: { integration_branch: 'quartio', production_branch: 'main', merge: 'squash', promote: 'fast_forward', auto_promote: true, environments: [] },
};

/** Mount the Git page for a sandbox. */
export function mountGitPage(ctx, sandbox) {
  const write = allows(sandbox.access, 'write');
  const admin = allows(sandbox.access, 'admin');
  const frame = pageFrame({ title: 'Git', sub: sandbox.repo ? `${sandbox.repo.owner}/${sandbox.repo.name}` : sandbox.name, actions: [iconButton('refresh', 'Refresh', () => reload())] });
  const deliveryHost = h('div');
  const overviewHost = h('div', { class: 'stack' });
  frame.body.append(deliveryHost, overviewHost);

  function renderDelivery() {
    const sb = ctx.store.sandboxById(sandbox.id) || sandbox;
    const profile = sb.profile || {};
    const envs = Array.isArray(profile.environments) ? profile.environments : [];
    setChildren(deliveryHost, h('div', { class: 'card' },
      h('div', { class: 'card-head' },
        h('span', { class: 'card-title' }, 'Delivery profile'),
        badge(profile.preset || 'custom', 'accent'),
        h('span', { class: 'spacer' }),
        admin ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => openProfileEditor(ctx, sb) }, 'Edit profile') : null),
      h('div', { class: 'card-body stack' },
        h('div', { class: 'row-wrap small' },
          h('span', null, 'feature branch'), '→', h('span', null, `PR into `, h('code', null, profile.integration_branch || '?'), ` (${profile.merge || '?'})`),
          envs.filter((e) => e.after === 'integration').map((e) => ['→', badge(`env: ${e.name}`)]),
          '→', h('span', null, `promote to `, h('code', null, profile.production_branch || '?'), ` (${profile.promote || '?'}${profile.auto_promote ? ', automatic' : ''})`),
          envs.filter((e) => e.after === 'production').map((e) => ['→', badge(`env: ${e.name}`)])),
        h('div', { class: 'row-wrap' },
          envs.map((env) => h('button', { type: 'button', class: 'btn btn-sm', disabled: !write, title: write ? `${env.command ? `Runs: ${env.command}` : 'Manual verification step'}` : 'Requires write access', onClick: () => runDeploy(ctx, sb, env.name) }, `Run deploy: ${env.name}`)),
          h('button', { type: 'button', class: 'btn btn-sm', disabled: !write, title: write ? null : 'Requires write access', onClick: async () => { await promote(ctx, sb); reload(); } }, `Promote ${profile.integration_branch || ''} → ${profile.production_branch || ''}`)),
        h('p', { class: 'muted xs' }, 'Deploys, merges and promotions run only when you click and confirm (GIT-002).'))));
  }

  const reload = loader(overviewHost, () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/git`), (git) => {
    const tasks = Array.isArray(git.tasks) ? git.tasks : [];
    const commits = Array.isArray(git.recent_commits) ? git.recent_commits : [];
    const pulls = Array.isArray(git.pulls) ? git.pulls : [];
    const commitUrl = (sha) => (sandbox.repo && sandbox.repo.url ? `${sandbox.repo.url}/commit/${sha}` : null);
    return [
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
        { label: 'Changes', class: 'num', render: (t) => h('span', { class: 'diffstat' }, `${t.files_changed ?? 0} files `, h('span', { class: 'ins' }, `+${t.insertions ?? 0}`), ' ', h('span', { class: 'del' }, `−${t.deletions ?? 0}`)) },
        { label: 'Pull request', render: (t) => prLine(t.pr) },
        { label: '', render: (t) => {
          const summary = ctx.store.findTask(t.task_id);
          return h('button', { type: 'button', class: 'btn btn-sm', disabled: !write || !summary, title: !write ? 'Requires write access' : null, onClick: () => openDeliveryDialog(ctx, summary) }, 'Deliver');
        } },
      ], tasks, { empty: 'No task branches yet.' }),
      h('div', { class: 'grid-2' },
        h('div', { class: 'card' },
          h('div', { class: 'card-head' }, h('span', { class: 'card-title' }, 'Pull requests')),
          pulls.length ? h('div', { class: 'list' }, pulls.map((pr) => h('div', { class: 'list-item' }, prLine(pr)))) : h('div', { class: 'placeholder' }, 'No pull requests.')),
        h('div', { class: 'card' },
          h('div', { class: 'card-head' }, h('span', { class: 'card-title' }, 'Recent commits')),
          commits.length ? h('div', { class: 'list' }, commits.map((c) => h('div', { class: 'list-item' },
            commitUrl(c.sha) ? h('a', { class: 'mono', href: commitUrl(c.sha), target: '_blank', rel: 'noopener noreferrer' }, String(c.sha).slice(0, 7)) : h('code', null, String(c.sha).slice(0, 7)),
            h('div', { class: 'list-main' }, h('div', { class: 'ellipsis' }, c.subject), h('div', { class: 'list-sub' }, `${c.author} · `, h('span', { title: dateTime(c.at) }, relShort(c.at))))))) : h('div', { class: 'placeholder' }, 'No commits.'))),
    ];
  });

  const off = ctx.store.on('sandboxes', renderDelivery);
  renderDelivery();
  return { el: frame.el, key: `git:${sandbox.id}`, destroy: off };
}

/** Edit the delivery profile (admin, PATCH /api/sandboxes/{sid}). */
function openProfileEditor(ctx, sandbox) {
  const profile = JSON.parse(JSON.stringify(sandbox.profile || PRESETS.standard));
  const preset = h('select', { class: 'select' }, ['standard', 'product', 'studio', 'custom'].map((p) => h('option', { value: p }, p)));
  preset.value = profile.preset || 'custom';
  const integration = h('input', { class: 'input mono', value: profile.integration_branch || '' });
  const production = h('input', { class: 'input mono', value: profile.production_branch || '' });
  const merge = h('select', { class: 'select' }, ['squash', 'merge', 'rebase'].map((m) => h('option', { value: m }, m)));
  merge.value = profile.merge || 'squash';
  const promoteSel = h('select', { class: 'select' }, ['merge_commit', 'fast_forward'].map((m) => h('option', { value: m }, m)));
  promoteSel.value = profile.promote || 'merge_commit';
  const autoPromote = h('input', { type: 'checkbox', checked: Boolean(profile.auto_promote) });
  let envs = Array.isArray(profile.environments) ? profile.environments.map((e) => ({ ...e })) : [];
  const envHost = h('div', { class: 'stack-sm' });

  const renderEnvs = () => setChildren(envHost,
    envs.length ? envs.map((env, i) => {
      const name = h('input', { class: 'input mono', value: env.name, placeholder: 'name', 'aria-label': 'Environment name', onInput: (e) => { env.name = e.target.value; } });
      const after = h('select', { class: 'select', 'aria-label': 'Runs after', onChange: (e) => { env.after = e.target.value; } }, h('option', { value: 'integration' }, 'after integration'), h('option', { value: 'production' }, 'after production'));
      after.value = env.after || 'integration';
      const command = h('input', { class: 'input mono', value: env.command || '', placeholder: 'deploy command (optional)', 'aria-label': 'Command', onInput: (e) => { env.command = e.target.value || null; } });
      const verify = h('select', { class: 'select', 'aria-label': 'Verification', onChange: (e) => { env.verify = e.target.value; } }, h('option', { value: 'manual' }, 'manual verify'), h('option', { value: 'command' }, 'command verify'));
      verify.value = env.verify || 'manual';
      return h('div', { class: 'card card-pad', style: { display: 'grid', gridTemplateColumns: '1fr 1fr 2fr 1fr auto', gap: '6px', alignItems: 'center' } },
        name, after, command, verify, iconButton('x', 'Remove environment', () => { envs.splice(i, 1); renderEnvs(); }));
    }) : h('div', { class: 'muted small' }, 'No environments.'),
    h('div', null, h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { envs.push({ name: '', after: 'integration', command: null, verify: 'manual' }); renderEnvs(); } }, 'Add environment')));

  preset.addEventListener('change', () => {
    const p = PRESETS[preset.value];
    if (!p) return;
    integration.value = p.integration_branch;
    production.value = p.production_branch;
    merge.value = p.merge;
    promoteSel.value = p.promote;
    autoPromote.checked = p.auto_promote;
    envs = p.environments.map((e) => ({ ...e }));
    renderEnvs();
  });
  renderEnvs();

  openModal({
    title: `Delivery profile · ${sandbox.name}`,
    wide: true,
    body: h('div', { class: 'stack' },
      h('div', { class: 'form-grid' }, field('Preset', preset), field('Integration branch', integration), field('Production branch', production), field('Merge method', merge), field('Promotion', promoteSel)),
      h('label', { class: 'check' }, autoPromote, h('span', null, 'Promote automatically after merge (continuous, e.g. Studio)')),
      h('div', { class: 'field' }, h('span', { class: 'field-label' }, 'Environments'), envHost)),
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
          environments: envs.filter((e) => e.name.trim()).map((e) => ({ name: e.name.trim(), after: e.after, command: e.command ? e.command.trim() : null, verify: e.verify })),
        };
        const updated = await ctx.api.patch(`/api/sandboxes/${enc(sandbox.id)}`, { profile: next });
        if (updated && updated.id) ctx.store.upsertSandbox(updated);
        toast('Delivery profile saved.', { kind: 'ok' });
      } },
    ],
  });
}
