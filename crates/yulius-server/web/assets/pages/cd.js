// CD (deployment) page (SPEC-134, ADR-0109/0110): the project's dev and production environments as
// yulius.toml defines them on their protected branches ([cd.dev] from the integration branch,
// [cd.prod] from what production deploys), what each runs, the secrets it needs, what is
// deployed, and the runs with their output. Dev deploys after a confirmation (people with write
// access; agents use cd_deploy); production only by an administrator who types the project's slug.
// "Set up CD with an agent" starts an Infra bootstrap task, so CD is configured through the harness.

import { enc } from '../api.js';
import { h, badge, itemCard, emptyState, setChildren, placeholder, openModal, toast, errorToast, confirmDialog, pollPaused, icon, bar, drawShape } from '../dom.js';
import { relLong, dateTime } from '../format.js';
import { allows } from '../status.js';
import { replaceQuietly, href } from '../router.js';
import { pageFrame, loader } from './common.js';
import { BRAND } from '../brand.js';
import { loadWaterfall } from '../stages.js';
import { pipelineHref } from './pipeline.js';

const STATE = {
  running: { label: 'Deploying', tone: 'warn', icon: 'refresh' },
  passed: { label: 'Deployed', tone: 'ok', icon: 'check' },
  failed: { label: 'Failed', tone: 'danger', icon: 'x' },
  error: { label: 'Error', tone: 'danger', icon: 'warn' },
  cancelled: { label: 'Cancelled', tone: '', icon: 'x' },
};
const ENV_STATE = {
  ready: { label: 'Ready', tone: 'ok' },
  empty: { label: 'Nothing to deploy yet', tone: '' },
  not_defined: { label: 'Not defined', tone: 'warn' },
};
const SECRET_STATE = {
  ready: { label: 'allowed', tone: 'ok' },
  not_allowed: { label: 'waiting for a person to allow it', tone: 'warn' },
  missing: { label: 'not in Secrets yet', tone: 'danger' },
  production: { label: 'a production secret: dev never gets it', tone: 'danger' },
};
const POLL_ACTIVE_MS = 3000;
const POLL_IDLE_MS = 30000;

const stateOf = (state) => STATE[state] || { label: state || 'Unknown', tone: '', icon: 'clock' };
const shortSha = (sha) => String(sha || '').slice(0, 7);
const title = (env) => (env.name === 'dev' ? 'Dev' : 'Production');

/** What a person asks an agent when CD is set up through the harness. */
const SETUP_PROMPT = `Set up this project's CD (deployment) in yulius.toml and scripts/cd/ (skill infra-bootstrap, step 3).

- Find out how the project is deployed today (deploy scripts, Dockerfile or compose, platform files, deploy jobs in GitHub workflows) and keep what works.
- Define [cd.dev] and [cd.prod] explicitly. Either may be empty (no deploy yet) with an \`about\` line saying why.
- Write idempotent bash scripts for deploy and verify (a smoke test of what is deployed), committed under scripts/cd/, with machines and credentials only from agent secrets.
- Ask me with needs_you for anything only a person can give (a machine's credentials on the Secrets page, allowing a secret).

How we deploy (fill in what you know):
- dev:
- production: `;

/** Mount the CD page. */
export function mountCdPage(ctx, sandbox, route) {
  const write = allows(sandbox.access, 'write');
  const base = `/api/sandboxes/${enc(sandbox.id)}`;
  const setupButton = h('button', { type: 'button', class: 'btn', disabled: !write, title: write ? 'Start a task that sets up or changes CD' : 'Requires write access', onClick: () => setUp() }, icon('spark', { size: 16 }), 'Set up CD with an agent');
  const frame = pageFrame({ title: 'CD', sub: `deploys run on this ${BRAND.name} machine, from protected branches`, actions: [setupButton] });
  const host = h('div');
  frame.body.append(host);
  let timer = null;
  let destroyed = false;
  let openRun = null;
  let last = { runs: [] };

  const reload = loader(host, 'CD', () => ctx.api.get(`${base}/cd`), (overview) => {
    schedule(overview);
    return render(overview);
  }, { shape: loadingLayout });

  /** The page while it loads (SPEC-120): two environment cards, the Runs heading, run cards. */
  function loadingLayout() {
    const card = () => h('div', { class: 'card card-pad stack-sm' }, h('div', { class: 'card-title' }, bar('lg')), bar(70), bar(50), bar(40));
    return h('div', { class: 'stack' }, h('div', { class: 'cd-envs' }, card(), card()), h('div', { class: 'ci-heading' }, bar('md')), drawShape('items'));
  }

  /** Refreshes quietly: every few seconds while a run is active, else now and then. */
  function schedule(overview) {
    last = overview;
    clearTimeout(timer);
    if (destroyed) return;
    const active = (overview.runs || []).some((r) => r.state === 'running');
    timer = setTimeout(async () => {
      timer = null;
      if (!pollPaused()) await reload({ quiet: true });
      if (timer === null) schedule(last);
    }, active ? POLL_ACTIVE_MS : POLL_IDLE_MS);
  }

  function render(overview) {
    const parts = [h('div', { class: 'cd-envs' }, (overview.environments || []).map((env) => envCard(env, overview)))];
    const runs = overview.runs || [];
    if (runs.length === 0) {
      parts.push(emptyState({ iconName: 'upload', title: 'No deploys yet', text: 'A deploy runs the environment’s deploy command and then its verify command, from a clean checkout of its branch, inside this project’s sandbox. Merges never deploy by themselves.' }));
    } else {
      parts.push(h('h2', { class: 'ci-heading' }, 'Runs'));
      parts.push(h('div', { class: 'item-list' }, runs.map((run) => runCard(run, overview))));
    }
    return h('div', { class: 'stack' }, parts);
  }

  function envCard(env, overview) {
    const st = ENV_STATE[env.state] || ENV_STATE.not_defined;
    const rows = [];
    if (env.about) rows.push(h('p', null, env.about));
    if (env.problem) rows.push(h('div', { class: 'banner banner-warn', role: 'status' }, icon('warn', { size: 16 }), h('span', { class: 'banner-text' }, `CD can’t be read on ${env.branch}: ${env.problem}`)));
    if (env.state === 'not_defined' && !env.problem) {
      rows.push(h('p', { class: 'muted' }, `Add [cd.${env.name}] to yulius.toml on ${env.branch}. An empty table says “nothing to deploy yet”.`));
    }
    const facts = [];
    if (env.promote) {
      // What a deploy promotes (SPEC-152): the integration branch is the dev environment's branch.
      const integration = (overview.environments || []).find((e) => e.name === 'dev');
      facts.push(['Promotes', [integration ? h('code', null, integration.branch) : 'the integration branch',
        env.promotes_from ? h('span', { class: 'muted' }, ` at ${shortSha(env.promotes_from)}`) : null, ` into ${env.branch}`]]);
    }
    if (env.deploy) facts.push(['Deploy', h('code', null, env.deploy)]);
    if (env.verify) facts.push(['Verify', h('code', null, env.verify)]);
    facts.push(['From', [h('code', null, env.branch), env.head ? h('span', { class: 'muted' }, ` at ${shortSha(env.head)}`) : null]]);
    facts.push(['Deployed', env.deployed
      ? [h('code', null, shortSha(env.deployed)), env.head && env.head === env.deployed ? h('span', { class: 'muted' }, ' · the branch head') : h('span', { class: 'muted' }, ' · the branch has moved on')]
      : h('span', { class: 'muted' }, 'never')]);
    rows.push(h('dl', { class: 'cd-facts' }, facts.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)])));
    if ((env.secrets || []).length) {
      const link = `/s/${encodeURIComponent(sandbox.slug)}/secrets?section=agents`;
      rows.push(h('div', { class: 'stack-sm' },
        h('div', { class: 'small muted' }, 'Secrets it gets'),
        h('ul', { class: 'cd-secrets' }, env.secrets.map((s) => {
          const ss = SECRET_STATE[s.state] || SECRET_STATE.missing;
          return h('li', null, h('code', null, s.name), badge(ss.label, ss.tone), s.state === 'ready' ? null : h('a', { href: link }, 'Secrets'));
        }))));
    }
    if (env.last_run) {
      const run = env.last_run;
      rows.push(h('p', { class: 'small muted' }, `Last run ${relLong(run.started_at)} by ${run.requested_by}: ${stateOf(run.state).label.toLowerCase()}`, run.reason ? ` · ${run.reason}` : ''));
    }
    const prod = env.name === 'prod';
    const allowed = prod ? overview.can_deploy_prod : overview.can_deploy_dev;
    const ready = env.state === 'ready';
    const running = (overview.runs || []).some((r) => r.environment === env.name && r.state === 'running');
    const why = !allowed ? (prod ? 'Only an administrator deploys production' : 'Requires write access')
      : !ready ? 'Nothing to deploy yet' : running ? 'A deploy is running' : null;
    // Dev's deploy is the card's main action only when it can run; production's colour is a warning.
    const deploy = h('button', { type: 'button', class: ['btn', prod ? 'btn-danger' : !why && 'btn-primary'], disabled: Boolean(why), title: why, onClick: () => (prod ? deployProd(env, overview) : deployDev(env)) },
      icon('upload', { size: 16 }), prod ? 'Deploy production…' : 'Deploy dev');
    return h('section', { class: 'card card-pad stack-sm cd-env', 'aria-label': `${title(env)} environment` },
      // The title and its badge wrap beside the button, which stays top right (SPEC-152).
      h('div', { class: 'cd-env-head' }, h('div', { class: 'row-wrap cd-env-name' }, h('h2', { class: 'card-title' }, title(env)), badge(st.label, st.tone)), deploy),
      rows,
      prod ? h('p', { class: 'small muted' }, 'Production is always deployed by a person. Agents never promote or deploy it; they ask with “Needs you”.') : null);
  }

  function runCard(run, overview) {
    const st = stateOf(run.state);
    const label = run.environment === 'dev' ? 'Dev' : 'Production';
    const controls = [];
    const may = run.environment === 'prod' ? overview.can_deploy_prod : overview.can_deploy_dev;
    if (may && run.state === 'running') controls.push(h('button', { type: 'button', class: 'btn btn-sm', 'aria-label': `Cancel the ${label.toLowerCase()} deploy at ${shortSha(run.commit)}`, onClick: () => cancel(run) }, 'Cancel'));
    return itemCard({
      iconEl: h('span', { class: ['ci-state', `ci-${run.state}`] }, icon(st.icon, { size: 18, label: st.label })),
      title: [label, ' ', h('code', { class: 'muted' }, shortSha(run.commit))],
      badges: [badge(st.label, st.tone)],
      description: [run.note || `${run.branch}`, run.reason ? ` · ${run.reason}` : null],
      extra: h('div', { class: 'item-meta' }, h('span', { title: dateTime(run.started_at) }, `Started ${relLong(run.started_at)}`), ` · by ${run.requested_by}`),
      controls,
      onOpen: () => showRun(run.run_id),
      openLabel: `Open the ${label.toLowerCase()} deploy at ${shortSha(run.commit)}`,
    });
  }

  async function deployDev(env) {
    const ok = await confirmDialog({
      title: 'Deploy dev?',
      message: `Runs [cd.dev] from ${env.branch}${env.head ? ` at ${shortSha(env.head)}` : ''}: ${[env.deploy && 'the deploy command', env.verify && 'then the verify command'].filter(Boolean).join(', ')}.`,
      confirmLabel: 'Deploy dev',
    });
    if (ok) await start('dev', null, env.head);
  }

  async function deployProd(env, overview) {
    const steps = [];
    if (env.promote) steps.push(`Promote the integration branch${env.promotes_from ? ` at ${shortSha(env.promotes_from)}` : ''} into ${env.branch} (with the project’s promotion method, as you).`);
    if (env.deploy) steps.push(`Run the deploy command at the new ${env.branch} head: ${env.deploy}`);
    if (env.verify) steps.push(`Then verify: ${env.verify}`);
    // How long the person kept the confirmation open is the run's confirmation wait (SPEC-137).
    const opened = performance.now();
    const ok = await confirmDialog({
      title: 'Deploy production?',
      message: 'This changes what your users run. It happens now:',
      details: h('ol', { class: 'cd-steps' }, steps.map((s) => h('li', null, s))),
      confirmLabel: 'Deploy production',
      danger: true,
      typeToConfirm: overview.slug,
    });
    if (ok) await start('prod', overview.slug, env.promote ? env.promotes_from : env.head, Math.round(performance.now() - opened));
  }

  async function start(env, confirm, expect, confirmMs) {
    try {
      // The commit the person saw: the server refuses if the branch moved since.
      const body = {};
      if (confirm) body.confirm = confirm;
      if (expect) body.expect = expect;
      if (Number.isFinite(confirmMs)) body.confirm_ms = Math.max(0, confirmMs);
      const run = await ctx.api.post(`${base}/cd/${env}/deploy`, body);
      toast(env === 'dev' ? 'Deploying dev.' : 'Deploying production.', { kind: 'ok' });
      reload({ quiet: true });
      showRun(run.run_id);
    } catch (error) {
      errorToast(error, env === 'dev' ? 'Couldn’t deploy dev' : 'Couldn’t deploy production');
      reload({ quiet: true });
    }
  }

  async function cancel(run) {
    if (!(await confirmDialog({ title: 'Cancel this deploy?', message: 'Its commands are stopped where they are; the environment may be half deployed until the next deploy.', confirmLabel: 'Cancel deploy', danger: true }))) return;
    try {
      await ctx.api.post(`${base}/cd/runs/${enc(run.run_id)}/cancel`);
      toast('Deploy cancelled.', { kind: 'ok' });
      reload({ quiet: true });
    } catch (error) {
      errorToast(error, 'Couldn’t cancel the deploy');
    }
  }

  /** A run's output in a dialog, refreshed while it runs. */
  function showRun(id) {
    if (openRun) openRun.close();
    let poll = null;
    const head = h('div', { class: 'row-wrap' });
    const output = h('pre', { class: 'code-view run-output', 'aria-live': 'polite', tabindex: '0' }, '');
    // Its stages (SPEC-137): checks, promotion, deploy, verify and what their output names.
    const stages = h('div', { class: 'run-stages' });
    const body = h('div', { class: 'stack' }, head, stages, output);
    const modal = openModal({
      title: 'Deploy', wide: true, body: placeholder('Loading the deploy…', 'loading'),
      actions: [{ label: 'Close' }],
      onClose: () => { clearTimeout(poll); openRun = null; if (!destroyed) replaceQuietly(`/s/${encodeURIComponent(sandbox.slug)}/cd`); },
    });
    openRun = modal;
    replaceQuietly(`/s/${encodeURIComponent(sandbox.slug)}/cd?run=${encodeURIComponent(id)}`);
    let first = true;
    const load = async () => {
      try {
        const run = await ctx.api.get(`${base}/cd/runs/${enc(id)}`);
        if (first) { setChildren(modal.body, body); first = false; }
        const st = stateOf(run.state);
        setChildren(head, badge(st.label, st.tone), h('strong', null, run.environment === 'dev' ? 'Dev' : 'Production'), h('code', { class: 'muted' }, shortSha(run.commit)),
          h('span', { class: 'muted small' }, `${run.branch} · by ${run.requested_by} · started ${relLong(run.started_at)}${run.reason ? ` · ${run.reason}` : ''}`),
          h('span', { class: 'spacer' }),
          h('a', { class: 'btn btn-sm', href: href(pipelineHref(sandbox, `cd_run:${id}`)), dataset: { route: pipelineHref(sandbox, `cd_run:${id}`) } }, icon('clock', { size: 14 }), 'In Pipeline'));
        await loadWaterfall(ctx, sandbox, `cd_run:${id}`, stages, { title: 'Stages of this deploy' });
        const atBottom = output.scrollTop + output.clientHeight >= output.scrollHeight - 8;
        const exit = run.exit_code === null || run.exit_code === undefined ? '' : `\n[exit code ${run.exit_code}]`;
        output.textContent = (run.output || (run.state === 'running' ? 'Waiting for output…' : 'No output.')) + exit;
        if (atBottom || run.state === 'running') output.scrollTop = output.scrollHeight;
        if (run.state === 'running' && modal.dialog.isConnected) poll = setTimeout(load, POLL_ACTIVE_MS);
      } catch (error) {
        if (first) setChildren(modal.body, placeholder(`Couldn’t load the deploy: ${error.message}`, 'error'));
      }
    };
    load();
  }

  /** Starts an Infra bootstrap task with a CD prompt the person completes (CDT-005). */
  function setUp() {
    const text = h('textarea', { class: 'input cd-setup', rows: 14, 'aria-label': 'What the agent should do', autofocus: true }, SETUP_PROMPT);
    openModal({
      title: 'Set up CD with an agent',
      wide: true,
      body: h('div', { class: 'stack-sm' },
        h('p', { class: 'muted' }, 'An agent writes the deploy and verify scripts, defines dev and production in yulius.toml and delivers them in a pull request. It asks you (“Needs you”) for anything only you can give, like a machine’s credentials on the Secrets page.'),
        text),
      actions: [
        { label: 'Cancel' },
        { label: 'Start task', kind: 'primary', run: async () => {
          const task = await ctx.api.post(`${base}/tasks`, { prompt: text.value, workflow_id: 'wf-infra-bootstrap', mode: 'goal', title: 'Set up CD (deployment)' });
          ctx.store.upsertTask(task);
          ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`);
        } },
      ],
    });
  }

  const deepRun = route && route.query && route.query.get('run');
  if (deepRun) showRun(deepRun);

  return {
    el: frame.el,
    key: `cd:${sandbox.id}`,
    destroy: () => { destroyed = true; clearTimeout(timer); if (openRun) openRun.close(); },
  };
}
