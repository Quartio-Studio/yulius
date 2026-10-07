// Mock backend for ?mock=1: an in-memory implementation of docs/api.md with fake
// WebSockets (events, task/shell/login terminals, sandbox browser). It lets the UI be
// exercised without `yulius serve`. Everything here is synthetic data.
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
    runs: new Map(),
    nextId: 100,
  };
  const profile = (preset) => ({
    standard: { preset: 'standard', integration_branch: 'dev', production_branch: 'main', merge: 'squash', promote: 'merge_commit', auto_promote: false, environments: [{ name: 'dev', after: 'integration', command: 'scripts/deploy dev', verify: 'manual' }] },
    product: { preset: 'product', integration_branch: 'quartio', production_branch: 'main', merge: 'squash', promote: 'fast_forward', auto_promote: false, environments: [{ name: 'dev-vm', after: 'integration', command: 'node scripts/deployer.mjs --target dev-vm', verify: 'manual' }, { name: 'production', after: 'production', command: 'node scripts/deployer.mjs --target fleet', verify: 'manual' }] },
    studio: { preset: 'studio', integration_branch: 'quartio', production_branch: 'main', merge: 'squash', promote: 'fast_forward', auto_promote: true, environments: [] },
  }[preset]);
  db.sandboxes = [
    { id: 'sb_studio', slug: 'studio', name: 'Quartio Studio', repo: { owner: 'Quartio-Studio', name: 'studio', url: 'https://github.com/Quartio-Studio/studio' }, access: role === 'member' ? 'write' : 'admin', default_branch: 'main', profile: profile('studio'), onboarding: 'ready', port_base: 20000, port_count: 20, counts: { running: 0, needs_attention: 0, queued: 0 } },
    { id: 'sb_product', slug: 'product', name: 'Product', repo: { owner: 'Quartio-Studio', name: 'product', url: 'https://github.com/Quartio-Studio/product' }, access: role === 'member' ? 'write' : 'admin', default_branch: 'main', profile: profile('product'), onboarding: 'preparing', port_base: 20020, port_count: 20, counts: { running: 0, needs_attention: 0, queued: 0 } },
    { id: 'sb_three', slug: 'sandbox-3', name: 'Sandbox 3', repo: { owner: 'Quartio-Studio', name: 'sandbox-3', url: 'https://github.com/Quartio-Studio/sandbox-3' }, access: 'read', default_branch: 'main', profile: profile('standard'), onboarding: 'not_needed', port_base: 20040, port_count: 20, counts: { running: 0, needs_attention: 0, queued: 0 } },
  ];
  const t = (sandbox, number, title, harness, status, activity, extra = {}) => {
    const id = `tsk_${sandbox.slice(3)}_${number}`;
    const done = extra.done ?? Math.min(PLAN.length, number % 5);
    const current = extra.current ?? (LIVE.has(status) && status !== 'queued');
    return {
      id, sandbox_id: sandbox, number, title, harness, status, activity,
      branch: `feat/${title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')}`,
      origin: extra.origin || 'ui', created_by: { login: extra.author || 'mkarp' },
      created_at: iso(extra.age || number * 3 * HOUR), updated_at: iso(extra.updated ?? number * 7 * MIN),
      unread_messages: extra.unread || 0, archived: Boolean(extra.archived),
      account_label: LIVE.has(status) ? (harness === 'claude' ? 'Claude #2' : 'Codex #1') : null,
      stepsList: extra.noSteps ? [] : makeSteps(id, done, current),
      model: harness === 'claude' ? 'claude-opus-4' : 'gpt-5-codex',
      effort: 'high',
    };
  };
  db.tasks = [
    t('sb_studio', 12, 'Checkout API', 'claude', 'running', 'working', { done: 2, unread: 2, updated: 20e3 }),
    t('sb_studio', 11, '3D model viewer', 'codex', 'running', 'waiting', { done: 2, author: 'anna-q' }),
    t('sb_studio', 10, 'AWS login for deploy', 'claude', 'running', 'needs_attention', { done: 4 }),
    t('sb_studio', 9, 'Translate session to Codex', 'codex', 'switching', null, { done: 3 }),
    t('sb_studio', 8, 'Nightly feedback digest', 'claude', 'queued', null, { origin: 'automation', noSteps: true }),
    t('sb_studio', 7, 'Search indexing', 'claude', 'waiting_capacity', 'needs_attention', { done: 1 }),
    t('sb_studio', 6, 'Fix flaky checkout test', 'codex', 'starting', null, { noSteps: true }),
    t('sb_studio', 5, 'Docs restructure', 'claude', 'closed', null, { done: 7, current: false, updated: 5 * HOUR }),
    t('sb_studio', 4, 'Payment webhook', 'claude', 'stopped_cancelled', null, { done: 3, current: false, updated: 9 * HOUR }),
    t('sb_studio', 3, 'Image pipeline', 'codex', 'stopped_crashed', null, { done: 2, current: false, updated: DAY }),
    t('sb_studio', 2, 'Legacy import', 'claude', 'failed_spawn_error', null, { noSteps: true, updated: 2 * DAY, origin: 'import' }),
    t('sb_studio', 1, 'Old landing page', 'claude', 'closed', null, { done: 7, current: false, archived: true, updated: 6 * DAY }),
    t('sb_product', 1, 'Prepare repository for Yulius', 'claude', 'running', 'working', { origin: 'onboarding', done: 1 }),
    t('sb_three', 2, 'Write README', 'claude', 'closed', null, { done: 7, current: false }),
    t('sb_three', 1, 'Explore data model', 'codex', 'running', 'working', { done: 1, author: 'anna-q' }),
  ];
  db.messages = [
    { id: 'msg_1', task: 'tsk_studio_12', from: { task_id: 'tsk_studio_11', login: null, title: '#11 3D model viewer (codex)' }, to_task_id: 'tsk_studio_12', kind: 'scope', subject: 'Scope: src/viewer/**', body: 'I am working in src/viewer/** and public/models/**. Please avoid those paths; I will stay out of src/api/**.', created_at: iso(42 * MIN), read: true },
    { id: 'msg_2', task: 'tsk_studio_12', from: { task_id: 'tsk_studio_10', login: null, title: '#10 AWS login for deploy (claude)' }, to_task_id: 'tsk_studio_12', kind: 'question', subject: 'Which port does the API use?', body: 'Is the checkout API served on http://localhost:20003 in this sandbox? I need it for the deploy smoke test.', created_at: iso(8 * MIN), read: false },
    { id: 'msg_3', task: 'tsk_studio_12', from: { task_id: null, login: 'mkarp', title: 'mkarp' }, to_task_id: 'tsk_studio_12', kind: 'info', subject: '', body: 'Use the Stripe test keys only. The AWS sign-in tab is open: ' + `${ORIGIN}/s/studio/browser?tab=tab-aws`, created_at: iso(3 * MIN), read: false },
  ];
  db.accounts = [
    { id: 'acc_c1', provider: 'anthropic', label: 'Claude #1', enabled: true, priority: 1, status: 'ok', usage: { primary: { used_percent: 42.0, resets_at: isoIn(2.2 * HOUR), window_minutes: 300 }, secondary: { used_percent: 18.5, resets_at: isoIn(4 * DAY), window_minutes: 10080 }, observed_at: iso(2 * MIN) }, last_error: null, in_use_by: 1 },
    { id: 'acc_c2', provider: 'anthropic', label: 'Claude #2', enabled: true, priority: 2, status: 'ok', usage: { primary: { used_percent: 87.3, resets_at: isoIn(1.1 * HOUR), window_minutes: 300 }, secondary: { used_percent: 61.0, resets_at: isoIn(2 * DAY), window_minutes: 10080 }, observed_at: iso(1 * MIN) }, last_error: null, in_use_by: 3 },
    { id: 'acc_c3', provider: 'anthropic', label: 'Claude #3', enabled: true, priority: 3, status: 'relogin', usage: { primary: null, secondary: null, observed_at: null }, last_error: 'OAuth refresh token expired; sign in again.', in_use_by: 0 },
    { id: 'acc_x1', provider: 'openai', label: 'Codex #1', enabled: true, priority: 1, status: 'ok', usage: { primary: { used_percent: 12.0, resets_at: isoIn(3.5 * HOUR), window_minutes: 300 }, secondary: { used_percent: 33.0, resets_at: isoIn(5 * DAY), window_minutes: 10080 }, observed_at: iso(4 * MIN) }, last_error: null, in_use_by: 2 },
    { id: 'acc_x2', provider: 'openai', label: 'Codex #2', enabled: true, priority: 2, status: 'limited', usage: { primary: { used_percent: 99.6, resets_at: isoIn(1.9 * HOUR), window_minutes: 300 }, secondary: { used_percent: 71.0, resets_at: isoIn(3 * DAY), window_minutes: 10080 }, observed_at: iso(30e3) }, last_error: null, in_use_by: 0 },
    { id: 'acc_x3', provider: 'openai', label: 'Codex spare', enabled: false, priority: 9, status: 'ok', usage: { primary: { used_percent: 0.5, resets_at: null, window_minutes: 300 }, secondary: null, observed_at: iso(3 * DAY) }, last_error: null, in_use_by: 0 },
  ];
  db.workflows = [
    { id: 'wf_feature', sandbox: null, name: 'Feature (spec → PR)', description: 'Default discipline: spec with plan, implementation, tests, PR.', steps: ['Write task spec', 'Implement', 'Test', 'Open PR'], instructions: 'Follow AGENTS.md. Keep the spec Plan checklist in sync.', gates: { require_spec: true, require_tests: true }, scope: 'instance' },
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
    { id: 'aut_1', sandbox: 'sb_studio', name: 'Daily customer feedback', cron: '0 9 * * *', utc_offset_minutes: 120, prompt: 'Create a task implementing yesterday’s customer feedback from the Quartio MCP.', workflow_id: 'wf_feature', harness: 'claude', enabled: true, created_via: 'agent', next_run_at: isoIn(14 * HOUR), last_run: { scheduled_for: iso(10 * HOUR), state: 'task_created', task_id: 'tsk_studio_8' } },
    { id: 'aut_2', sandbox: 'sb_studio', name: 'Weekly dependency audit', cron: '0 7 * * 1', utc_offset_minutes: 0, prompt: 'Audit dependencies for advisories and open a PR with safe upgrades.', workflow_id: null, harness: 'codex', enabled: false, created_via: 'ui', next_run_at: null, last_run: null },
  ];
  db.mcp = [
    { id: 'mcp_quartio', sandbox: null, scope: 'instance', name: 'quartio', transport: 'http', command: null, args: [], url: 'https://mcp.quartio.ai/mcp', env_keys: [], header_keys: ['Authorization'], enabled: true },
    { id: 'mcp_apify', sandbox: 'sb_studio', scope: 'sandbox', name: 'apify', transport: 'stdio', command: 'npx', args: ['-y', '@apify/actors-mcp-server'], url: null, env_keys: ['APIFY_TOKEN'], header_keys: [], enabled: true },
  ];
  db.users = [
    { id: 'usr_1', github_login: 'mkarp', display_name: 'Michal Karp', is_instance_admin: true, disabled: false, is_superadmin: false, last_seen_at: iso(MIN) },
    { id: 'usr_2', github_login: 'anna-q', display_name: 'Anna', is_instance_admin: false, disabled: false, is_superadmin: false, last_seen_at: iso(3 * HOUR) },
    { id: 'usr_3', github_login: 'old-contractor', display_name: '', is_instance_admin: false, disabled: true, is_superadmin: false, last_seen_at: iso(40 * DAY) },
  ];
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

function prFor(task) {
  if (task.number === 5) return { number: 214, url: 'https://github.com/Quartio-Studio/studio/pull/214', title: 'Docs restructure', state: 'open', base: 'quartio', head: task.branch, checks: 'success' };
  if (task.number === 4) return { number: 209, url: 'https://github.com/Quartio-Studio/studio/pull/209', title: 'Payment webhook', state: 'open', base: 'quartio', head: task.branch, checks: 'failure' };
  if (task.number === 1) return { number: 180, url: 'https://github.com/Quartio-Studio/studio/pull/180', title: 'Old landing page', state: 'merged', base: 'quartio', head: task.branch, checks: 'success' };
  return null;
}

function delivery(task) {
  const pr = prFor(task);
  const specValid = task.number !== 12;
  const stepsOpen = task.stepsList.filter((s) => s.status !== 'done' && s.status !== 'skipped').length;
  let stage = 'none';
  let next = specValid ? ['open_pr'] : ['open_pr'];
  if (pr && pr.state === 'open') { stage = 'pr_open'; next = ['merge']; }
  if (pr && pr.state === 'merged') { stage = 'promoted'; next = []; }
  if (task.deliveryStage) { stage = task.deliveryStage; next = task.deliveryNext; }
  return {
    stage, pr: task.pr || pr, spec_valid: specValid,
    spec_problems: specValid ? [] : ['Plan: 3 unchecked items without notes', 'Missing section: Verification', 'Decision log does not reference the ADR for the payment provider'],
    steps_open: stepsOpen, next_actions: next,
  };
}

function detail(db, task) {
  return {
    ...summary(task),
    spec_path: task.stepsList.length ? `docs/specs/2026-10-07-${task.branch.replace('feat/', '')}.md` : null,
    worktree: `/srv/yulius/sandboxes/${task.sandbox_id.slice(3)}/worktrees/${task.number}`,
    attempts: [{ seq: 1, harness: task.harness, account_label: task.account_label || 'Claude #1', launch_kind: 'new', state: LIVE.has(task.status) ? 'live' : 'exited', started_at: task.created_at, ended_at: LIVE.has(task.status) ? null : task.updated_at, exit_code: LIVE.has(task.status) ? null : 0, exit_reason: LIVE.has(task.status) ? null : 'normal' }],
    steps_list: task.stepsList,
    metrics: { wall_ms: Date.now() - Date.parse(task.created_at), active_ms: 41 * MIN, input_tokens: 182340, output_tokens: 23410, cache_read_tokens: 1204000, cache_write_tokens: 90210, models: [task.model], switches: task.number === 9 ? 1 : 0, turns: 17 },
    feedback: [],
    delivery: delivery(task),
    switches: task.number === 9 ? [{ at: iso(2 * MIN), reason: 'claude accounts exhausted', from: 'claude/Claude #2', to: 'codex/Codex #1' }] : [],
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

function touch(db, task, changes = {}) {
  Object.assign(task, changes, { updated_at: new Date().toISOString() });
  recount(db);
  emit(db, { type: 'task.updated', task: summary(task) });
  const sandbox = db.sandboxes.find((s) => s.id === task.sandbox_id);
  if (sandbox) emit(db, { type: 'sandbox.updated', sandbox });
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
  if (!db.setupRequired) fail(409, 'conflict', 'This instance is already set up.');
  if (body.token !== 'YUL-SETUP-1234') fail(422, 'invalid', 'Wrong setup token. (Mock token: YUL-SETUP-1234)');
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
route('POST', '/api/auth/mock-github', ({ db }) => { db.signedIn = true; return db.me; });
route('POST', '/api/auth/logout', ({ db }) => { db.signedIn = false; for (const s of db.eventSockets) s.serverClose(1000); return json(204); });
route('GET', '/api/me', ({ db }) => db.me);
route('POST', '/api/device/approve', ({ body }) => {
  if (!/^[A-Z0-9]{4}-?[A-Z0-9]{4}$/.test(body.user_code || '')) fail(404, 'not_found', 'Unknown or expired code.');
  return json(204);
});

route('GET', '/api/sandboxes', ({ db }) => db.sandboxes);
route('GET', '/api/github/repositories', ({ db }) => [
  ...db.sandboxes.map((s) => ({ owner: s.repo.owner, name: s.repo.name, private: true, access: s.access, already_sandbox: true })),
  { owner: 'Quartio-Studio', name: 'website', private: false, access: 'admin', already_sandbox: false },
  { owner: 'Quartio-Studio', name: 'mobile-app', private: true, access: 'admin', already_sandbox: false },
  { owner: 'Quartio-Studio', name: 'infra', private: true, access: 'write', already_sandbox: false },
  { owner: 'Quartio-Studio', name: 'customer-portal', private: true, access: 'admin', already_sandbox: false },
]);
route('GET', '/api/templates', () => [
  { id: 'empty', name: 'Empty project (Yulius format only)', description: 'Docs format, AGENTS.md and a minimal test command.' },
  { id: 'web', name: 'Web application', description: 'Small web app skeleton with a test runner and dev server on the sandbox port range.' },
  { id: 'python-data', name: 'Python data project', description: 'Python package with notebooks folder, pytest and a data/README.' },
  { id: 'rust-service', name: 'Rust service', description: 'Axum service skeleton with cargo test and CI workflow.' },
]);
route('POST', '/api/sandboxes', ({ db, body }) => {
  let name;
  let repoName;
  if (body.mode === 'import') {
    if (!body.repo || !body.repo.includes('/')) fail(422, 'invalid', 'Choose a repository.');
    repoName = body.repo.split('/')[1];
    name = repoName;
    if (db.sandboxes.some((s) => s.repo.name === repoName)) fail(409, 'conflict', 'That repository is already a sandbox.');
  } else {
    if (!/^[A-Za-z0-9._-]{1,100}$/.test(body.name || '')) fail(422, 'invalid', 'Invalid repository name.');
    repoName = body.name;
    name = body.name;
  }
  db.nextId += 1;
  const sandbox = {
    id: `sb_${db.nextId}`, slug: repoName.toLowerCase(), name,
    repo: { owner: 'Quartio-Studio', name: repoName, url: `https://github.com/Quartio-Studio/${repoName}` },
    access: 'admin', default_branch: 'main',
    profile: { preset: 'standard', integration_branch: 'dev', production_branch: 'main', merge: 'squash', promote: 'merge_commit', auto_promote: false, environments: [{ name: 'dev', after: 'integration', command: null, verify: 'manual' }] },
    onboarding: body.mode === 'import' ? 'pending' : 'not_needed',
    port_base: 20000 + db.sandboxes.length * 20, port_count: 20, counts: { running: 0, needs_attention: 0, queued: 0 },
  };
  db.sandboxes.push(sandbox);
  if (body.mode === 'import') {
    later(() => { sandbox.onboarding = 'preparing'; emit(db, { type: 'sandbox.updated', sandbox }); }, 2500);
  }
  return json(201, sandbox);
});
route('GET', `/api/sandboxes/${ID}`, ({ db, p }) => sandboxOf(db, p[0]));
route('PATCH', `/api/sandboxes/${ID}`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'admin');
  if (body.profile) sandbox.profile = body.profile;
  emit(db, { type: 'sandbox.updated', sandbox });
  return sandbox;
});
route('POST', `/api/sandboxes/${ID}/onboarding/skip`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'admin');
  sandbox.onboarding = 'skipped';
  emit(db, { type: 'sandbox.updated', sandbox });
  return sandbox;
});
route('DELETE', `/api/sandboxes/${ID}`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'admin');
  db.sandboxes = db.sandboxes.filter((s) => s.id !== sandbox.id);
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
  if (!['claude', 'codex'].includes(body.harness)) fail(422, 'invalid', 'Unknown harness.');
  const number = Math.max(0, ...db.tasks.filter((t) => t.sandbox_id === sandbox.id).map((t) => t.number)) + 1;
  const title = body.title || body.prompt.trim().split('\n')[0].slice(0, 60);
  const wf = db.workflows.find((w) => w.id === body.workflow_id);
  const id = `tsk_${sandbox.id.slice(3)}_${number}`;
  const task = {
    id, sandbox_id: sandbox.id, number, title, harness: body.harness, status: 'queued', activity: null,
    branch: `feat/${title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 40) || `task-${number}`}`,
    origin: 'ui', created_by: { login: db.me.user.login }, created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    unread_messages: 0, archived: false, account_label: null,
    stepsList: wf ? wf.steps.map((title2, i) => ({ id: `${id}-s${i + 1}`, ordinal: i + 1, title: title2, status: 'pending', note: '', updated_by: 'workflow', updated_at: new Date().toISOString() })) : [],
    model: body.model || (body.harness === 'claude' ? 'claude-opus-4' : 'gpt-5-codex'), effort: body.effort || 'default', prompt: body.prompt,
  };
  db.tasks.push(task);
  recount(db);
  later(() => touch(db, task, { status: 'starting' }), 700);
  later(() => touch(db, task, { status: 'running', activity: 'working', account_label: task.harness === 'claude' ? 'Claude #1' : 'Codex #1' }), 1800);
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
  if (body && body.harness) task.harness = body.harness;
  touch(db, task, { status: 'starting', activity: null });
  later(() => touch(db, task, { status: 'running', activity: 'waiting', account_label: task.harness === 'claude' ? 'Claude #1' : 'Codex #1' }), 1300);
  return summary(task);
};
route('POST', `/api/tasks/${ID}/continue`, continueHandler);
route('POST', `/api/tasks/${ID}/switch`, ({ db, p, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (!['running', 'waiting_capacity'].includes(task.status)) fail(409, 'conflict', 'Only running tasks can switch.');
  const from = `${task.harness}/${task.account_label}`;
  const target = body.harness || task.harness;
  const account = db.accounts.find((a) => a.id === body.account_id);
  touch(db, task, { status: 'switching' });
  later(() => {
    touch(db, task, { status: 'running', activity: 'waiting', harness: target, account_label: account ? account.label : target === 'claude' ? 'Claude #1' : 'Codex #1' });
    const session = db.sessions.get(task.id);
    if (session) session.notice(`↻ switched from ${from} to ${target}/${task.account_label}`);
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
  if (!['correct', 'incorrect'].includes(body.verdict)) fail(422, 'invalid', 'Choose a verdict.');
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
  const stored = { id: `msg_${db.nextId}`, task: task.id, from: { task_id: null, login: db.me.user.login, title: db.me.user.login }, to_task_id: task.id, kind: 'info', subject: '', body: body.body, created_at: new Date().toISOString(), read: true };
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

route('GET', `/api/sandboxes/${ID}/git`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  const tasks = db.tasks.filter((t) => t.sandbox_id === sandbox.id && !t.archived && t.stepsList.length);
  return {
    integration_branch: sandbox.profile.integration_branch, production_branch: sandbox.profile.production_branch, default_branch: sandbox.default_branch,
    tasks: tasks.map((t) => ({ task_id: t.id, title: t.title, branch: t.branch, ahead: (t.number * 3) % 11 + 1, behind: t.number % 3, files_changed: (t.number * 7) % 23 + 1, insertions: (t.number * 131) % 900 + 12, deletions: (t.number * 37) % 200, pr: t.pr || prFor(t) })),
    recent_commits: [
      { sha: '9f3c2a17b0e4d5c6a7b8c9d0e1f2a3b4c5d6e7f8', subject: 'feat(checkout): cart totals with tax rules', author: 'task #12 (claude)', at: iso(25 * MIN) },
      { sha: '4b1d9e0c7a6f5e4d3c2b1a0f9e8d7c6b5a4f3e2d', subject: 'docs: restructure architecture section', author: 'task #5 (claude)', at: iso(5 * HOUR) },
      { sha: 'c0ffee1234567890abcdef1234567890abcdef12', subject: 'fix(viewer): dispose WebGL context on unmount', author: 'task #11 (codex)', at: iso(7 * HOUR) },
      { sha: '1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b', subject: 'Merge pull request #180 from feat/old-landing-page', author: 'mkarp', at: iso(6 * DAY) },
    ],
    pulls: tasks.map((t) => t.pr || prFor(t)).filter(Boolean),
  };
});
route('GET', `/api/tasks/${ID}/delivery`, ({ db, p }) => delivery(taskOf(db, p[0])));
route('POST', `/api/tasks/${ID}/delivery/pr`, ({ db, p, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (!delivery(task).spec_valid) fail(422, 'invalid', 'The task spec is not valid yet: missing Verification section.');
  db.nextId += 1;
  task.pr = { number: 300 + db.nextId, url: `https://github.com/Quartio-Studio/studio/pull/${300 + db.nextId}`, title: body.title || task.title, state: 'open', base: sandboxOf(db, task.sandbox_id).profile.integration_branch, head: task.branch, checks: 'pending' };
  task.deliveryStage = 'pr_open';
  task.deliveryNext = ['merge'];
  later(() => { task.pr.checks = 'success'; }, 4000);
  return delivery(task);
});
route('POST', `/api/tasks/${ID}/delivery/merge`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  const d = delivery(task);
  if (!d.pr || d.pr.checks !== 'success') fail(409, 'conflict', `Checks are ${d.pr ? d.pr.checks : 'missing'}; merge requires green checks.`);
  task.pr = { ...d.pr, state: 'merged' };
  const sandbox = sandboxOf(db, task.sandbox_id);
  const env = sandbox.profile.environments.find((e) => e.after === 'integration');
  task.deliveryStage = 'merged_integration';
  task.deliveryNext = env ? [`run_env:${env.name}`, 'promote'] : ['promote'];
  return delivery(task);
});
route('POST', `/api/sandboxes/${ID}/delivery/promote`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  return { result: sandbox.profile.promote === 'fast_forward' ? 'fast_forwarded' : 'promotion_pr_merged', sha: 'e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3' };
});
route('POST', `/api/sandboxes/${ID}/delivery/environments/${ID}/runs`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  if (body.confirm !== true) fail(422, 'invalid', 'Deploys need explicit confirmation.');
  const env = sandbox.profile.environments.find((e) => e.name === p[1]);
  if (!env) fail(404, 'not_found', 'No such environment.');
  db.nextId += 1;
  const runId = `run_${db.nextId}`;
  db.runs.set(runId, { started: Date.now(), env });
  return { run_id: runId };
});
route('GET', `/api/sandboxes/${ID}/delivery/runs/${ID}`, ({ db, p }) => {
  const run = db.runs.get(p[1]);
  if (!run) fail(404, 'not_found', 'No such run.');
  const lines = [`$ ${run.env.command || '(manual verification)'}`, 'Resolving target…', 'Uploading build artifact (12.4 MiB)…', 'Restarting service…', 'Health check: GET /healthz → 200', 'Smoke test: checkout flow ✓', 'Deploy finished.'];
  const elapsed = (Date.now() - run.started) / 1000;
  const shown = Math.min(lines.length, Math.floor(elapsed) + 1);
  const done = shown >= lines.length;
  return { state: done ? 'succeeded' : 'running', output: lines.slice(0, shown).join('\n'), exit_code: done ? 0 : null };
});

route('GET', `/api/sandboxes/${ID}/skills`, ({ db, p }) => { sandboxOf(db, p[0]); return db.skills; });
route('GET', `/api/sandboxes/${ID}/skills/${ID}`, ({ db, p }) => {
  const skill = db.skills.find((s) => s.name === p[1]);
  if (!skill) fail(404, 'not_found', 'No such skill.');
  return { name: skill.name, content: `---\nname: ${skill.name}\ndescription: ${skill.description}\n---\n\n# ${skill.name}\n\n1. Read AGENTS.md and the task spec.\n2. Make the smallest complete change.\n3. Verify with tests and report evidence.\n` };
});
route('PUT', `/api/sandboxes/${ID}/skills/${ID}`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  const skill = db.skills.find((s) => s.name === p[1]);
  if (!skill) fail(404, 'not_found', 'No such skill.');
  skill.enabled = Boolean(body.enabled);
  return json(204);
});

route('GET', `/api/sandboxes/${ID}/workflows`, ({ db, p }) => db.workflows.filter((w) => w.scope === 'instance' || w.sandbox === p[0]).map(({ sandbox: _s, ...w }) => w));
route('POST', `/api/sandboxes/${ID}/workflows`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  db.nextId += 1;
  const wf = { id: `wf_${db.nextId}`, sandbox: p[0], name: body.name, description: body.description || '', steps: body.steps || [], instructions: body.instructions || '', gates: body.gates || {}, scope: body.scope === 'instance' ? 'instance' : 'sandbox' };
  db.workflows.push(wf);
  const { sandbox: _s, ...out } = wf;
  return json(201, out);
});
route('PUT', `/api/workflows/${ID}`, ({ db, p, body }) => {
  const wf = db.workflows.find((w) => w.id === p[0]);
  if (!wf) fail(404, 'not_found', 'No such workflow.');
  Object.assign(wf, { name: body.name, description: body.description, steps: body.steps, instructions: body.instructions, gates: body.gates });
  const { sandbox: _s, ...out } = wf;
  return out;
});
route('DELETE', `/api/workflows/${ID}`, ({ db, p }) => { db.workflows = db.workflows.filter((w) => w.id !== p[0]); return json(204); });

route('GET', `/api/sandboxes/${ID}/automations`, ({ db, p }) => db.automations.filter((a) => a.sandbox === p[0]).map(({ sandbox: _s, ...a }) => a));
route('POST', `/api/sandboxes/${ID}/automations`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  db.nextId += 1;
  const a = { id: `aut_${db.nextId}`, sandbox: p[0], ...body, created_via: 'ui', next_run_at: body.enabled ? isoIn(5 * HOUR) : null, last_run: null };
  db.automations.push(a);
  const { sandbox: _s, ...out } = a;
  return json(201, out);
});
route('PUT', `/api/automations/${ID}`, ({ db, p, body }) => {
  const a = db.automations.find((x) => x.id === p[0]);
  if (!a) fail(404, 'not_found', 'No such automation.');
  Object.assign(a, body, { next_run_at: body.enabled ? isoIn(5 * HOUR) : null });
  const { sandbox: _s, ...out } = a;
  return out;
});
route('DELETE', `/api/automations/${ID}`, ({ db, p }) => { db.automations = db.automations.filter((a) => a.id !== p[0]); return json(204); });
route('POST', `/api/automations/${ID}/run`, ({ db, p }) => {
  const a = db.automations.find((x) => x.id === p[0]);
  if (!a) fail(404, 'not_found', 'No such automation.');
  a.last_run = { scheduled_for: new Date().toISOString(), state: 'scheduled', task_id: null };
  later(() => {
    a.last_run = { ...a.last_run, state: 'task_created', task_id: 'tsk_studio_8' };
    emit(db, { type: 'automation.run', automation_id: a.id, state: 'task_created', task_id: 'tsk_studio_8' });
  }, 1500);
  return json(204);
});
route('GET', `/api/automations/${ID}/runs`, ({ db, p }) => {
  const a = db.automations.find((x) => x.id === p[0]);
  if (!a) fail(404, 'not_found', 'No such automation.');
  return [0, 1, 2, 3, 4].map((i) => ({ scheduled_for: iso((i + 1) * DAY - 10 * HOUR), state: i === 2 ? 'skipped' : i === 4 ? 'failed' : 'task_created', task_id: i === 2 || i === 4 ? null : 'tsk_studio_8', error: i === 4 ? 'Quartio MCP returned 502' : i === 2 ? null : null }));
});

route('GET', `/api/sandboxes/${ID}/mcp`, ({ db, p }) => db.mcp.filter((m) => m.scope === 'instance' || m.sandbox === p[0]).map(({ sandbox: _s, ...m }) => m));
route('POST', `/api/sandboxes/${ID}/mcp`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  db.nextId += 1;
  const m = { id: `mcp_${db.nextId}`, sandbox: p[0], scope: body.scope === 'instance' ? 'instance' : 'sandbox', name: body.name, transport: body.transport, command: body.command || null, args: body.args || [], url: body.url || null, env_keys: Object.keys(body.env || {}), header_keys: Object.keys(body.headers || {}), enabled: body.enabled !== false };
  db.mcp.push(m);
  const { sandbox: _s, ...out } = m;
  return json(201, out);
});
route('PUT', `/api/mcp/${ID}`, ({ db, p, body }) => {
  const m = db.mcp.find((x) => x.id === p[0]);
  if (!m) fail(404, 'not_found', 'No such server.');
  if (m.scope === 'instance' && !db.me.user.is_instance_admin) fail(403, 'forbidden', 'Instance servers need an instance admin.');
  Object.assign(m, { name: body.name, transport: body.transport, command: body.command, args: body.args, url: body.url, enabled: body.enabled });
  if (body.env) m.env_keys = Object.keys(body.env);
  if (body.headers) m.header_keys = Object.keys(body.headers);
  const { sandbox: _s, ...out } = m;
  return out;
});
route('DELETE', `/api/mcp/${ID}`, ({ db, p }) => { db.mcp = db.mcp.filter((m) => m.id !== p[0]); return json(204); });
route('POST', `/api/mcp/${ID}/test`, ({ db, p }) => {
  const m = db.mcp.find((x) => x.id === p[0]);
  if (!m) fail(404, 'not_found', 'No such server.');
  return m.name === 'apify' ? { ok: false, tools: [], error: 'APIFY_TOKEN rejected (401)' } : { ok: true, tools: ['search_knowledge', 'get_customer', 'list_feedback', 'get_product_doc'], error: null };
});

route('POST', `/api/sandboxes/${ID}/shells`, ({ db, p }) => {
  need(sandboxOf(db, p[0]), 'write');
  db.nextId += 1;
  return { id: `sh_${p[0]}_${db.nextId}` };
});

const adminOnly = (db) => { if (!db.me.user.is_instance_admin && !db.me.user.is_superadmin) fail(403, 'forbidden', 'Instance administrators only.'); };
const superOnly = (db) => { if (!db.me.user.is_superadmin) fail(403, 'forbidden', 'Superadmin only.'); };
route('GET', '/api/admin/users', ({ db }) => { superOnly(db); return db.users; });
route('POST', '/api/admin/users', ({ db, body }) => {
  superOnly(db);
  if (db.users.some((u) => u.github_login.toLowerCase() === String(body.github_login).toLowerCase())) fail(409, 'conflict', 'Already admitted.');
  db.nextId += 1;
  const user = { id: `usr_${db.nextId}`, github_login: body.github_login, display_name: body.display_name || '', is_instance_admin: Boolean(body.is_instance_admin), disabled: false, is_superadmin: false, last_seen_at: null };
  db.users.push(user);
  return json(201, user);
});
route('PATCH', `/api/admin/users/${ID}`, ({ db, p, body }) => {
  superOnly(db);
  const user = db.users.find((u) => u.id === p[0]);
  if (!user) fail(404, 'not_found', 'No such user.');
  Object.assign(user, body);
  return user;
});
route('GET', '/api/admin/accounts', ({ db }) => { adminOnly(db); return db.accounts; });
route('POST', '/api/admin/accounts', ({ db, body }) => {
  adminOnly(db);
  db.nextId += 1;
  const account = { id: `acc_${db.nextId}`, provider: body.provider, label: body.label, enabled: true, priority: 5, status: 'relogin', usage: { primary: null, secondary: null, observed_at: null }, last_error: null, in_use_by: 0 };
  db.accounts.push(account);
  return { account, login_id: `login_${account.id}` };
});
route('PATCH', `/api/admin/accounts/${ID}`, ({ db, p, body }) => {
  adminOnly(db);
  const account = db.accounts.find((a) => a.id === p[0]);
  if (!account) fail(404, 'not_found', 'No such account.');
  Object.assign(account, body);
  return account;
});
route('DELETE', `/api/admin/accounts/${ID}`, ({ db, p }) => { adminOnly(db); db.accounts = db.accounts.filter((a) => a.id !== p[0]); return json(204); });
route('POST', `/api/admin/accounts/${ID}/relogin`, ({ db, p }) => { adminOnly(db); return { login_id: `login_${p[0]}` }; });
route('GET', '/api/admin/github', ({ db }) => {
  superOnly(db);
  return db.me.instance.github_configured ? { configured: true, app_slug: 'yulius-quartio', owner_login: 'Quartio-Studio', install_url: 'https://github.com/apps/yulius-quartio/installations/new' } : { configured: false, app_slug: null, owner_login: null, install_url: null };
});
route('POST', '/api/admin/github/manifest', ({ db, body }) => {
  superOnly(db);
  return { post_url: body.owner_kind === 'org' ? `https://github.com/organizations/${body.owner_login}/settings/apps/new?state=mock` : 'https://github.com/settings/apps/new?state=mock', manifest: { name: 'Yulius', url: ORIGIN, hook_attributes: { url: `${ORIGIN}/api/github/webhook` }, redirect_url: `${ORIGIN}/api/admin/github/manifest/callback`, public: false, default_permissions: { contents: 'write', pull_requests: 'write', checks: 'read', metadata: 'read', administration: 'write' } } };
});

route('GET', '/api/analytics/summary', ({ db, query }) => {
  const from = Date.parse(query.get('from') || iso(30 * DAY));
  const to = Date.parse(query.get('to') || new Date().toISOString());
  const days = Math.max(1, Math.round((to - from) / DAY));
  const perDay = [];
  for (let i = days - 1; i >= 0; i -= 1) {
    const date = new Date(to - i * DAY).toISOString().slice(0, 10);
    const weekday = new Date(date).getUTCDay();
    const base = weekday === 0 || weekday === 6 ? 1 : 4;
    perDay.push({ date, count: (hashString(date + query.get('sandbox')) % 5) + base });
  }
  const total = perDay.reduce((a, d) => a + d.count, 0);
  const scale = total / 120;
  return {
    from: query.get('from'), to: query.get('to'),
    tasks: { total, by_status: { closed: Math.round(total * 0.62), stopped_cancelled: Math.round(total * 0.12), running: 3, failed_spawn_error: 1, stopped_crashed: 2 }, by_harness: { claude: Math.round(total * 0.68), codex: Math.round(total * 0.32) } },
    tasks_per_day: perDay,
    duration_ms: { median: 47 * MIN, p90: 3.2 * HOUR, total_active: total * 31 * MIN, total_wall: total * 58 * MIN },
    tokens: { input: Math.round(41e6 * scale), output: Math.round(5.2e6 * scale), cache_read: Math.round(310e6 * scale), cache_write: Math.round(22e6 * scale) },
    tokens_by_model: [{ model: 'claude-opus-4', input: Math.round(26e6 * scale), output: Math.round(3.1e6 * scale) }, { model: 'gpt-5-codex', input: Math.round(12e6 * scale), output: Math.round(1.7e6 * scale) }, { model: 'claude-sonnet-4.5', input: Math.round(3e6 * scale), output: Math.round(0.4e6 * scale) }],
    tokens_by_harness: [{ harness: 'claude', input: Math.round(29e6 * scale), output: Math.round(3.5e6 * scale) }, { harness: 'codex', input: Math.round(12e6 * scale), output: Math.round(1.7e6 * scale) }],
    tokens_by_account: db.accounts.filter((a) => a.enabled).map((a, i) => ({ account_label: a.label, input: Math.round((14e6 / (i + 1)) * scale), output: Math.round((1.8e6 / (i + 1)) * scale) })),
    switches: { total: Math.round(total * 0.15), account: Math.round(total * 0.11), provider: Math.round(total * 0.04) },
    feedback: { correct: Math.round(total * 0.3), incorrect: Math.round(total * 0.06) },
  };
});
route('GET', '/api/analytics/tasks', ({ db, query }) => {
  const rows = db.tasks.map((t) => ({ task_id: t.id, sandbox: t.sandbox_id, number: t.number, title: t.title, harness: t.harness, status: t.status, created_at: t.created_at }));
  if (query.get('format') === 'csv') {
    const csv = ['task_id,sandbox,number,title,harness,status,created_at', ...rows.map((r) => [r.task_id, r.sandbox, r.number, `"${r.title}"`, r.harness, r.status, r.created_at].join(','))].join('\n');
    return new Response(csv, { status: 200, headers: { 'Content-Type': 'text/csv' } });
  }
  return rows;
});
route('GET', '/api/status', ({ db }) => {
  const wobble = (Date.now() / 1000) % 60;
  return {
    vm: { cpu_percent: 22 + Math.sin(wobble) * 9, cpu_count: 8, memory_used_bytes: 9.4 * 2 ** 30, memory_total_bytes: 16 * 2 ** 30, disk_used_bytes: 141 * 2 ** 30, disk_total_bytes: 200 * 2 ** 30, load: [1.21, 0.97, 0.74], uptime_s: 9 * 86400 + 3600 * 4 },
    sessions: { live: db.tasks.filter((t) => t.status === 'running').length, limit: 8, queued: db.tasks.filter((t) => t.status === 'queued').length },
    sandboxes: db.sandboxes.map((s, i) => ({ id: s.id, name: s.name, live_sessions: s.counts.running, disk_bytes: (12 + i * 7) * 2 ** 30 })),
    accounts: db.accounts,
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
  `  ${dim('⎿  Delivered to #11 3D model viewer (codex)')}`,
  `${c(173, '✻')} ${c(173, 'Pondering…')} ${dim('(esc to interrupt)')}`,
  `${c(255, '⏺')} Done with this step. Anything else before I write the UI wiring?`,
];

const SCRIPT_CODEX = [
  `${c(141, '•')} Explored`,
  `  ${dim('└ Read viewer.tsx, scene.ts')}`,
  `${c(141, '•')} Ran ${bold('pnpm test --filter viewer')}`,
  `  ${dim('└ 23 passed')}`,
  `${c(141, '•')} Edited src/viewer/scene.ts ${c(114, '(+12')} ${c(203, '-3)')}`,
  `${c(141, '•')} Preview: ${c(39, 'http://localhost:20005/')}`,
  `${c(141, '•')} Working ${dim('(12s • esc to interrupt)')}`,
  `${c(255, '›')} Ready. Tell me what to change next.`,
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
    this.controller = null;
    this.cols = 120;
    this.rows = 36;
    this.script = task.harness === 'codex' ? SCRIPT_CODEX : SCRIPT_CLAUDE;
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
    if (this.task.harness === 'codex') {
      this.lines.push(`${c(141, '>_')} ${bold('OpenAI Codex')} ${dim('(v0.46.0)')}`, `   ${dim('model:')} ${this.task.model} ${this.task.effort}   ${dim('/model to change')}`, `   ${dim('directory:')} ${cwd}`, '');
    } else {
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
    const hint = this.task.harness === 'codex'
      ? `${dim(' ⏎ send   ⇧⏎ newline   ⌃C quit')}${' '.repeat(6)}${c(141, `${this.task.model} · ${this.task.effort}`)} ${dim('· 62% context left')}`
      : `${dim(' ⏵⏵ accept edits on (shift+tab to cycle)')}${' '.repeat(6)}${c(173, `${this.task.model} · effort ${this.task.effort}`)}`;
    const visible = hint.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, '').length;
    const short = this.task.harness === 'codex' ? c(141, ` ${this.task.model}`) : c(173, ` ${this.task.model}`);
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

  state(ws) {
    return JSON.stringify({ type: 'state', attempt_seq: 1, harness: this.task.harness, cols: this.cols, rows: this.rows, can_write: this.canWrite(), controller: ws === this.controller, status: this.task.status });
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
      if (!this.controller && this.canWrite()) {
        this.controller = ws;
        if (msg.cols && msg.rows) { this.cols = Math.max(20, Math.min(400, msg.cols)); this.rows = Math.max(8, Math.min(200, msg.rows)); }
      }
      ws.push(this.state(ws));
      this.sendSnapshot(ws);
      this.ensureTicking();
    } else if (msg.type === 'resize') {
      if (ws !== this.controller) return;
      const cols = Math.max(20, Math.min(400, msg.cols));
      const rows = Math.max(8, Math.min(200, msg.rows));
      if (cols === this.cols && rows === this.rows) return;
      this.cols = cols;
      this.rows = rows;
      this.seq += 1;
      for (const viewer of this.viewers) { viewer.push(this.state(viewer)); viewer.push(terminalFrame(2, this.seq, this.snapshot())); }
    } else if (msg.type === 'take_control') {
      if (!this.canWrite()) return;
      this.controller = ws;
      this.sendStates();
    } else if (msg.type === 'input') {
      if (!this.canWrite()) { ws.push(JSON.stringify({ type: 'error', code: 'read_only', message: 'Input requires write permission.' })); return; }
      if (this.controller !== ws) { this.controller = ws; this.sendStates(); }
      this.onInput(String(msg.data || ''));
    }
  }

  closed(ws) {
    this.viewers.delete(ws);
    if (this.controller === ws) this.controller = [...this.viewers][0] || null;
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

/** Fake bash in the sandbox shell panel. */
class ShellSession {
  constructor(slug) {
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
      ws.push(JSON.stringify({ type: 'state', attempt_seq: 1, harness: null, cols: msg.cols, rows: msg.rows, can_write: true, controller: true, status: 'running' }));
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
      if (this.ws) this.ws.push(JSON.stringify({ type: 'exit', status: 'closed', code: 0 }));
      return;
    }
    const result = cmd ? outputs[cmd] ?? `bash: ${cmd.split(' ')[0]}: command not found (mock shell)` : '';
    this.out(`\r\n${result ? `${result}\r\n` : ''}${this.prompt}`);
  }

  closed() { this.ws = null; }
}

/** Fake provider login for the account login terminal. */
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
      const account = this.db.accounts.find((a) => a.id === this.accountId);
      const codex = account && account.provider === 'openai';
      ws.push(JSON.stringify({ type: 'state', attempt_seq: 1, harness: codex ? 'codex' : 'claude', cols: msg.cols, rows: msg.rows, can_write: true, controller: true, status: 'running' }));
      this.seq += 1;
      ws.push(terminalFrame(2, this.seq, [
        bold(codex ? 'Codex login' : 'Claude Code login'),
        '',
        'Open this URL in your browser and approve access:',
        c(39, codex ? 'https://auth.openai.com/oauth/authorize?client_id=mock&state=xyz' : 'https://claude.ai/oauth/authorize?code=true&client_id=mock&state=xyz'),
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

  finish() {
    this.done = true;
    this.out(`\r\n${c(114, '✔')} Logged in. Credentials stored encrypted for this account.\r\n`);
    const account = this.db.accounts.find((a) => a.id === this.accountId);
    if (account) {
      Object.assign(account, { status: 'ok', last_error: null, usage: { primary: { used_percent: 3.0, resets_at: isoIn(5 * HOUR), window_minutes: 300 }, secondary: { used_percent: 1.0, resets_at: isoIn(7 * DAY), window_minutes: 10080 }, observed_at: new Date().toISOString() } });
      emit(this.db, { type: 'account.updated', account });
    }
    if (this.ws) this.ws.push(JSON.stringify({ type: 'exit', status: 'closed', code: 0 }));
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
    this.render();
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
      case 'open': {
        this.next += 1;
        for (const t of this.tabs) t.active = false;
        let host = msg.url;
        try { host = new URL(msg.url).host || msg.url; } catch { /* keep raw */ }
        this.tabs.push({ id: `tab-${this.next}`, title: host, url: msg.url, active: true, action_requested: null });
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
    const slug = (db.sandboxes.find((s) => decodeURIComponent(m[1]).includes(s.id)) || { slug: 'sandbox' }).slug;
    ws.server = new ShellSession(slug);
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
      touch(db, switching, { status: 'running', activity: 'waiting', account_label: 'Codex #1' });
      const session = db.sessions.get(switching.id);
      if (session) session.notice('↻ switched from claude/Claude #2 to codex/Codex #1 (account exhausted)');
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
