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
// role=member|admin|superadmin (default superadmin), contained=0 (host mode), github=0.

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
    });
  }
  return steps;
}

function seed(params) {
  const role = params.get('role') || 'superadmin';
  const db = {
    setupRequired: params.get('setup') === '1',
    signedIn: params.get('signedout') !== '1',
    me: {
      user: {
        id: 'usr_1', login: 'mkarp', display_name: 'Michal Karp', avatar_url: null,
        is_superadmin: role === 'superadmin', is_instance_admin: role !== 'member', github_linked: true,
      },
      csrf_token: 'mock-csrf-token',
      instance: { owner_login: 'Quartio-Studio', owner_kind: 'org', public_url: ORIGIN, github_configured: params.get('github') !== '0', contained: params.get('contained') !== '0' },
    },
    sandboxes: [],
    tasks: [],
    messages: [],
    eventSockets: new Set(),
    sessions: new Map(),
    browsers: new Map(),
    liveShells: new Set(),

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
      created_at: iso(extra.age || number * 3 * HOUR), updated_at: iso(extra.updated ?? number * 7 * MIN),
      unread_messages: extra.unread || 0, archived: Boolean(extra.archived),
      account_label: LIVE.has(status) ? 'Claude #2' : null,
      stepsList: extra.noSteps ? [] : makeSteps(id, done, current),
      model: 'claude-opus-4',
      effort: 'high',
      delivery: extra.delivery || null, // {stage, environment, pr} once delivery started
    };
  };
  const pr = (number, title, state, base, checks) => ({ number, url: `https://github.com/Quartio-Studio/${number > 400 ? 'product' : 'studio'}/pull/${number}`, title, state, base, head: `feat/${title.toLowerCase().replace(/[^a-z0-9]+/g, '-')}`, checks });
  db.tasks = [
    t('sb_studio', 12, 'Checkout API', 'claude', 'running', 'working', { done: 2, unread: 2, updated: 20e3 }),
    t('sb_studio', 11, '3D model viewer', 'claude', 'running', 'waiting', { done: 2, author: 'anna-q' }),
    t('sb_studio', 10, 'AWS login for deploy', 'claude', 'running', 'needs_attention', { done: 4 }),
    t('sb_studio', 9, 'Move to the next account', 'claude', 'switching', null, { done: 3 }),
    t('sb_studio', 8, 'Nightly feedback digest', 'claude', 'queued', null, { origin: 'automation', noSteps: true }),
    t('sb_studio', 7, 'Search indexing', 'claude', 'waiting_capacity', 'needs_attention', { done: 1 }),
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
    t('sb_three', 2, 'Write README', 'claude', 'closed', null, { done: 7, current: false }),
    t('sb_three', 1, 'Explore data model', 'claude', 'running', 'working', { done: 1, author: 'anna-q' }),
  ];
  db.messages = [
    { id: 'msg_1', task: 'tsk_studio_12', from: { task_id: 'tsk_studio_11', login: null, title: '#11 3D model viewer (claude)' }, to_task_id: 'tsk_studio_12', kind: 'scope', subject: 'Scope: src/viewer/**', body: 'I am working in src/viewer/** and public/models/**. Please avoid those paths; I will stay out of src/api/**.', created_at: iso(42 * MIN), read: true },
    { id: 'msg_2', task: 'tsk_studio_12', from: { task_id: 'tsk_studio_10', login: null, title: '#10 AWS login for deploy (claude)' }, to_task_id: 'tsk_studio_12', kind: 'question', subject: 'Which port does the API use?', body: 'Is the checkout API served on http://localhost:20003 in this sandbox? I need it for the deploy smoke test.', created_at: iso(8 * MIN), read: false },
    { id: 'msg_3', task: 'tsk_studio_12', from: { task_id: null, login: 'mkarp', title: 'mkarp' }, to_task_id: 'tsk_studio_12', kind: 'info', subject: '', body: 'Use the Stripe test keys only. The AWS sign-in tab is open: ' + `${ORIGIN}/s/studio/browser?tab=tab-aws`, created_at: iso(3 * MIN), read: false },
  ];
  db.accounts = [
    { id: 'acc_c1', provider: 'anthropic', label: 'Claude #1', email: 'michal.karp@example.com', plan: 'Max 20x', enabled: true, priority: 1, status: 'ok', usage: { primary: { used_percent: 42.0, resets_at: isoIn(2.2 * HOUR), window_minutes: 300 }, secondary: { used_percent: 18.5, resets_at: isoIn(4 * DAY), window_minutes: 10080 }, observed_at: iso(2 * MIN) }, last_error: null, in_use_by: 1 },
    { id: 'acc_c2', provider: 'anthropic', label: 'Claude #2', email: 'team.dev@example.com', plan: 'Max 20x', enabled: true, priority: 2, status: 'ok', usage: { primary: { used_percent: 87.3, resets_at: isoIn(1.1 * HOUR), window_minutes: 300 }, secondary: { used_percent: 61.0, resets_at: isoIn(2 * DAY), window_minutes: 10080 }, observed_at: iso(1 * MIN) }, last_error: null, in_use_by: 3 },
    { id: 'acc_c3', provider: 'anthropic', label: 'Claude #3', email: 'ops@example.com', plan: 'Max 5x', enabled: true, priority: 3, status: 'relogin', usage: { primary: null, secondary: null, observed_at: null }, last_error: 'OAuth refresh token expired; sign in again.', in_use_by: 0 },
    { id: 'acc_x1', provider: 'anthropic', label: 'Claude #4', email: 'studio@example.com', plan: 'Max 5x', enabled: true, priority: 1, status: 'ok', usage: { primary: { used_percent: 12.0, resets_at: isoIn(3.5 * HOUR), window_minutes: 300 }, secondary: { used_percent: 33.0, resets_at: isoIn(5 * DAY), window_minutes: 10080 }, observed_at: iso(4 * MIN) }, last_error: null, in_use_by: 2 },
    { id: 'acc_x2', provider: 'anthropic', label: 'Claude #5', email: 'product@example.com', plan: 'Pro', enabled: true, priority: 2, status: 'limited', usage: { primary: { used_percent: 99.6, resets_at: isoIn(1.9 * HOUR), window_minutes: 300 }, secondary: { used_percent: 71.0, resets_at: isoIn(3 * DAY), window_minutes: 10080 }, observed_at: iso(30e3) }, last_error: null, in_use_by: 0 },
    { id: 'acc_x3', provider: 'anthropic', label: 'Claude spare', email: null, plan: null, enabled: false, priority: 9, status: 'ok', usage: { primary: { used_percent: 0.5, resets_at: null, window_minutes: 300 }, secondary: null, observed_at: iso(3 * DAY) }, last_error: null, in_use_by: 0 },
  ];
  // `wf-feature` and `wf-quick-task` are the built-in instance workflows the server seeds.
  db.workflows = [
    { id: 'wf-feature', sandbox: null, name: 'Feature', description: 'Default discipline: spec with plan, implementation, tests, PR.', steps: ['Write task spec', 'Implement', 'Test', 'Open PR'], instructions: 'Follow AGENTS.md. Keep the spec Plan checklist in sync.', gates: { require_spec: true, require_tests: true }, scope: 'instance' },
    { id: 'wf-quick-task', sandbox: null, name: 'Quick task', description: 'Small change without a spec gate.', steps: ['Implement', 'Test'], instructions: '', gates: { require_spec: false, require_tests: true }, scope: 'instance' },
    { id: 'wf_bugfix', sandbox: 'sb_studio', name: 'Bugfix', description: 'Reproduce with a failing test first.', steps: ['Reproduce with a failing test', 'Fix', 'Open PR'], instructions: '', gates: { require_spec: true, require_tests: true }, scope: 'sandbox' },
  ];
  db.skills = [
    { name: 'yulius-rust-slice', description: 'Implement a vertical Rust slice with tests and docs.', source: 'repository', path: '.agents/skills/yulius-rust-slice/SKILL.md', enabled: true },
    { name: 'design', description: 'Make UI designs consistent with the Quartio design system.', source: 'repository', path: '.claude/skills/design/SKILL.md', enabled: true },
    { name: 'quartio-brand', description: 'Brand voice and visual identity for customer-facing copy.', source: 'instance', path: 'instance/skills/quartio-brand/SKILL.md', enabled: true },
    { name: 'om-code-review', description: 'Structured code review checklist (Open Mercato).', source: 'open-mercato', path: 'open-mercato/om-code-review/SKILL.md', enabled: true },
    { name: 'om-testing', description: 'Write meaningful tests for every behavior change (Open Mercato).', source: 'open-mercato', path: 'open-mercato/om-testing/SKILL.md', enabled: false },
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
  const pastRun = (n, env, kind, by, ago) => ({ run_id: `run_seed_${n}`, sandbox_id: 'sb_product', environment: env, kind, state: 'succeeded', command: kind === 'manual' ? null : 'node scripts/deployer.mjs --target dev-vm', commit: 'b7e9f01c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a', requested_by: by, output: kind === 'manual' ? `${by} confirmed ${env}.` : 'deploying… done in 2m 14s', exit_code: kind === 'manual' ? null : 0, started_at: iso(ago), ended_at: iso(ago - 2 * MIN), startedMs: Date.now() - ago - 1e9 });
  db.runs.push(pastRun(1, 'dev-vm', 'command', 'mkarp', 2 * DAY + HOUR), pastRun(2, 'dev-vm', 'manual', 'mkarp', 2 * DAY + 30 * MIN), pastRun(3, 'rollout', 'manual', 'anna-q', 2 * DAY - HOUR), pastRun(4, 'dev-vm', 'command', 'mkarp', 3 * HOUR));
  db.promotionPr = new Map(); // sandbox id → open promotion PR URL (merge_commit promotion)
  recount(db);
  return db;
}

// ------------------------------------------------------------------ helpers

function stepsSummary(task) {
  const list = task.stepsList;
  const current = list.find((s) => s.status === 'in_progress');
  return { done: list.filter((s) => s.status === 'done').length, total: list.length, current: current ? current.title : null };
}

function summary(task) {
  return {
    id: task.id, sandbox_id: task.sandbox_id, number: task.number, title: task.title, harness: task.harness,
    status: task.status, activity: task.status === 'running' || task.status === 'waiting_capacity' ? task.activity : null,
    branch: task.branch, origin: task.origin, created_by: task.created_by, created_at: task.created_at, updated_at: task.updated_at,
    steps: stepsSummary(task), unread_messages: task.unread_messages, archived: task.archived, account_label: task.account_label,
  };
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
    if (profile.require_checks !== false && checks !== 'success') blocked.push({ action: 'merge', reason: `checks are ${checks}; the profile requires green checks` });
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

function recount(db) {
  for (const sandbox of db.sandboxes) {
    const tasks = db.tasks.filter((t) => t.sandbox_id === sandbox.id && !t.archived);
    sandbox.counts = {
      running: tasks.filter((t) => t.status === 'running').length,
      needs_attention: tasks.filter((t) => (t.status === 'running' && t.activity === 'needs_attention') || t.status === 'waiting_capacity').length,
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
route('POST', '/api/device/approve', ({ body }) => {
  if (!/^[A-Z0-9]{4}-?[A-Z0-9]{4}$/.test(body.user_code || '')) fail(404, 'not_found', 'not found');
  return json(204);
});

route('GET', '/api/sandboxes', ({ db }) => db.sandboxes);
route('GET', '/api/github/repositories', ({ db }) => {
  if (!db.me.user.github_linked) fail(403, 'forbidden', 'a GitHub-linked account is required');
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
route('POST', `/api/sandboxes/${ID}/onboarding/skip`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'admin');
  if (!['pending', 'preparing'].includes(sandbox.onboarding)) fail(409, 'conflict', 'nothing awaits preparation');
  sandbox.onboarding = 'skipped';
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

route('GET', `/api/sandboxes/${ID}/tasks`, ({ db, p, query }) => {
  sandboxOf(db, p[0]);
  const archived = query.get('archived') === 'true';
  return db.tasks.filter((t) => t.sandbox_id === p[0] && t.archived === archived).map(summary);
});
route('POST', `/api/sandboxes/${ID}/tasks`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  if (sandbox.onboarding === 'pending' || sandbox.onboarding === 'preparing') fail(409, 'conflict', 'The project is being prepared; new tasks start after the preparation task lands (or an admin skips it).');
  if (!body.prompt || !body.prompt.trim()) fail(422, 'invalid', 'The prompt is empty.');
  if (body.harness && body.harness !== 'claude') fail(422, 'invalid', 'harness must be claude (Yulius runs Claude Code only)');
  const number = Math.max(0, ...db.tasks.filter((t) => t.sandbox_id === sandbox.id).map((t) => t.number)) + 1;
  const title = body.title || body.prompt.trim().split('\n')[0].slice(0, 60);
  const wf = db.workflows.find((w) => w.id === body.workflow_id);
  const id = `tsk_${sandbox.id.slice(3)}_${number}`;
  const task = {
    id, sandbox_id: sandbox.id, number, title, harness: 'claude', status: 'queued', activity: null,
    branch: `feat/${title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 40) || `task-${number}`}`,
    origin: 'ui', created_by: { login: db.me.user.login }, created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    unread_messages: 0, archived: false, account_label: null,
    stepsList: wf ? wf.steps.map((title2, i) => ({ id: `${id}-s${i + 1}`, ordinal: i + 1, title: title2, status: 'pending', note: '', updated_by: 'workflow', updated_at: new Date().toISOString() })) : [],
    model: body.model || 'claude-opus-4', effort: body.effort || 'default', prompt: body.prompt,
    delivery: null,
  };
  db.tasks.push(task);
  recount(db);
  later(() => touch(db, task, { status: 'starting' }), 700);
  later(() => touch(db, task, { status: 'running', activity: 'working', account_label: 'Claude #1' }), 1800);
  return json(201, summary(task));
});
route('GET', `/api/tasks/${ID}`, ({ db, p }) => detail(db, taskOf(db, p[0])));
route('POST', `/api/tasks/${ID}/stop`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
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
route('POST', `/api/tasks/${ID}/archive`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (LIVE.has(task.status)) fail(409, 'conflict', 'Stop the task before archiving it.');
  touch(db, task, { archived: true });
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
route('POST', `/api/tasks/${ID}/messages`, ({ db, p, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (!body.body || !body.body.trim()) fail(422, 'invalid', 'Empty message.');
  db.nextId += 1;
  const stored = { id: `msg_${db.nextId}`, task: task.id, from: { task_id: null, login: db.me.user.login, title: db.me.user.display_name || db.me.user.login }, to_task_id: task.id, kind: 'info', subject: '', body: body.body, created_at: new Date().toISOString(), read: true };
  db.messages.push(stored);
  const { task: _t, ...message } = stored;
  later(() => {
    db.nextId += 1;
    const reply = { id: `msg_${db.nextId}`, task: task.id, from: { task_id: task.id, login: null, title: `#${task.number} ${task.title} (${task.harness})` }, to_task_id: null, kind: 'info', subject: 'Re: your message', body: 'Noted — I will take this into account at my next turn.', created_at: new Date().toISOString(), read: false };
    db.messages.push(reply);
    const { task: _r, ...out } = reply;
    emit(db, { type: 'message', task_id: task.id, message: out });
  }, 2500);
  return message;
});
route('GET', '/api/archived-tasks', ({ db }) => db.tasks.filter((t) => t.archived && db.sandboxes.some((s) => s.id === t.sandbox_id)).map(summary));

// ---- Git and delivery

route('GET', '/api/delivery/presets', () => [clone(PRESETS.standard), clone(PRESETS.product), clone(PRESETS.studio)]);
route('GET', `/api/sandboxes/${ID}/git`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  const tasks = db.tasks.filter((t) => t.sandbox_id === sandbox.id && !t.archived && t.branch);
  const pull = (t) => (t.delivery && t.delivery.pr ? { ...t.delivery.pr, checks: t.delivery.pr.state === 'open' ? t.delivery.pr.checks : 'none' } : null);
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
    notes: sandbox.id === 'sb_three' ? ['GitHub: pull requests could not be read (rate limited); lists may be incomplete.'] : [],
  };
});
route('GET', `/api/tasks/${ID}/delivery`, ({ db, p }) => delivery(db, taskOf(db, p[0])));
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
    notes.push(`Promoted ${sandbox.profile.integration_branch} → ${sandbox.profile.production_branch} automatically (auto_promote).`);
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

// ---- Skills, workflows, automations, MCP

route('GET', `/api/sandboxes/${ID}/skills`, ({ db, p }) => { sandboxOf(db, p[0]); return db.skills; });
route('GET', `/api/sandboxes/${ID}/skills/${ID}`, ({ db, p }) => {
  const skill = db.skills.find((s) => s.name === p[1]);
  if (!skill) fail(404, 'not_found', 'not found');
  return { name: skill.name, content: `---\nname: ${skill.name}\ndescription: ${skill.description}\n---\n\n# ${skill.name}\n\n1. Read AGENTS.md and the task spec.\n2. Make the smallest complete change.\n3. Verify with tests and report evidence.\n` };
});
route('PUT', `/api/sandboxes/${ID}/skills/${ID}`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  const skill = db.skills.find((s) => s.name === p[1]);
  if (!skill) fail(404, 'not_found', 'not found');
  if (skill.source === 'repository') fail(422, 'invalid', 'repository skills are always available inside the repository');
  if (skill.source === 'instance' && !db.me.user.is_instance_admin) fail(403, 'forbidden', 'instance skills are managed by instance administrators');
  skill.enabled = Boolean(body.enabled);
  return json(204);
});

route('GET', `/api/sandboxes/${ID}/workflows`, ({ db, p }) => db.workflows.filter((w) => w.scope === 'instance' || w.sandbox === p[0]).map(({ sandbox: _s, ...w }) => w));
route('POST', `/api/sandboxes/${ID}/workflows`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  if (body.scope === 'instance' && !db.me.user.is_instance_admin) fail(403, 'forbidden', 'instance workflows are managed by instance administrators');
  db.nextId += 1;
  const wf = { id: `wf_${db.nextId}`, sandbox: body.scope === 'instance' ? null : p[0], name: body.name, description: body.description || '', steps: body.steps || [], instructions: body.instructions || '', gates: { require_spec: Boolean(body.gates && body.gates.require_spec), require_tests: Boolean(body.gates && body.gates.require_tests) }, scope: body.scope === 'instance' ? 'instance' : 'sandbox' };
  db.workflows.push(wf);
  const { sandbox: _s, ...out } = wf;
  return json(201, out);
});
route('PUT', `/api/workflows/${ID}`, ({ db, p, body }) => {
  const wf = db.workflows.find((w) => w.id === p[0]);
  if (!wf) fail(404, 'not_found', 'not found');
  Object.assign(wf, { name: body.name, description: body.description, steps: body.steps, instructions: body.instructions, gates: body.gates });
  const { sandbox: _s, ...out } = wf;
  return out;
});
route('DELETE', `/api/workflows/${ID}`, ({ db, p }) => { db.workflows = db.workflows.filter((w) => w.id !== p[0]); return json(204); });

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

// ---- Shells: 201 {"id"}; an empty body or {"task_id"}; at most 4 live shells per sandbox.
route('POST', `/api/sandboxes/${ID}/shells`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  if (body.task_id) {
    const task = db.tasks.find((t) => t.id === body.task_id && t.sandbox_id === p[0]);
    if (!task) fail(404, 'not_found', 'not found');
  }
  const prefix = `sh_${p[0]}_`;
  if ([...db.liveShells].filter((id) => id.startsWith(prefix)).length >= 4) fail(429, 'capacity', 'at most 4 live shells per sandbox');
  db.nextId += 1;
  const id = `${prefix}${db.nextId}`;
  db.liveShells.add(id);
  return json(201, { id });
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
  const account = { id: `acc_${db.nextId}`, provider: body.provider, label: body.label, enabled: true, priority: 100, status: 'pending_login', usage: { primary: null, secondary: null, observed_at: null }, last_error: null, in_use_by: 0 };
  db.accounts.push(account);
  return json(201, { account, login_id: `login_${account.id}` });
});
route('PATCH', `/api/admin/accounts/${ID}`, ({ db, p, body }) => {
  adminOnly(db);
  const account = db.accounts.find((a) => a.id === p[0]);
  if (!account) fail(404, 'not_found', 'not found');
  if (body.priority !== undefined && !(Number.isInteger(body.priority) && body.priority >= 0 && body.priority <= 10000)) fail(422, 'invalid', 'priority must be 0–10000');
  if (body.label !== undefined && !(body.label.length >= 1 && body.label.length <= 80)) fail(422, 'invalid', 'label must be 1–80 characters');
  for (const key of ['enabled', 'priority', 'label']) if (body[key] !== undefined) account[key] = body[key];
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
      ...db.accounts.filter((a) => a.enabled).map((a, i) => ({ account_id: a.id, account_label: a.label, ...tokenTotals(Math.round(runs / (i + 2)), Math.round(total / (i + 2)), n(14e6 / (i + 1)), n(1.8e6 / (i + 1)), null, null, 0) })),
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
      ? { label: a.label, provider: a.provider, plan: a.plan, status: a.status, enabled: a.enabled, usage: a.usage, id: a.id, priority: a.priority, last_error: a.last_error, in_use_by: a.in_use_by, email: a.email }
      : { label: a.label, provider: a.provider, plan: a.plan, status: a.status, enabled: a.enabled, usage: a.usage })),
    contained: db.me.instance.contained,
  };
});

// ------------------------------------------------------------------ fetch

const PUBLIC = ['/api/setup/status', '/api/setup', '/api/auth/password', '/api/auth/mock-github'];

async function handleFetch(db, url, init = {}) {
  await delay(50 + Math.random() * 150);
  const u = new URL(url, ORIGIN);
  const method = (init.method || 'GET').toUpperCase();
  let body = {};
  if (init.body) {
    try { body = JSON.parse(init.body); } catch { return json(422, { error: { code: 'invalid', message: 'Malformed JSON' } }); }
  }
  if (!PUBLIC.includes(u.pathname) && !db.signedIn) return json(401, { error: { code: 'unauthenticated', message: 'Sign in first.' } });
  const headers = init.headers || {};
  if (method !== 'GET' && !PUBLIC.includes(u.pathname) && headers['X-Yulius-CSRF'] !== db.me.csrf_token) {
    return json(403, { error: { code: 'forbidden', message: 'Missing or wrong CSRF token.' } });
  }
  for (const [m, re, handler] of R) {
    if (m !== method) continue;
    const match = u.pathname.match(re);
    if (!match) continue;
    try {
      const result = handler({ db, p: match.slice(1).map(decodeURIComponent), body, query: u.searchParams });
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
  `${c(255, '⏺')} ${bold('yulius')} - message_send (MCP)(to: "#11", kind: "scope", subject: "I own src/api/**")`,
  `  ${dim('⎿  Delivered to #11 3D model viewer')}`,
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
    this.input = '';
    this.viewers = new Set();
    this.cols = 120;
    this.rows = 36;
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
    this.lines.push(`${c(245, '>')} ${this.task.prompt || this.task.title}`, '');
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
    }
    if (this.exited) this.lines.push('', dim(`Session ended (${this.task.status}).`));
  }

  footer() {
    const sep = dim('─'.repeat(Math.max(10, this.cols - 1)));
    const prompt = `${c(245, '>')} ${this.input}`;
    const hint = `${dim(' ⏵⏵ accept edits on (shift+tab to cycle)')}${' '.repeat(6)}${c(173, `${this.task.model} · effort ${this.task.effort}`)}`;
    const visible = hint.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, '').length;
    const short = c(173, ` ${this.task.model}`);
    return { sep, prompt, hint: visible < this.cols ? hint : short };
  }

  regionBottom() {
    return Math.max(3, this.rows - 3);
  }

  snapshot() {
    const bottom = this.regionBottom();
    const visible = this.lines.slice(-bottom);
    while (visible.length < bottom) visible.unshift(''); // bottom-align like a scrolled TUI
    const f = this.footer();
    let out = `${E}?25l${E}2J${E}r${E}H`;
    out += visible.join('\r\n');
    out += `${E}1;${bottom}r`;
    out += `${E}${bottom + 1};1H${f.sep}${E}${bottom + 2};1H${E}2K${f.prompt}${E}${bottom + 3};1H${E}2K${f.hint}`;
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

  // Like the server: every viewer with write access controls the size (controller = can_write).
  state(_ws) {
    const write = this.canWrite();
    return JSON.stringify({ type: 'state', attempt_seq: 1, harness: this.task.harness, cols: this.cols, rows: this.rows, can_write: write, controller: write, status: this.task.status });
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
      if (this.canWrite() && msg.cols && msg.rows) { this.cols = Math.max(20, Math.min(400, msg.cols)); this.rows = Math.max(8, Math.min(200, msg.rows)); }
      ws.push(this.state(ws));
      this.sendSnapshot(ws);
      this.ensureTicking();
    } else if (msg.type === 'resize') {
      if (!this.canWrite()) return;
      const cols = Math.max(20, Math.min(400, msg.cols));
      const rows = Math.max(8, Math.min(200, msg.rows));
      if (cols === this.cols && rows === this.rows) return;
      this.cols = cols;
      this.rows = rows;
      this.seq += 1;
      for (const viewer of this.viewers) { viewer.push(this.state(viewer)); viewer.push(terminalFrame(2, this.seq, this.snapshot())); }
    } else if (msg.type === 'take_control') {
      ws.push(this.state(ws));
    } else if (msg.type === 'input') {
      if (!this.canWrite()) { ws.push(JSON.stringify({ type: 'error', code: 'forbidden', message: 'Input requires write access to the repository.' })); return; }
      this.onInput(String(msg.data || ''));
    }
  }

  closed(ws) {
    this.viewers.delete(ws);
    if (this.viewers.size === 0) this.stopTicking();
  }

  onInput(data) {
    for (const ch of data.replace(/\x1b\[[AB]/g, '')) {
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

/** Fake bash in the sandbox shell panel (`state` has attempt_seq and harness null). */
class ShellSession {
  constructor(db, id, slug) {
    this.db = db;
    this.id = id;
    this.slug = slug;
    this.seq = 0;
    this.input = '';
    this.ws = null;
    this.prompt = `${c(114, `yulius@${slug}`)}:${c(39, '~/sandbox')}$ `;
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
      ws.push(JSON.stringify({ type: 'state', attempt_seq: null, harness: null, cols: msg.cols, rows: msg.rows, can_write: true, controller: true, status: 'running' }));
      this.seq += 1;
      ws.push(terminalFrame(2, this.seq, `${dim('Contained sandbox shell (mock). Try: ls, pwd, git status, exit')}\r\n${this.prompt}`));
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
      pwd: `/srv/yulius/sandboxes/${this.slug}/main`,
      'git status': 'On branch main\r\nYour branch is up to date with \'origin/main\'.\r\n\r\nnothing to commit, working tree clean',
      whoami: 'yulius-sandbox',
    };
    if (cmd === 'exit') {
      this.out('\r\nlogout\r\n');
      // The server sends `exit` and closes the socket when the shell process ends.
      if (this.ws) { this.ws.push(JSON.stringify({ type: 'exit', status: 'closed', code: 0 })); const ws = this.ws; setTimeout(() => ws.serverClose(1000, 'shell ended'), 50); }
      this.db.liveShells.delete(this.id);
      return;
    }
    const result = cmd ? outputs[cmd] ?? `bash: ${cmd.split(' ')[0]}: command not found (mock shell)` : '';
    this.out(`\r\n${result ? `${result}\r\n` : ''}${this.prompt}`);
  }

  closed() { this.ws = null; }
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

/** Fake shared browser for one sandbox: tabs and rendered JPEG frames. */
class BrowserSession {
  constructor(db, sandbox) {
    this.db = db;
    this.sandbox = sandbox;
    this.viewers = new Set();
    this.size = { width: 1280, height: 800 };
    this.cursor = { x: 200, y: 200 };
    this.typed = new Map();
    this.next = 3;
    this.renderPending = false;
    const port = sandbox.port_base || 20000;
    this.tabs = [
      { id: 'tab-app', title: `Checkout — localhost:${port + 3}`, url: `http://localhost:${port + 3}/`, active: true, action_requested: null },
      { id: 'tab-aws', title: 'Amazon Web Services Sign-In', url: 'https://signin.aws.amazon.com/', active: false, action_requested: sandbox.id === 'sb_studio' ? { reason: 'Sign in to AWS so task #10 can deploy', task_id: 'tsk_studio_10' } : null },
    ];
  }

  open(ws) {
    this.viewers.add(ws);
    this.sendTabs();
    this.domUnavailable(ws);
    this.render();
  }

  /** The mock renders pixels only: the panel shows its pixel view at once. */
  domUnavailable(ws) {
    const active = this.tabs.find((t) => t.active);
    if (active) ws.push(JSON.stringify({ type: 'dom_unavailable', tab_id: active.id, reason: 'the mock browser renders pixels only' }));
  }

  receive(ws, data) {
    let msg;
    try { msg = JSON.parse(data); } catch { return; }
    const tab = this.tabs.find((t) => t.id === msg.tab_id);
    switch (msg.type) {
      case 'viewport':
        this.viewers.add(ws);
        this.size = { width: Math.max(320, Math.min(2560, msg.width | 0)), height: Math.max(240, Math.min(1600, msg.height | 0)) };
        this.sendTabs();
        this.render();
        break;
      case 'view':
        if (msg.mode === 'dom') this.domUnavailable(ws);
        break;
      case 'open': {
        this.next += 1;
        for (const t of this.tabs) t.active = false;
        const url = msg.url || 'about:blank';
        let host = url;
        try { host = new URL(url).host || 'New tab'; } catch { /* keep raw */ }
        this.tabs.push({ id: `tab-${this.next}`, title: host, url, active: true, action_requested: null });
        this.sendTabs();
        this.render();
        break;
      }
      case 'navigate':
        if (tab) { tab.url = msg.url; try { tab.title = new URL(msg.url).host || msg.url; } catch { tab.title = msg.url; } this.sendTabs(); this.render(); }
        break;
      case 'activate':
        if (tab) { for (const t of this.tabs) t.active = t === tab; this.sendTabs(); this.render(); }
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
      case 'mouse':
        if (!tab) break;
        this.cursor = { x: msg.x, y: msg.y };
        if (msg.event === 'down' && tab.id === 'tab-aws' && tab.action_requested && msg.x > 60 && msg.x < 360 && msg.y > 330 && msg.y < 380) {
          tab.action_requested = null;
          tab.title = 'AWS Management Console';
          tab.url = 'https://console.aws.amazon.com/';
          this.sendTabs();
        }
        this.render();
        break;
      case 'key':
        if (!tab) break;
        if (msg.event === 'down' && msg.key === 'Backspace') this.typed.set(tab.id, (this.typed.get(tab.id) || '').slice(0, -1));
        else if ((msg.event === 'down' || msg.event === 'char') && msg.text && msg.text !== '\r') this.typed.set(tab.id, ((this.typed.get(tab.id) || '') + msg.text).slice(-80));
        this.render();
        break;
      default:
        break;
    }
  }

  closed(ws) {
    this.viewers.delete(ws);
  }

  sendTabs() {
    const data = JSON.stringify({ type: 'tabs', tabs: this.tabs });
    for (const ws of this.viewers) ws.push(data);
  }

  render() {
    if (this.renderPending || this.viewers.size === 0) return;
    this.renderPending = true;
    setTimeout(() => { this.renderPending = false; this.drawFrame(); }, 60);
  }

  async drawFrame() {
    const tab = this.tabs.find((t) => t.active);
    if (!tab || this.viewers.size === 0) return;
    const { width, height } = this.size;
    const canvas = typeof OffscreenCanvas !== 'undefined' ? new OffscreenCanvas(width, height) : Object.assign(document.createElement('canvas'), { width, height });
    const g = canvas.getContext('2d');
    const aws = tab.id === 'tab-aws';
    g.fillStyle = '#ffffff';
    g.fillRect(0, 0, width, height);
    g.fillStyle = aws ? '#232f3e' : '#0f172a';
    g.fillRect(0, 0, width, 56);
    g.fillStyle = '#ffffff';
    g.font = 'bold 20px system-ui, sans-serif';
    g.fillText(aws ? 'aws' : 'Checkout (dev)', 24, 36);
    g.fillStyle = '#111827';
    g.font = 'bold 28px system-ui, sans-serif';
    g.fillText(tab.title, 60, 120);
    g.font = '15px ui-monospace, monospace';
    g.fillStyle = '#6b7280';
    g.fillText(tab.url, 60, 150);
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
    } else {
      for (let i = 0; i < 3; i += 1) {
        g.fillStyle = '#f3f4f6';
        g.fillRect(60 + i * 240, 330, 220, 160);
        g.fillStyle = '#374151';
        g.font = '15px system-ui, sans-serif';
        g.fillText(['Espresso cup', 'Pour-over kit', 'Grinder'][i], 76 + i * 240, 520);
      }
    }
    g.fillStyle = 'rgba(17, 24, 39, 0.85)';
    g.beginPath();
    g.moveTo(this.cursor.x, this.cursor.y);
    g.lineTo(this.cursor.x + 12, this.cursor.y + 30);
    g.lineTo(this.cursor.x + 18, this.cursor.y + 18);
    g.lineTo(this.cursor.x + 30, this.cursor.y + 12);
    g.closePath();
    g.fill();
    g.fillStyle = '#9ca3af';
    g.font = '12px system-ui, sans-serif';
    g.fillText(`Mock sandbox browser · ${this.sandbox.name} · ${width}×${height}`, 60, height - 20);
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
    for (const ws of this.viewers) ws.push(frame.buffer);
  }
}

function openSocket(db, path) {
  const ws = new MockSocket(path);
  const fatal = (code, message) => {
    ws.server = { open: () => { ws.push(JSON.stringify({ type: 'error', code, message })); ws.serverClose(1008, code); } };
  };
  if (!db.signedIn) { fatal('unauthenticated', 'Sign in first.'); return ws; }
  let m;
  if (path === '/api/events') {
    ws.server = {
      open: () => db.eventSockets.add(ws),
      closed: () => db.eventSockets.delete(ws),
    };
    startAmbient(db);
  } else if ((m = path.match(/^\/api\/tasks\/([^/]+)\/terminal$/))) {
    const task = db.tasks.find((t) => t.id === decodeURIComponent(m[1]));
    if (!task) { fatal('not_found', 'No such task.'); return ws; }
    let session = db.sessions.get(task.id);
    if (!session) { session = new TermSession(db, task); db.sessions.set(task.id, session); }
    ws.server = session;
  } else if ((m = path.match(/^\/api\/shells\/([^/]+)\/terminal$/))) {
    const id = decodeURIComponent(m[1]);
    if (!db.liveShells.has(id)) { fatal('not_found', 'not found'); return ws; }
    const slug = (db.sandboxes.find((s) => id.includes(s.id)) || { slug: 'sandbox' }).slug;
    ws.server = new ShellSession(db, id, slug);
  } else if ((m = path.match(/^\/api\/admin\/accounts\/login\/([^/]+)\/terminal$/))) {
    ws.server = new LoginSession(db, decodeURIComponent(m[1]));
  } else if ((m = path.match(/^\/api\/sandboxes\/([^/]+)\/browser$/))) {
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
  return {
    kind: 'mock',
    fetch: (url, init) => handleFetch(db, url, init),
    openSocket: (path) => openSocket(db, path),
  };
}
