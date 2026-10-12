// Mock backend for ?mock=1: an in-memory implementation of docs/api.md with fake
// WebSockets (events, task/shell/login terminals, sandbox browser). It lets the UI be
// exercised without `yulius serve` and documents the real response shapes (the routes
// here are checked against the server router by crates/yulius/tests/ui_contract.rs).
// Everything here is synthetic data.
//
// Like the real server it sends `resync` (not `sandbox.updated`) after sandbox changes,
// answers 404 for the GitHub App admin endpoints that the server does not route yet, and
// keeps unknown usage `null`.
//
// URL options (with ?mock=1): setup=1 (setup required), signedout=1 (start signed out),
// role=member|admin|superadmin (default superadmin), contained=0 (not isolated), github=0,
// update=1|fail (an update is on offer | its install rolls back), helper=0 (no update helper),
// jev=config (a Jev key file is configured), fail=<path> (GETs of that path answer 500).

import { taskInfo } from './status.js';

const MIN = 60e3;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const ORIGIN = window.location.origin;

const enc = new TextEncoder();
const iso = (msAgo) => new Date(Date.now() - msAgo).toISOString();
const isoIn = (msAhead) => new Date(Date.now() + msAhead).toISOString();
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const LIVE = new Set(['queued', 'starting', 'running', 'switching', 'waiting_capacity', 'stopping']);

class MockError extends Error {
  constructor(status, code, message) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

const fail = (status, code, message) => { throw new MockError(status, code, message); };
const clone = (value) => JSON.parse(JSON.stringify(value));

// Named delivery presets, as `GET /api/delivery/presets` returns them (yulius-domain).
const PRESETS = {
  standard: { preset: 'standard', integration_branch: 'dev', production_branch: 'main', merge: 'squash', promote: 'merge_commit', auto_promote: false, require_checks: true, environments: [{ name: 'dev', after: 'integration', command: null, verify: 'manual' }] },
  product: { preset: 'product', integration_branch: 'quartio', production_branch: 'main', merge: 'merge', promote: 'fast_forward', auto_promote: false, require_checks: true, environments: [{ name: 'dev-vm', after: 'integration', command: null, verify: 'manual' }, { name: 'rollout', after: 'production', command: null, verify: 'manual' }] },
  studio: { preset: 'studio', integration_branch: 'quartio', production_branch: 'main', merge: 'merge', promote: 'fast_forward', auto_promote: true, require_checks: false, environments: [] },
};

/** Server-side relabeling: a named preset stays only while every setting equals its defaults. */
function relabel(profile) {
  const named = PRESETS[profile.preset];
  const p = { ...profile, require_checks: profile.require_checks ?? (profile.preset === 'studio' ? false : true) };
  if (!named) return { ...p, preset: 'custom' };
  const same = JSON.stringify({ ...p, preset: '' }) === JSON.stringify({ ...named, preset: '' });
  return same ? p : { ...p, preset: 'custom' };
}

const ENV_NAME_RE = /^[a-z0-9][a-z0-9_-]{0,63}$/;
const BRANCH_RE = /^[A-Za-z0-9._/-]{1,200}$/;

/** PATCH /api/sandboxes/{sid} profile validation (subset of the server's rules). */
function validateProfile(profile) {
  if (!profile || typeof profile !== 'object') fail(422, 'invalid', 'profile is required');
  for (const key of ['integration_branch', 'production_branch']) if (!BRANCH_RE.test(profile[key] || '')) fail(422, 'invalid', `invalid branch name: ${profile[key] || ''}`);
  if (profile.integration_branch === profile.production_branch) fail(422, 'invalid', 'integration and production branches must differ');
  if (!['squash', 'merge', 'rebase'].includes(profile.merge)) fail(422, 'invalid', 'merge must be squash, merge or rebase');
  if (!['merge_commit', 'fast_forward'].includes(profile.promote)) fail(422, 'invalid', 'promote must be merge_commit or fast_forward');
  const envs = Array.isArray(profile.environments) ? profile.environments : [];
  if (envs.length > 8) fail(422, 'invalid', 'at most 8 environments');
  const seen = new Set();
  for (const env of envs) {
    if (!ENV_NAME_RE.test(env.name || '')) fail(422, 'invalid', `invalid environment name: ${env.name || ''}`);
    if (seen.has(env.name)) fail(422, 'invalid', `duplicate environment: ${env.name}`);
    seen.add(env.name);
    if (env.command !== null && env.command !== undefined && (!String(env.command).trim() || /[\r\n]/.test(env.command) || env.command.length > 2000)) fail(422, 'invalid', `invalid command for ${env.name}`);
    if (env.verify === 'command' && !env.command) fail(422, 'invalid', `${env.name}: verify "command" needs a command`);
  }
}

function json(status, body) {
  if (status === 204) return new Response(null, { status });
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
}

// ------------------------------------------------------------------ seed data

const PLAN = ['Write task spec', 'Design the API', 'Implement checkout API', 'Write tests', 'Wire the UI', 'QA in the browser', 'Open PR'];

function makeSteps(taskId, done, current, total = PLAN.length) {
  const steps = [];
  for (let i = 0; i < total; i += 1) {
    let status = 'pending';
    if (i < done) status = 'done';
    else if (i === done && current) status = 'in_progress';
    steps.push({
      id: `${taskId}-s${i + 1}`,
      ordinal: i + 1,
      title: PLAN[i] || `Step ${i + 1}`,
      status,
      note: status === 'in_progress' ? 'Handlers for /cart and /checkout; Stripe test keys only.' : status === 'done' && i === 0 ? 'Spec at docs/specs/2026-10-07-checkout-api.md' : '',
      updated_by: i < done ? (i % 2 ? 'task #12 (claude)' : 'mkarp') : status === 'in_progress' ? 'task #12 (claude)' : '',
      updated_at: status === 'pending' ? null : iso((total - i) * 11 * MIN),
      // The one step that cannot be split declares a larger effort (STP-006).
      effort: PLAN[i] === 'Implement checkout API' ? 3 : 1,
    });
  }
  return steps;
}

function seed(params) {
  const role = params.get('role') || 'superadmin';
  // `github_linked=0`: the signed-in account has no GitHub link. Like the server, only the
  // superadmin still sees every sandbox (ACC-012); anyone else sees none.
  const linked = params.get('github_linked') !== '0';
  const db = {
    setupRequired: params.get('setup') === '1',
    signedIn: params.get('signedout') !== '1',
    me: {
      user: {
        id: 'usr_1', login: 'mkarp', display_name: 'Michal Karp', avatar_url: null,
        is_superadmin: role === 'superadmin', is_instance_admin: role !== 'member', github_linked: linked,
      },
      csrf_token: 'mock-csrf-token',
      instance: { owner_login: 'Quartio-Studio', owner_kind: 'org', public_url: ORIGIN, github_configured: params.get('github') !== '0', contained: params.get('contained') !== '0', upload_max_bytes: 25 * 1024 * 1024 },
    },
    sandboxes: [],
    tasks: [],
    messages: [],
    recovery: { available: true, locked: false, attempts_left: 3 },
    eventSockets: new Set(),
    sessions: new Map(),
    browsers: new Map(),
    shells: new Map(), // id → {id, sandbox_id, name, task_id, created_at, session}
    // Instance updates and the Jev key (SPEC-002 ADR-18): `update=1` (main moved on), `update=fail`
    // (main moved on and its install is rolled back), `helper=0` (no update helper), `jev=config`.
    update: {
      current: '68b4f5f2c1d0e9a8b7c6d5e4f3a2b1c0d9e8f7a6',
      offer: ['1', 'fail'].includes(params.get('update')) ? '9c1e7d3b5a2f4e6d8c0b1a3f5e7d9c2b4a6f8e0d' : null,
      fail: params.get('update') === 'fail',
      helper: params.get('helper') !== '0',
      checkedAt: ['1', 'fail'].includes(params.get('update')) ? iso(10 * MIN) : null,
      run: null,
    },
    jev: { key: null, setAt: null, setBy: null, config: params.get('jev') === 'config' },
    // fail=<path> answers 500 to GET requests of that exact path, to show how a view says a load
    // failed (in place, naming what failed, with Try again; #147).
    failPath: params.get('fail') || null,

    nextId: 100,
  };
  // Product: the product preset with a dev-vm deploy command (so it is "custom"): exercises
  // command runs plus manual confirmation. Studio: continuous, no environments.
  const productProfile = relabel({ ...clone(PRESETS.product), environments: [{ name: 'dev-vm', after: 'integration', command: 'node scripts/deployer.mjs --target dev-vm', verify: 'manual' }, { name: 'rollout', after: 'production', command: null, verify: 'manual' }] });
  db.sandboxes = [
    { id: 'sb_studio', slug: 'studio', name: 'studio', repo: { owner: 'Quartio-Studio', name: 'studio', url: 'https://github.com/Quartio-Studio/studio' }, access: role === 'member' ? 'write' : 'admin', default_branch: 'main', profile: clone(PRESETS.studio), onboarding: 'ready', port_base: 20000, port_count: 20, counts: { running: 0, needs_attention: 0, queued: 0 } },
    { id: 'sb_product', slug: 'product', name: 'product', repo: { owner: 'Quartio-Studio', name: 'product', url: 'https://github.com/Quartio-Studio/product' }, access: role === 'member' ? 'write' : 'admin', default_branch: 'main', profile: productProfile, onboarding: 'ready', port_base: 20020, port_count: 20, counts: { running: 0, needs_attention: 0, queued: 0 } },
    { id: 'sb_three', slug: 'sandbox-3', name: 'sandbox-3', repo: { owner: 'Quartio-Studio', name: 'sandbox-3', url: 'https://github.com/Quartio-Studio/sandbox-3' }, access: 'read', default_branch: 'main', profile: clone(PRESETS.standard), onboarding: 'not_needed', port_base: 20040, port_count: 20, counts: { running: 0, needs_attention: 0, queued: 0 } },
  ];
  const t = (sandbox, number, title, harness, status, activity, extra = {}) => {
    const id = `tsk_${sandbox.slice(3)}_${number}`;
    const done = extra.done ?? Math.min(PLAN.length, number % 5);
    const current = extra.current ?? (LIVE.has(status) && status !== 'queued');
    return {
      id, sandbox_id: sandbox, number, title, harness, status, activity,
      branch: extra.branch === null ? null : `feat/${title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')}`,
      origin: extra.origin || 'ui', created_by: { login: extra.author || 'mkarp' },
      // Created before its last change (an old task's sessions never last a negative time).
      created_at: iso(Math.max(extra.age || number * 3 * HOUR, (extra.updated ?? number * 7 * MIN) + HOUR)), updated_at: iso(extra.updated ?? number * 7 * MIN),
      unread_messages: extra.unread || 0, archived: Boolean(extra.archived),
      account_label: LIVE.has(status) ? 'Claude #2' : null,
      stepsList: extra.noSteps ? [] : makeSteps(id, done, current),
      model: 'claude-opus-4',
      effort: 'high',
      delivery: extra.delivery || null, // {stage, environment, pr} once delivery started
      // Paused for subscription capacity: when the earliest window resets (ROT-010).
      resumes_at: status === 'waiting_capacity' ? new Date(Date.now() + 38 * MIN).toISOString() : null,
      ...(extra.mode ? { mode: extra.mode } : {}),
      // Background shells the idle agent left running (TSK-014).
      background_shells: extra.shells ?? null,
      background_shells_since: extra.shells ? new Date(Date.now() - 12 * MIN).toISOString() : null,
    };
  };
  const pr = (number, title, state, base, checks) => ({ number, url: `https://github.com/Quartio-Studio/${number > 400 ? 'product' : 'studio'}/pull/${number}`, title, state, base, head: `feat/${title.toLowerCase().replace(/[^a-z0-9]+/g, '-')}`, checks });
  db.tasks = [
    // Goal achieved after a clean turn, no shell left: Done with a green check although its pull
    // request's checks still run (TSK-014, F 2026-10-10).
    t('sb_studio', 13, 'Improve the code', 'claude', 'running', 'done', { mode: 'goal', done: 7, current: false, updated: 6 * MIN, shells: 0, delivery: { stage: 'pr_open', environment: null, pr: pr(520, 'Improve the code', 'open', 'dev', 'pending') } }),
    // Goal achieved, but a background shell (a dev server) still runs: Working, with the shell said.
    t('sb_studio', 14, 'Start the preview server', 'claude', 'running', 'done', { mode: 'goal', done: 7, current: false, updated: 2 * MIN, shells: 1 }),
    t('sb_studio', 12, 'Checkout API', 'claude', 'running', 'working', { done: 2, unread: 2, updated: 20e3 }),
    // Finished its turn with a tested pull request (checks green) and a complete plan: "Done" (#35, #139).
    t('sb_studio', 11, '3D model viewer', 'claude', 'running', 'waiting', { done: 7, current: false, author: 'anna-q', delivery: { stage: 'pr_open', environment: null, pr: pr(221, '3D model viewer', 'open', 'dev', 'success') } }),
    t('sb_studio', 10, 'AWS login for deploy', 'claude', 'running', 'needs_attention', { done: 4 }),
    t('sb_studio', 9, 'Move to the next account', 'claude', 'switching', null, { done: 3 }),
    t('sb_studio', 8, 'Nightly feedback digest', 'claude', 'queued', null, { origin: 'automation', noSteps: true }),
    t('sb_studio', 7, 'Search indexing', 'claude', 'waiting_capacity', null, { done: 1 }),
    t('sb_studio', 6, 'Fix flaky checkout test', 'claude', 'starting', null, { noSteps: true }),
    t('sb_studio', 5, 'Docs restructure', 'claude', 'closed', null, { done: 7, current: false, updated: 5 * HOUR, delivery: { stage: 'pr_open', environment: null, pr: pr(214, 'Docs restructure', 'open', 'quartio', 'success') } }),
    t('sb_studio', 4, 'Payment webhook', 'claude', 'stopped_cancelled', null, { done: 3, current: false, updated: 9 * HOUR, delivery: { stage: 'pr_open', environment: null, pr: pr(209, 'Payment webhook', 'open', 'quartio', 'failure') } }),
    t('sb_studio', 3, 'Image pipeline', 'claude', 'stopped_crashed', null, { done: 2, current: false, updated: DAY }),
    t('sb_studio', 2, 'Legacy import', 'claude', 'failed_spawn_error', null, { noSteps: true, updated: 2 * DAY, origin: 'import' }),
    t('sb_studio', 1, 'Old landing page', 'claude', 'closed', null, { done: 7, current: false, archived: true, updated: 6 * DAY, delivery: { stage: 'production_done', environment: null, pr: pr(180, 'Old landing page', 'merged', 'quartio', 'success') } }),
    t('sb_product', 1, 'Prepare repository for Yulius', 'claude', 'closed', null, { origin: 'onboarding', done: 7, current: false, updated: 2 * DAY }),
    t('sb_product', 2, 'Pricing page', 'claude', 'closed', null, { done: 7, current: false, updated: 3 * HOUR, delivery: { stage: 'merged_integration', environment: null, pr: pr(512, 'Pricing page', 'merged', 'quartio', 'none') } }),
    t('sb_product', 3, 'Search filters', 'claude', 'closed', null, { done: 7, current: false, updated: 70 * MIN, author: 'anna-q', delivery: { stage: 'pr_open', environment: null, pr: pr(515, 'Search filters', 'open', 'quartio', 'success') } }),
    t('sb_product', 4, 'Invoice PDF export', 'claude', 'running', 'working', { done: 3, updated: 4 * MIN }),
    t('sb_product', 5, 'Onboarding emails', 'claude', 'closed', null, { done: 7, current: false, updated: 2 * DAY, delivery: { stage: 'production_done', environment: null, pr: pr(498, 'Onboarding emails', 'merged', 'quartio', 'none') } }),
    t('sb_product', 6, 'Team invites', 'claude', 'closed', null, { done: 7, current: false, updated: 26 * HOUR, author: 'anna-q', delivery: { stage: 'merged_integration', environment: null, pr: pr(505, 'Team invites', 'merged', 'quartio', 'none') } }),
    // Waits for #4 to be Done (TSK-012).
    t('sb_product', 7, 'Invoice emails', 'claude', 'chained', null, { noSteps: true, updated: 3 * MIN }),
    t('sb_three', 2, 'Write README', 'claude', 'closed', null, { done: 7, current: false }),
    t('sb_three', 1, 'Explore data model', 'claude', 'running', 'working', { done: 1, author: 'anna-q' }),
  ];
  {
    const invoices = db.tasks.find((x) => x.id === 'tsk_product_4');
    const emails = db.tasks.find((x) => x.id === 'tsk_product_7');
    if (invoices && emails) {
      chainAfter(emails, invoices);
      // A prompt typed into #4 that the scope check held (TSK-013).
      invoices.heldPrompt = { id: 'hold_1', class: 'new_scope', excerpt: 'Also add a dark mode to the invoice settings page.', percent: 92, typed_by: 'mkarp', created_at: iso(2 * MIN) };
    }
  }
  db.messages = [
    // Notices from Yulius itself (delivery and disk notices, scope changes); tasks do not message each other.
    { id: 'msg_1', task: 'tsk_studio_12', from: { task_id: null, login: null, title: 'Yulius' }, to_task_id: 'tsk_studio_12', kind: 'request', subject: 'Scope changed by the user', body: 'The user dropped the invoice export from this task. Stop work on it and continue with the checkout API.', created_at: iso(42 * MIN), read: true },
    { id: 'msg_2', task: 'tsk_studio_12', from: { task_id: null, login: null, title: 'Yulius' }, to_task_id: 'tsk_studio_12', kind: 'request', subject: 'Checks failed on the pull request', body: 'The CI gate failed on the head of your pull request. Fix it on this branch, test-first, then push.', created_at: iso(8 * MIN), read: false },
    // The agent asks people (message_send to="people"): they answer in its terminal (F 2026-10-10).
    { id: 'msg_3', task: 'tsk_studio_12', from: { task_id: 'tsk_studio_12', login: null, title: '#12 Checkout API' }, to_task_id: null, kind: 'question', subject: 'Stripe keys', body: 'Should checkout use the Stripe test keys or the live ones? The AWS sign-in tab is open: ' + `${ORIGIN}/s/studio/browser?tab=tab-aws`, created_at: iso(3 * MIN), read: true },
  ];
  db.accounts = [
    { id: 'acc_c1', provider: 'anthropic', label: 'Claude #1', status: 'ok', usage: { primary: { used_percent: 42.0, resets_at: isoIn(2.2 * HOUR), window_minutes: 300 }, secondary: { used_percent: 18.5, resets_at: isoIn(4 * DAY), window_minutes: 10080 }, observed_at: iso(2 * MIN) }, last_error: null, in_use_by: 1 },
    { id: 'acc_c2', provider: 'anthropic', label: 'Claude #2', status: 'ok', usage: { primary: { used_percent: 87.3, resets_at: isoIn(1.1 * HOUR), window_minutes: 300 }, secondary: { used_percent: 61.0, resets_at: isoIn(2 * DAY), window_minutes: 10080 }, observed_at: iso(1 * MIN) }, last_error: 'usage: Usage updates again in about 12 min.', in_use_by: 3 },
    { id: 'acc_c3', provider: 'anthropic', label: 'Claude #3', status: 'relogin', usage: { primary: null, secondary: null, observed_at: null }, last_error: 'OAuth refresh token expired; sign in again.', in_use_by: 0 },
    { id: 'acc_x1', provider: 'anthropic', label: 'Claude #4', status: 'ok', usage: { primary: { used_percent: 12.0, resets_at: isoIn(3.5 * HOUR), window_minutes: 300 }, secondary: { used_percent: 33.0, resets_at: isoIn(5 * DAY), window_minutes: 10080 }, observed_at: iso(4 * MIN) }, last_error: null, in_use_by: 2 },
    { id: 'acc_x2', provider: 'anthropic', label: 'Claude #5', status: 'limited', usage: { primary: { used_percent: 99.6, resets_at: isoIn(1.9 * HOUR), window_minutes: 300 }, secondary: { used_percent: 71.0, resets_at: isoIn(3 * DAY), window_minutes: 10080 }, observed_at: iso(30e3) }, last_error: null, in_use_by: 0 },
    { id: 'acc_x3', provider: 'anthropic', label: 'Claude spare', status: 'ok', usage: { primary: { used_percent: 0.5, resets_at: null, window_minutes: 300 }, secondary: null, observed_at: iso(3 * DAY) }, last_error: null, in_use_by: 0 },
  ];
  // The default workflows ship with the server (catalog/workflows/*.md, AUT-006): read-only, first.
  db.workflows = [
    { id: 'wf-feature', sandbox: null, name: 'Feature', description: 'New or changed behaviour, from a spec through review and tests to production.', when_to_use: 'The task adds or changes behaviour that users or other code rely on.', steps: ['Write the spec', 'Implement', 'Review the change', 'Test', 'Deliver to the integration branch', 'Release to production'], instructions: 'Start with the task spec. Implement only what it describes, prove it with tests, review your own diff, then deliver through the delivery profile.', gates: { require_spec: true, require_tests: true }, scope: 'instance', builtin: true },
    { id: 'wf-fix', sandbox: null, name: 'Fix', description: 'A bug fixed from a reproduction, with a regression test.', when_to_use: 'Something that used to work, or should work, behaves wrongly.', steps: ['Reproduce with a failing test', 'Fix the cause', 'Review the change', 'Deliver'], instructions: 'Reproduce the bug first, with a test that fails because of it.', gates: { require_spec: false, require_tests: true }, scope: 'instance', builtin: true },
    { id: 'wf-research', sandbox: null, name: 'Research', description: 'Investigate a question and report findings, without changing the product.', when_to_use: 'The task is a question about the product, the code or an option to choose.', steps: ['Investigate', 'Report the findings'], instructions: 'Answer with evidence. Do not change the product’s code.', gates: { require_spec: false, require_tests: false }, scope: 'instance', builtin: true },
    { id: 'wf_release', sandbox: 'sb_studio', name: 'Release notes', description: 'Collect what shipped and write the release notes.', when_to_use: '', steps: ['Collect merged pull requests', 'Write the notes'], instructions: '', gates: { require_spec: false, require_tests: false }, scope: 'sandbox', builtin: false },
  ];
  db.skills = [
    { name: 'yulius-rust-slice', description: 'Implement a vertical Rust slice with tests and docs.', source: 'repository', path: '.agents/skills/yulius-rust-slice/SKILL.md', enabled: true },
    { name: 'design', description: 'Make UI designs consistent with the Quartio design system.', source: 'repository', path: '.claude/skills/design/SKILL.md', enabled: true },
    { name: 'quartio-brand', description: 'Brand voice and visual identity for customer-facing copy.', source: 'instance', path: 'instance/skills/quartio-brand/SKILL.md', enabled: true },
    { name: 'delivery-pipeline', description: 'The one autonomous path every feature and fix takes, from a ready issue to verified production.', source: 'yulius', path: 'skills/yulius/delivery-pipeline/SKILL.md', enabled: true },
    { name: 'om-code-review', description: 'Review a diff against correctness, security, contracts and tests with a mechanical verdict.', source: 'open-mercato', path: 'skills/open-mercato/om-code-review/SKILL.md', enabled: true },
    { name: 'test-driven-development', description: 'Write the test first, watch it fail for the right reason, then the minimal code.', source: 'superpowers', path: 'skills/superpowers/test-driven-development/SKILL.md', enabled: false },
  ];
  db.automations = [
    { id: 'aut_1', sandbox: 'sb_studio', name: 'Daily customer feedback', cron: '0 9 * * *', utc_offset_minutes: 120, prompt: 'Create a task implementing yesterday’s customer feedback from the Quartio MCP.', workflow_id: 'wf-feature', harness: 'claude', enabled: true, created_via: 'agent', next_run_at: isoIn(14 * HOUR), last_run: { scheduled_for: iso(10 * HOUR), state: 'task_created', task_id: 'tsk_studio_8' } },
    { id: 'aut_2', sandbox: 'sb_studio', name: 'Weekly dependency audit', cron: '0 7 * * 1', utc_offset_minutes: 0, prompt: 'Audit dependencies for advisories and open a PR with safe upgrades.', workflow_id: null, harness: 'claude', enabled: false, created_via: 'ui', next_run_at: null, last_run: null },
  ];
  db.mcp = [
    { id: 'mcp_quartio', sandbox: null, scope: 'instance', name: 'quartio', transport: 'http', command: null, args: [], url: 'https://mcp.quartio.ai/mcp', env_keys: [], header_keys: ['Authorization'], enabled: true },
    { id: 'mcp_apify', sandbox: 'sb_studio', scope: 'sandbox', name: 'apify', transport: 'stdio', command: 'npx', args: ['-y', '@apify/actors-mcp-server'], url: null, env_keys: ['APIFY_TOKEN'], header_keys: [], enabled: true },
  ];
  // AdminUser (the superadmin is not listed).
  db.users = [
    { id: 'usr_1', github_login: 'mkarp', display_name: 'Michal Karp', is_instance_admin: true, disabled: false, github_linked: true },
    { id: 'usr_2', github_login: 'anna-q', display_name: 'Anna', is_instance_admin: false, disabled: false, github_linked: true },
    { id: 'usr_3', github_login: 'new-hire', display_name: 'new-hire', is_instance_admin: false, disabled: false, github_linked: false },
    { id: 'usr_4', github_login: 'old-contractor', display_name: 'old-contractor', is_instance_admin: false, disabled: true, github_linked: true },
  ];
  db.runs = []; // EnvironmentRun records, newest last
  db.ci = seedCi();
  const pastRun = (n, env, kind, by, ago) => ({ run_id: `run_seed_${n}`, sandbox_id: 'sb_product', environment: env, kind, state: 'succeeded', command: kind === 'manual' ? null : 'node scripts/deployer.mjs --target dev-vm', commit: 'b7e9f01c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a', requested_by: by, output: kind === 'manual' ? `${by} confirmed ${env}.` : 'deploying… done in 2m 14s', exit_code: kind === 'manual' ? null : 0, started_at: iso(ago), ended_at: iso(ago - 2 * MIN), startedMs: Date.now() - ago - 1e9 });
  db.runs.push(pastRun(1, 'dev-vm', 'command', 'mkarp', 2 * DAY + HOUR), pastRun(2, 'dev-vm', 'manual', 'mkarp', 2 * DAY + 30 * MIN), pastRun(3, 'rollout', 'manual', 'anna-q', 2 * DAY - HOUR), pastRun(4, 'dev-vm', 'command', 'mkarp', 3 * HOUR));
  db.promotionPr = new Map(); // sandbox id → open promotion PR URL (merge_commit promotion)
  // `sandboxes=0`: a fresh instance with no sandbox yet; the home screen shows its first steps
  // (import a repository, create one from a template).
  if ((!linked && role !== 'superadmin') || params.get('sandboxes') === '0') {
    db.sandboxes = [];
    db.tasks = [];
  }
  // `tasks=0`: sandboxes without any task yet (the composer then offers no start rule, SPEC-022).
  if (params.get('tasks') === '0') db.tasks = [];
  recount(db);
  return db;
}

// ------------------------------------------------------------------ helpers

function stepsSummary(task) {
  const list = task.stepsList;
  const current = list.find((s) => s.status === 'in_progress');
  // Skipped steps count as finished (STP-005); progress is weighted by declared effort (STP-006).
  const finished = list.filter((s) => s.status === 'done' || s.status === 'skipped');
  const effort = (s) => Math.min(3, Math.max(1, s.effort || 1));
  const total = list.reduce((sum, s) => sum + effort(s), 0);
  const percent = total ? Math.floor((finished.reduce((sum, s) => sum + effort(s), 0) * 100) / total) : 0;
  return { done: finished.length, total: list.length, current: current ? current.title : null, percent };
}

function summary(task) {
  return {
    id: task.id, sandbox_id: task.sandbox_id, number: task.number, title: task.title, harness: task.harness,
    status: task.status, activity: task.status === 'running' || task.status === 'waiting_capacity' ? task.activity : null,
    branch: task.branch, origin: task.origin, created_by: task.created_by, created_at: task.created_at, updated_at: task.updated_at,
    steps: stepsSummary(task), unread_messages: task.unread_messages, archived: task.archived, account_label: task.account_label,
    resumes_at: task.status === 'waiting_capacity' ? task.resumes_at || null : null,
    // Chained tasks (TSK-012) and a held prompt (TSK-013).
    after: task.afterLink ? { ...task.afterLink } : null,
    next: (task.nextLinks || []).map((link) => ({ ...link })),
    held_prompt: task.heldPrompt ? { ...task.heldPrompt } : null,
    ...(task.mode ? { mode: task.mode } : {}),
    // How Auto chose the mode and workflow (TSK-015).
    auto: task.auto ? { ...task.auto } : null,
    // Background shells an idle agent left running (TSK-014).
    background_shells: task.status === 'running' ? task.background_shells ?? null : null,
    background_shells_since: task.status === 'running' && task.background_shells ? task.background_shells_since ?? null : null,
  };
}

/** Links `task` to run after `before` (TSK-012); both summaries show the chain. */
function chainAfter(task, before, rule = null) {
  task.afterLink = { task_id: before.id, number: before.number, title: before.title, status: before.status, blocked: null, rule };
  before.nextLinks = [...(before.nextLinks || []), { task_id: task.id, number: task.number, title: task.title, status: task.status, blocked: null }];
}

/** Where a stage sits in the profile: the next environment, promotion, or nothing (domain `position`). */
function position(profile, stage, environment) {
  const envs = Array.isArray(profile.environments) ? profile.environments : [];
  const integration = envs.filter((e) => e.after === 'integration');
  const production = envs.filter((e) => e.after === 'production');
  if (stage === 'none') return { kind: 'no_pr' };
  if (stage === 'pr_open') return { kind: 'pr_open' };
  if (stage === 'merged_integration') return integration[0] ? { kind: 'env', env: integration[0] } : { kind: 'promote' };
  if (stage === 'env_done') {
    const i = integration.findIndex((e) => e.name === environment);
    if (i >= 0) return integration[i + 1] ? { kind: 'env', env: integration[i + 1] } : { kind: 'promote' };
    const j = production.findIndex((e) => e.name === environment);
    if (j >= 0 && production[j + 1]) return { kind: 'env', env: production[j + 1] };
    return { kind: 'done' };
  }
  if (stage === 'promoted') return production[0] ? { kind: 'env', env: production[0] } : { kind: 'done' };
  return { kind: 'done' };
}

function specProblems(task) {
  return task.number === 12 && task.sandbox_id === 'sb_studio' ? ['Plan: 3 unchecked items without notes', 'Missing section: Verification'] : [];
}

/** `Delivery` of a task: next actions the domain accepts now, blocked ones with the reason. */
/**
 * Check results the GitHub App may not read (#111), as the server reports them: the studio
 * sandbox's App lacks "Commit statuses", so its results come from check runs alone. The link to
 * the App's permissions is for instance administrators only.
 */
function checksAccess(db, sandbox) {
  if (sandbox.id !== 'sb_studio') return null;
  const repo = `${sandbox.repo.owner}/${sandbox.repo.name}`;
  return {
    repo,
    missing: ['Commit statuses'],
    read_from: ['check runs'],
    message: `Yulius can't read commit statuses for ${repo}: the GitHub App needs the 'Commit statuses: read' permission. Results come from check runs only.`,
    settings_url: db.me.user.is_superadmin || db.me.user.is_instance_admin ? `https://github.com/organizations/${sandbox.repo.owner}/settings/apps/yulius-quartio-studio/permissions` : null,
  };
}

function delivery(db, task) {
  const sandbox = sandboxOf(db, task.sandbox_id);
  const profile = sandbox.profile;
  const state = task.delivery || { stage: 'none', environment: null, pr: null };
  const problems = specProblems(task);
  const stepsOpen = task.stepsList.filter((s) => s.status !== 'done' && s.status !== 'skipped').length;
  const next = [];
  const blocked = [];
  const pos = position(profile, state.stage, state.environment);
  if (pos.kind === 'no_pr') {
    if (task.branch === null) blocked.push({ action: 'open_pr', reason: 'the task has no branch yet' });
    else if (problems.length) blocked.push({ action: 'open_pr', reason: `the task spec is not valid: ${problems[0]}` });
    else next.push('open_pr');
  } else if (pos.kind === 'pr_open') {
    const checks = state.pr ? state.pr.checks : 'none';
    // As the server: checks that can't be read never count as passed (#111).
    if (profile.require_checks !== false && checks === 'unknown') blocked.push({ action: 'merge', reason: 'the pull request\u2019s checks can\u2019t be read, so they don\u2019t count as passed' });
    else if (profile.require_checks !== false && checks !== 'success') blocked.push({ action: 'merge', reason: `checks are ${checks}; the profile requires green checks` });
    else if (stepsOpen) blocked.push({ action: 'merge', reason: `${stepsOpen} plan steps are still open` });
    else next.push('merge');
  } else if (pos.kind === 'env') {
    if (pos.env.command) next.push(`run_env:${pos.env.name}`);
    if (pos.env.verify === 'manual') next.push(`confirm_env:${pos.env.name}`);
  } else if (pos.kind === 'promote') {
    next.push('promote');
  }
  const pull = state.pr ? { ...state.pr, checks: state.pr.state === 'open' ? state.pr.checks : 'none' } : null;
  return {
    stage: state.stage,
    environment: state.stage === 'env_done' ? state.environment : null,
    pr: pull,
    spec_path: task.stepsList.length && task.branch ? `docs/specs/2026-10-07-${task.branch.replace('feat/', '')}.md` : null,
    spec_valid: problems.length === 0 && task.stepsList.length > 0,
    spec_problems: task.stepsList.length ? problems : ['No spec is registered and none under docs/specs names this task.'],
    steps_open: stepsOpen,
    next_actions: next,
    blocked,
    notes: [],
    checks_access: pull && pull.state === 'open' ? checksAccess(db, sandbox) : null,
  };
}

/** Advance every task of the sandbox for which `envName` is due (after a run or confirmation). */
function advanceEnvironment(db, sandbox, envName) {
  const envs = sandbox.profile.environments || [];
  const production = envs.filter((e) => e.after === 'production');
  for (const task of db.tasks.filter((t) => t.sandbox_id === sandbox.id && t.delivery)) {
    const pos = position(sandbox.profile, task.delivery.stage, task.delivery.environment);
    if (pos.kind !== 'env' || pos.env.name !== envName) continue;
    const last = production.length && production[production.length - 1].name === envName;
    task.delivery = { ...task.delivery, stage: last ? 'production_done' : 'env_done', environment: last ? null : envName };
    touch(db, task);
  }
}

function detail(db, task) {
  const live = LIVE.has(task.status);
  const ran = task.origin !== 'onboarding' || task.stepsList.some((s) => s.status === 'done');
  return {
    ...summary(task),
    spec_path: task.stepsList.length && task.branch ? `docs/specs/2026-10-07-${task.branch.replace('feat/', '')}.md` : null,
    worktree: task.branch ? `/srv/yulius/sandboxes/${task.sandbox_id.slice(3)}/worktrees/${task.number}` : null,
    attempts: ran ? [{ seq: 1, harness: task.harness, account_label: task.account_label || 'Claude #1', launch_kind: 'new', state: live ? 'live' : 'exited', started_at: task.created_at, ended_at: live ? null : task.updated_at, exit_code: live ? null : 0, exit_reason: live ? null : 'normal' }] : [],
    steps_list: task.stepsList,
    // Unknown usage stays null (task #6 never reported tokens).
    metrics: task.number === 6
      ? { wall_ms: Date.now() - Date.parse(task.created_at), active_ms: null, input_tokens: null, output_tokens: null, cache_read_tokens: null, cache_write_tokens: null, models: [], switches: 0, turns: null }
      : { wall_ms: Date.now() - Date.parse(task.created_at), active_ms: 41 * MIN, input_tokens: 182340, output_tokens: 23410, cache_read_tokens: 1204000, cache_write_tokens: 90210, models: [task.model], switches: task.number === 9 ? 1 : 0, turns: 17 },
    feedback: [],
    delivery: delivery(db, task),
    switches: task.number === 9 ? [{ at: iso(2 * MIN), reason: 'account exhausted', from: 'Claude Code (Claude #2)', to: 'Claude Code (Claude #4)' }] : [],
  };
}

/** A task's pull request as the Git overview shows it (`TaskBranch.pr`), or null. */
function pullOf(task) {
  const pr = task.branch && task.delivery && task.delivery.pr;
  return pr ? { ...pr, checks: pr.state === 'open' ? pr.checks : 'none' } : null;
}

/**
 * The sandbox counters. "Needs you" is the sidebar's rule (status.js taskInfo, the server's
 * `AttentionFacts`), so a card, a tab and the sidebar never disagree (#147).
 */
function recount(db) {
  for (const sandbox of db.sandboxes) {
    const tasks = db.tasks.filter((t) => t.sandbox_id === sandbox.id && !t.archived);
    sandbox.counts = {
      // A live process, as the server counts it (TaskStatus::is_live).
      running: tasks.filter((t) => ['starting', 'running', 'switching', 'stopping'].includes(t.status)).length,
      needs_attention: tasks.filter((t) => taskInfo(summary(t), null, pullOf(t)).group === 'needs').length,
      queued: tasks.filter((t) => t.status === 'queued').length,
    };
  }
}

function emit(db, event) {
  const data = JSON.stringify(event);
  for (const socket of db.eventSockets) socket.push(data);
}

/** Sandbox changes: the server publishes `resync` (sandbox views differ per user). */
function resync(db) {
  emit(db, { type: 'resync' });
}

/** Task change: `task.updated` only; sandbox counters are not pushed (clients refetch). */
function touch(db, task, changes = {}) {
  Object.assign(task, changes, { updated_at: new Date().toISOString() });
  recount(db);
  emit(db, { type: 'task.updated', task: summary(task) });
  const session = db.sessions.get(task.id);
  if (session) session.taskChanged();
}

function sandboxOf(db, sid) {
  const sandbox = db.sandboxes.find((s) => s.id === sid);
  if (!sandbox) fail(404, 'not_found', 'No such sandbox');
  return sandbox;
}

function taskOf(db, tid) {
  const task = db.tasks.find((t) => t.id === tid);
  if (!task || !db.sandboxes.some((s) => s.id === task.sandbox_id)) fail(404, 'not_found', 'No such task');
  return task;
}

const RANK = { read: 1, write: 2, admin: 3 };
function need(sandbox, level) {
  if ((RANK[sandbox.access] || 0) < RANK[level]) fail(403, 'forbidden', `Requires ${level} permission on ${sandbox.repo.owner}/${sandbox.repo.name} (you have ${sandbox.access}).`);
}

function needTaskWrite(db, task) {
  need(sandboxOf(db, task.sandbox_id), 'write');
}

function hashString(s) {
  let h = 2166136261;
  for (let i = 0; i < s.length; i += 1) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619); }
  return h >>> 0;
}

function later(fn, ms) {
  setTimeout(fn, ms);
}

// ------------------------------------------------------------------ HTTP routes

const R = [];
const route = (method, pattern, handler) => R.push([method, new RegExp(`^${pattern}$`), handler]);
const ID = '([^/]+)';

route('GET', '/api/setup/status', ({ db }) => ({ setup_required: db.setupRequired }));
route('POST', '/api/setup', ({ db, body }) => {
  if (!db.setupRequired) fail(403, 'forbidden', 'This instance is already set up.');
  if (body.token !== 'YUL-SETUP-1234') fail(403, 'forbidden', 'Wrong setup token. (Mock token: YUL-SETUP-1234)');
  if (body.password !== body.password_confirm) fail(422, 'invalid', 'Passwords do not match.');
  if (!body.confirm_saved) fail(422, 'invalid', 'Confirm that the password is stored safely.');
  db.setupRequired = false;
  db.signedIn = true;
  db.me.user = { id: 'usr_root', login: 'superadmin', display_name: 'Superadmin', avatar_url: null, is_superadmin: true, is_instance_admin: true, github_linked: false };
  db.me.instance.github_configured = false;
  return json(201, db.me);
});
route('POST', '/api/auth/password', ({ db, body }) => {
  if (body.password !== 'correct horse battery') fail(401, 'unauthenticated', 'Wrong password. (Mock password: correct horse battery)');
  db.signedIn = true;
  db.me.user = { ...db.me.user, is_superadmin: true };
  return db.me;
});
// Mock only: stands in for the GitHub OAuth round trip (/api/auth/github/start → callback).
route('POST', '/api/auth/mock-github', ({ db }) => { db.signedIn = true; return db.me; });
route('POST', '/api/auth/logout', ({ db }) => { db.signedIn = false; for (const s of db.eventSockets) s.serverClose(1000); return json(204); });
route('GET', '/api/me', ({ db }) => db.me);
// Superadmin recovery (ACC-011): a code is set, three attempts left.
route('GET', '/api/auth/recovery', ({ db }) => clone(db.recovery));
route('POST', '/api/auth/recover', ({ db, body }) => {
  if (db.recovery.locked) fail(403, 'forbidden', 'recovery is locked');
  if (!body || body.code !== '123456') {
    db.recovery.attempts_left = Math.max(0, db.recovery.attempts_left - 1);
    if (db.recovery.attempts_left === 0) { db.recovery.locked = true; db.recovery.available = false; }
    fail(422, 'invalid', `wrong code (${db.recovery.attempts_left} attempts left; mock code: 123456)`);
  }
  return { recovery_code: '654321' };
});
route('POST', '/api/admin/recovery-code', ({ db, body }) => {
  if (!body || body.password !== 'correct horse battery') fail(403, 'forbidden', 'Wrong password. (Mock password: correct horse battery)');
  db.recovery = { available: true, locked: false, attempts_left: 3 };
  return { recovery_code: '482913' };
});
// The instance's GitHub App (ACC-006).
route('GET', '/api/admin/github', ({ db }) => (db.me.instance.github_configured
  ? { configured: true, app_slug: 'yulius-quartio-studio', owner_login: db.me.instance.owner_login, install_url: 'https://github.com/apps/yulius-quartio-studio/installations/new', installed: true }
  : { configured: false, app_slug: null, owner_login: null, install_url: null, installed: false }));
route('POST', '/api/admin/github/manifest', ({ db, body }) => {
  if (db.me.instance.github_configured) fail(409, 'conflict', 'a GitHub App is already configured');
  const owner = (body && body.owner_login) || 'Quartio-Studio';
  return { post_url: `https://github.com/organizations/${owner}/settings/apps/new?state=mock`, manifest: { name: 'Yulius (mock)', url: ORIGIN } };
});
route('POST', '/api/device/approve', ({ body }) => {
  if (!/^[A-Z0-9]{4}-?[A-Z0-9]{4}$/.test(body.user_code || '')) fail(404, 'not_found', 'not found');
  return json(204);
});

route('GET', '/api/sandboxes', ({ db }) => db.sandboxes);
route('GET', '/api/github/repositories', ({ db }) => {
  if (!db.me.user.github_linked && !db.me.user.is_superadmin) fail(403, 'forbidden', 'sign in with GitHub to work with repositories');
  if (!db.me.instance.github_configured) fail(409, 'conflict', 'the GitHub App is not configured');
  return [
    ...db.sandboxes.map((s) => ({ owner: s.repo.owner, name: s.repo.name, private: true, access: s.access, already_sandbox: true })),
    { owner: 'Quartio-Studio', name: 'website', private: false, access: 'admin', already_sandbox: false },
    { owner: 'Quartio-Studio', name: 'mobile-app', private: true, access: 'admin', already_sandbox: false },
    { owner: 'Quartio-Studio', name: 'infra', private: true, access: 'write', already_sandbox: false },
    { owner: 'Quartio-Studio', name: 'handbook', private: true, access: 'read', already_sandbox: false },
  ];
});
route('GET', '/api/templates', () => [
  { id: 'empty', name: 'Empty project (Yulius format only)', description: 'Documentation format and rules only; the first task chooses the technology.' },
  { id: 'web', name: 'Web application', description: 'Node.js HTTP server with a testable request handler; no dependencies.' },
  { id: 'python-data', name: 'Python data project', description: 'Python package summarizing CSV data with unittest tests; standard library only.' },
  { id: 'rust-service', name: 'Rust service', description: 'Rust HTTP service on std::net with unit tests; no dependencies.' },
]);
route('POST', '/api/sandboxes', ({ db, body }) => {
  let repoName;
  if (body.mode === 'import') {
    if (!body.repo || !body.repo.includes('/')) fail(422, 'invalid', 'repo must be owner/name');
    const [owner, name] = body.repo.split('/');
    if (owner !== 'Quartio-Studio') fail(422, 'invalid', 'only repositories of the instance owner can be imported');
    if (['infra', 'handbook'].includes(name)) fail(403, 'forbidden', 'importing needs admin or maintain permission on the repository');
    repoName = name;
    if (db.sandboxes.some((s) => s.repo.name === repoName)) fail(409, 'conflict', 'That repository is already a sandbox.');
  } else if (body.mode === 'create') {
    if (!db.me.user.is_instance_admin) fail(403, 'forbidden', 'only instance administrators can create projects');
    if (!/^[A-Za-z0-9._-]{1,100}$/.test(body.name || '')) fail(422, 'invalid', 'Invalid repository name.');
    repoName = body.name;
  } else {
    fail(422, 'invalid', 'mode must be import or create');
  }
  db.nextId += 1;
  const sandbox = {
    id: `sb_${db.nextId}`, slug: repoName.toLowerCase(), name: repoName,
    repo: { owner: 'Quartio-Studio', name: repoName, url: `https://github.com/Quartio-Studio/${repoName}` },
    access: 'admin', default_branch: 'main',
    profile: clone(PRESETS.standard),
    // An imported repository that does not follow the documentation format gets a resting
    // preparation task; created projects start from a conforming template.
    onboarding: body.mode === 'import' ? 'pending' : 'not_needed',
    port_base: 20000 + db.sandboxes.length * 20, port_count: 20, counts: { running: 0, needs_attention: 0, queued: 0 },
  };
  db.sandboxes.push(sandbox);
  if (body.mode === 'import') {
    const id = `tsk_${sandbox.id.slice(3)}_1`;
    db.tasks.push({
      id, sandbox_id: sandbox.id, number: 1, title: 'Prepare repository for Yulius', harness: 'claude', status: 'closed', activity: null,
      branch: null, origin: 'onboarding', created_by: { login: db.me.user.login }, created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
      unread_messages: 0, archived: false, account_label: null,
      stepsList: ['Assess the documentation format', 'Add AGENTS.md and docs/specs', 'Open PR'].map((title, i) => ({ id: `${id}-s${i + 1}`, ordinal: i + 1, title, status: 'pending', note: '', updated_by: null, updated_at: null })),
      model: 'claude-opus-4', effort: 'high', delivery: null, prompt: 'Adapt this repository to the Yulius documentation format.',
    });
  }
  recount(db);
  resync(db);
  return json(201, sandbox);
});
route('GET', `/api/sandboxes/${ID}`, ({ db, p }) => sandboxOf(db, p[0]));
route('PATCH', `/api/sandboxes/${ID}`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'admin');
  if (body.profile) {
    validateProfile(body.profile);
    sandbox.profile = relabel({ ...body.profile, environments: (body.profile.environments || []).map((e) => ({ ...e, command: e.command && e.command.trim() ? e.command : null })) });
  }
  return sandbox;
});
/** Starts the tasks that waited for a sandbox's preparation (U-08): it is ready or skipped. */
function releaseWaiting(db, sandbox) {
  const preparation = db.tasks.find((t) => t.sandbox_id === sandbox.id && t.origin === 'onboarding');
  if (!preparation) return;
  for (const task of db.tasks.filter((t) => t.status === 'chained' && t.afterLink && t.afterLink.task_id === preparation.id)) {
    touch(db, task, { status: 'queued' });
    later(() => touch(db, task, { status: 'starting' }), 700);
    later(() => touch(db, task, { status: 'running', activity: 'working', account_label: 'Claude #1' }), 1800);
  }
}
route('POST', `/api/sandboxes/${ID}/onboarding/skip`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'admin');
  if (!['pending', 'preparing'].includes(sandbox.onboarding)) fail(409, 'conflict', 'nothing awaits preparation');
  sandbox.onboarding = 'skipped';
  releaseWaiting(db, sandbox);
  resync(db);
  return sandbox;
});
route('POST', `/api/sandboxes/${ID}/onboarding/approve`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  if (sandbox.onboarding !== 'pending') fail(409, 'conflict', 'nothing awaits approval');
  sandbox.onboarding = 'preparing';
  const task = db.tasks.find((t) => t.sandbox_id === sandbox.id && t.origin === 'onboarding');
  if (task) {
    touch(db, task, { status: 'starting', branch: 'yulius/prepare-repository' });
    later(() => touch(db, task, { status: 'running', activity: 'working', account_label: 'Claude #1' }), 1200);
    later(() => {
      task.stepsList = task.stepsList.map((s) => ({ ...s, status: 'done', updated_by: 'task #1 (claude)', updated_at: new Date().toISOString() }));
      touch(db, task, { status: 'closed', activity: null, account_label: null });
      sandbox.onboarding = 'ready';
      releaseWaiting(db, sandbox);
      resync(db);
    }, 9000);
  }
  resync(db);
  return sandbox;
});
route('DELETE', `/api/sandboxes/${ID}`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'admin');
  db.sandboxes = db.sandboxes.filter((s) => s.id !== sandbox.id);
  resync(db);
  return json(204);
});

// Live previews (ADR-14, "Open live in a new window"): the mock has no preview proxy, so a
// live page is a small data: page.
const MOCK_PREVIEW = ['<!doctype html><html lang="en"><title>Mock app</title><body><h1>Mock app on port ', '</h1><p>On a real server this tab shows the dev server live, running in your browser.</p></body></html>'];
route('GET', `/api/sandboxes/${ID}/previews`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  const first = sandbox.port_base || 20000;
  return {
    enabled: true,
    origin_template: `data:text/html;charset=utf-8,${encodeURIComponent(MOCK_PREVIEW[0])}{port}${encodeURIComponent(MOCK_PREVIEW[1])}`,
    first_port: first,
    last_port: first + (sandbox.port_count || 20) - 1,
    listening: [first + 3],
    excluded: [],
  };
});
route('GET', `/api/sandboxes/${ID}/tasks`, ({ db, p, query }) => {
  sandboxOf(db, p[0]);
  const archived = query.get('archived') === 'true';
  return db.tasks.filter((t) => t.sandbox_id === p[0] && t.archived === archived).map(summary);
});
route('POST', `/api/sandboxes/${ID}/tasks`, async ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  if (!body.prompt || !body.prompt.trim()) fail(422, 'invalid', 'The prompt is empty.');
  if (body.harness && body.harness !== 'claude') fail(422, 'invalid', 'harness must be claude (Yulius runs Claude Code only)');
  if (body.mode && !['goal', 'prompt', 'auto'].includes(body.mode)) fail(422, 'invalid', 'mode must be goal, prompt or auto');
  const visible = db.workflows.filter((w) => w.scope === 'instance' || w.sandbox === sandbox.id);
  if (body.workflow_id && body.workflow_id !== 'auto' && !visible.some((w) => w.id === body.workflow_id)) fail(422, 'invalid', `unknown workflow \`${body.workflow_id}\``);
  // Auto (TSK-015): the mock's stand-in for Jev answers after a moment, like the server.
  const choice = body.mode === 'auto' || body.workflow_id === 'auto' ? await mockAutoChoice(body, visible) : null;
  const number = Math.max(0, ...db.tasks.filter((t) => t.sandbox_id === sandbox.id).map((t) => t.number)) + 1;
  const title = body.title || body.prompt.trim().split('\n')[0].slice(0, 60);
  const wf = choice ? choice.workflow : db.workflows.find((w) => w.id === body.workflow_id);
  const id = `tsk_${sandbox.id.slice(3)}_${number}`;
  // While the sandbox is being prepared a new task waits after the preparation task (U-08).
  const preparing = sandbox.onboarding === 'pending' || sandbox.onboarding === 'preparing';
  const preparation = preparing ? db.tasks.find((t) => t.sandbox_id === sandbox.id && t.origin === 'onboarding') : null;
  // A start rule (SPEC-017): the mock checks the shape the server checks, and anchors the chain
  // on the first task the rule names.
  let rule = null;
  if (body.start_rule && body.start_rule.trim()) {
    if (body.after_task_id) fail(422, 'invalid', 'give a task to run after or a start rule, not both');
    rule = body.start_rule.trim().replace(/\s+/g, ' ');
    if (!/^[#\w\s()!&|]+$/.test(rule) || !/#\d/.test(rule)) fail(422, 'invalid', 'the rule must name tasks like #12');
    const named = [...rule.matchAll(/#(\d+)/g)].map((m) => Number(m[1]));
    const missing = named.find((n) => !db.tasks.some((x) => x.sandbox_id === sandbox.id && x.number === n));
    if (missing) fail(422, 'invalid', `the rule names #${missing}, which does not exist in this sandbox`);
  }
  const before = rule ? db.tasks.find((x) => x.sandbox_id === sandbox.id && x.number === Number(rule.match(/#(\d+)/)[1])) : body.after_task_id ? db.tasks.find((x) => x.id === body.after_task_id && x.sandbox_id === sandbox.id) : preparation;
  if (body.after_task_id && (!before || !(LIVE.has(before.status) || before.status === 'chained'))) fail(422, 'invalid', 'the task to run after does not exist or has ended');
  const task = {
    id, sandbox_id: sandbox.id, number, title, harness: 'claude', status: before ? 'chained' : 'queued', activity: null,
    branch: `feat/${title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 40) || `task-${number}`}`,
    origin: 'ui', created_by: { login: db.me.user.login }, created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    unread_messages: 0, archived: false, account_label: null,
    stepsList: wf ? wf.steps.map((title2, i) => ({ id: `${id}-s${i + 1}`, ordinal: i + 1, title: title2, status: 'pending', note: '', updated_by: 'workflow', updated_at: new Date().toISOString() })) : [],
    model: body.model || 'claude-opus-4', effort: body.effort || 'default', prompt: body.prompt,
    delivery: null, mode: choice ? choice.mode : body.mode || 'goal', auto: choice ? choice.auto : null,
  };
  db.tasks.push(task);
  recount(db);
  if (before) {
    chainAfter(task, before, rule);
    emit(db, { type: 'task.updated', task: summary(before) });
    return json(201, summary(task));
  }
  later(() => touch(db, task, { status: 'starting' }), 700);
  later(() => touch(db, task, { status: 'running', activity: 'working', account_label: 'Claude #1' }), 1800);
  return json(201, summary(task));
});
/**
 * The mock's Auto choice (TSK-015): a question ("?", or starting with how/why/what…) is a prompt
 * with the Research workflow (never a spec-first one); "fix", "bug" or "crash" gets Fix; anything
 * else is a goal with Feature. "maybe" makes the stand-in unsure, so the defaults apply (Goal and
 * Feature; no workflow for a prompt).
 */
async function mockAutoChoice(body, workflows) {
  await delay(700);
  const text = String(body.prompt || '').trim();
  const unsure = /\bmaybe\b/i.test(text);
  const question = /\?|^(how|why|what|where|when|which|who|is|are|does|do|can|could|should)\b/i.test(text);
  const named = (name) => workflows.find((w) => w.name.toLowerCase() === name) || null;
  const autoMode = body.mode === 'auto';
  const mode = autoMode ? (!unsure && question ? 'prompt' : 'goal') : body.mode || 'goal';
  let workflow = body.workflow_id && body.workflow_id !== 'auto' ? workflows.find((w) => w.id === body.workflow_id) : null;
  if (body.workflow_id === 'auto') {
    const noSpec = (w) => (w && !(w.gates && w.gates.require_spec) ? w : null);
    if (unsure) workflow = mode === 'prompt' ? null : named('feature');
    else if (mode === 'prompt') workflow = noSpec(named('research'));
    else workflow = /\b(fix|bug|crash)/i.test(text) ? named('fix') || named('feature') : named('feature');
  }
  return {
    mode,
    workflow,
    auto: {
      mode: autoMode ? (unsure ? 'default' : 'jev') : null,
      workflow: body.workflow_id === 'auto' ? (unsure ? 'default' : 'jev') : null,
      workflow_name: workflow ? workflow.name : null,
    },
  };
}

// Task intake (TSK-011): requests of 40+ words mixing UI and backend work look like several
// tasks; the proposal groups lines by area (UX first, backend second), after a moment (Claude).
const MOCK_BACKEND = /backend|api|server|database|pool|queue/i;
route('POST', `/api/sandboxes/${ID}/intake/check`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  const text = String(body.prompt || '');
  const words = text.split(/\s+/).filter(Boolean).length;
  if (words < 40) return { verdict: 'one', checked: false };
  const lines = text.split('\n').filter((l) => /^\s*(\d+[.)]|[-*•])\s/.test(l));
  const several = lines.some((l) => MOCK_BACKEND.test(l)) && lines.some((l) => !MOCK_BACKEND.test(l));
  return { verdict: several ? 'several' : 'one', checked: true };
});
route('POST', `/api/sandboxes/${ID}/intake/proposal`, async ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  // A test can hold the answer (`__yuliusMock.holdProposal = promise`) to look at the waiting card.
  await (db.holdProposal || delay(900));
  const lines = String(body.prompt || '').split('\n').filter((l) => /^\s*(\d+[.)]|[-*•])\s/.test(l)).map((l) => l.trim());
  const backend = lines.filter((l) => MOCK_BACKEND.test(l));
  const ux = lines.filter((l) => !MOCK_BACKEND.test(l));
  if (!backend.length || !ux.length) return { tasks: [] };
  return { tasks: [
    { title: 'UX: interface fixes', prompt: ux.join('\n'), after_previous: false },
    { title: 'Backend: server work', prompt: backend.join('\n'), after_previous: false },
  ] };
});
route('POST', `/api/tasks/${ID}/start-now`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (task.status !== 'chained') fail(409, 'conflict', `the task is ${task.status}, not waiting in a chain`);
  touch(db, task, { status: 'queued' });
  later(() => touch(db, task, { status: 'starting' }), 700);
  later(() => touch(db, task, { status: 'running', activity: 'working', account_label: 'Claude #1' }), 1800);
  return summary(task);
});
route('POST', `/api/tasks/${ID}/held-prompts/${ID}`, ({ db, p, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  const hold = task.heldPrompt;
  if (!hold || hold.id !== p[1]) fail(404, 'not_found', 'not found');
  const action = body.action;
  if (action === 'revert' && !body.confirm) fail(422, 'invalid', 'reverting cancels this task and resets its branch; confirm it explicitly');
  if (!['parallel', 'after', 'send', 'skip_after', 'revert', 'dismiss'].includes(action)) fail(422, 'invalid', 'unknown action');
  task.heldPrompt = null;
  let created = null;
  if (['parallel', 'after', 'skip_after', 'revert'].includes(action)) {
    const number = Math.max(0, ...db.tasks.filter((x) => x.sandbox_id === task.sandbox_id).map((x) => x.number)) + 1;
    const id = `tsk_${task.sandbox_id.slice(3)}_${number}`;
    const chained = action === 'after' || action === 'skip_after';
    created = { ...clone(task), id, number, title: hold.excerpt.slice(0, 60), status: chained ? 'chained' : 'queued', activity: null, stepsList: [], heldPrompt: null, afterLink: null, nextLinks: [], delivery: null, created_at: new Date().toISOString() };
    db.tasks.push(created);
    if (chained) chainAfter(created, task);
  }
  if (action === 'revert') touch(db, task, { status: 'stopped_cancelled', archived: true, activity: null });
  else touch(db, task, {});
  const notes = { dismiss: 'Dismissed. The prompt is still in Claude Code’s input box.', send: 'Sent to the agent unchanged.', parallel: `Started #${created && created.number} in parallel.`, after: `#${created && created.number} runs after this task.`, skip_after: `Steps skipped; #${created && created.number} runs after this task.`, revert: `#${task.number} was cancelled and its changes reverted. Its work is kept on branch yulius/reverted/${task.number}-1; nothing was deleted.` };
  recount(db);
  return { task: summary(task), created: created ? summary(created) : null, note: notes[action] };
});
route('GET', `/api/tasks/${ID}`, ({ db, p }) => detail(db, taskOf(db, p[0])));
route('POST', `/api/tasks/${ID}/stop`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (task.status === 'chained') { touch(db, task, { status: 'stopped_cancelled' }); return summary(task); }
  if (!LIVE.has(task.status)) fail(409, 'conflict', 'The task is not running.');
  touch(db, task, { status: 'stopping', activity: null });
  later(() => touch(db, task, { status: 'stopped_cancelled', account_label: null }), 1400);
  return summary(task);
});
const continueHandler = ({ db, p, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (LIVE.has(task.status)) fail(409, 'conflict', 'The task is already running.');
  touch(db, task, { status: 'starting', activity: null });
  const account = db.accounts.find((a) => body && a.id === body.account_id);
  later(() => touch(db, task, { status: 'running', activity: 'waiting', account_label: account ? account.label : 'Claude #1' }), 1300);
  return summary(task);
};
route('POST', `/api/tasks/${ID}/continue`, continueHandler);
route('POST', `/api/tasks/${ID}/switch`, ({ db, p, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (!['running', 'waiting_capacity'].includes(task.status)) fail(409, 'conflict', 'Only running tasks can switch.');
  const from = task.account_label;
  const account = db.accounts.find((a) => a.id === body.account_id);
  touch(db, task, { status: 'switching' });
  later(() => {
    touch(db, task, { status: 'running', activity: 'waiting', account_label: account ? account.label : 'Claude #1' });
    const session = db.sessions.get(task.id);
    if (session) session.notice(`↻ switched from ${from} to ${task.account_label}`);
  }, 2500);
  return summary(task);
});
route('DELETE', `/api/tasks/${ID}`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (['starting', 'running', 'switching', 'stopping'].includes(task.status)) fail(409, 'conflict', 'stop the task before deleting it');
  db.tasks = db.tasks.filter((t) => t.id !== task.id);
  recount(db);
  emit(db, { type: 'task.removed', task_id: task.id });
  return json(204);
});
route('POST', `/api/tasks/${ID}/archive`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  // A finished task (Done by the sidebar's rule, the server's task_is_done) is closed as it is
  // archived (TSK-014); any other live task has to be stopped first.
  const finished = task.status === 'running' && taskInfo(summary(task), null, pullOf(task)).group === 'done';
  if (LIVE.has(task.status) && !finished) fail(409, 'conflict', 'Stop the task before archiving it.');
  touch(db, task, finished ? { archived: true, status: 'closed', activity: null } : { archived: true });
  return summary(task);
});
route('POST', `/api/tasks/${ID}/restore`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  touch(db, task, { archived: false });
  return summary(task);
});
route('POST', `/api/tasks/${ID}/feedback`, ({ db, p, body }) => {
  taskOf(db, p[0]);
  if (!['correct', 'incorrect'].includes(body.verdict)) fail(422, 'invalid', 'verdict must be correct or incorrect');
  return json(204);
});
route('GET', `/api/tasks/${ID}/messages`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  const list = db.messages.filter((m) => m.task === task.id).map(({ task: _t, ...m }) => ({ ...m }));
  db.messages.filter((m) => m.task === task.id).forEach((m) => { m.read = true; });
  if (task.unread_messages) touch(db, task, { unread_messages: 0 });
  return list;
});
route('GET', '/api/archived-tasks', ({ db }) => db.tasks.filter((t) => t.archived && db.sandboxes.some((s) => s.id === t.sandbox_id)).map(summary));

// ---- Git and delivery

route('GET', '/api/delivery/presets', () => [clone(PRESETS.standard), clone(PRESETS.product), clone(PRESETS.studio)]);
route('GET', `/api/sandboxes/${ID}/git`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  const tasks = db.tasks.filter((t) => t.sandbox_id === sandbox.id && !t.archived && t.branch);
  const pull = pullOf;
  return {
    integration_branch: sandbox.profile.integration_branch, production_branch: sandbox.profile.production_branch, default_branch: sandbox.default_branch,
    tasks: tasks.map((t) => ({ task_id: t.id, title: t.title, branch: t.branch, ahead: (t.number * 3) % 11 + 1, behind: t.number % 3, files_changed: (t.number * 7) % 23 + 1, insertions: (t.number * 131) % 900 + 12, deletions: (t.number * 37) % 200, pr: pull(t) })),
    recent_commits: [
      { sha: '9f3c2a17b0e4d5c6a7b8c9d0e1f2a3b4c5d6e7f8', subject: 'feat(checkout): cart totals with tax rules', author: 'task #12 (claude)', at: iso(25 * MIN) },
      { sha: '4b1d9e0c7a6f5e4d3c2b1a0f9e8d7c6b5a4f3e2d', subject: 'docs: restructure architecture section', author: 'task #5 (claude)', at: iso(5 * HOUR) },
      { sha: 'c0ffee1234567890abcdef1234567890abcdef12', subject: 'fix(viewer): dispose WebGL context on unmount', author: 'task #11', at: iso(7 * HOUR) },
    ],
    // Open pull requests only.
    pulls: tasks.map(pull).filter((pr) => pr && pr.state === 'open'),
    // Reads degrade: a GitHub failure becomes a note instead of an error.
    notes: sandbox.id === 'sb_three' ? ['Pull requests can\u2019t be listed: GitHub rate limit exceeded; lists may be incomplete.'] : [],
    checks_access: checksAccess(db, sandbox),
  };
});
route('GET', `/api/tasks/${ID}/delivery`, ({ db, p }) => delivery(db, taskOf(db, p[0])));
// The server has no task changes API yet (GET /api/tasks/{tid}/changes and /commits are proposed in
// docs/api.md): the mock answers 404 like it, so the Git review shows its Branch-only view here too.

route('POST', `/api/tasks/${ID}/delivery/pr`, ({ db, p, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  const d = delivery(db, task);
  const refusal = d.blocked.find((b) => b.action === 'open_pr');
  if (refusal) fail(409, 'conflict', refusal.reason);
  if (!d.next_actions.includes('open_pr')) fail(409, 'conflict', 'a pull request is already open');
  db.nextId += 1;
  const sandbox = sandboxOf(db, task.sandbox_id);
  const number = 300 + db.nextId;
  task.delivery = { stage: 'pr_open', environment: null, pr: { number, url: `${sandbox.repo.url}/pull/${number}`, title: body.title || task.title, state: 'open', base: sandbox.profile.integration_branch, head: task.branch, checks: 'pending' } };
  later(() => { if (task.delivery && task.delivery.pr) task.delivery.pr.checks = 'success'; }, 4000);
  return delivery(db, task);
});
route('POST', `/api/tasks/${ID}/delivery/merge`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  const d = delivery(db, task);
  const refusal = d.blocked.find((b) => b.action === 'merge');
  if (refusal) fail(409, 'conflict', refusal.reason);
  if (!d.next_actions.includes('merge')) fail(409, 'conflict', 'no open pull request to merge');
  const sandbox = sandboxOf(db, task.sandbox_id);
  task.delivery = { stage: 'merged_integration', environment: null, pr: { ...task.delivery.pr, state: 'merged' } };
  const notes = [];
  const pos = position(sandbox.profile, 'merged_integration', null);
  if (sandbox.profile.auto_promote && pos.kind === 'promote') {
    const production = (sandbox.profile.environments || []).filter((e) => e.after === 'production');
    task.delivery.stage = production.length ? 'promoted' : 'production_done';
  }
  touch(db, task);
  return { ...delivery(db, task), notes };
});
route('POST', `/api/sandboxes/${ID}/delivery/promote`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  const waiting = db.tasks.filter((t) => t.sandbox_id === sandbox.id && t.delivery && position(sandbox.profile, t.delivery.stage, t.delivery.environment).kind === 'promote');
  const pendingEnv = db.tasks.some((t) => t.sandbox_id === sandbox.id && t.delivery && ['merged_integration', 'env_done'].includes(t.delivery.stage) && position(sandbox.profile, t.delivery.stage, t.delivery.environment).kind === 'env' && position(sandbox.profile, t.delivery.stage, t.delivery.environment).env.after === 'integration');
  if (pendingEnv) fail(409, 'conflict', 'a merged task still has an integration environment pending');
  if (waiting.length === 0) fail(409, 'conflict', `${sandbox.profile.production_branch} already equals ${sandbox.profile.integration_branch}`);
  if (sandbox.profile.promote === 'merge_commit' && !db.promotionPr.has(sandbox.id)) {
    db.nextId += 1;
    const url = `${sandbox.repo.url}/pull/${600 + db.nextId}`;
    db.promotionPr.set(sandbox.id, url);
    return { result: 'pr_opened', sha: url };
  }
  db.promotionPr.delete(sandbox.id);
  const production = (sandbox.profile.environments || []).filter((e) => e.after === 'production');
  for (const task of waiting) {
    task.delivery = { ...task.delivery, stage: production.length ? 'promoted' : 'production_done', environment: null };
    touch(db, task);
  }
  return { result: 'promoted', sha: 'e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3' };
});

function newRun(db, sandbox, env, kind) {
  db.nextId += 1;
  const run = {
    run_id: `run_${db.nextId}`, sandbox_id: sandbox.id, environment: env.name, kind, state: kind === 'manual' ? 'succeeded' : 'running',
    command: kind === 'manual' ? null : env.command, commit: '9f3c2a17b0e4d5c6a7b8c9d0e1f2a3b4c5d6e7f8', requested_by: db.me.user.login,
    output: kind === 'manual' ? `${db.me.user.login} confirmed ${env.name}.` : '', exit_code: null,
    started_at: new Date().toISOString(), ended_at: kind === 'manual' ? new Date().toISOString() : null, startedMs: Date.now(),
  };
  db.runs.push(run);
  if (db.runs.length > 50) db.runs.shift();
  return run;
}

const RUN_LINES = ['Resolving target…', 'Uploading build artifact (12.4 MiB)…', 'Restarting service…', 'Health check: GET /healthz → 200', 'Smoke test: checkout flow ✓', 'Deploy finished.'];

/** Advance a command run with the clock (one line per second) and apply its outcome once. */
function progressRun(db, run) {
  if (run.state !== 'running') return run;
  const lines = [`$ ${run.command}`, ...RUN_LINES];
  const shown = Math.min(lines.length, Math.floor((Date.now() - run.startedMs) / 1000) + 1);
  run.output = lines.slice(0, shown).join('\n');
  if (shown >= lines.length) {
    Object.assign(run, { state: 'succeeded', exit_code: 0, ended_at: new Date().toISOString() });
    const sandbox = db.sandboxes.find((s) => s.id === run.sandbox_id);
    const env = sandbox && (sandbox.profile.environments || []).find((e) => e.name === run.environment);
    if (sandbox && env && env.verify === 'command') advanceEnvironment(db, sandbox, env.name);
  }
  return run;
}

const runView = (run) => {
  const { sandbox_id: _s, startedMs: _t, ...out } = run;
  return { ...out, output: out.output.slice(-2048) };
};

route('POST', `/api/sandboxes/${ID}/delivery/environments/${ID}/runs`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  if (body.confirm !== true) fail(422, 'invalid', 'environment runs need {"confirm": true}');
  const env = (sandbox.profile.environments || []).find((e) => e.name === p[1]);
  if (!env) fail(404, 'not_found', 'not found');
  if (!env.command) {
    const run = newRun(db, sandbox, env, 'manual');
    advanceEnvironment(db, sandbox, env.name);
    return runView(run);
  }
  if (db.runs.some((r) => r.sandbox_id === sandbox.id && progressRun(db, r).state === 'running')) fail(409, 'conflict', 'another environment run is in progress for this sandbox');
  return runView(newRun(db, sandbox, env, 'command'));
});
route('POST', `/api/sandboxes/${ID}/delivery/environments/${ID}/confirm`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  if (body.confirm !== true) fail(422, 'invalid', 'confirmation needs {"confirm": true}');
  const env = (sandbox.profile.environments || []).find((e) => e.name === p[1]);
  if (!env) fail(404, 'not_found', 'not found');
  if (env.command && env.verify !== 'manual') fail(409, 'conflict', `${env.name} is verified by its command`);
  const run = newRun(db, sandbox, env, 'manual');
  advanceEnvironment(db, sandbox, env.name);
  return runView(run);
});
route('GET', `/api/sandboxes/${ID}/delivery/runs`, ({ db, p }) => {
  sandboxOf(db, p[0]);
  return db.runs.filter((r) => r.sandbox_id === p[0]).map((r) => runView(progressRun(db, r))).reverse().slice(0, 20);
});
route('GET', `/api/sandboxes/${ID}/delivery/runs/${ID}`, ({ db, p }) => {
  const run = db.runs.find((r) => r.run_id === p[1] && r.sandbox_id === p[0]);
  if (!run) fail(404, 'not_found', 'not found');
  return runView(progressRun(db, run));
});

// ---- Yulius-run CI and repository hygiene (docs/api.md "CI")

const CI_JOBS = [
  { name: 'lint', command: 'cargo clippy --all-targets -- -D warnings', matrix: null },
  { name: 'test (1/2)', command: 'cargo nextest run --partition "count:$YULIUS_CI_MATRIX"', matrix: '1/2' },
  { name: 'test (2/2)', command: 'cargo nextest run --partition "count:$YULIUS_CI_MATRIX"', matrix: '2/2' },
];

function ciOutput(job, ok) {
  const lines = [`$ ${job.command}`, '   Compiling yulius-domain v0.1.0', '   Compiling yulius-app v0.1.0', '    Finished `test` profile in 41.2s'];
  if (job.name.startsWith('lint')) lines.push(ok ? 'lint: no warnings' : 'error: unused variable `x`');
  else lines.push(ok ? 'Summary: 214 tests run: 214 passed, 0 skipped' : 'test delivery::merge_waits_for_ci ... FAILED\nSummary: 213 tests run: 212 passed, 1 failed');
  return `${lines.join('\n')}\n`;
}

function ciRun(id, reference, sha, state, minutesAgo, { failJob = null, reason = null, by = 'yulius' } = {}) {
  const queued = Date.now() - minutesAgo * 60000;
  const started = queued + 4000;
  const ended = state === 'running' || state === 'queued' ? null : started + 3 * 60000 + 12000;
  const jobs = CI_JOBS.map((job, i) => {
    const failed = failJob === job.name;
    const jobState = state === 'passed' ? 'passed' : state === 'failed' ? (failed ? 'failed' : 'passed') : state === 'running' ? (i === 0 ? 'passed' : 'running') : state;
    return {
      id: `${id}_j${i}`, name: job.name, command: job.command, matrix: job.matrix, state: jobState, reason: null,
      exit_code: jobState === 'passed' ? 0 : jobState === 'failed' ? 101 : null,
      duration_ms: jobState === 'passed' || jobState === 'failed' ? 120000 + i * 30000 : null,
      output: jobState === 'queued' ? '' : jobState === 'running' ? `$ ${job.command}\n   Compiling yulius-app v0.1.0\n` : ciOutput(job, !failed),
    };
  });
  const pr = reference.startsWith('pr/') ? Number(reference.slice(3)) : null;
  return {
    id, reference, label: pr ? `PR #${pr}` : reference.slice(7), pr_number: pr, sha, state, source: 'yulius_toml', jobs, reason,
    queued_at: new Date(queued).toISOString(), started_at: state === 'queued' ? null : new Date(started).toISOString(),
    ended_at: ended ? new Date(ended).toISOString() : null, duration_ms: ended ? ended - started : null,
    requested_by: by, status: state === 'running' || state === 'queued' ? 'pending' : 'published', created: Date.now(),
  };
}

function seedCi() {
  const studioGate = { source: 'yulius_toml', jobs: CI_JOBS.map(({ name, command, matrix }) => ({ name, command, matrix })) };
  return {
    sb_studio: {
      gate: studioGate, gate_problem: null, status_access: null, actions_workflows: [],
      runs: [
        ciRun('ci_s4', 'branch/quartio', '9f3c2a1b7e4d', 'running', 1),
        ciRun('ci_s3', 'pr/221', '4be81c0d2a9f', 'passed', 22),
        ciRun('ci_s2', 'pr/218', '0d7e5f3a1c2b', 'failed', 47, { failJob: 'test (2/2)' }),
        ciRun('ci_s1', 'pr/218', 'a1b2c3d4e5f6', 'cancelled', 63, { reason: 'superseded by 0d7e5f3' }),
      ],
      hygiene: {
        scanned_at: new Date(Date.now() - 2 * 3600000).toISOString(), notes: [],
        items: [
          { kind: 'remote_branch', name: 'feat/12-old-landing', reason_code: 'pull_merged', reason: 'pull request #12 was merged', safe: true },
          { kind: 'local_branch', name: 'feat/12-old-landing', reason_code: 'local_copy', reason: 'no worktree uses it and its commits are on GitHub', safe: true },
          { kind: 'worktree', name: 'worktrees/tsk_studio_9', reason_code: 'task_archived', reason: 'task #9 is archived', safe: true },
          { kind: 'remote_branch', name: 'spike/websocket-pool', reason_code: 'idle', reason: 'no pull request and no commit for 21 days', safe: false },
        ],
      },
    },
    sb_product: {
      gate: null, gate_problem: null, status_access: null, actions_workflows: ['ci.yml', 'deploy.yml'], runs: [],
      hygiene: { scanned_at: new Date(Date.now() - 3600000).toISOString(), notes: [], items: [] },
    },
    sb_three: {
      gate: { source: 'profile', jobs: [{ name: 'gate', command: 'make ci', matrix: null }] }, gate_problem: null, status_access: null, actions_workflows: [],
      runs: [ciRun('ci_t1', 'branch/dev', '77aa01bc3d4e', 'passed', 180)],
      hygiene: { scanned_at: null, notes: [], items: [] },
    },
  };
}

function ciOf(db, sid) {
  sandboxOf(db, sid);
  if (!db.ci[sid]) db.ci[sid] = { gate: null, gate_problem: null, status_access: null, actions_workflows: [], runs: [], hygiene: { scanned_at: null, notes: [], items: [] } };
  return db.ci[sid];
}

/** A running mock run finishes about 20 s after it was created. */
function progressCi(run) {
  if (run.state === 'running' && Date.now() - run.created > 20000) {
    run.state = 'passed';
    run.status = 'published';
    run.ended_at = new Date().toISOString();
    run.duration_ms = Date.now() - Date.parse(run.started_at);
    for (const job of run.jobs) {
      job.state = 'passed';
      job.exit_code = 0;
      job.duration_ms = job.duration_ms || 90000;
      job.output = ciOutput(job, true);
    }
  }
  return run;
}

const ciSummary = (run) => ({ ...run, created: undefined, jobs: run.jobs.map((j) => ({ ...j, output: undefined })) });

function ciOverview(db, sid) {
  const sandbox = sandboxOf(db, sid);
  const ci = ciOf(db, sid);
  return {
    gate: clone(ci.gate), gate_problem: ci.gate_problem, runs: ci.runs.map((r) => ciSummary(progressCi(r))),
    can_run: (RANK[sandbox.access] || 0) >= RANK.write, status_access: ci.status_access, actions_workflows: ci.actions_workflows.slice(),
  };
}

route('GET', `/api/sandboxes/${ID}/ci`, ({ db, p }) => ciOverview(db, p[0]));
route('POST', `/api/sandboxes/${ID}/ci/sync`, ({ db, p }) => {
  need(sandboxOf(db, p[0]), 'write');
  return ciOverview(db, p[0]);
});
route('GET', `/api/sandboxes/${ID}/ci/runs/${ID}`, ({ db, p }) => {
  const run = ciOf(db, p[0]).runs.find((r) => r.id === p[1]);
  if (!run) fail(404, 'not_found', 'not found');
  return { ...progressCi(run), created: undefined };
});
route('POST', `/api/sandboxes/${ID}/ci/runs/${ID}/cancel`, ({ db, p }) => {
  need(sandboxOf(db, p[0]), 'write');
  const run = ciOf(db, p[0]).runs.find((r) => r.id === p[1]);
  if (!run) fail(404, 'not_found', 'not found');
  if (run.state !== 'running' && run.state !== 'queued') fail(409, 'conflict', 'this run has already ended');
  run.state = 'cancelled';
  run.reason = `cancelled by ${db.me.user.login}`;
  run.ended_at = new Date().toISOString();
  for (const job of run.jobs) if (job.state === 'running' || job.state === 'queued') { job.state = 'cancelled'; job.reason = run.reason; }
  return ciSummary(run);
});
route('POST', `/api/sandboxes/${ID}/ci/runs/${ID}/rerun`, ({ db, p }) => {
  need(sandboxOf(db, p[0]), 'write');
  const ci = ciOf(db, p[0]);
  const run = ci.runs.find((r) => r.id === p[1]);
  if (!run) fail(404, 'not_found', 'not found');
  if (ci.runs.some((r) => r.sha === run.sha && (r.state === 'running' || r.state === 'queued'))) fail(409, 'conflict', 'a run of this commit is already queued or running');
  db.nextId += 1;
  const fresh = ciRun(`ci_${db.nextId}`, run.reference, run.sha, 'running', 0, { by: db.me.user.login });
  ci.runs.unshift(fresh);
  return ciSummary(fresh);
});
route('GET', `/api/sandboxes/${ID}/hygiene`, ({ db, p }) => clone(ciOf(db, p[0]).hygiene));
route('POST', `/api/sandboxes/${ID}/hygiene/scan`, ({ db, p }) => {
  need(sandboxOf(db, p[0]), 'write');
  const hygiene = ciOf(db, p[0]).hygiene;
  hygiene.scanned_at = new Date().toISOString();
  return clone(hygiene);
});
route('POST', `/api/sandboxes/${ID}/hygiene/cleanup`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  const hygiene = ciOf(db, p[0]).hygiene;
  const force = Boolean(body && body.force);
  const wanted = body && Array.isArray(body.items) && body.items.length ? body.items : null;
  const removed = [];
  const skipped = [];
  hygiene.items = hygiene.items.filter((item) => {
    const chosen = wanted ? wanted.some((w) => w.kind === item.kind && w.name === item.name) : (item.safe || force);
    if (!chosen) return true;
    if (!item.safe && !force) { skipped.push({ kind: item.kind, name: item.name, reason: `${item.reason}; pass force to delete unmerged work` }); return true; }
    removed.push({ kind: item.kind, name: item.name });
    return false;
  });
  hygiene.scanned_at = new Date().toISOString();
  return { removed, skipped, report: clone(hygiene) };
});

// ---- Skills, workflows, automations, MCP

// A believable SKILL.md: frontmatter, headings, steps, a checklist, a code block and a table.
function skillContent(skill) {
  return `---
name: ${skill.name}
description: ${skill.description}
---

# ${skill.name}

Use this skill when the task matches the description above. Keep changes small and **verify them** before you report.

## Steps

1. Read \`AGENTS.md\` and the task spec.
2. Make the smallest complete change.
3. Verify with tests and report *evidence*, not intentions.

## Checklist

- [x] The spec names the goal and the plan
- [ ] Tests cover the new behaviour
- [ ] The PR description links the spec

> Ask before changing public APIs or the database schema.

## Commands

\`\`\`bash
cargo test --workspace
scripts/dev check
\`\`\`

| Check | When |
| --- | --- |
| Unit tests | every change |
| UI contracts | web changes |

See [the contributing guide](https://example.com/contributing) for the details.
`;
}
// The sandbox repository's files, as the files API serves them (root `sandbox`, or a task's
// worktree as `task:<id>`), kept in memory per sandbox and root. Repository skills are files here
// (.agents/skills/<name>/SKILL.md, .claude/skills/<name>/SKILL.md), as on the server, so editing or
// adding one through the files API shows in the skill catalog.
const REPO_SKILL = /^(\.agents\/skills|\.claude\/skills)\/([a-z0-9][a-z0-9-]*)\/SKILL\.md$/;
function mockSha(text) {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i += 1) { h ^= text.charCodeAt(i); h = Math.imul(h, 0x01000193) >>> 0; }
  return (h.toString(16).padStart(8, '0') + text.length.toString(16)).padEnd(64, '0').slice(0, 64);
}
function repoTree(db, sid, root = 'sandbox') {
  db.repoTrees = db.repoTrees || {};
  const key = `${sid}|${root}`;
  if (!db.repoTrees[key]) {
    const files = new Map();
    for (const skill of db.skills.filter((sk) => sk.source === 'repository')) files.set(skill.path, skillContent(skill));
    files.set('README.md', `# ${(sandboxOf(db, sid).repo || {}).name || sandboxOf(db, sid).slug}\n\nThe ${sandboxOf(db, sid).name} storefront.\n`);
    files.set('AGENTS.md', '# Agents\n\nRead the task spec first. Keep changes small; run the tests before you report.\n');
    files.set('src/api/checkout.rs', 'pub fn cart_total(cart: &Cart) -> Money {\n    cart.subtotal()\n}\n');
    files.set('docs/specs/2026-10-10-improve-the-code.md', '# Improve the code\n\n## Goal\nSmaller handlers.\n\n## Plan\n- [ ] Split the checkout handler\n');
    db.repoTrees[key] = { files, dirs: new Set() };
  }
  return db.repoTrees[key];
}
function repoPath(raw) {
  const path = String(raw || '').replace(/^\/+|\/+$/g, '');
  if (path.split('/').some((part) => part === '..' || part === '.git')) fail(403, 'forbidden', 'that path is outside the folder or reserved');
  return path;
}
const isDir = (tree, path) => path === '' || tree.dirs.has(path) || [...tree.files.keys(), ...tree.dirs].some((f) => f.startsWith(`${path}/`));
const parentOf = (path) => (path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '');
function skillCatalog(db, sid) {
  const tree = repoTree(db, sid);
  const repo = [...tree.files.keys()].map((path) => [path, path.match(REPO_SKILL)]).filter(([, m]) => m).map(([path, m]) => {
    const text = tree.files.get(path);
    const description = text.match(/^---\n[\s\S]*?^description:\s*(.+)$[\s\S]*?\n---/m);
    return { name: m[2], description: description ? description[1].replace(/^["']|["']$/g, '').trim() : '', source: 'repository', path, enabled: true };
  });
  return [...repo, ...db.skills.filter((sk) => sk.source !== 'repository')];
}
// A task's changes and commits (docs/api.md "Task changes", #28): synthetic but
// consistent with the overview's counts, so the Git page's review shows believable diffs.
const CHANGE_FILES = [
  ['src/api/checkout.rs', 'rs'], ['src/domain/tax.rs', 'rs'], ['src/api/cart.rs', 'rs'], ['tests/checkout_test.rs', 'rs'],
  ['web/src/pages/Checkout.tsx', 'tsx'], ['web/src/components/CartSummary.tsx', 'tsx'], ['web/src/api/client.ts', 'ts'],
  ['docs/specs/checkout.md', 'md'], ['migrations/0007_tax_rules.sql', 'sql'], ['Cargo.toml', 'toml'], ['src/lib.rs', 'rs'],
  ['web/src/styles/checkout.css', 'css'], ['.agents/skills/checkout/SKILL.md', 'md'], ['config/tax.yaml', 'yaml'],
  ['src/domain/money.rs', 'rs'], ['web/src/hooks/useCart.ts', 'ts'], ['README.md', 'md'], ['src/api/mod.rs', 'rs'],
  ['web/src/components/TaxBreakdown.tsx', 'tsx'], ['tests/tax_rules_test.rs', 'rs'], ['src/domain/discount.rs', 'rs'],
  ['web/src/pages/Cart.tsx', 'tsx'], ['docs/adr/0012-tax-engine.md', 'md'],
];
const CODE = {
  rs: ['use crate::domain::{Cart, Money};', 'pub fn cart_total(cart: &Cart, rules: &TaxRules) -> Result<Money, CheckoutError> {', '    let subtotal = cart.subtotal();', '    let tax = rules.tax_for(&cart.region)?;', '    let discount = cart.discount().unwrap_or_default();', '    Ok(subtotal + tax - discount)', '}', '', '#[derive(Debug, Clone, PartialEq)]', 'pub struct TaxLine { pub label: String, pub amount: Money }', 'impl TaxRules {', '    pub fn for_region(region: &Region) -> Option<&TaxRule> {', '        self.rules.iter().find(|rule| rule.applies_to(region))', '    }', '}', '#[test]', 'fn totals_include_tax() {', '    assert_eq!(cart_total(&cart(), &rules()).unwrap(), Money::eur(12_30));', '}'],
  tsx: ['import { useCart } from "../hooks/useCart";', 'export function CartSummary({ cart }: Props) {', '  const { total, tax } = useCart(cart);', '  return (', '    <section className="cart-summary">', '      <TaxBreakdown lines={tax.lines} />', '      <p className="total">{format(total)}</p>', '    </section>', '  );', '}', '', "const format = (money: Money) => money.toLocaleString('en', { style: 'currency', currency: money.currency });"],
  ts: ["export async function getCart(id: string): Promise<Cart> {", "  const res = await fetch(`/api/carts/${id}`);", "  if (!res.ok) throw new ApiError(res.status, await res.text());", '  return res.json();', '}', '', 'export type Cart = { id: string; lines: CartLine[]; region: string };'],
  md: ['# Checkout', '', '## Goal', 'Customers see the tax for their region before they pay.', '', '## Plan', '- [x] Tax rules per region', '- [x] Cart totals with tax', '- [ ] Checkout page shows the breakdown', '', 'Totals are computed on the server; the page only formats them.'],
  sql: ['CREATE TABLE tax_rules (', '  id BIGSERIAL PRIMARY KEY,', '  region TEXT NOT NULL,', '  rate_basis_points INTEGER NOT NULL,', '  valid_from DATE NOT NULL', ');', 'CREATE INDEX tax_rules_region ON tax_rules (region, valid_from);'],
  toml: ['[dependencies]', 'rust_decimal = "1.36"', 'serde = { version = "1", features = ["derive"] }', 'thiserror = "2"'],
  css: ['.cart-summary {', '  display: grid;', '  gap: 8px;', '}', '.cart-summary .total { font-weight: 600; }'],
  yaml: ['regions:', '  - code: PL', '    rate: 23', '  - code: DE', '    rate: 19', '  - code: FR', '    rate: 20'],
};
function taskChanges(db, task) {
  const sandbox = sandboxOf(db, task.sandbox_id);
  const n = task.number;
  const count = (n * 7) % 23 + 1;
  let adds = (n * 131) % 900 + 12;
  let dels = (n * 37) % 200;
  const files = [];
  for (let i = 0; i < count; i += 1) {
    const [path, ext] = CHANGE_FILES[(i + n) % CHANGE_FILES.length];
    const left = count - i;
    const reserve = adds >= left ? left - 1 : 0; // every later file keeps at least one line when it can
    const add = i === count - 1 ? Math.max(0, adds) : Math.max(adds >= left ? 1 : 0, Math.min(adds - reserve, Math.round(adds / left * (0.6 + ((i * 7 + n) % 9) / 10))));
    const del = i === count - 1 ? dels : Math.min(dels, Math.round(dels / left * (((i * 5 + n) % 7) / 4)));
    adds -= add;
    dels -= del;
    const status = del === 0 && i % 3 === 0 ? 'added' : 'modified';
    const lines = CODE[ext] || CODE.ts;
    const hunks = [];
    let oldLine = status === 'added' ? 0 : 10 + ((i * 13) % 40);
    let newLine = status === 'added' ? 1 : oldLine;
    let a = add;
    let d = del;
    let k = i;
    while (a > 0 || d > 0) {
      const chunkA = Math.min(a, 6 + ((k * 3) % 14));
      const chunkD = Math.min(d, (k * 2) % 5);
      const body = [];
      if (status !== 'added') for (let c = 0; c < 3; c += 1) body.push({ kind: 'ctx', old: oldLine + c, new: newLine + c, text: lines[(k + c) % lines.length] });
      const oStart = oldLine;
      const nStart = newLine;
      if (status !== 'added') { oldLine += 3; newLine += 3; }
      for (let c = 0; c < chunkD; c += 1) body.push({ kind: 'del', old: oldLine++, new: null, text: lines[(k + c + 5) % lines.length] });
      for (let c = 0; c < chunkA; c += 1) body.push({ kind: 'add', old: null, new: newLine++, text: lines[(k + c + 2) % lines.length] });
      const oldCount = body.filter((l) => l.kind !== 'add').length;
      const newCount = body.filter((l) => l.kind !== 'del').length;
      hunks.push({ header: `@@ -${oStart},${oldCount} +${nStart},${newCount} @@`, lines: body });
      a -= chunkA;
      d -= chunkD;
      oldLine += 20 + (k % 30);
      newLine += 20 + (k % 30);
      k += 1;
    }
    files.push({ path, old_path: null, status, insertions: add, deletions: del, binary: false, hunks });
  }
  const profile = sandbox.profile || {};
  return { base: profile.integration_branch || sandbox.default_branch, head: task.branch, base_sha: '2c9e41f07a1d', head_sha: `${(n * 2654435761 >>> 0).toString(16)}f00d`, ahead: (n * 3) % 11 + 1, behind: n % 3, uncommitted: n % 4 === 0 ? 2 : 0, files, truncated: false };
}
function taskCommits(task) {
  const n = task.number;
  const subjects = [`feat: ${task.title.toLowerCase()}`, 'test: cover the new rules', 'refactor: split the handler', 'docs: update the task spec', 'fix: rounding of tax lines', 'chore: format'];
  const count = (n * 3) % 11 + 1;
  return Array.from({ length: Math.min(count, 12) }, (_, i) => ({
    sha: `${((n * 40503 + i * 2654435761) >>> 0).toString(16).padStart(8, '0')}${'ab12cd34ef56ab78cd90ef12ab34cd56'.slice(0, 32)}`,
    subject: subjects[i % subjects.length], author: `task #${n} (claude)`, at: iso((i * 37 + 5) * MIN), files: (i * 3) % 7 + 1, insertions: (i * 41 + n) % 160 + 3, deletions: (i * 13 + n) % 40,
  }));
}
route('GET', `/api/tasks/${ID}/changes`, ({ db, p }) => { const task = taskOf(db, p[0]); return task.branch ? taskChanges(db, task) : json(404, { error: { code: 'not_found', message: 'This task has no branch yet.' } }); });
route('GET', `/api/tasks/${ID}/commits`, ({ db, p }) => { const task = taskOf(db, p[0]); return task.branch ? taskCommits(task) : []; });
route('GET', `/api/sandboxes/${ID}/skills`, ({ db, p }) => { sandboxOf(db, p[0]); return skillCatalog(db, p[0]); });
route('GET', `/api/sandboxes/${ID}/skills/${ID}`, ({ db, p }) => {
  const skill = skillCatalog(db, p[0]).find((s) => s.name === p[1]);
  if (!skill) fail(404, 'not_found', 'not found');
  if (skill.source === 'repository') return { name: skill.name, content: repoTree(db, p[0]).files.get(skill.path) };
  const stored = db.skills.find((s) => s.name === p[1]);
  if (stored.content === undefined) stored.content = skillContent(stored);
  return { name: stored.name, content: stored.content };
});
// PUT {enabled} toggles an instance or Open Mercato skill (SetSkillEnabled; repository skills: 422).
route('PUT', `/api/sandboxes/${ID}/skills/${ID}`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  if (typeof body.enabled !== 'boolean') fail(422, 'invalid', 'missing field `enabled`');
  const skill = skillCatalog(db, p[0]).find((s) => s.name === p[1]);
  if (!skill) fail(404, 'not_found', 'not found');
  if (skill.source === 'repository') fail(422, 'invalid', 'repository skills are always available inside the repository');
  if (skill.source === 'instance' && !db.me.user.is_instance_admin) fail(403, 'forbidden', 'instance skills are managed by instance administrators');
  db.skills.find((s) => s.name === p[1]).enabled = body.enabled;
  return json(204);
});

// ---- Built-in editor files API (EDT-001..004): list, read, write (atomic, base_sha256), create, delete.
route('GET', `/api/sandboxes/${ID}/files`, ({ db, p, query }) => {
  sandboxOf(db, p[0]);
  const tree = repoTree(db, p[0], query.get('root') || 'sandbox');
  const dir = repoPath(query.get('path'));
  if (!isDir(tree, dir)) fail(404, 'not_found', 'not found');
  const prefix = dir ? `${dir}/` : '';
  const entries = new Map();
  for (const path of [...tree.dirs, ...tree.files.keys()]) {
    if (!path.startsWith(prefix) || path === dir) continue;
    const rest = path.slice(prefix.length);
    const name = rest.split('/')[0];
    const isFile = !rest.includes('/') && tree.files.has(path);
    if (!entries.has(name) || !isFile) entries.set(name, { name, path: prefix + name, kind: isFile ? 'file' : 'dir', size: isFile ? tree.files.get(path).length : 0 });
  }
  const list = [...entries.values()].sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === 'dir' ? -1 : 1));
  return { path: dir, entries: list, truncated: false };
});
route('GET', `/api/sandboxes/${ID}/files/content`, ({ db, p, query }) => {
  sandboxOf(db, p[0]);
  const tree = repoTree(db, p[0], query.get('root') || 'sandbox');
  const path = repoPath(query.get('path'));
  if (!tree.files.has(path)) fail(404, 'not_found', 'not found');
  const content = tree.files.get(path);
  if (tree.binary && tree.binary.has(path)) return { path, content: null, binary: true, too_large: false, size: content.length, sha256: mockSha(content) };
  return { path, content, binary: false, too_large: false, size: content.length, sha256: mockSha(content) };
});
route('PUT', `/api/sandboxes/${ID}/files/content`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  const tree = repoTree(db, p[0], body.root || 'sandbox');
  const path = repoPath(body.path);
  if (typeof body.content !== 'string') fail(422, 'invalid', 'content is required');
  if (!isDir(tree, parentOf(path))) fail(404, 'not_found', 'the folder does not exist');
  const exists = tree.files.has(path);
  if (body.base_sha256 ? !exists || mockSha(tree.files.get(path)) !== body.base_sha256 : exists) fail(409, 'conflict', exists ? 'the file changed since it was read' : 'the file no longer exists');
  tree.files.set(path, body.content);
  return { path, sha256: mockSha(body.content), size: body.content.length };
});
route('POST', `/api/sandboxes/${ID}/files`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  const tree = repoTree(db, p[0], body.root || 'sandbox');
  const path = repoPath(body.path);
  if (!path) fail(422, 'invalid', 'path is required');
  if (tree.files.has(path) || isDir(tree, path)) fail(409, 'conflict', 'it already exists');
  if (!isDir(tree, parentOf(path))) fail(404, 'not_found', 'the folder does not exist');
  if (body.kind === 'dir') tree.dirs.add(path); else tree.files.set(path, '');
  return json(201, { name: path.split('/').pop(), path, kind: body.kind === 'dir' ? 'dir' : 'file', size: 0 });
});
// ---- Dropped files (EDT-006, UI-048): a raw file into the repository or a worktree (never
// replacing one unless asked), and terminal attachments with safe, unique names.
const uploadTooLarge = (db, size) => {
  const limit = db.me.instance.upload_max_bytes;
  if (size > limit) fail(422, 'invalid', `the file is larger than the upload limit of ${Math.round(limit / (1024 * 1024))} MB`);
};
route('PUT', `/api/sandboxes/${ID}/files/upload`, ({ db, p, query, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  const tree = repoTree(db, p[0], query.get('root') || 'sandbox');
  const path = repoPath(query.get('path'));
  if (!path || path.split('/').some((part) => !part || /[\u0000-\u001f\u007f]/.test(part))) fail(422, 'invalid', 'a file path is required');
  if (!body.blob) fail(422, 'invalid', 'the request body is the file');
  uploadTooLarge(db, body.size);
  const name = path.split('/').pop();
  const parents = path.split('/').slice(0, -1).map((_, i, parts) => parts.slice(0, i + 1).join('/'));
  const blocker = parents.find((dir) => tree.files.has(dir));
  if (blocker) fail(409, 'conflict', `${blocker.split('/').pop()} is a file, not a folder`);
  if (isDir(tree, path)) fail(409, 'conflict', `a folder named ${name} exists`);
  const replaced = tree.files.has(path);
  if (replaced && query.get('overwrite') !== 'true') fail(409, 'conflict', `${name} exists`);
  const text = /^(text\/|application\/(json|javascript|xml))/.test(body.type || '') || (!body.type && body.text.indexOf('\u0000') < 0);
  tree.binary = tree.binary || new Set();
  if (text) tree.binary.delete(path); else tree.binary.add(path);
  tree.files.set(path, text ? body.text : body.text.slice(0, 64));
  return json(201, { path, name, size: body.size, sha256: mockSha(body.text), replaced });
});
function mockAttachmentName(raw) {
  let out = '';
  for (const c of String(raw || '').split(/[\\/]/).pop()) {
    if (/[A-Za-z0-9._+]/.test(c) || (c.codePointAt(0) > 127 && !/\s/u.test(c))) { if (c === '.' && out.endsWith('-')) out = out.slice(0, -1); out += c; }
    else if (!out.endsWith('-')) out += '-';
  }
  out = out.replace(/^[.-]+/, '').replace(/-+$/, '');
  return out && !/^\.+$/.test(out) ? out : 'attachment';
}
function storeAttachment(db, sid, folder, query, body) {
  if (!body.blob) fail(422, 'invalid', 'the request body is the file');
  uploadTooLarge(db, body.size);
  db.attachments = db.attachments || new Map();
  const names = db.attachments.get(folder) || new Set();
  const base = mockAttachmentName(query.get('name'));
  const dot = base.lastIndexOf('.');
  const [stem, ext] = dot > 0 ? [base.slice(0, dot), base.slice(dot)] : [base, ''];
  let name = base;
  for (let n = 2; names.has(name); n += 1) name = `${stem}-${n}${ext}`;
  names.add(name);
  db.attachments.set(folder, names);
  return json(201, { path: `/var/lib/yulius/sandboxes/${sid}/attachments/${folder}/${name}`, name, size: body.size, sha256: mockSha(body.text || ''), replaced: false });
}
route('POST', `/api/tasks/${ID}/attachments`, ({ db, p, query, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  return storeAttachment(db, task.sandbox_id, `tasks/${task.id}`, query, body);
});
route('POST', `/api/sandboxes/${ID}/attachments`, ({ db, p, query, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  return storeAttachment(db, p[0], `drafts/${db.me.user.id}`, query, body);
});
route('POST', `/api/sandboxes/${ID}/shells/${ID}/attachments`, ({ db, p, query, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  const shell = db.shells.get(p[1]);
  if (!shell || shell.sandbox_id !== p[0]) fail(404, 'not_found', 'not found');
  return storeAttachment(db, p[0], `shells/${shell.id}`, query, body);
});
route('DELETE', `/api/sandboxes/${ID}/files`, ({ db, p, query }) => {
  need(sandboxOf(db, p[0]), 'write');
  const tree = repoTree(db, p[0], query.get('root') || 'sandbox');
  const path = repoPath(query.get('path'));
  if (!path) fail(403, 'forbidden', 'the folder itself cannot be deleted');
  for (const f of [...tree.files.keys()]) if (f === path || f.startsWith(`${path}/`)) tree.files.delete(f);
  for (const d of [...tree.dirs]) if (d === path || d.startsWith(`${path}/`)) tree.dirs.delete(d);
  return json(204);
});

route('GET', `/api/sandboxes/${ID}/workflows`, ({ db, p }) => db.workflows.filter((w) => w.scope === 'instance' || w.sandbox === p[0]).map(({ sandbox: _s, ...w }) => w));
route('POST', `/api/sandboxes/${ID}/workflows`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  if (body.scope === 'instance' && !db.me.user.is_instance_admin) fail(403, 'forbidden', 'instance workflows are managed by instance administrators');
  db.nextId += 1;
  const wf = { id: `wf_${db.nextId}`, sandbox: body.scope === 'instance' ? null : p[0], name: body.name, description: body.description || '', when_to_use: '', steps: body.steps || [], instructions: body.instructions || '', gates: { require_spec: Boolean(body.gates && body.gates.require_spec), require_tests: Boolean(body.gates && body.gates.require_tests) }, scope: body.scope === 'instance' ? 'instance' : 'sandbox', builtin: false };
  db.workflows.push(wf);
  const { sandbox: _s, ...out } = wf;
  return json(201, out);
});
// Default workflows are kept current by the server: duplicate one to customise it.
function mutableWorkflow(db, id) {
  const wf = db.workflows.find((w) => w.id === id);
  if (!wf) fail(404, 'not_found', 'not found');
  if (wf.builtin) fail(409, 'conflict', `${wf.name} is a default workflow kept current by Yulius: duplicate it to customise`);
  return wf;
}
route('PUT', `/api/workflows/${ID}`, ({ db, p, body }) => {
  const wf = mutableWorkflow(db, p[0]);
  Object.assign(wf, { name: body.name, description: body.description, steps: body.steps, instructions: body.instructions, gates: body.gates });
  const { sandbox: _s, ...out } = wf;
  return out;
});
route('DELETE', `/api/workflows/${ID}`, ({ db, p }) => { mutableWorkflow(db, p[0]); db.workflows = db.workflows.filter((w) => w.id !== p[0]); return json(204); });

route('GET', `/api/sandboxes/${ID}/automations`, ({ db, p }) => db.automations.filter((a) => a.sandbox === p[0]).map(({ sandbox: _s, ...a }) => a));
route('POST', `/api/sandboxes/${ID}/automations`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  if (Math.abs(Number(body.utc_offset_minutes) || 0) > 1080) fail(422, 'invalid', 'utc_offset_minutes must be within ±1080');
  db.nextId += 1;
  const a = { id: `aut_${db.nextId}`, sandbox: p[0], ...body, created_via: 'ui', next_run_at: body.enabled ? isoIn(5 * HOUR) : null, last_run: null };
  db.automations.push(a);
  const { sandbox: _s, ...out } = a;
  return json(201, out);
});
route('PUT', `/api/automations/${ID}`, ({ db, p, body }) => {
  const a = db.automations.find((x) => x.id === p[0]);
  if (!a) fail(404, 'not_found', 'not found');
  Object.assign(a, body, { next_run_at: body.enabled ? isoIn(5 * HOUR) : null });
  const { sandbox: _s, ...out } = a;
  return out;
});
route('DELETE', `/api/automations/${ID}`, ({ db, p }) => { db.automations = db.automations.filter((a) => a.id !== p[0]); return json(204); });
// Run now: one run, also when disabled; 200 with the AutomationRun.
route('POST', `/api/automations/${ID}/run`, ({ db, p }) => {
  const a = db.automations.find((x) => x.id === p[0]);
  if (!a) fail(404, 'not_found', 'not found');
  const previous = a.last_run && a.last_run.task_id ? db.tasks.find((t) => t.id === a.last_run.task_id) : null;
  const run = previous && LIVE.has(previous.status)
    ? { scheduled_for: new Date().toISOString(), state: 'skipped_previous_active', task_id: null, error: null }
    : { scheduled_for: new Date().toISOString(), state: 'task_created', task_id: 'tsk_studio_8', error: null };
  a.last_run = run;
  emit(db, { type: 'automation.run', automation_id: a.id, state: run.state, task_id: run.task_id });
  return run;
});
route('GET', `/api/automations/${ID}/runs`, ({ db, p }) => {
  const a = db.automations.find((x) => x.id === p[0]);
  if (!a) fail(404, 'not_found', 'not found');
  return [0, 1, 2, 3, 4].map((i) => ({ scheduled_for: iso((i + 1) * DAY - 10 * HOUR), state: i === 2 ? 'skipped_previous_active' : i === 4 ? 'failed' : 'task_created', task_id: i === 2 || i === 4 ? null : 'tsk_studio_8', error: i === 4 ? 'the task could not be created: capacity reached' : null }));
});

// ------------------------------------------------------------------ secrets vault (VLT-001..009)
// Synthetic values only. Reveals need a confirmation within 10 minutes (mock password below);
// agent secret values are write-only and never returned.

const REAUTH_MS = 10 * MIN;
const MOCK_SUPERADMIN_PASSWORD = 'correct horse battery';

function vaultOf(db) {
  if (!db.vault) {
    db.vault = {
      reauthAt: 0,
      team: [{ id: 'ts_gmail', title: 'Shared Gmail', url: 'https://mail.google.com', username: 'team@example.test', password: 'mock-not-a-real-password', notes: 'Recovery codes: mock', totp: 'JBSWY3DPEHPK3PXPJBSWY3DP', created_by: 'mkarp', updated_by: 'mkarp', updated_at: iso(3 * DAY), value_updated_at: iso(3 * DAY) }],
      agent: [{ id: 'as_stripe', sandbox: 'sb_studio', scope: 'sandbox', name: 'STRIPE_KEY', description: 'Stripe test-mode key for the checkout', hosts: ['api.stripe.com'], injection: 'bearer', header_name: null, username: null, value: 'mock-agent-value-1', totp: null, created_by: 'mkarp', updated_at: iso(DAY), value_updated_at: iso(DAY), last_used_at: iso(2 * HOUR) }],
      activity: [{ at: iso(HOUR), actor: 'mkarp', action: 'vault.team.reveal', secret_id: 'ts_gmail', detail: 'Shared Gmail (password)' }],
    };
  }
  return db.vault;
}

const INJECTION_TEXT = { bearer: 'Authorization: Bearer <value>', basic: 'Authorization: Basic <username:value>', none: 'no header (placeholders and browser filling only)' };

function teamView(s) {
  return { id: s.id, title: s.title, url: s.url || null, username: s.username || null, has_password: Boolean(s.password), has_notes: Boolean(s.notes), has_totp: Boolean(s.totp), created_by: s.created_by, updated_by: s.updated_by, updated_at: s.updated_at, value_updated_at: s.value_updated_at, rotate_due: false };
}

function agentView(db, s, sandbox) {
  const manage = s.scope === 'instance' ? Boolean(db.me.user.is_instance_admin || db.me.user.is_superadmin) : RANK[sandbox.access] >= RANK.write;
  return { id: s.id, name: s.name, description: s.description, scope: s.scope, hosts: s.hosts, injection: s.injection, header_name: s.header_name, injection_text: s.injection === 'header' ? `${s.header_name}: <value>` : INJECTION_TEXT[s.injection], placeholder: `yulius-secret-${s.name}`, has_username: Boolean(s.username), has_totp: Boolean(s.totp), created_by: s.created_by, updated_at: s.updated_at, value_updated_at: s.value_updated_at, last_used_at: s.last_used_at, rotate_due: false, can_manage: manage };
}

function reauthStatus(db) {
  const vault = vaultOf(db);
  const fresh = Date.now() - vault.reauthAt < REAUTH_MS;
  const methods = [];
  if (db.me.user.is_superadmin) methods.push('password');
  if (db.me.user.github_linked) methods.push('github');
  return { fresh, fresh_until: fresh ? new Date(vault.reauthAt + REAUTH_MS).toISOString() : null, methods };
}

function needFresh(db) {
  const status = reauthStatus(db);
  if (status.fresh) return null;
  return json(403, { error: { code: 'reauth_required', message: 'Confirm it is you to reveal secrets (password or GitHub sign-in in the last 10 minutes).' }, reauth: status });
}

function audit(db, action, id, detail) {
  vaultOf(db).activity.unshift({ at: new Date().toISOString(), actor: db.me.user.login, action, secret_id: id, detail });
}

function teamSecret(db, id) {
  return vaultOf(db).team.find((s) => s.id === id) || fail(404, 'not_found', 'not found');
}

const AGENT_NAME_RE = /^[A-Z][A-Z0-9_]{0,63}$/;
function validateAgent(body, creating) {
  if (body.name !== undefined && !AGENT_NAME_RE.test(body.name)) fail(422, 'invalid', `invalid name "${body.name}"`);
  if (body.hosts !== undefined && (!Array.isArray(body.hosts) || body.hosts.length === 0)) fail(422, 'invalid', 'bind the secret to at least one host');
  if ((creating || body.value !== undefined) && [...(body.value || '')].length < 8) fail(422, 'invalid', 'an agent secret needs at least 8 characters');
}

route('GET', '/api/team-secrets', ({ db }) => vaultOf(db).team.map(teamView));
route('POST', '/api/team-secrets', ({ db, body }) => {
  if (!String(body.title || '').trim()) fail(422, 'invalid', 'a title is required');
  const secret = { id: `ts_${db.nextId++}`, title: body.title.trim(), url: body.url || null, username: body.username || null, password: body.password || null, notes: body.notes || null, totp: body.totp || null, created_by: db.me.user.login, updated_by: db.me.user.login, updated_at: new Date().toISOString(), value_updated_at: new Date().toISOString() };
  vaultOf(db).team.push(secret);
  audit(db, 'vault.team.create', secret.id, secret.title);
  return json(201, teamView(secret));
});
route('PATCH', `/api/team-secrets/${ID}`, ({ db, p, body }) => {
  const secret = teamSecret(db, p[0]);
  for (const key of ['title', 'url', 'username', 'password', 'notes', 'totp']) if (body[key] !== undefined) secret[key] = body[key] || (key === 'title' ? secret.title : null);
  secret.updated_at = new Date().toISOString();
  audit(db, 'vault.team.update', secret.id, secret.title);
  return teamView(secret);
});
route('DELETE', `/api/team-secrets/${ID}`, ({ db, p }) => {
  const secret = teamSecret(db, p[0]);
  vaultOf(db).team = vaultOf(db).team.filter((s) => s !== secret);
  audit(db, 'vault.team.delete', secret.id, secret.title);
  return json(204);
});
route('POST', `/api/team-secrets/${ID}/reveal`, ({ db, p, body }) => {
  const secret = teamSecret(db, p[0]);
  const refused = needFresh(db);
  if (refused) return refused;
  const fields = body.fields || ['password', 'notes', 'totp'];
  audit(db, body.purpose === 'copy' ? 'vault.team.copy' : 'vault.team.reveal', secret.id, secret.title);
  return { password: fields.includes('password') ? secret.password : null, notes: fields.includes('notes') ? secret.notes : null, totp: fields.includes('totp') ? secret.totp : null };
});
route('POST', `/api/team-secrets/${ID}/totp`, ({ db, p }) => {
  const secret = teamSecret(db, p[0]);
  const refused = needFresh(db);
  if (refused) return refused;
  if (!secret.totp) fail(422, 'invalid', 'this secret has no TOTP seed');
  audit(db, 'vault.team.totp', secret.id, secret.title);
  return { code: String(100000 + (Math.floor(Date.now() / 30000) % 900000)), period: 30, remaining: 30 - (Math.floor(Date.now() / 1000) % 30) };
});
route('GET', '/api/auth/reauth', ({ db }) => reauthStatus(db));
route('POST', '/api/auth/reauth', ({ db, body }) => {
  if (!db.me.user.is_superadmin) fail(403, 'forbidden', 'this account has no password: sign in with GitHub again to confirm');
  if (body.password !== MOCK_SUPERADMIN_PASSWORD) { audit(db, 'vault.reauth.failed', null, 'password'); fail(403, 'forbidden', `wrong password (mock: ${MOCK_SUPERADMIN_PASSWORD})`); }
  vaultOf(db).reauthAt = Date.now();
  audit(db, 'vault.reauth', null, 'password');
  return reauthStatus(db);
});
route('GET', '/api/secrets/activity', ({ db }) => vaultOf(db).activity.slice(0, 100));
route('GET', `/api/sandboxes/${ID}/agent-secrets`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  return vaultOf(db).agent.filter((s) => s.scope === 'instance' || s.sandbox === sandbox.id).map((s) => agentView(db, s, sandbox));
});
route('POST', `/api/sandboxes/${ID}/agent-secrets`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  const scope = body.scope || 'sandbox';
  if (scope === 'instance' && !(db.me.user.is_instance_admin || db.me.user.is_superadmin)) fail(403, 'forbidden', 'secrets for every sandbox are managed by instance administrators');
  if (scope === 'sandbox') need(sandbox, 'write');
  validateAgent(body, true);
  if (vaultOf(db).agent.some((s) => s.name === body.name && (s.scope === 'instance' || s.sandbox === sandbox.id))) fail(409, 'conflict', `an agent secret named ${body.name} already applies here`);
  const secret = { id: `as_${db.nextId++}`, sandbox: scope === 'sandbox' ? sandbox.id : null, scope, name: body.name, description: body.description || '', hosts: body.hosts, injection: body.injection || 'bearer', header_name: body.header_name || null, username: body.username || null, value: body.value, totp: body.totp || null, created_by: db.me.user.login, updated_at: new Date().toISOString(), value_updated_at: new Date().toISOString(), last_used_at: null };
  vaultOf(db).agent.push(secret);
  audit(db, 'vault.agent.create', secret.id, `${secret.name} → ${secret.hosts.join(', ')}`);
  return json(201, agentView(db, secret, sandbox));
});
route('PATCH', `/api/agent-secrets/${ID}`, ({ db, p, body }) => {
  const secret = vaultOf(db).agent.find((s) => s.id === p[0]) || fail(404, 'not_found', 'not found');
  const sandbox = secret.sandbox ? sandboxOf(db, secret.sandbox) : db.sandboxes[0];
  if (secret.scope === 'sandbox') need(sandbox, 'write');
  validateAgent(body, false);
  for (const key of ['name', 'description', 'hosts', 'injection', 'header_name', 'username', 'value', 'totp']) if (body[key] !== undefined) secret[key] = body[key];
  secret.updated_at = new Date().toISOString();
  audit(db, 'vault.agent.update', secret.id, secret.name);
  return agentView(db, secret, sandbox);
});
route('DELETE', `/api/agent-secrets/${ID}`, ({ db, p }) => {
  const secret = vaultOf(db).agent.find((s) => s.id === p[0]) || fail(404, 'not_found', 'not found');
  if (secret.scope === 'sandbox') need(sandboxOf(db, secret.sandbox), 'write');
  vaultOf(db).agent = vaultOf(db).agent.filter((s) => s !== secret);
  audit(db, 'vault.agent.delete', secret.id, secret.name);
  return json(204);
});

route('GET', `/api/sandboxes/${ID}/mcp`, ({ db, p }) => db.mcp.filter((m) => m.scope === 'instance' || m.sandbox === p[0]).map(({ sandbox: _s, ...m }) => m));
route('POST', `/api/sandboxes/${ID}/mcp`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  if (!/^[A-Za-z0-9_-]{1,64}$/.test(body.name || '') || body.name.toLowerCase() === 'yulius') fail(422, 'invalid', 'invalid or reserved server name');
  if (body.scope === 'instance' && !db.me.user.is_instance_admin) fail(403, 'forbidden', 'instance servers need an instance administrator');
  db.nextId += 1;
  const m = { id: `mcp_${db.nextId}`, sandbox: p[0], scope: body.scope === 'instance' ? 'instance' : 'sandbox', name: body.name, transport: body.transport, command: body.command || null, args: body.args || [], url: body.url || null, env_keys: Object.keys(body.env || {}), header_keys: Object.keys(body.headers || {}), enabled: body.enabled !== false };
  db.mcp.push(m);
  const { sandbox: _s, ...out } = m;
  return json(201, out);
});
route('PUT', `/api/mcp/${ID}`, ({ db, p, body }) => {
  const m = db.mcp.find((x) => x.id === p[0]);
  if (!m) fail(404, 'not_found', 'not found');
  if (m.scope === 'instance' && !db.me.user.is_instance_admin) fail(403, 'forbidden', 'instance servers need an instance administrator');
  Object.assign(m, { name: body.name, transport: body.transport, command: body.command ?? null, args: body.args || [], url: body.url ?? null });
  if (body.enabled !== undefined) m.enabled = Boolean(body.enabled);
  // Secrets are write-only: an omitted map keeps the stored values, a given map replaces them.
  if (body.env) m.env_keys = Object.keys(body.env);
  if (body.headers) m.header_keys = Object.keys(body.headers);
  const { sandbox: _s, ...out } = m;
  return out;
});
route('DELETE', `/api/mcp/${ID}`, ({ db, p }) => { db.mcp = db.mcp.filter((m) => m.id !== p[0]); return json(204); });
route('POST', `/api/mcp/${ID}/test`, ({ db, p }) => {
  const m = db.mcp.find((x) => x.id === p[0]);
  if (!m) fail(404, 'not_found', 'not found');
  if (m.scope === 'instance' && !db.me.user.is_instance_admin) fail(403, 'forbidden', 'instance servers need an instance administrator');
  return m.name === 'apify' ? { ok: false, tools: [], error: 'APIFY_TOKEN rejected (401)' } : { ok: true, tools: ['search_knowledge', 'get_customer', 'list_feedback', 'get_product_doc'], error: null };
});

// ---- Shells (UI-016): the user's terminals per sandbox; at most 8 live ones per user and sandbox.
/** Where a shell started, as its processes see it (the sandbox repository or the task's worktree). */
function shellFolder(db, shell) {
  const slug = (db.sandboxes.find((s) => s.id === shell.sandbox_id) || { slug: 'sandbox' }).slug;
  const task = shell.task_id ? db.tasks.find((t) => t.id === shell.task_id) : null;
  return task ? `/srv/yulius/sandboxes/${slug}/worktrees/${task.number}` : `/srv/yulius/sandboxes/${slug}/repo`;
}
const shellDto = (shell) => ({ id: shell.id, name: shell.name, task_id: shell.task_id, folder: shell.folder, created_at: shell.created_at });
route('GET', `/api/sandboxes/${ID}/shells`, ({ db, p }) => {
  need(sandboxOf(db, p[0]), 'write');
  return [...db.shells.values()].filter((shell) => shell.sandbox_id === p[0] && !shell.cockpit).map(shellDto);
});
route('POST', `/api/sandboxes/${ID}/shells`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  if (body.task_id) {
    const task = db.tasks.find((t) => t.id === body.task_id && t.sandbox_id === p[0]);
    if (!task) fail(404, 'not_found', 'not found');
  }
  const own = [...db.shells.values()].filter((shell) => shell.sandbox_id === p[0] && !shell.cockpit);
  if (own.length >= 8) fail(429, 'capacity', 'at most 8 terminals per sandbox; close one first');
  const name = typeof body.name === 'string' && body.name.trim() ? body.name.trim().slice(0, 40)
    : `Terminal ${[1, 2, 3, 4, 5, 6, 7, 8, 9].find((n) => !own.some((shell) => shell.name === `Terminal ${n}`))}`;
  db.nextId += 1;
  const shell = { id: `sh_${p[0]}_${db.nextId}`, sandbox_id: p[0], name, task_id: body.task_id || null, created_at: new Date().toISOString(), session: null };
  shell.folder = shellFolder(db, shell);
  db.shells.set(shell.id, shell);
  return json(201, shellDto(shell));
});
// The user's Cockpit (SPEC-021): one plain Claude Code session per sandbox, started on demand,
// never listed, renamed or closed as a terminal. `&cockpit=fail` makes it fail to start;
// `&cockpit=gone` answers the first call with a Cockpit whose process is already gone (a reboot).
route('POST', `/api/sandboxes/${ID}/cockpit`, ({ db, p }) => {
  need(sandboxOf(db, p[0]), 'write');
  const scenario = new URLSearchParams(location.search).get('cockpit');
  if (scenario === 'fail') fail(429, 'capacity', 'no subscription has capacity right now');
  if (scenario === 'gone' && !db.cockpitGone) {
    db.cockpitGone = true;
    return { id: `ck_${p[0]}_gone`, name: 'Cockpit', task_id: null, folder: `/srv/yulius/sandboxes/studio/repo`, created_at: new Date().toISOString() };
  }
  let shell = [...db.shells.values()].find((s) => s.sandbox_id === p[0] && s.cockpit);
  if (!shell) {
    db.nextId += 1;
    db.cockpitStarts = (db.cockpitStarts || 0) + 1;
    shell = { id: `ck_${p[0]}_${db.nextId}`, sandbox_id: p[0], name: 'Cockpit', task_id: null, created_at: new Date().toISOString(), session: null, cockpit: true, resumed: db.cockpitStarts > 1 };
    shell.folder = shellFolder(db, shell);
    db.shells.set(shell.id, shell);
  }
  return shellDto(shell);
});
route('PATCH', `/api/sandboxes/${ID}/shells/${ID}`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  const shell = db.shells.get(p[1]);
  if (!shell || shell.sandbox_id !== p[0] || shell.cockpit) fail(404, 'not_found', 'not found');
  const name = typeof body.name === 'string' ? body.name.trim() : '';
  if (!name || name.length > 40) fail(422, 'invalid', 'a shell name has 1 to 40 characters and no control characters');
  shell.name = name;
  return shellDto(shell);
});
route('DELETE', `/api/sandboxes/${ID}/shells/${ID}`, ({ db, p }) => {
  need(sandboxOf(db, p[0]), 'write');
  const shell = db.shells.get(p[1]);
  if (!shell || shell.sandbox_id !== p[0] || shell.cockpit) fail(404, 'not_found', 'not found');
  if (shell.session) shell.session.end(null);
  db.shells.delete(shell.id);
  return json(204);
});

// ---- Administration

const adminOnly = (db) => { if (!db.me.user.is_instance_admin && !db.me.user.is_superadmin) fail(403, 'forbidden', 'instance administrators only'); };
const superOnly = (db) => { if (!db.me.user.is_superadmin) fail(403, 'forbidden', 'superadmin only'); };
route('GET', '/api/admin/users', ({ db }) => { superOnly(db); return db.users; });
route('POST', '/api/admin/users', ({ db, body }) => {
  superOnly(db);
  if (!/^[A-Za-z0-9-]{1,39}$/.test(body.github_login || '')) fail(422, 'invalid', 'not a valid GitHub login');
  if (db.users.some((u) => u.github_login.toLowerCase() === String(body.github_login).toLowerCase())) fail(409, 'conflict', 'Already admitted.');
  db.nextId += 1;
  const user = { id: `usr_${db.nextId}`, github_login: body.github_login, display_name: body.display_name || body.github_login, is_instance_admin: Boolean(body.is_instance_admin), disabled: false, github_linked: false };
  db.users.push(user);
  return json(201, user);
});
route('PATCH', `/api/admin/users/${ID}`, ({ db, p, body }) => {
  superOnly(db);
  const user = db.users.find((u) => u.id === p[0]);
  if (!user) fail(404, 'not_found', 'not found');
  if (body.disabled !== undefined) user.disabled = Boolean(body.disabled);
  if (body.is_instance_admin !== undefined) user.is_instance_admin = Boolean(body.is_instance_admin);
  return user;
});
route('GET', '/api/admin/accounts', ({ db }) => { adminOnly(db); return db.accounts; });
// 201 {account, login_id}: the account waits in `pending_login` until the login terminal finishes.
route('POST', '/api/admin/accounts', ({ db, body }) => {
  adminOnly(db);
  if (body.provider !== 'anthropic') fail(422, 'invalid', 'provider must be anthropic (Claude)');
  if (!body.label || body.label.length > 80) fail(422, 'invalid', 'label must be 1–80 characters');
  db.nextId += 1;
  const account = { id: `acc_${db.nextId}`, provider: body.provider, label: body.label, status: 'pending_login', usage: { primary: null, secondary: null, observed_at: null }, last_error: null, in_use_by: 0 };
  db.accounts.push(account);
  return json(201, { account, login_id: `login_${account.id}` });
});
route('PATCH', `/api/admin/accounts/${ID}`, ({ db, p, body }) => {
  adminOnly(db);
  const account = db.accounts.find((a) => a.id === p[0]);
  if (!account) fail(404, 'not_found', 'not found');
  // Only the name: the pool has no priorities or switches (F 2026-10-10).
  if (Object.keys(body).some((key) => key !== 'label')) fail(422, 'invalid', 'unknown field; only `label` can change');
  if (typeof body.label !== 'string' || !(body.label.trim().length >= 1 && body.label.length <= 80)) fail(422, 'invalid', 'label must be 1–80 characters');
  account.label = body.label.trim();
  emit(db, { type: 'account.updated', account });
  return account;
});
route('DELETE', `/api/admin/accounts/${ID}`, ({ db, p }) => {
  adminOnly(db);
  const account = db.accounts.find((a) => a.id === p[0]);
  if (!account) fail(404, 'not_found', 'not found');
  if (account.in_use_by > 0) fail(409, 'conflict', `${account.label} is used by ${account.in_use_by} live task(s)`);
  db.accounts = db.accounts.filter((a) => a.id !== p[0]);
  return json(204);
});
// Refreshes the provider's usage windows; an account that must sign in again gets the provider's 502.
route('POST', `/api/admin/accounts/${ID}/usage`, ({ db, p }) => {
  adminOnly(db);
  const account = db.accounts.find((a) => a.id === p[0]);
  if (!account) fail(404, 'not_found', 'not found');
  if (account.status === 'relogin' || account.status === 'pending_login') fail(502, 'upstream', account.last_error || 'the provider refused the usage request; sign in again');
  account.usage = { ...account.usage, observed_at: new Date().toISOString() };
  account.last_error = null;
  return account;
});
route('POST', `/api/admin/accounts/${ID}/relogin`, ({ db, p }) => {
  adminOnly(db);
  const account = db.accounts.find((a) => a.id === p[0]);
  if (!account) fail(404, 'not_found', 'not found');
  return { account, login_id: `login_${account.id}` };
});
// GET /api/admin/github and POST /api/admin/github/manifest are documented in docs/api.md but
// not routed by the server yet: like the server, the mock answers 404 (the page falls back
// to /api/me).

// ---- Analytics and status

const tokenTotals = (runs, tasks, input, output, cacheRead, cacheWrite, unknown) => ({ runs, tasks, input, output, cache_read: cacheRead, cache_write: cacheWrite, unknown_usage_runs: unknown });

route('GET', '/api/analytics/summary', ({ db, query }) => {
  if (query.get('sandbox') && !db.sandboxes.some((s) => s.id === query.get('sandbox'))) fail(404, 'not_found', 'not found');
  const fromDay = query.get('from');
  const toDay = query.get('to');
  const to = Date.parse(toDay || new Date().toISOString().slice(0, 10));
  const from = Date.parse(fromDay || new Date(to - 29 * DAY).toISOString().slice(0, 10));
  const days = Math.max(1, Math.round((to - from) / DAY) + 1);
  const perDay = [];
  for (let i = 0; i < days; i += 1) {
    const date = new Date(from + i * DAY).toISOString().slice(0, 10);
    const weekday = new Date(date).getUTCDay();
    const base = weekday === 0 || weekday === 6 ? 0 : 3;
    perDay.push({ date, count: (hashString(date + query.get('sandbox')) % 5) + base });
  }
  const total = perDay.reduce((a, d) => a + d.count, 0);
  const scale = total / 120;
  const runs = Math.round(total * 1.3);
  const unknown = Math.round(total * 0.1);
  const n = (v) => Math.round(v * scale);
  return {
    range: { sandbox: query.get('sandbox') || null, from: fromDay || null, to: toDay || null },
    tasks: {
      total,
      closed: Math.round(total * 0.62),
      by_status: { closed: Math.round(total * 0.62), stopped_cancelled: Math.round(total * 0.12), running: 3, failed_spawn_error: 1, stopped_crashed: 2 },
      by_harness: { claude: total },
      by_origin: { ui: Math.round(total * 0.8), automation: Math.round(total * 0.15), import: Math.round(total * 0.05) },
    },
    tasks_per_day: perDay,
    duration_ms: { median: 47 * MIN, p90: 3.2 * HOUR, total_wall: total * 58 * MIN, total_active: total * 31 * MIN },
    tokens: tokenTotals(runs, total, n(41e6), n(5.2e6), n(310e6), null, unknown),
    tokens_by_model: [
      { model: 'claude-opus-4', ...tokenTotals(Math.round(runs * 0.6), Math.round(total * 0.6), n(26e6), n(3.1e6), n(200e6), null, 0) },
      { model: 'claude-sonnet-4', ...tokenTotals(Math.round(runs * 0.3), Math.round(total * 0.3), n(12e6), n(1.7e6), n(90e6), null, 0) },
      { model: 'unknown', ...tokenTotals(unknown, unknown, null, null, null, null, unknown) },
    ],
    tokens_by_harness: [
      { harness: 'claude', ...tokenTotals(runs, total, n(41e6), n(5.2e6), n(310e6), null, unknown) },
    ],
    tokens_by_account: [
      ...db.accounts.filter((a) => a.status !== 'pending_login').map((a, i) => ({ account_id: a.id, account_label: a.label, ...tokenTotals(Math.round(runs / (i + 2)), Math.round(total / (i + 2)), n(14e6 / (i + 1)), n(1.8e6 / (i + 1)), null, null, 0) })),
      { account_id: 'acc_deleted', account_label: null, ...tokenTotals(2, 1, n(1e6), n(0.1e6), null, null, 0) },
    ],
    switches: { total: Math.round(total * 0.15) },
    feedback: { correct: Math.round(total * 0.3), incorrect: Math.round(total * 0.06) },
    truncated: false,
  };
});
const TASK_ROW_COLUMNS = ['task_id', 'sandbox', 'number', 'title', 'status', 'origin', 'harness', 'harnesses', 'models', 'accounts', 'created_at', 'closed_at', 'attempts', 'wall_ms', 'active_ms', 'input_tokens', 'output_tokens', 'cache_read_tokens', 'cache_write_tokens', 'unknown_usage_runs', 'switches', 'feedback', 'feedback_correct', 'feedback_incorrect'];
route('GET', '/api/analytics/tasks', ({ db, query }) => {
  const format = query.get('format') || 'json';
  if (format !== 'json' && format !== 'csv') fail(422, 'invalid', 'format must be json or csv');
  const sandboxFilter = query.get('sandbox');
  const rows = db.tasks.filter((t) => !sandboxFilter || t.sandbox_id === sandboxFilter).map((t) => {
    const known = t.number !== 6;
    return {
      task_id: t.id, sandbox: (db.sandboxes.find((s) => s.id === t.sandbox_id) || {}).slug || t.sandbox_id, number: t.number, title: t.title, status: t.status,
      origin: t.origin, harness: t.harness, harnesses: t.harness, models: known ? t.model : '', accounts: t.account_label || '',
      created_at: t.created_at, closed_at: t.status === 'closed' ? t.updated_at : null, attempts: 1, wall_ms: Date.now() - Date.parse(t.created_at),
      active_ms: known ? 41 * MIN : null, input_tokens: known ? 182340 : null, output_tokens: known ? 23410 : null, cache_read_tokens: known ? 1204000 : null,
      cache_write_tokens: null, unknown_usage_runs: known ? 0 : 1, switches: t.number === 9 ? 1 : 0, feedback: null, feedback_correct: 0, feedback_incorrect: 0,
    };
  });
  if (format === 'csv') {
    const cell = (v) => {
      if (v === null || v === undefined) return '';
      let text = String(v);
      if (typeof v === 'string' && /^[=+\-@\t\r]/.test(text)) text = `'${text}`;
      return /[",\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
    };
    const csv = [TASK_ROW_COLUMNS.join(','), ...rows.map((r) => TASK_ROW_COLUMNS.map((k) => cell(r[k])).join(','))].join('\r\n');
    return new Response(`${csv}\r\n`, { status: 200, headers: { 'Content-Type': 'text/csv; charset=utf-8', 'Content-Disposition': `attachment; filename="yulius-tasks-${query.get('from') || 'start'}-${query.get('to') || 'now'}.csv"` } });
  }
  return rows;
});
route('GET', '/api/status', ({ db }) => {
  const wobble = (Date.now() / 1000) % 60;
  const admin = db.me.user.is_instance_admin || db.me.user.is_superadmin;
  return {
    // Disk total unknown on purpose: unknown metrics are null, never zero.
    vm: { cpu_percent: 22 + Math.sin(wobble) * 9, cpu_count: 8, memory_used_bytes: 9.4 * 2 ** 30, memory_total_bytes: 16 * 2 ** 30, disk_used_bytes: 141 * 2 ** 30, disk_total_bytes: null, load: [1.21, 0.97, 0.74], load_1m: 1.21, uptime_s: 9 * 86400 + 3600 * 4 },
    sessions: { live: db.tasks.filter((t) => t.status === 'running').length, limit: 8, queued: db.tasks.filter((t) => t.status === 'queued').length },
    sandboxes: db.sandboxes.map((s) => ({ id: s.id, slug: s.slug, name: s.name, live_sessions: s.counts.running })),
    accounts: db.accounts.map((a) => (admin
      ? { label: a.label, provider: a.provider, status: a.status, usage: a.usage, id: a.id, last_error: a.last_error, in_use_by: a.in_use_by }
      : { label: a.label, provider: a.provider, status: a.status, usage: a.usage })),
    contained: db.me.instance.contained,
  };
});
// ------------------------------------------------------------------ instance updates (ADR-18)

const UPDATE_COMMITS = [
  { title: 'An update banner and update now', author: 'MrFishPL' },
  { title: 'A solid wordmark beside the mark, with a quiet tagline', author: 'MrFishPL' },
  { title: 'Keep the streamed browser live over a real link', author: 'BartlomiejNowak04' },
  { title: 'Drag and drop files into the editor and Claude', author: 'MrFishPL' },
];
/** How long each phase of a mock update takes (ms). */
const UPDATE_STEP_MS = 1500;
const UPDATE_STATES = ['building', 'pausing', 'restarting', 'resuming'];
const short7 = (sha) => (sha ? sha.slice(0, 7) : null);

function updateView(db) {
  const u = db.update;
  const superadmin = Boolean(db.me.user.is_superadmin);
  const running = u.run && UPDATE_STATES.includes(u.run.state);
  const view = { current: { commit: u.current, short: short7(u.current) }, phase: running ? u.run.state : null, helper_installed: null, available: null, check: null, run: null };
  if (!superadmin) return view;
  view.helper_installed = u.helper;
  view.check = u.checkedAt ? { checked_at: u.checkedAt, error: null, via: 'app', relation: u.offer ? 'behind' : 'up_to_date', head: u.offer || u.current } : { checked_at: null, error: null, via: null, relation: null, head: null };
  if (u.offer && u.checkedAt) {
    view.available = {
      commit: u.offer, short: short7(u.offer), relation: 'behind', count: UPDATE_COMMITS.length,
      summary: `${UPDATE_COMMITS.slice(0, 3).map((c) => c.title).join(' · ')} and ${UPDATE_COMMITS.length - 3} more`,
      commits: UPDATE_COMMITS.map((c, i) => ({ sha: `${i}${u.offer.slice(1)}`, short: `${i}${u.offer.slice(1, 7)}`, title: c.title, author: c.author, date: iso(i * HOUR) })),
    };
  }
  if (u.run) view.run = clone(u.run);
  return view;
}

function moveUpdate(db, state, patch = {}) {
  const run = db.update.run;
  if (!run) return;
  Object.assign(run, { state, state_since: new Date().toISOString(), progress: null, tasks: null, step: null }, patch);
  run.can_cancel = state === 'building' || state === 'pausing';
  run.can_proceed = state === 'pausing';
  if (!UPDATE_STATES.includes(state)) run.finished_at = new Date().toISOString();
  emit(db, { type: 'update.updated', phase: UPDATE_STATES.includes(state) ? state : null });
}

/** Walks a started update through its phases, like the server and helper do. */
function runUpdate(db, id) {
  const live = () => db.update.run && db.update.run.id === id && UPDATE_STATES.includes(db.update.run.state);
  const steps = 4;
  for (let i = 1; i <= steps; i += 1) {
    later(() => {
      if (!live() || db.update.run.state !== 'building') return;
      db.update.run.progress = { done: Math.round((340 * i) / steps), total: 340 };
      db.update.run.step = 'compiling';
      db.update.run.message = 'Building the new version. Tasks keep working meanwhile.';
      emit(db, { type: 'update.updated', phase: 'building' });
    }, (UPDATE_STEP_MS * i) / steps);
  }
  later(() => {
    if (!live()) return;
    moveUpdate(db, 'pausing', { step: 'pausing', message: 'Pausing tasks at their next step: 2 of 3 paused.', tasks: { paused: 2, total: 3, waiting: [{ id: 'tsk_x', number: 4, title: 'Fix the flaky export test', sandbox: 'studio' }], deadline: new Date(Date.now() + 30 * MIN).toISOString() } });
  }, UPDATE_STEP_MS + 100);
  later(() => {
    if (!live()) return;
    moveUpdate(db, 'restarting', { step: 'installing', message: 'Installing the new version.' });
  }, UPDATE_STEP_MS * 2);
  later(() => {
    if (!live()) return;
    if (!db.update.fail) db.update.current = db.update.run.target.commit;
    moveUpdate(db, 'resuming', { step: 'resuming', message: 'Resuming tasks: 1 of 3.', progress: { done: 1, total: 3 } });
  }, UPDATE_STEP_MS * 3);
  later(() => {
    if (!live()) return;
    if (db.update.fail) {
      moveUpdate(db, 'rolled_back', { message: 'The update was rolled back; the previous version runs.', error: 'The new server did not become healthy within 120 s (HTTP 500). The previous version and its database were restored.', log_tail: ['yulius update-helper: checking', 'yulius update-helper: rolling_back'] });
    } else {
      db.update.offer = null;
      moveUpdate(db, 'succeeded', { message: `Updated to ${short7(db.update.current)}.` });
    }
  }, UPDATE_STEP_MS * 4);
}

route('GET', '/api/update', ({ db }) => updateView(db));
route('POST', '/api/admin/update/check', ({ db }) => {
  if (!db.me.user.is_superadmin) fail(403, 'forbidden', 'superadmin only');
  db.update.checkedAt = new Date().toISOString();
  return updateView(db);
});
route('POST', '/api/admin/update', ({ db, body }) => {
  if (!db.me.user.is_superadmin) fail(403, 'forbidden', 'superadmin only');
  if (body.confirm !== true) fail(422, 'invalid', 'confirm the update: tasks pause while the server restarts');
  const u = db.update;
  if (!u.helper) fail(409, 'conflict', 'the update helper is not installed on this server (see the operations guide)');
  if (u.run && UPDATE_STATES.includes(u.run.state)) fail(409, 'conflict', 'an update is already running');
  if (!u.offer || body.commit !== u.offer) fail(409, 'conflict', `${short7(body.commit || '')} is not the update on offer; check for updates again`);
  const now = new Date().toISOString();
  db.nextId += 1;
  u.run = {
    id: `upd_${db.nextId}`, target: { commit: u.offer, short: short7(u.offer) }, from: { commit: u.current, short: short7(u.current) },
    summary: updateView(db).available.summary, state: 'building', step: 'fetching', message: 'Fetching the new version from GitHub.',
    progress: null, tasks: null, error: null, log_tail: [], requested_by: db.me.user.login, started_at: now, state_since: now, finished_at: null, can_cancel: true, can_proceed: false,
  };
  emit(db, { type: 'update.updated', phase: 'building' });
  runUpdate(db, u.run.id);
  return json(202, updateView(db));
});
route('POST', '/api/admin/update/cancel', ({ db }) => {
  if (!db.me.user.is_superadmin) fail(403, 'forbidden', 'superadmin only');
  const run = db.update.run;
  if (!run || !['building', 'pausing'].includes(run.state)) fail(409, 'conflict', 'no update can be cancelled now (it is restarting or done)');
  moveUpdate(db, 'cancelled', { error: `Cancelled by ${db.me.user.login}; nothing was installed.`, message: 'Cancelled; nothing was installed.' });
  return updateView(db);
});
route('POST', '/api/admin/update/proceed', ({ db }) => {
  if (!db.me.user.is_superadmin) fail(403, 'forbidden', 'superadmin only');
  if (!db.update.run || db.update.run.state !== 'pausing') fail(409, 'conflict', 'the update is not pausing tasks');
  return updateView(db);
});

function jevView(db) {
  const j = db.jev;
  const source = j.key ? 'settings' : j.config ? 'config' : null;
  return { configured: Boolean(source), source, set_at: j.key ? j.setAt : null, set_by: j.key ? j.setBy : null, config_present: j.config, config_error: null };
}
const superadminOnly = (db) => { if (!db.me.user.is_superadmin) fail(403, 'forbidden', 'superadmin only'); };
route('GET', '/api/admin/jev', ({ db }) => { superadminOnly(db); return jevView(db); });
route('PUT', '/api/admin/jev', ({ db, body }) => {
  superadminOnly(db);
  const key = String(body.key || '').trim();
  if (key.length < 8 || /\s/.test(key)) fail(422, 'invalid', 'the Jev key must be 8-4096 printable characters without spaces');
  Object.assign(db.jev, { key, setAt: new Date().toISOString(), setBy: db.me.user.login });
  return jevView(db);
});
route('DELETE', '/api/admin/jev', ({ db }) => { superadminOnly(db); db.jev.key = null; return jevView(db); });
route('POST', '/api/admin/jev/adopt', ({ db }) => {
  superadminOnly(db);
  if (!db.jev.config) fail(409, 'conflict', 'YULIUS_JEV_API_KEY_FILE holds no usable key');
  Object.assign(db.jev, { key: 'from-config', setAt: new Date().toISOString(), setBy: db.me.user.login });
  return jevView(db);
});
route('POST', '/api/admin/jev/test', ({ db }) => {
  superadminOnly(db);
  const key = db.jev.key || (db.jev.config ? 'from-config' : null);
  if (!key) return { ok: false, result: 'none', message: 'No Jev key is set.', latency_ms: null };
  if (key.includes('bad')) return { ok: false, result: 'refused', message: 'Jev refused this key. Check it and save it again.', latency_ms: 210 };
  return { ok: true, result: 'ok', message: 'Jev answered: the key works.', latency_ms: 240 };
});

// GET /api/capacity (`CapacityView`, ROT-013): the pool, burn rate and forecast. The mock's pool
// burns faster than it refills and tasks waited this week, so it recommends one more subscription.
route('GET', '/api/capacity', ({ db }) => {
  const admin = db.me.user.is_instance_admin || db.me.user.is_superadmin;
  const threshold = 98;
  const healthy = db.accounts.filter((a) => a.status === 'ok' || a.status === 'limited');
  const left = (key) => {
    const known = healthy.map((a) => a.usage && a.usage[key] && a.usage[key].used_percent).filter(Number.isFinite);
    return known.length ? known.reduce((sum, used) => sum + Math.max(0, threshold - used), 0) : null;
  };
  const isAvailable = (a) => healthy.includes(a) && ['primary', 'secondary'].every((key) => !(a.usage && a.usage[key] && a.usage[key].used_percent >= threshold));
  const paused = db.tasks.filter((t) => t.status === 'waiting_capacity');
  return {
    threshold,
    subscriptions: healthy.length,
    unknown_usage: healthy.filter((a) => !(a.usage && a.usage.secondary && Number.isFinite(a.usage.secondary.used_percent))).length,
    available: db.accounts.filter(isAvailable).length,
    weekly_left: left('secondary'),
    five_hour_left: left('primary'),
    burn_24h: 2.4,
    burn_7d: 1.9,
    runs_out_at: isoIn(1.6 * DAY),
    paused_hours_7d: 6.5,
    active_hours_7d: 118,
    needed: healthy.length + 1,
    add: 1,
    recommendation: 'Add about 1 subscription to stop pausing',
    paused_tasks: paused.length,
    resumes_at: null,
    accounts: db.accounts.map((a) => ({
      label: a.label, status: a.status, usage: a.usage, available: isAvailable(a),
      ...(admin ? { id: a.id, in_use_by: a.in_use_by } : {}),
    })),
  };
});

// ------------------------------------------------------------------ sessions (SES-001..005)

// Session documents: task sessions and Claude Code sessions people uploaded with `yulius onboard`,
// seeded on first use. Transcripts are normalized entries as the server returns them (redacted).
function sessionDocs(db) {
  if (db.sessionDocs) return db.sessionDocs;
  const text = (t) => ({ type: 'text', text: t, truncated: false });
  const call = (id, name, summary, input, diff = null) => ({ type: 'tool_call', id, name, summary, input: JSON.stringify(input, null, 2), diff });
  const result = (id, t, isError = false) => ({ type: 'tool_result', tool_use_id: id, text: t, is_error: isError, truncated: false });
  const entry = (line, role, blocks, extra = {}) => ({ line, role, at: extra.at || null, sidechain: Boolean(extra.sidechain), blocks });
  const checkout = [
    entry(0, 'user', [text('Add Stripe checkout to the cart page. Use the test keys from .env.')], { at: iso(3 * DAY) }),
    entry(1, 'assistant', [{ type: 'thinking', text: 'I should read the env file and the cart component first.', truncated: false }, text('I’ll look at the cart and the environment first.'), call('t1', 'Read', '/repo/.env', { file_path: '/repo/.env' })], { at: iso(3 * DAY - MIN) }),
    entry(2, 'user', [result('t1', '     1→STRIPE_SECRET_KEY=[REDACTED:env-value]\n     2→PORT=3000')]),
    entry(3, 'assistant', [text('Adding the checkout session endpoint with an idempotency key, so a double click never charges twice.'), call('t2', 'Edit', 'src/api/checkout.ts', { file_path: 'src/api/checkout.ts' }, { path: 'src/api/checkout.ts', hunks: [{ old: 'export async function checkout(cart) {\n  return null;\n}', new: 'export async function checkout(cart, key) {\n  return stripe.checkout.sessions.create(toSession(cart), { idempotencyKey: key });\n}' }] })]),
    entry(4, 'user', [result('t2', 'The file src/api/checkout.ts has been updated.')]),
    entry(5, 'assistant', [call('t3', 'Bash', 'npm test -- checkout', { command: 'npm test -- checkout', description: 'Run the checkout tests' })]),
    entry(6, 'user', [result('t3', 'PASS src/api/checkout.test.ts (4 tests)')]),
    entry(7, 'assistant', [text('Checkout works with Stripe test mode; the webhook verifies signatures before fulfilling orders.')], { at: iso(3 * DAY - 20 * MIN) }),
  ];
  const flaky = [
    entry(0, 'user', [text('Why is the e2e login test flaky on CI?')], { at: iso(5 * DAY) }),
    entry(1, 'assistant', [call('g1', 'Grep', 'waitForTimeout in e2e', { pattern: 'waitForTimeout', path: 'e2e' })]),
    entry(2, 'user', [result('g1', 'e2e/login.spec.ts:14:  await page.waitForTimeout(500);')]),
    entry(3, 'assistant', [text('The test sleeps 500 ms instead of waiting for the session cookie; on a slow runner the redirect is not done yet. Waiting for the dashboard heading fixes it.')]),
  ];
  const webhook = [
    entry(0, 'user', [text('Payment webhook: verify Stripe signatures and retry fulfilment with backoff.')], { at: iso(9 * HOUR) }),
    entry(1, 'assistant', [text('I’ll verify the signature with the webhook secret and queue fulfilment with three retries and exponential backoff.')]),
  ];
  const task = (id, number) => db.tasks.find((t) => t.id === id && t.number === number);
  const docs = [];
  const add = (doc) => docs.push({ bytes: 4096, redactions: 0, git_branch: 'main', continued_in: null, ...doc });
  const t12 = task('tsk_studio_12', 12);
  const t4 = task('tsk_studio_4', 4);
  if (t12) add({ id: 'ses_task_12', sandbox_id: 'sb_studio', kind: 'task', task_id: t12.id, session_id: 'b1f0-12', title: t12.title, author: t12.created_by, git_branch: t12.branch, started_at: iso(2 * HOUR), ended_at: iso(MIN), imported_at: null, transcript: checkout.slice(0, 4) });
  if (t4) add({ id: 'ses_task_4', sandbox_id: 'sb_studio', kind: 'task', task_id: t4.id, session_id: 'b1f0-04', title: t4.title, author: t4.created_by, git_branch: t4.branch, started_at: iso(10 * HOUR), ended_at: iso(9 * HOUR), imported_at: null, transcript: webhook });
  add({ id: 'ses_imp_checkout', sandbox_id: 'sb_studio', kind: 'imported', task_id: null, session_id: '5d2c-checkout', title: 'Add Stripe checkout to the cart page', author: { login: 'mkarp' }, started_at: iso(3 * DAY), ended_at: iso(3 * DAY - 20 * MIN), imported_at: iso(2 * DAY), redactions: 3, bytes: 182000, transcript: checkout });
  add({ id: 'ses_imp_flaky', sandbox_id: 'sb_studio', kind: 'imported', task_id: null, session_id: '7a1e-flaky', title: 'Why is the e2e login test flaky on CI?', author: { login: 'anna-q' }, git_branch: 'fix/e2e', started_at: iso(5 * DAY), ended_at: iso(5 * DAY - 30 * MIN), imported_at: iso(4 * DAY), bytes: 64000, transcript: flaky });
  db.sessionDocs = docs;
  return docs;
}

function sessionView(db, doc) {
  const sandbox = sandboxOf(db, doc.sandbox_id);
  const write = (RANK[sandbox.access] || 0) >= RANK.write;
  const admin = sandbox.access === 'admin';
  const taskRef = (id) => {
    const t = id ? db.tasks.find((x) => x.id === id) : null;
    return t ? { id: t.id, number: t.number, title: t.title, status: t.status } : null;
  };
  const imported = doc.kind === 'imported';
  return {
    id: doc.id, sandbox_id: doc.sandbox_id, kind: doc.kind, title: doc.kind === 'task' && taskRef(doc.task_id) ? taskRef(doc.task_id).title : doc.title,
    harness: 'claude', session_id: doc.session_id, author: doc.author, task: taskRef(doc.task_id), continued_in: taskRef(doc.continued_in),
    git_branch: doc.git_branch, started_at: doc.started_at, ended_at: doc.ended_at, imported_at: doc.imported_at, bytes: doc.bytes, redactions: doc.redactions,
    can_continue: imported && write, can_delete: imported && (admin || (write && doc.author && doc.author.login === db.me.user.login)), hits: [],
  };
}

function sessionDocOf(db, id) {
  const doc = sessionDocs(db).find((d) => d.id === id);
  if (!doc || !db.sandboxes.some((s) => s.id === doc.sandbox_id)) fail(404, 'not_found', 'No such session');
  return doc;
}

/** The searchable text of an entry, like the server's index passage. */
function entryText(entry) {
  return entry.blocks.map((b) => [b.text, b.name && `${b.name} ${b.summary}`, b.diff && b.diff.hunks.map((x) => x.new).join('\n')].filter(Boolean).join('\n')).join('\n');
}

function snippetOf(text, words) {
  const lower = text.toLowerCase();
  const at = Math.max(0, Math.min(...words.map((w) => { const i = lower.indexOf(w); return i < 0 ? Infinity : i; })) - 40);
  const window = text.slice(at, at + 160);
  const parts = [];
  let rest = window;
  while (rest) {
    const lowerRest = rest.toLowerCase();
    let best = null;
    for (const w of words) { const i = lowerRest.indexOf(w); if (i >= 0 && (best === null || i < best[0])) best = [i, w.length]; }
    if (!best) { parts.push({ text: rest, hit: false }); break; }
    if (best[0] > 0) parts.push({ text: rest.slice(0, best[0]), hit: false });
    parts.push({ text: rest.slice(best[0], best[0] + best[1]), hit: true });
    rest = rest.slice(best[0] + best[1]);
  }
  return [...(at > 0 ? [{ text: '…', hit: false }] : []), ...parts];
}

route('GET', `/api/sandboxes/${ID}/sessions`, ({ db, p, query }) => {
  sandboxOf(db, p[0]);
  const kind = query.get('kind') || '';
  if (kind && !['all', 'task', 'imported'].includes(kind)) fail(422, 'invalid', 'kind must be all, task or imported');
  const author = (query.get('author') || '').toLowerCase();
  const q = (query.get('q') || '').trim();
  const words = q.toLowerCase().split(/\s+/).filter(Boolean);
  const docs = sessionDocs(db).filter((d) => d.sandbox_id === p[0]);
  const authors = [...new Set(docs.map((d) => d.author && d.author.login).filter(Boolean))].sort();
  let views = docs
    .filter((d) => (!kind || kind === 'all' || d.kind === kind) && (!author || (d.author && d.author.login.toLowerCase() === author)))
    .sort((a, b) => Date.parse(b.ended_at || 0) - Date.parse(a.ended_at || 0))
    .map((d) => ({ doc: d, view: sessionView(db, d) }));
  if (words.length) {
    views = views.map(({ doc, view }) => {
      const hits = doc.transcript
        .map((e) => ({ e, text: entryText(e) }))
        .filter(({ text }) => words.every((w) => text.toLowerCase().includes(w)))
        .slice(0, 3)
        .map(({ e, text }) => ({ line: e.line, role: e.role === 'user' && e.blocks.every((b) => b.type === 'tool_result') ? 'tool' : e.role, snippet: snippetOf(text, words) }));
      return { doc, view: { ...view, hits } };
    }).filter(({ view }) => view.hits.length);
  }
  return { sessions: views.map(({ view }) => view), authors, total: views.length, query: q || null };
});
route('GET', `/api/sessions/${ID}`, ({ db, p }) => sessionView(db, sessionDocOf(db, p[0])));
route('GET', `/api/sessions/${ID}/transcript`, ({ db, p, query }) => {
  const doc = sessionDocOf(db, p[0]);
  const from = Number(query.get('from') || 0);
  const limit = Math.min(500, Math.max(1, Number(query.get('limit') || 200)));
  const entries = doc.transcript.filter((e) => e.line >= from).slice(0, limit);
  const last = entries.length ? entries[entries.length - 1].line : from;
  const more = doc.transcript.some((e) => e.line > last) && entries.length === limit;
  return { session: sessionView(db, doc), entries, next: more ? last + 1 : null, skipped_lines: 0 };
});
route('POST', `/api/sessions/${ID}/continue`, ({ db, p }) => {
  const doc = sessionDocOf(db, p[0]);
  const sandbox = sandboxOf(db, doc.sandbox_id);
  need(sandbox, 'write');
  if (doc.kind !== 'imported') fail(422, 'invalid', 'a task’s session is continued from its task');
  if (sandbox.onboarding === 'pending' || sandbox.onboarding === 'preparing') fail(409, 'conflict', 'this project is being prepared; tasks can start once the preparation task has made it ready (or an admin skips it)');
  const existing = doc.continued_in ? db.tasks.find((t) => t.id === doc.continued_in) : null;
  if (existing) return summary(existing);
  const number = Math.max(0, ...db.tasks.filter((t) => t.sandbox_id === sandbox.id).map((t) => t.number)) + 1;
  const id = `tsk_${sandbox.id.slice(3)}_${number}`;
  const task = {
    id, sandbox_id: sandbox.id, number, title: doc.title, harness: 'claude', status: 'starting', activity: null,
    branch: `feat/${doc.title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 40)}`,
    origin: 'import', created_by: { login: db.me.user.login }, created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    unread_messages: 0, archived: false, account_label: null, stepsList: [], model: 'claude-opus-4', effort: 'default', prompt: doc.title, delivery: null,
  };
  db.tasks.push(task);
  doc.continued_in = id;
  recount(db);
  later(() => touch(db, task, { status: 'running', activity: 'working', account_label: 'Claude #1' }), 900);
  return summary(task);
});
route('DELETE', `/api/sessions/${ID}`, ({ db, p }) => {
  const doc = sessionDocOf(db, p[0]);
  if (doc.kind !== 'imported') fail(422, 'invalid', 'a task’s session is deleted with its task');
  if (!sessionView(db, doc).can_delete) fail(403, 'forbidden', 'only the person who uploaded a session or an admin can delete it');
  db.sessionDocs = db.sessionDocs.filter((d) => d.id !== doc.id);
  return json(204);
});

// ------------------------------------------------------------------ fetch

const PUBLIC = ['/api/setup/status', '/api/setup', '/api/auth/password', '/api/auth/mock-github'];

async function handleFetch(db, url, init = {}) {
  await delay(50 + Math.random() * 150);
  if (db.offline) throw new TypeError('Failed to fetch'); // simulated outage (__yuliusMock.goOffline())
  const u = new URL(url, ORIGIN);
  const method = (init.method || 'GET').toUpperCase();
  let body = {};
  if (init.body instanceof Blob) {
    // An upload (EDT-006, UI-048): the raw file; small ones are kept as text.
    const blob = init.body;
    body = { blob, size: blob.size, type: blob.type, text: blob.size <= 2 * 1024 * 1024 ? await blob.text() : '' };
  } else if (init.body) {
    try { body = JSON.parse(init.body); } catch { return json(422, { error: { code: 'invalid', message: 'Malformed JSON' } }); }
  }
  if (!PUBLIC.includes(u.pathname) && !db.signedIn) return json(401, { error: { code: 'unauthenticated', message: 'Sign in first.' } });
  if (db.failPath && method === 'GET' && u.pathname === db.failPath) return json(500, { error: { code: 'internal', message: 'The mock server failed on purpose' } });
  const headers = init.headers || {};
  if (method !== 'GET' && !PUBLIC.includes(u.pathname) && headers['X-Yulius-CSRF'] !== db.me.csrf_token) {
    return json(403, { error: { code: 'forbidden', message: 'Missing or wrong CSRF token.' } });
  }
  for (const [m, re, handler] of R) {
    if (m !== method) continue;
    const match = u.pathname.match(re);
    if (!match) continue;
    try {
      // A handler may be async (one that takes a while, like Jev or Claude on the server).
      const result = await handler({ db, p: match.slice(1).map(decodeURIComponent), body, query: u.searchParams });
      return result instanceof Response ? result : json(200, result);
    } catch (error) {
      if (error instanceof MockError) return json(error.status, { error: { code: error.code, message: error.message } });
      console.error(error);
      return json(500, { error: { code: 'internal', message: String(error) } });
    }
  }
  return json(404, { error: { code: 'not_found', message: `No mock route for ${method} ${u.pathname}` } });
}

// ------------------------------------------------------------------ fake WebSocket

class MockSocket {
  constructor(path) {
    this.path = path;
    this.readyState = 0;
    this.binaryType = 'blob';
    this.onopen = null;
    this.onmessage = null;
    this.onclose = null;
    this.onerror = null;
    this.server = null;
    setTimeout(() => {
      if (this.readyState !== 0) return;
      this.readyState = 1;
      if (this.onopen) this.onopen({});
      if (this.server && this.server.open) this.server.open(this);
    }, 40);
  }

  send(data) {
    if (this.readyState !== 1) throw new Error('WebSocket is not open');
    setTimeout(() => { if (this.readyState === 1 && this.server && this.server.receive) this.server.receive(this, data); }, 0);
  }

  close(code = 1000, reason = '') {
    if (this.readyState === 3) return;
    this.readyState = 3;
    if (this.server && this.server.closed) this.server.closed(this);
    setTimeout(() => { if (this.onclose) this.onclose({ code, reason, wasClean: true }); }, 0);
  }

  /** Server → client message (string or ArrayBuffer). */
  push(data) {
    if (this.readyState !== 1) return;
    setTimeout(() => { if (this.readyState === 1 && this.onmessage) this.onmessage({ data }); }, 0);
  }

  /** Server-initiated close. */
  serverClose(code = 1000, reason = '') {
    if (this.readyState === 3) return;
    this.readyState = 3;
    if (this.server && this.server.closed) this.server.closed(this);
    setTimeout(() => { if (this.onclose) this.onclose({ code, reason, wasClean: code === 1000 }); }, 10);
  }
}

function terminalFrame(kind, seq, text) {
  const payload = typeof text === 'string' ? enc.encode(text) : text;
  const buffer = new ArrayBuffer(9 + payload.length);
  const view = new DataView(buffer);
  view.setUint8(0, kind);
  view.setBigUint64(1, BigInt(seq), false);
  new Uint8Array(buffer, 9).set(payload);
  return buffer;
}

// ANSI helpers
const E = '\x1b[';
const c = (n, s) => `${E}38;5;${n}m${s}${E}0m`;
const bold = (s) => `${E}1m${s}${E}0m`;
const dim = (s) => `${E}2m${s}${E}0m`;

const SCRIPT_CLAUDE = [
  `${c(255, '⏺')} I'll continue with the checkout handlers. Reading the current state first.`,
  `${c(255, '⏺')} ${bold('Read')}(src/api/checkout.rs)`,
  `  ${dim('⎿  Read 142 lines')}`,
  `${c(255, '⏺')} ${bold('Search')}(pattern: "fn cart_total", path: "src/")`,
  `  ${dim('⎿  Found 2 files')}`,
  `${c(255, '⏺')} ${bold('Update')}(src/api/checkout.rs)`,
  `  ${dim('⎿  Updated src/api/checkout.rs with 18 additions and 4 removals')}`,
  `     ${c(242, '41')} ${c(114, '+    let tax = rules.tax_for(&cart.region)?;')}`,
  `     ${c(242, '42')} ${c(114, '+    let total = cart.subtotal() + tax - discount;')}`,
  `     ${c(242, '43')} ${c(203, '-    let total = cart.subtotal();')}`,
  `${c(255, '⏺')} ${bold('yulius')} - steps_update (MCP)(step: "Implement checkout API", status: "in_progress", note: "tax rules wired")`,
  `  ${dim('⎿  Step 3/7 updated')}`,
  `${c(255, '⏺')} ${bold('Bash')}(cargo test -p checkout)`,
  `  ${dim('⎿  running 14 tests')}`,
  `     ${c(114, 'test cart::totals_include_tax ... ok')}`,
  `     ${c(114, 'test cart::discount_never_negative ... ok')}`,
  `     ${c(114, 'test result: ok. 14 passed; 0 failed')}`,
  `${c(255, '⏺')} The API is running at ${c(39, 'http://localhost:20003/api/cart')} — I opened it in the sandbox browser.`,
  `${c(255, '⏺')} ${bold('yulius')} - scope_claim (MCP)(pattern: "src/api/**")`,
  `  ${dim('⎿  Claimed src/api/**; no overlap with other tasks')}`,
  `${c(173, '✻')} ${c(173, 'Pondering…')} ${dim('(esc to interrupt)')}`,
  `${c(255, '⏺')} Done with this step. Anything else before I write the UI wiring?`,
];

/** Persistent fake PTY for one task: a scroll region above a native-looking footer. */
class TermSession {
  constructor(db, task) {
    this.db = db;
    this.task = task;
    this.seq = 0;
    this.lines = [];
    // Lines scrolled up from the bottom (the full-screen TUI scrolls its own transcript).
    this.scroll = 0;
    this.input = '';
    this.viewers = new Set();
    this.cols = 120;
    this.rows = 36;
    // ?othercols=N[&otherrows=M]: another, active viewer sets the size (N×M) until this one types
    // or clicks into the terminal, as on the server (F 2026-10-10); this viewer's own size is kept
    // for then.
    const params = new URLSearchParams(location.search);
    const otherCols = Number(params.get('othercols'));
    const otherRows = Number(params.get('otherrows'));
    this.otherController = Number.isFinite(otherCols) && otherCols >= 20;
    if (this.otherController) {
      this.cols = Math.min(400, otherCols);
      if (Number.isFinite(otherRows) && otherRows >= 8) this.rows = Math.min(200, otherRows);
    }
    this.viewerSize = null;
    this.script = SCRIPT_CLAUDE;
    this.scriptIndex = 0;
    this.timer = null;
    this.exited = !LIVE.has(task.status);
    this.writeHeader();
  }

  sandbox() {
    return this.db.sandboxes.find((s) => s.id === this.task.sandbox_id) || { slug: 'x', access: 'read' };
  }

  writeHeader() {
    const cwd = `/srv/yulius/sandboxes/${this.sandbox().slug}/worktrees/${this.task.number}`;
    {
      const w = 54;
      const row = (s, visible) => `${c(173, '│')} ${s}${' '.repeat(Math.max(0, w - 3 - visible))}${c(173, '│')}`;
      this.lines.push(
        c(173, `╭${'─'.repeat(w - 2)}╮`),
        row(`${c(173, '✻')} Welcome to ${bold('Claude Code')}!`, 26),
        row('', 0),
        row(dim('/help for help, /status for your current setup'), 46),
        row('', 0),
        row(dim(`cwd: ${cwd}`), 5 + cwd.length),
        c(173, `╰${'─'.repeat(w - 2)}╯`),
        '',
      );
    }
    // A multi-line prompt shows as lines (a bare \n would only move the cursor down).
    this.lines.push(...`${c(245, '>')} ${this.task.prompt || this.task.title}`.split('\n'), '');
    if (this.task.number === 10 && this.task.sandbox_id === 'sb_studio') {
      this.lines.push(
        `${c(255, '⏺')} The deploy needs an AWS console session. I can't sign in for you.`,
        `${c(255, '⏺')} ${bold('yulius')} - browser_request_user_action (MCP)(tab: "tab-aws", reason: "Sign in to AWS")`,
        `  ${dim('⎿  Tab highlighted for the user')}`,
        `${c(214, '!')} Please sign in to AWS in the sandbox browser: ${c(39, `${ORIGIN}/s/studio/browser?tab=tab-aws`)}`,
        `  Docs for the deploy role: ${c(39, 'https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles.html')}`,
      );
    } else {
      for (const line of this.script.slice(0, 6)) this.lines.push(line);
      this.scriptIndex = 6;
      // File references in every form the terminal links to the editor (F 2026-10-10) — a bare
      // path, `path:line:col` in backticks, a name in parentheses, an absolute path with #L, an OSC 8
      // file:// hyperlink — and some it must not: a missing file, a path above the worktree, .git,
      // a URL (a web link) and an OSC 8 link outside the worktree.
      const osc8 = (uri, label) => `\x1b]8;;${uri}\x1b\\${label}\x1b]8;;\x1b\\`;
      this.lines.push(
        `${c(255, '⏺')} Spec: docs/specs/2026-10-10-improve-the-code.md`,
        `  ${dim('⎿')}  See \`src/api/checkout.rs:2:5\`, (AGENTS.md), ${cwd}/README.md#L3 and ${osc8(`file://${cwd}/src/api/checkout.rs#L1`, 'the checkout source')}`,
        `  ${dim('⎿')}  ${dim('Not links:')} docs/missing.md, ../outside.md, .git/config, https://example.com/docs/guide.md, ${osc8('file:///etc/hosts', 'hosts')}`,
      );
    }
    if (this.exited) this.lines.push('', dim(`Session ended (${this.task.status}).`));
  }

  // The bottom of Claude Code as it really looks: the input between two rules, then the status line
  // (the repository and the task's branch, as Yulius would configure it) and the mode line —
  // contained runs use --dangerously-skip-permissions, so "bypass permissions on" — with the
  // background shells and agents when there are any.
  footer() {
    const sep = dim('─'.repeat(Math.max(10, this.cols - 1)));
    const prompt = `${bold('❯')} ${this.input}`;
    const sandbox = this.sandbox();
    const repo = (sandbox.repo && sandbox.repo.name) || sandbox.slug;
    const branch = this.task.branch || sandbox.default_branch || 'main';
    const status = ` ${c(114, '→')} ${c(80, repo)} ${c(75, 'git:(')}${c(203, branch)}${c(75, ')')}`;
    const contained = !(this.db.me.instance && this.db.me.instance.contained === false);
    const extras = [];
    if (!this.exited && this.task.number % 2 === 0) extras.push(c(80, '1 shell'));
    if (!this.exited && this.task.unread_messages) extras.push(dim(`← ${this.task.unread_messages === 1 ? '1 notice' : `${this.task.unread_messages} notices`}`));
    const mode = contained
      ? ` ${c(204, '⏵⏵ bypass permissions on')}${extras.length ? '' : dim(' (shift+tab to cycle)')}`
      : ` ${dim('? for shortcuts')}`;
    return { sep, prompt, status, mode: `${mode}${extras.map((x) => ` ${dim('·')} ${x}`).join('')}` };
  }

  regionBottom() {
    return Math.max(3, this.rows - 5);
  }

  snapshot() {
    const bottom = this.regionBottom();
    const end = this.lines.length - this.scroll;
    const visible = this.lines.slice(Math.max(0, end - bottom), end);
    while (visible.length < bottom) visible.unshift(''); // bottom-align like a scrolled TUI
    const f = this.footer();
    // Like Claude Code 2.1.292: full screen (alternate buffer) with mouse reporting, so the wheel
    // scrolls the transcript inside the app (#34).
    let out = `${E}?1049h${E}?1000h${E}?1006h${E}?25l${E}2J${E}r${E}H`;
    out += visible.join('\r\n');
    out += `${E}1;${bottom}r`;
    out += `${E}${bottom + 1};1H${f.sep}${E}${bottom + 2};1H${E}2K${f.prompt}${E}${bottom + 3};1H${f.sep}${E}${bottom + 4};1H${E}2K${f.status}${E}${bottom + 5};1H${E}2K${f.mode}`;
    out += `${E}${bottom + 2};${3 + this.input.length}H${E}?25h`;
    return out;
  }

  broadcast(text) {
    this.seq += 1;
    const frame = terminalFrame(1, this.seq, text);
    for (const ws of this.viewers) ws.push(frame);
  }

  sendSnapshot(ws) {
    ws.push(terminalFrame(2, this.seq, this.snapshot()));
  }

  appendLine(line) {
    this.lines.push(line);
    if (this.lines.length > 400) this.lines.splice(0, this.lines.length - 400);
    if (this.scroll) {
      // New output while scrolled up: back to the bottom, as the real TUI does.
      this.scroll = 0;
      this.broadcast(this.snapshot());
      return;
    }
    const bottom = this.regionBottom();
    this.broadcast(`${E}?25l${E}${bottom};1H\r\n${line}${E}${bottom + 2};${3 + this.input.length}H${E}?25h`);
  }

  redrawPrompt() {
    const bottom = this.regionBottom();
    this.broadcast(`${E}${bottom + 2};1H${E}2K${this.footer().prompt}`);
  }

  canWrite() {
    return (RANK[this.sandbox().access] || 0) >= RANK.write && LIVE.has(this.task.status);
  }

  // Like the server: a viewer with write access controls the size unless another one does.
  state(_ws) {
    const write = this.canWrite();
    return JSON.stringify({ type: 'state', attempt_seq: 1, harness: this.task.harness, cols: this.cols, rows: this.rows, can_write: write, controller: write && !this.otherController, status: this.task.status });
  }

  sendStates() {
    for (const ws of this.viewers) ws.push(this.state(ws));
  }

  open() {}

  receive(ws, data) {
    let msg;
    try { msg = JSON.parse(data); } catch { return; }
    if (msg.type === 'hello') {
      this.viewers.add(ws);
      if (this.canWrite() && msg.cols && msg.rows) this.viewerSize = [msg.cols, msg.rows];
      if (this.canWrite() && !this.otherController && msg.cols && msg.rows) { this.cols = Math.max(20, Math.min(400, msg.cols)); this.rows = Math.max(8, Math.min(200, msg.rows)); }
      ws.push(this.state(ws));
      this.sendSnapshot(ws);
      this.ensureTicking();
    } else if (msg.type === 'resize') {
      if (!this.canWrite()) return;
      this.viewerSize = [msg.cols, msg.rows];
      if (!this.otherController) this.applySize(msg.cols, msg.rows);
    } else if (msg.type === 'take_control') {
      this.takeControl();
    } else if (msg.type === 'input') {
      if (!this.canWrite()) { ws.push(JSON.stringify({ type: 'error', code: 'forbidden', message: 'Input requires write access to the repository.' })); return; }
      if (this.otherController && !msg.scroll) this.takeControl(); // the typist controls
      this.onInput(String(msg.data || ''));
    }
    // `active` / `idle`: the other mock viewer is always active, so presence alone moves nothing.
  }

  closed(ws) {
    this.viewers.delete(ws);
    if (this.viewers.size === 0) this.stopTicking();
  }

  /** The PTY takes a new size: every viewer gets the state and a redraw. */
  applySize(cols, rows) {
    const c2 = Math.max(20, Math.min(400, cols));
    const r2 = Math.max(8, Math.min(200, rows));
    if (c2 === this.cols && r2 === this.rows) { this.sendStates(); return; }
    this.cols = c2;
    this.rows = r2;
    this.seq += 1;
    for (const viewer of this.viewers) { viewer.push(this.state(viewer)); viewer.push(terminalFrame(2, this.seq, this.snapshot())); }
  }

  /** This viewer takes the size from the other one: the PTY takes its window's size at once. */
  takeControl() {
    if (!this.otherController) return;
    this.otherController = false;
    if (this.viewerSize) this.applySize(this.viewerSize[0], this.viewerSize[1]);
    else this.sendStates();
  }

  /** A wheel turn (SGR button 64 up, 65 down) scrolls the transcript by three lines. */
  wheel(button) {
    const most = Math.max(0, this.lines.length - this.regionBottom());
    const next = Math.min(most, Math.max(0, this.scroll + (button === 64 ? 3 : button === 65 ? -3 : 0)));
    if (next === this.scroll) return;
    this.scroll = next;
    this.broadcast(this.snapshot());
  }

  onInput(data) {
    // Mouse reports (the viewer's wheel) are not typed text.
    const typed = data.replace(/\x1b\[<(\d+);\d+;\d+[Mm]/g, (_, button) => { this.wheel(Number(button)); return ''; });
    for (const ch of typed.replace(/\x1b\[[AB]/g, '')) {
      if (ch === '\r' || ch === '\n') this.submit();
      else if (ch === '\x7f' || ch === '\b') { this.input = this.input.slice(0, -1); this.redrawPrompt(); }
      else if (ch === '\x03') { this.input = ''; this.redrawPrompt(); this.appendLine(dim('Interrupted by user')); }
      else if (ch === '\x1b') { this.appendLine(dim('(esc) interrupted')); }
      else if (ch === '\t') { /* no completion in the mock */ }
      else if (ch >= ' ' && this.input.length < 200) { this.input += ch; this.broadcast(ch); }
    }
  }

  submit() {
    const text = this.input.trim();
    this.input = '';
    this.redrawPrompt();
    if (!text) return;
    this.appendLine(`${c(245, '>')} ${text}`);
    if (text === '/exit') {
      this.appendLine(dim('Goodbye.'));
      this.finish('closed', 0);
      return;
    }
    if (text === '/lag') {
      this.appendLine(dim('(mock) closing this viewer with code 4008 to exercise reconnect…'));
      setTimeout(() => { for (const ws of [...this.viewers]) ws.serverClose(4008, 'lagged'); }, 300);
      return;
    }
    if (text === '/help') {
      this.appendLine(dim('Mock commands: /exit (close task), /lag (force reconnect), /model, anything else gets a canned reply.'));
      return;
    }
    if (text.startsWith('/model')) {
      this.appendLine(`  ${dim('⎿')}  Model set to ${bold(this.task.model)} (mock)`);
      return;
    }
    touch(this.db, this.task, { activity: 'working' });
    const replies = [
      `${c(255, '⏺')} Understood: “${text.slice(0, 60)}”. Let me look at the relevant files.`,
      `${c(255, '⏺')} ${bold('Read')}(docs/specs/2026-10-07-${this.task.branch.replace('feat/', '')}.md)`,
      `  ${dim('⎿  Read 64 lines')}`,
      /aws|login|browser/i.test(text)
        ? `${c(214, '!')} Open the sign-in tab: ${c(39, `${ORIGIN}/s/${this.sandbox().slug}/browser?tab=tab-aws`)}`
        : `${c(255, '⏺')} Preview is at ${c(39, `http://localhost:${this.sandbox().port_base || 20000}/`)}`,
      `${c(255, '⏺')} I'll update the plan and continue.`,
    ];
    replies.forEach((line, i) => setTimeout(() => { if (!this.exited) this.appendLine(line); }, 400 + i * 450));
    setTimeout(() => { if (!this.exited) touch(this.db, this.task, { activity: 'waiting' }); }, 400 + replies.length * 450);
  }

  finish(status, code) {
    this.exited = true;
    this.stopTicking();
    touch(this.db, this.task, { status, activity: null, account_label: null });
    for (const ws of this.viewers) ws.push(JSON.stringify({ type: 'exit', status, code }));
    this.sendStates();
  }

  notice(text) {
    this.appendLine(c(141, text));
    for (const ws of this.viewers) ws.push(JSON.stringify({ type: 'notice', text }));
  }

  taskChanged() {
    const live = LIVE.has(this.task.status);
    if (live && this.exited) {
      this.exited = false;
      this.appendLine(dim(`Resuming session (${this.task.harness})…`));
      this.sendStates();
    } else if (!live && !this.exited) {
      this.exited = true;
      this.stopTicking();
      this.appendLine(dim(`Session ended (${this.task.status}).`));
      for (const ws of this.viewers) ws.push(JSON.stringify({ type: 'exit', status: this.task.status, code: this.task.status === 'closed' ? 0 : null }));
      this.sendStates();
    } else {
      this.sendStates();
    }
    this.ensureTicking();
  }

  ensureTicking() {
    if (this.timer || this.viewers.size === 0) return;
    if (!(this.task.status === 'running' && this.task.activity === 'working')) return;
    this.timer = setInterval(() => {
      if (this.task.status !== 'running' || this.task.activity !== 'working' || this.scriptIndex >= this.script.length) {
        if (this.scriptIndex >= this.script.length && this.task.activity === 'working') {
          this.appendLine(`${c(173, '✻')} ${dim('Waiting for your input.')}`);
          touch(this.db, this.task, { activity: 'waiting' });
        }
        this.stopTicking();
        return;
      }
      this.appendLine(this.script[this.scriptIndex]);
      this.scriptIndex += 1;
      if (this.scriptIndex === 12) this.advanceStep();
    }, 1300);
  }

  advanceStep() {
    const list = this.task.stepsList;
    const index = list.findIndex((s) => s.status === 'in_progress');
    if (index < 0) return;
    list[index] = { ...list[index], status: 'done', updated_by: `task #${this.task.number} (${this.task.harness})`, updated_at: new Date().toISOString(), note: 'tests green' };
    if (list[index + 1]) list[index + 1] = { ...list[index + 1], status: 'in_progress', updated_by: `task #${this.task.number} (${this.task.harness})`, updated_at: new Date().toISOString() };
    emit(this.db, { type: 'steps.updated', task_id: this.task.id });
    touch(this.db, this.task);
  }

  stopTicking() {
    clearInterval(this.timer);
    this.timer = null;
  }
}

/**
 * Fake bash behind a sandbox terminal (`state` has attempt_seq and harness null). It outlives its
 * viewers like the real shell: a reload or another tab reattaches and gets the screen so far.
 */
class ShellSession {
  constructor(db, shell, slug) {
    this.db = db;
    this.shell = shell;
    this.slug = slug;
    this.seq = 0;
    this.input = '';
    this.viewers = new Set();
    this.cols = 100;
    this.rows = 30;
    const cwd = shell.task_id ? `~/sandbox/worktrees/${shell.task_id}` : '~/sandbox';
    // Like bash with Yulius's PROMPT_COMMAND: the working directory as OSC 7 before each prompt.
    this.prompt = `\x1b]7;file://sandbox${shell.folder || ''}\x1b\\${c(114, `you@${slug}`)}:${c(39, cwd)}$ `;
    this.screen = `${dim('Contained sandbox shell (mock). Try: ls, pwd, git status, exit')}\r\n${this.prompt}`;
    if (shell.cockpit) {
      // A plain Claude Code session (mock): its welcome and prompt; /exit ends it.
      this.prompt = `${c(39, '>')} `;
      this.screen = `${c(208, '✻')} Welcome to Claude Code${shell.resumed ? dim(' · resumed your last conversation') : ''}\r\n${dim(`  cwd: ${shell.folder}`)}\r\n\r\n${this.prompt}`;
    }
  }

  out(text) {
    this.seq += 1;
    this.screen = (this.screen + text).slice(-8000);
    for (const ws of this.viewers) ws.push(terminalFrame(1, this.seq, text));
  }

  open() {}

  receive(ws, data) {
    let msg;
    try { msg = JSON.parse(data); } catch { return; }
    if (msg.type === 'hello') {
      this.viewers.add(ws);
      if (msg.cols && msg.rows) { this.cols = msg.cols; this.rows = msg.rows; }
      ws.push(JSON.stringify({ type: 'state', attempt_seq: null, harness: null, cols: this.cols, rows: this.rows, can_write: true, controller: true, status: 'running' }));
      ws.push(terminalFrame(2, this.seq, `${E}2J${E}H${this.screen}`));
    } else if (msg.type === 'resize') {
      if (msg.cols && msg.rows) { this.cols = msg.cols; this.rows = msg.rows; }
    } else if (msg.type === 'input') {
      for (const ch of String(msg.data || '')) {
        if (ch === '\r') this.run();
        else if (ch === '\x7f') { if (this.input) { this.input = this.input.slice(0, -1); this.out('\b \b'); } }
        else if (ch === '\x03') { this.input = ''; this.out(`^C\r\n${this.prompt}`); }
        else if (ch >= ' ') { this.input += ch; this.out(ch); }
      }
    }
  }

  run() {
    const cmd = this.input.trim();
    this.input = '';
    const outputs = {
      ls: 'AGENTS.md  Cargo.toml  README.md  docs  src  tests',
      pwd: this.shell.folder || `/srv/yulius/sandboxes/${this.slug}/repo`,
      'git status': 'On branch main\r\nYour branch is up to date with \'origin/main\'.\r\n\r\nnothing to commit, working tree clean',
      whoami: 'you',
    };
    if (this.shell.cockpit) {
      if (cmd === '/exit') { this.out('\r\n'); this.end(0); return; }
      if (cmd === '/clear') { this.out(`${E}2J${E}H${this.prompt}`); return; }
      this.out(`\r\n${cmd ? `${c(208, '⏺')} ${cmd.endsWith('?') ? 'Good question. (mock answer)' : 'Done. (mock)'}\r\n\r\n` : ''}${this.prompt}`);
      return;
    }
    if (cmd === 'exit') {
      this.out('\r\nlogout\r\n');
      this.end(0);
      return;
    }
    const result = cmd ? outputs[cmd] ?? `bash: ${cmd.split(' ')[0]}: command not found (mock shell)` : '';
    this.out(`\r\n${result ? `${result}\r\n` : ''}${this.prompt}`);
  }

  /** The shell process ended: the server sends `exit` and closes every viewer's socket. */
  end(code) {
    for (const ws of [...this.viewers]) {
      ws.push(JSON.stringify({ type: 'exit', status: 'closed', code }));
      setTimeout(() => ws.serverClose(1000, 'shell ended'), 50);
    }
    this.viewers.clear();
    this.db.shells.delete(this.shell.id);
  }

  closed(ws) { this.viewers.delete(ws); }
}

/**
 * Fake provider login for the account login terminal: a fixed 500×40 PTY; `state.status`
 * goes running → ok and the exit frame carries the same status. Tokens are masked.
 */
class LoginSession {
  constructor(db, loginId) {
    this.db = db;
    this.accountId = loginId.replace(/^login_/, '');
    this.seq = 0;
    this.input = '';
    this.ws = null;
    this.done = false;
  }

  out(text) {
    this.seq += 1;
    if (this.ws) this.ws.push(terminalFrame(1, this.seq, text));
  }

  open() {}

  receive(ws, data) {
    let msg;
    try { msg = JSON.parse(data); } catch { return; }
    if (msg.type === 'hello') {
      this.ws = ws;
      ws.push(JSON.stringify(this.state()));
      this.seq += 1;
      ws.push(terminalFrame(2, this.seq, [
        bold('Claude Code login'),
        '',
        'Open this URL in your browser and approve access:',
        c(39, 'https://claude.ai/oauth/authorize?code=true&client_id=mock&state=xyz'),
        '',
        'Paste the authorization code here (any text in mock mode), then press Enter:',
        '> ',
      ].join('\r\n')));
    } else if (msg.type === 'input' && !this.done) {
      for (const ch of String(msg.data || '')) {
        if (ch === '\r') { this.finish(); return; }
        if (ch === '\x7f') { if (this.input) { this.input = this.input.slice(0, -1); this.out('\b \b'); } } else if (ch >= ' ') { this.input += ch; this.out('*'); }
      }
    }
  }

  state() {
    return { type: 'state', attempt_seq: null, harness: 'claude', cols: 500, rows: 40, can_write: !this.done, controller: true, status: this.done ? 'ok' : 'running' };
  }

  finish() {
    this.done = true;
    this.out(`\r\nToken: sk-ant-oat01-[REDACTED]\r\n${c(114, '✔')} Logged in. Credentials stored encrypted for this account.\r\n`);
    const account = this.db.accounts.find((a) => a.id === this.accountId);
    if (account) {
      Object.assign(account, { status: 'ok', last_error: null, usage: { primary: { used_percent: 3.0, resets_at: isoIn(5 * HOUR), window_minutes: 300 }, secondary: { used_percent: 1.0, resets_at: isoIn(7 * DAY), window_minutes: 10080 }, observed_at: new Date().toISOString() } });
      emit(this.db, { type: 'account.updated', account });
    }
    if (this.ws) {
      this.ws.push(JSON.stringify(this.state()));
      this.ws.push(JSON.stringify({ type: 'exit', status: 'ok', code: 0 }));
    }
  }

  closed() { this.ws = null; }
}

/**
 * Fake shared browser for one sandbox (SPEC-002 ADR-15): tabs, a page drawn on a canvas and sent
 * as video — encoded in the browser with WebCodecs (VP8, which every engine with WebCodecs
 * encodes; the real server sends H.264) — or as JPEG frames when a viewer asks for pixels or
 * WebCodecs is missing. Clicks and keys change the drawn page; copy answers with the typed text.
 */
class BrowserSession {
  constructor(db, sandbox) {
    this.db = db;
    this.sandbox = sandbox;
    this.viewers = new Map(); // ws → 'video' | 'pixels'
    this.size = { width: 1280, height: 800 };
    this.cursor = { x: 200, y: 200 };
    this.typed = new Map();
    this.next = 3;
    this.renderPending = false;
    this.encoder = null; // VideoEncoder
    this.encoderSize = null;
    this.config = null; // the video_config message
    this.needKey = true;
    this.frameNo = 0;
    this.acked = 0; // video frames the viewer acknowledged (flow control, F 2026-10-10)
    const port = sandbox.port_base || 20000;
    this.tabs = [
      { id: 'tab-app', title: `Checkout — localhost:${port + 3}`, url: `http://localhost:${port + 3}/`, active: true, action_requested: null },
      { id: 'tab-aws', title: 'Amazon Web Services Sign-In', url: 'https://signin.aws.amazon.com/', active: false, action_requested: sandbox.id === 'sb_studio' ? { reason: 'Sign in to AWS so task #10 can deploy', task_id: 'tsk_studio_10' } : null },
    ];
  }

  open(ws) {
    this.viewers.set(ws, 'video');
    ws.push(JSON.stringify({ type: 'tabs', tabs: this.tabs }));
    ws.push(JSON.stringify({ type: 'viewport', ...this.size }));
    this.announce(ws);
  }

  /** Tells a viewer what it gets and starts its stream from a keyframe. */
  announce(ws) {
    const wanted = this.viewers.get(ws);
    const video = wanted === 'video' && typeof VideoEncoder === 'function';
    if (wanted === 'video' && !video) this.viewers.set(ws, 'pixels');
    ws.push(JSON.stringify(video ? { type: 'stream', mode: 'video' } : { type: 'stream', mode: 'pixels', reason: wanted === 'video' ? 'the mock browser cannot encode video here' : undefined }));
    if (video && this.config) ws.push(JSON.stringify(this.config));
    this.needKey = true;
    this.render();
  }

  receive(ws, data) {
    let msg;
    try { msg = JSON.parse(data); } catch { return; }
    const tab = this.tabs.find((t) => t.id === msg.tab_id);
    const activate = (target) => { for (const t of this.tabs) t.active = t === target; };
    switch (msg.type) {
      case 'viewport':
        this.size = { width: Math.max(320, Math.min(1920, msg.width | 0)) & ~1, height: Math.max(240, Math.min(1200, msg.height | 0)) & ~1 };
        this.broadcast(JSON.stringify({ type: 'viewport', ...this.size }));
        this.render();
        break;
      case 'view':
        this.viewers.set(ws, msg.mode === 'pixels' ? 'pixels' : 'video');
        this.announce(ws);
        break;
      case 'watch':
        // The display shows one tab: watching brings it to the front.
        if (tab) { activate(tab); this.sendTabs(); this.render(); }
        break;
      case 'open': {
        this.next += 1;
        const url = msg.url || `http://127.0.0.1:8081/browser/welcome?sandbox=${this.sandbox.id}`;
        let host = url;
        try { host = new URL(url).host || 'New tab'; } catch { /* keep raw */ }
        const created = { id: `tab-${this.next}`, title: msg.url ? host : 'Welcome', url, active: true, action_requested: null };
        this.tabs.push(created);
        activate(created);
        this.sendTabs();
        this.render();
        break;
      }
      case 'navigate':
        if (tab) { tab.url = msg.url; try { tab.title = new URL(msg.url).host || msg.url; } catch { tab.title = msg.url; } this.sendTabs(); this.render(); }
        break;
      case 'activate':
        if (tab) { activate(tab); this.sendTabs(); this.render(); }
        break;
      case 'history':
      case 'reload':
        this.render();
        break;
      case 'close':
        if (tab) {
          const wasActive = tab.active;
          this.tabs = this.tabs.filter((t) => t !== tab);
          if (wasActive && this.tabs.length) this.tabs[this.tabs.length - 1].active = true;
          this.sendTabs();
          this.render();
        }
        break;
      case 'copy':
        ws.push(JSON.stringify({ type: 'clipboard', text: this.typed.get(msg.tab_id) || '' }));
        break;
      case 'ack':
        // Flow control of the video: the mock streams too little to pace.
        this.acked = Math.max(this.acked, Number(msg.frames) || 0);
        break;
      case 'keyframe':
        this.needKey = true;
        this.render();
        break;
      case 'mouse': {
        const shown = this.tabs.find((t) => t.active);
        if (!shown || msg.event === 'wheel') break;
        this.cursor = { x: msg.x, y: msg.y };
        if (msg.event === 'down' && shown.id === 'tab-aws' && shown.action_requested && msg.x > 60 && msg.x < 360 && msg.y > 330 && msg.y < 380) {
          shown.action_requested = null;
          shown.title = 'AWS Management Console';
          shown.url = 'https://console.aws.amazon.com/';
          this.sendTabs();
        }
        this.render();
        break;
      }
      case 'key': {
        const shown = this.tabs.find((t) => t.active);
        if (!shown) break;
        const typed = this.typed.get(shown.id) || '';
        if (msg.event === 'down' && msg.key === 'Backspace') this.typed.set(shown.id, typed.slice(0, -1));
        else if (msg.event === 'down' && typeof msg.key === 'string' && msg.key.length === 1) this.typed.set(shown.id, (typed + msg.key).slice(-80));
        else if (msg.event === 'char' && msg.text) this.typed.set(shown.id, (typed + msg.text).slice(-80));
        this.render();
        break;
      }
      default:
        break;
    }
  }

  closed(ws) {
    this.viewers.delete(ws);
  }

  broadcast(data) {
    for (const ws of this.viewers.keys()) ws.push(data);
  }

  sendTabs() {
    this.broadcast(JSON.stringify({ type: 'tabs', tabs: this.tabs }));
  }

  render() {
    if (this.renderPending || this.viewers.size === 0) return;
    this.renderPending = true;
    setTimeout(() => { this.renderPending = false; this.drawFrame(); }, 60);
  }

  /** The video encoder for the current size (re-created on a resize, announced to viewers). */
  videoEncoder() {
    const { width, height } = this.size;
    if (this.encoder && this.encoderSize && this.encoderSize.width === width && this.encoderSize.height === height) return this.encoder;
    if (this.encoder) { try { this.encoder.close(); } catch { /* closed */ } }
    this.encoderSize = { width, height };
    this.config = { type: 'video_config', codec: 'vp8', description: '', width, height };
    this.needKey = true;
    try {
      this.encoder = new VideoEncoder({
        output: (chunk) => {
          const bytes = new Uint8Array(10 + chunk.byteLength);
          bytes[0] = 4;
          bytes[1] = chunk.type === 'key' ? 1 : 0;
          new DataView(bytes.buffer).setBigUint64(2, BigInt(Math.max(0, Math.round(chunk.timestamp))), false);
          chunk.copyTo(bytes.subarray(10));
          for (const [ws, mode] of this.viewers) if (mode === 'video') ws.push(bytes.buffer.slice(0));
        },
        error: () => { this.encoder = null; },
      });
      this.encoder.configure({ codec: 'vp8', width, height, bitrate: 2_000_000, framerate: 30, latencyMode: 'realtime' });
    } catch {
      this.encoder = null;
    }
    for (const [ws, mode] of this.viewers) if (mode === 'video') ws.push(JSON.stringify(this.config));
    return this.encoder;
  }

  async drawFrame() {
    const tab = this.tabs.find((t) => t.active);
    if (!tab || this.viewers.size === 0) return;
    const { width, height } = this.size;
    const canvas = typeof OffscreenCanvas !== 'undefined' ? new OffscreenCanvas(width, height) : Object.assign(document.createElement('canvas'), { width, height });
    const g = canvas.getContext('2d');
    const aws = tab.id === 'tab-aws';
    const welcome = /\/browser\/welcome/.test(tab.url);
    g.fillStyle = '#ffffff';
    g.fillRect(0, 0, width, height);
    g.fillStyle = aws ? '#232f3e' : '#0f172a';
    g.fillRect(0, 0, width, 56);
    g.fillStyle = '#ffffff';
    g.font = 'bold 20px system-ui, sans-serif';
    g.fillText(aws ? 'aws' : welcome ? 'Welcome' : 'Checkout (dev)', 24, 36);
    g.fillStyle = '#111827';
    g.font = 'bold 28px system-ui, sans-serif';
    g.fillText(welcome ? `Browser of ${this.sandbox.name || this.sandbox.slug}` : tab.title, 60, 120);
    g.font = '15px ui-monospace, monospace';
    g.fillStyle = '#6b7280';
    g.fillText(welcome ? `localhost:${(this.sandbox.port_base || 20000) + 3} is listening` : tab.url, 60, 150);
    g.strokeStyle = '#d1d5db';
    g.lineWidth = 1;
    g.strokeRect(60, 250, 300, 40);
    g.fillStyle = '#111827';
    g.font = '16px system-ui, sans-serif';
    g.fillText(this.typed.get(tab.id) || (aws ? 'Account ID or alias' : 'Search products…'), 72, 276);
    if (aws && tab.action_requested) {
      g.fillStyle = '#ff9900';
      g.fillRect(60, 330, 300, 50);
      g.fillStyle = '#111827';
      g.font = 'bold 18px system-ui, sans-serif';
      g.fillText('Sign in (click me)', 130, 362);
    } else if (aws) {
      g.fillStyle = '#16a34a';
      g.font = 'bold 18px system-ui, sans-serif';
      g.fillText('✓ Signed in — the agent can continue.', 60, 362);
    } else if (!welcome) {
      for (let i = 0; i < 3; i += 1) {
        g.fillStyle = '#f3f4f6';
        g.fillRect(60 + i * 240, 330, 220, 160);
        g.fillStyle = '#374151';
        g.font = '15px system-ui, sans-serif';
        g.fillText(['Espresso cup', 'Pour-over kit', 'Grinder'][i], 76 + i * 240, 520);
      }
    }
    g.fillStyle = '#9ca3af';
    g.font = '12px system-ui, sans-serif';
    g.fillText(`Mock sandbox browser · ${this.sandbox.name} · ${width}×${height}`, 60, height - 20);
    const modes = [...this.viewers.values()];
    if (modes.includes('video')) {
      const encoder = this.videoEncoder();
      if (encoder && encoder.state === 'configured') {
        this.frameNo += 1;
        const frame = new VideoFrame(canvas, { timestamp: this.frameNo * 33_333 });
        encoder.encode(frame, { keyFrame: this.needKey });
        frame.close();
        this.needKey = false;
      }
    }
    if (!modes.includes('pixels')) return;
    let blob;
    if (canvas.convertToBlob) blob = await canvas.convertToBlob({ type: 'image/jpeg', quality: 0.8 });
    else blob = await new Promise((resolve) => canvas.toBlob(resolve, 'image/jpeg', 0.8));
    if (!blob) return;
    const jpeg = new Uint8Array(await blob.arrayBuffer());
    const frame = new Uint8Array(33 + jpeg.length);
    frame[0] = 3;
    const id = tab.id.padEnd(32, ' ').slice(0, 32);
    for (let i = 0; i < 32; i += 1) frame[1 + i] = id.charCodeAt(i);
    frame.set(jpeg, 33);
    for (const [ws, mode] of this.viewers) if (mode === 'pixels') ws.push(frame.buffer.slice(0));
  }
}

function openSocket(db, path) {
  const ws = new MockSocket(path);
  if (db.offline) {
    // Simulated outage: the connection never opens and fails as an unreachable server's would.
    ws.readyState = 3;
    setTimeout(() => { if (ws.onerror) ws.onerror({}); if (ws.onclose) ws.onclose({ code: 1006, reason: '', wasClean: false }); }, 30);
    return ws;
  }
  const fatal = (code, message) => {
    ws.server = { open: () => { ws.push(JSON.stringify({ type: 'error', code, message })); ws.serverClose(1008, code); } };
  };
  if (!db.signedIn) { fatal('unauthenticated', 'Sign in first.'); return ws; }
  let m;
  // Terminal sockets may carry the viewer's size (`?cols=&rows=`), applied on hello here.
  const route_ = path.split('?')[0];
  if (route_ === '/api/events') {
    ws.server = {
      open: () => db.eventSockets.add(ws),
      closed: () => db.eventSockets.delete(ws),
    };
    startAmbient(db);
  } else if ((m = route_.match(/^\/api\/tasks\/([^/]+)\/terminal$/))) {
    const task = db.tasks.find((t) => t.id === decodeURIComponent(m[1]));
    if (!task) { fatal('not_found', 'No such task.'); return ws; }
    let session = db.sessions.get(task.id);
    if (!session) { session = new TermSession(db, task); db.sessions.set(task.id, session); }
    ws.server = session;
  } else if ((m = route_.match(/^\/api\/shells\/([^/]+)\/terminal$/))) {
    const shell = db.shells.get(decodeURIComponent(m[1]));
    if (!shell) { fatal('not_found', 'not found'); return ws; }
    // One session per terminal: a reload or a second tab reattaches to the same screen.
    if (!shell.session) {
      const slug = (db.sandboxes.find((s) => s.id === shell.sandbox_id) || { slug: 'sandbox' }).slug;
      shell.session = new ShellSession(db, shell, slug);
    }
    ws.server = shell.session;
  } else if ((m = route_.match(/^\/api\/admin\/accounts\/login\/([^/]+)\/terminal$/))) {
    ws.server = new LoginSession(db, decodeURIComponent(m[1]));
  } else if ((m = route_.match(/^\/api\/sandboxes\/([^/]+)\/browser$/))) {
    const sandbox = db.sandboxes.find((s) => s.id === decodeURIComponent(m[1]));
    if (!sandbox) { fatal('not_found', 'No such sandbox.'); return ws; }
    let session = db.browsers.get(sandbox.id);
    if (!session) { session = new BrowserSession(db, sandbox); db.browsers.set(sandbox.id, session); }
    ws.server = session;
  } else {
    fatal('not_found', `No mock socket for ${path}`);
  }
  return ws;
}

/** Ambient activity so live updates are visible: one bounded timer for the whole mock. */
function startAmbient(db) {
  if (db.ambient) return;
  db.ambient = true;
  let tick = 0;
  setTimeout(() => {
    // Like the server, events reach only viewers of the sandbox (none without GitHub).
    if (!db.sandboxes.some((s) => s.id === 'sb_studio')) return;
    emit(db, { type: 'browser.action_requested', sandbox_id: 'sb_studio', tab_id: 'tab-aws', reason: 'Sign in to AWS so task #10 can deploy', task_id: 'tsk_studio_10' });
  }, 5000);
  setInterval(() => {
    if (db.eventSockets.size === 0) return;
    tick += 1;
    const queued = db.tasks.find((t) => t.status === 'queued' && t.sandbox_id === 'sb_studio' && !t.archived && t.number === 8);
    if (tick === 3 && queued) touch(db, queued, { status: 'starting' });
    if (tick === 5 && queued) touch(db, queued, { status: 'running', activity: 'working', account_label: 'Claude #1' });
    const switching = db.tasks.find((t) => t.id === 'tsk_studio_9');
    if (tick === 4 && switching && switching.status === 'switching') {
      touch(db, switching, { status: 'running', activity: 'waiting', account_label: 'Claude #4' });
      const session = db.sessions.get(switching.id);
      if (session) session.notice('↻ switched from Claude #2 to Claude #4 (account exhausted)');
    }
    const working = db.tasks.find((t) => t.id === 'tsk_studio_12');
    if (working && tick % 4 === 0 && working.status === 'running') touch(db, working);
  }, 4000);
}

/** Create the mock transport used when the page URL has ?mock=1. */
export function createMockTransport(params) {
  const db = seed(params);
  window.__yuliusMock = db; // debugging aid in the browser console (mock mode only)
  // Tests change a task as the server would announce it (`touchTask('sb_studio', 12, {…})`).
  db.touchTask = (sandboxId, number, changes) => touch(db, db.tasks.find((t) => t.sandbox_id === sandboxId && t.number === number), changes);
  // Simulate an outage and its end: __yuliusMock.goOffline() / .goOnline().
  db.goOffline = () => {
    db.offline = true;
    for (const ws of [...db.eventSockets]) ws.serverClose(1006, '');
    for (const session of db.sessions.values()) for (const ws of [...(session.viewers || [])]) ws.serverClose(1006, '');
  };
  db.goOnline = () => { db.offline = false; };
  return {
    kind: 'mock',
    // Like window.fetch, an aborted signal (a timeout, a view that went away) rejects the request.
    fetch: (url, init = {}) => {
      const work = handleFetch(db, url, init);
      const { signal } = init;
      if (!signal) return work;
      return new Promise((resolve, reject) => {
        const abort = () => reject(new DOMException('The operation was aborted.', 'AbortError'));
        if (signal.aborted) { abort(); return; }
        signal.addEventListener('abort', abort, { once: true });
        work.then(resolve, reject);
      });
    },
    // Uploads report progress in a few steps, then answer like fetch (the real one uses XHR).
    upload: (url, init, onProgress) => new Promise((resolve, reject) => {
      const size = (init.body && init.body.size) || 0;
      let step = 0;
      const tick = () => {
        if (init.signal && init.signal.aborted) { reject(new DOMException('The operation was aborted.', 'AbortError')); return; }
        step += 1;
        onProgress(Math.round((size * step) / 4), size);
        if (step < 4) setTimeout(tick, 60);
        else handleFetch(db, url, init).then(resolve, reject);
      };
      setTimeout(tick, 30);
    }),
    openSocket: (path) => openSocket(db, path),
  };
}
