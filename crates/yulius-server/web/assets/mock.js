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
// recovery=1 (setup issues a recovery code), update=1|fail (an update is on offer | its install rolls back), helper=0 (no update helper),
// jev=config (a Jev key file is configured), vidoc=config (a Vidoc key file), fail=<path> (GETs of that path answer 500),
// failpost=<path> (POSTs of that path answer 500), holdpost=<path> (POSTs of that path wait for
// `__yuliusMock.release()`, as hold= does for GETs; `heldWaiting` counts those waiting), as=<login> (signed in as another member, a
// teammate in a second window: mock pages of one origin relay presence to each other, SPEC-127).

import { counter, taskInfo } from './status.js';
import { parsePrompt } from './promptparts.js';

const MIN = 60e3;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const ORIGIN = window.location.origin;
// Where the mock's sandboxes live, and its composer attachments as an upload reports them
// (SPEC-043).
const MOCK_SANDBOXES = '/srv/yulius/sandboxes';
// Where the mock's own upload routes put files (as the server reports them).
const MOCK_UPLOADS = '/var/lib/yulius/sandboxes';
const MOCK_ATTACHMENTS = `${MOCK_SANDBOXES}/sb_studio/attachments/drafts/usr_mkarp`;
// A file gone since (drafts are removed after a week).
const MOCK_GONE = new Set([`${MOCK_ATTACHMENTS}/old-mockup.png`]);
// A 1×1 PNG served for every attached image.
const MOCK_PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==';

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
    // `recovery=1`: setup issues a recovery code, as the server does (SPEC-283).
    setupRecoveryCode: params.get('recovery') === '1' ? '246810' : null,
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
    // Whether sandboxes get Docker (SPEC-203): `docker=rebuild` (the base lacks the host's
    // dockerd and is being rebuilt; a sandbox on its own docker.io; the update's re-check found a
    // problem), `docker=kept` (the last host check failed items but kept Docker, #674; with
    // `update=1` there is none until the update's), `docker=legacy` (sandboxes not isolated);
    // `smoke=fail` (the smoke's pull fails).
    docker: { mode: params.get('docker') || 'ok', smokeFails: params.get('smoke') === 'fail', smoke: null, hostCheck: undefined },
    // Leftovers on the disk (SPEC-215): `leftovers=none` scans to an empty list.
    leftovers: { none: params.get('leftovers') === 'none', items: null, scannedAt: null, job: null },
    // `disk=low|critical` (SPEC-276 RDA-007): the server's data disk, 386 GiB, as the warning pill reads it.
    disk: params.get('disk') === 'critical'
      ? { level: 'critical', used: 370 * 1024 ** 3, total: 386 * 1024 ** 3 }
      : params.get('disk') === 'low'
        ? { level: 'low', used: 340 * 1024 ** 3, total: 386 * 1024 ** 3 }
        : { level: 'ok', used: 141 * 1024 ** 3, total: 386 * 1024 ** 3 },
    jev: { key: null, setAt: null, setBy: null, config: params.get('jev') === 'config' },
    // The instance Vidoc key (SPEC-195): `vidoc=config` (YULIUS_VIDOC_API_KEY_FILE is set).
    vidoc: { key: null, setAt: null, setBy: null, config: params.get('vidoc') === 'config' },
    // fail=<path> answers 500 to GET requests of that exact path, to show how a view says a load
    // failed (in place, naming what failed, with Try again; #147). A test may set `failPath` later;
    // `failedReads` counts the GET requests failed on purpose.
    failPath: params.get('fail') || null,
    failedReads: 0,
    holdPath: params.get('hold') || null,
    holdPostPath: params.get('holdpost') || null,
    // failpost=<path> does the same to POST requests, to show how an action says it failed;
    // `failedPosts` counts them.
    failPostPath: params.get('failpost') || null,
    failedPosts: 0,

    nextId: 100,
    // Account sign-ins by login id, and the ids a person cancelled (SPEC-243).
    logins: new Map(),
    loginCancels: [],
    // Accounts added here that never signed in: they hold no credential.
    unsignedAccounts: new Set(),
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
      // A task someone handed over has an owner other than its author (SPEC-070).
      ...(extra.owner ? { owner: { login: extra.owner } } : {}),
      // Created before its last change (an old task's sessions never last a negative time).
      created_at: iso(Math.max(extra.age || number * 3 * HOUR, (extra.updated ?? number * 7 * MIN) + HOUR)), updated_at: iso(extra.updated ?? number * 7 * MIN),
      archived: Boolean(extra.archived),
      account_label: LIVE.has(status) ? 'Claude #2' : null,
      stepsList: extra.noSteps ? [] : makeSteps(id, done, current),
      model: 'claude-opus-4',
      effort: 'high',
      // The prompt it started with: the text, then an `@path` line per attached file (SPEC-043).
      prompt: extra.prompt ?? title,
      delivery: extra.delivery || null, // {stage, environment, pr} once delivery started
      // Paused for subscription capacity: when the earliest window resets (ROT-010).
      resumes_at: status === 'waiting_capacity' ? new Date(Date.now() + 38 * MIN).toISOString() : null,
      ...(extra.mode ? { mode: extra.mode } : {}),
      // Background shells the idle agent left running (TSK-014).
      background_shells: extra.shells ?? null,
      background_shells_since: extra.shells ? new Date(Date.now() - 12 * MIN).toISOString() : null,
      // Since when it is in its state, and the start of its working stretch, which goes on through
      // the turns its background work wakes (SPEC-087): a task's last change is often newer.
      state_since: iso(extra.state ?? extra.updated ?? number * 7 * MIN),
      working_since: extra.working === undefined ? null : iso(extra.working),
    };
  };
  const pr = (number, title, state, base, checks) => ({ number, url: `https://github.com/Quartio-Studio/${number > 400 ? 'product' : 'studio'}/pull/${number}`, title, state, base, head: `feat/${title.toLowerCase().replace(/[^a-z0-9]+/g, '-')}`, checks });
  db.tasks = [
    // Goal achieved after a clean turn, no shell left: Done with a green check although its pull
    // request's checks still run (TSK-014, F 2026-10-10).
    t('sb_studio', 13, 'Improve the code', 'claude', 'running', 'done', { mode: 'goal', done: 7, current: false, updated: 6 * MIN, shells: 0, delivery: { stage: 'pr_open', environment: null, pr: pr(520, 'Improve the code', 'open', 'dev', 'pending') } }),
    // Goal achieved, but a background shell (a dev server) still runs: Working, with the shell said.
    t('sb_studio', 14, 'Start the preview server', 'claude', 'running', 'done', { mode: 'goal', done: 7, current: false, updated: 2 * MIN, shells: 1, working: 95 * MIN }),
    // Working for two hours: its last turn began a moment ago, woken by its own background work.
    t('sb_studio', 12, 'Checkout API', 'claude', 'running', 'working', { done: 2, updated: 20e3, working: 2 * HOUR + 14 * MIN, prompt: `# Checkout API\n\nBuild the checkout endpoint:\n\n- Stripe payment intents\n- an idempotency key per order\n\n@${MOCK_ATTACHMENTS}/checkout-error.png\n@${MOCK_ATTACHMENTS}/notes.txt\n@${MOCK_ATTACHMENTS}/old-mockup.png` }),
    // Finished its turn with a tested pull request (checks green) and a complete plan: "Done" (#35, #139).
    t('sb_studio', 11, '3D model viewer', 'claude', 'running', 'waiting', { done: 7, current: false, author: 'anna-q', owner: 'mkarp', delivery: { stage: 'pr_open', environment: null, pr: pr(221, '3D model viewer', 'open', 'dev', 'success') } }),
    t('sb_studio', 10, 'AWS login for deploy', 'claude', 'running', 'needs_attention', { done: 4 }),
    t('sb_studio', 9, 'Move to the next account', 'claude', 'switching', null, { done: 3, working: 3 * HOUR }),
    t('sb_studio', 8, 'Nightly feedback digest', 'claude', 'queued', null, { origin: 'automation', noSteps: true }),
    t('sb_studio', 7, 'Search indexing', 'claude', 'waiting_capacity', null, { done: 1 }),
    t('sb_studio', 6, 'Fix flaky checkout test', 'claude', 'starting', null, { noSteps: true, working: MIN }),
    t('sb_studio', 5, 'Docs restructure', 'claude', 'closed', null, { done: 7, current: false, updated: 5 * HOUR, delivery: { stage: 'pr_open', environment: null, pr: pr(214, 'Docs restructure', 'open', 'quartio', 'success') } }),
    t('sb_studio', 4, 'Payment webhook', 'claude', 'stopped_cancelled', null, { done: 3, current: false, updated: 9 * HOUR, delivery: { stage: 'pr_open', environment: null, pr: pr(209, 'Payment webhook', 'open', 'quartio', 'failure') } }),
    t('sb_studio', 3, 'Image pipeline', 'claude', 'stopped_crashed', null, { done: 2, current: false, updated: DAY }),
    t('sb_studio', 2, 'Legacy import', 'claude', 'failed_spawn_error', null, { noSteps: true, updated: 2 * DAY, origin: 'import' }),
    t('sb_studio', 1, 'Old landing page', 'claude', 'closed', null, { done: 7, current: false, archived: true, updated: 6 * DAY, delivery: { stage: 'production_done', environment: null, pr: pr(180, 'Old landing page', 'merged', 'quartio', 'success') } }),
    t('sb_product', 1, 'Prepare repository for Yulius', 'claude', 'closed', null, { origin: 'onboarding', done: 7, current: false, updated: 2 * DAY }),
    t('sb_product', 2, 'Pricing page', 'claude', 'closed', null, { done: 7, current: false, updated: 3 * HOUR, delivery: { stage: 'merged_integration', environment: null, pr: pr(512, 'Pricing page', 'merged', 'quartio', 'none') } }),
    t('sb_product', 3, 'Search filters', 'claude', 'closed', null, { done: 7, current: false, updated: 70 * MIN, author: 'anna-q', delivery: { stage: 'pr_open', environment: null, pr: pr(515, 'Search filters', 'open', 'quartio', 'success') } }),
    t('sb_product', 4, 'Invoice PDF export', 'claude', 'running', 'working', { done: 3, updated: 4 * MIN, working: 50 * MIN }),
    t('sb_product', 5, 'Onboarding emails', 'claude', 'closed', null, { done: 7, current: false, updated: 2 * DAY, delivery: { stage: 'production_done', environment: null, pr: pr(498, 'Onboarding emails', 'merged', 'quartio', 'none') } }),
    t('sb_product', 6, 'Team invites', 'claude', 'closed', null, { done: 7, current: false, updated: 26 * HOUR, author: 'anna-q', delivery: { stage: 'merged_integration', environment: null, pr: pr(505, 'Team invites', 'merged', 'quartio', 'none') } }),
    // Waits for #4 to be Done (TSK-012).
    t('sb_product', 7, 'Invoice emails', 'claude', 'chained', null, { noSteps: true, updated: 3 * MIN }),
    t('sb_three', 2, 'Write README', 'claude', 'closed', null, { done: 7, current: false }),
    // anna-q's request, owned by mkarp, who can only read the project now: their own task's terminal
    // stays read-only for want of write access (someone else's is watched as its transcript, SPEC-179).
    t('sb_three', 1, 'Explore data model', 'claude', 'running', 'working', { done: 1, author: 'anna-q', owner: 'mkarp', working: 7 * MIN }),
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
  db.accounts = [
    { id: 'acc_c1', provider: 'anthropic', email: 'mkarp@example.com', label: 'Claude #1', status: 'ok', usage: { primary: { used_percent: 42.0, resets_at: isoIn(2.2 * HOUR), window_minutes: 300 }, secondary: { used_percent: 18.5, resets_at: isoIn(4 * DAY), window_minutes: 10080 }, observed_at: iso(2 * MIN) }, last_error: null, in_use_by: 1 },
    { id: 'acc_c2', provider: 'anthropic', email: 'team@example.com', label: 'Claude #2', status: 'ok', usage: { primary: { used_percent: 87.3, resets_at: isoIn(1.1 * HOUR), window_minutes: 300 }, secondary: { used_percent: 61.0, resets_at: isoIn(2 * DAY), window_minutes: 10080 }, observed_at: iso(1 * MIN) }, last_error: 'usage: Usage updates again in about 12 min.', in_use_by: 3 },
    { id: 'acc_c3', provider: 'anthropic', email: 'ops@example.com', label: 'Claude #3', status: 'relogin', usage: { primary: null, secondary: null, observed_at: null }, last_error: 'OAuth refresh token expired; sign in again.', in_use_by: 0 },
    { id: 'acc_x1', provider: 'anthropic', email: 'pawel@example.com', label: 'Claude #4', status: 'ok', usage: { primary: { used_percent: 12.0, resets_at: isoIn(3.5 * HOUR), window_minutes: 300 }, secondary: { used_percent: 33.0, resets_at: isoIn(5 * DAY), window_minutes: 10080 }, observed_at: iso(4 * MIN) }, last_error: null, in_use_by: 2 },
    { id: 'acc_x2', provider: 'anthropic', email: 'studio@example.com', label: 'Claude #5', status: 'limited', usage: { primary: { used_percent: 99.6, resets_at: isoIn(1.9 * HOUR), window_minutes: 300 }, secondary: { used_percent: 71.0, resets_at: isoIn(3 * DAY), window_minutes: 10080 }, observed_at: iso(30e3) }, last_error: null, in_use_by: 0 },
    { id: 'acc_x3', provider: 'anthropic', email: null, label: 'Claude spare', status: 'ok', usage: { primary: { used_percent: 0.5, resets_at: null, window_minutes: 300 }, secondary: null, observed_at: iso(3 * DAY) }, last_error: null, in_use_by: 0 },
  ];
  seededDb = db;
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
    { id: 'aut_1', sandbox: 'sb_studio', name: 'Daily customer feedback', cron: '0 9 * * *', utc_offset_minutes: 120, prompt: 'Create a task implementing yesterday’s customer feedback from the Quartio MCP.', workflow_id: 'wf-feature', harness: 'claude', enabled: true, supertask: false, after_task_id: null, after_task_number: null, start_rule: null, created_via: 'agent', next_run_at: isoIn(14 * HOUR), last_run: { scheduled_for: iso(10 * HOUR), state: 'task_created', task_id: 'tsk_studio_8' } },
    { id: 'aut_2', sandbox: 'sb_studio', name: 'Weekly dependency audit', cron: '0 7 * * 1', utc_offset_minutes: 0, prompt: 'Audit dependencies for advisories and open a PR with safe upgrades.', workflow_id: null, harness: 'claude', enabled: false, supertask: false, after_task_id: null, after_task_number: null, start_rule: null, created_via: 'ui', next_run_at: null, last_run: null },
    // A shipped automation an agent created by name (SPEC-042 OPS-010): a default all the same (SPEC-197).
    { id: 'aut_3', sandbox: 'sb_studio', name: 'Work through open issues', cron: '0 3 * * *', utc_offset_minutes: 120, prompt: 'Work through this repository’s open GitHub issues written by the core team or their agents.', workflow_id: null, harness: 'claude', enabled: true, supertask: false, after_task_id: null, after_task_number: null, start_rule: null, created_via: 'agent', next_run_at: isoIn(14 * HOUR), last_run: null },
  ];
  db.mcp = [
    { id: 'mcp_quartio', sandbox: null, scope: 'instance', name: 'quartio', transport: 'http', command: null, args: [], url: 'https://mcp.quartio.ai/mcp', env_keys: [], header_keys: ['Authorization'], enabled: true, origin: 'person' },
    { id: 'mcp_apify', sandbox: 'sb_studio', scope: 'sandbox', name: 'apify', transport: 'stdio', command: 'npx', args: ['-y', '@apify/actors-mcp-server'], url: null, env_keys: ['APIFY_TOKEN'], header_keys: [], enabled: true, origin: 'person' },
  ];
  // AdminUser (the superadmin is not listed).
  db.users = [
    { id: 'usr_1', github_login: 'mkarp', display_name: 'Michal Karp', is_instance_admin: true, disabled: false, github_linked: true, two_factor: 'on' },
    { id: 'usr_2', github_login: 'anna-q', display_name: 'Anna', is_instance_admin: false, disabled: false, github_linked: true, two_factor: 'on' },
    { id: 'usr_3', github_login: 'new-hire', display_name: 'new-hire', is_instance_admin: false, disabled: false, github_linked: false, two_factor: 'missing' },
    { id: 'usr_4', github_login: 'old-contractor', display_name: 'old-contractor', is_instance_admin: false, disabled: true, github_linked: true, two_factor: 'pending' },
  ];
  db.runs = []; // EnvironmentRun records, newest last
  db.ci = seedCi();
  db.cd = seedCd();
  const pastRun = (n, env, kind, by, ago) => ({ run_id: `run_seed_${n}`, sandbox_id: 'sb_product', environment: env, kind, state: 'succeeded', command: kind === 'manual' ? null : 'node scripts/deployer.mjs --target dev-vm', commit: 'b7e9f01c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a', requested_by: by, output: kind === 'manual' ? `${by} confirmed ${env}.` : 'deploying… done in 2m 14s', exit_code: kind === 'manual' ? null : 0, started_at: iso(ago), ended_at: iso(ago - 2 * MIN), startedMs: Date.now() - ago - 1e9 });
  db.runs.push(pastRun(1, 'dev-vm', 'command', 'mkarp', 2 * DAY + HOUR), pastRun(2, 'dev-vm', 'manual', 'mkarp', 2 * DAY + 30 * MIN), pastRun(3, 'rollout', 'manual', 'anna-q', 2 * DAY - HOUR), pastRun(4, 'dev-vm', 'command', 'mkarp', 3 * HOUR));
  db.promotionPr = new Map(); // sandbox id → open promotion PR URL (merge_commit promotion)
  // `sandboxes=0`: a fresh instance with no sandbox yet; the home screen shows its first steps
  // (import a repository, create one from a template).
  // `as=<login>`: a teammate with write access, for a second window (SPEC-127).
  if (params.get('as')) {
    const login = params.get('as').replace(/[^A-Za-z0-9-]/g, '');
    Object.assign(db.me.user, { id: `usr_${login}`, login, display_name: login, is_superadmin: false, is_instance_admin: false });
  }
  if ((!linked && role !== 'superadmin') || params.get('sandboxes') === '0') {
    db.sandboxes = [];
    db.tasks = [];
  }
  // `onboarding=preparing`: `product` is being onboarded (SPEC-096); its onboarding task is #1.
  if (params.get('onboarding') === 'preparing') db.sandboxes.find((s) => s.id === 'sb_product').onboarding = 'preparing';
  // `claude=0`: no Claude account is connected (SPEC-246): nothing that runs an agent starts.
  // `onboarding=failed`: `product` waits for onboarding, whose start failed for want of one.
  if (params.get('claude') === '0') db.accounts = [];
  if (params.get('onboarding') === 'failed') {
    const product = db.sandboxes.find((s) => s.id === 'sb_product');
    product.onboarding = 'pending';
    Object.assign(preparationTask(db, product), { status: 'failed_no_account', neverRan: false, awaitsStart: false });
  }
  // `tasks=0`: sandboxes without any task yet (the composer then offers no start rule, SPEC-022).
  if (params.get('tasks') === '0') db.tasks = [];
  // `hold=<path>`: its GET requests wait until `__yuliusMock.release()`, which may come first;
  // `hold=frames` keeps the sandbox Chrome's picture back instead.
  if (db.holdPath || db.holdPostPath) db.held = new Promise((resolve) => { db.release = () => { db.released = true; resolve(); }; });
  // `tasks=many`: 30 more queued tasks in studio, a long "Run after" menu (SPEC-038).
  if (params.get('tasks') === 'many') {
    for (let number = 15; number < 45; number += 1) db.tasks.push(t('sb_studio', number, `Queued idea ${number}`, 'claude', 'queued', null, { noSteps: true }));
  }
  // `tasks=graph`: studio's tasks form a structure (SPEC-164): a chain #12 → #15 → #16, a rule
  // #17 over #12 and #16, a supertask #20 holding #8 and #21 in the queue, prepared #18 and #19,
  // and a rule #22 naming the archived #1. `graph=cycle` adds two tasks waiting for each other.
  if (params.get('tasks') === 'graph') {
    const find = (number) => db.tasks.find((x) => x.sandbox_id === 'sb_studio' && x.number === number);
    const add = (...args) => { const task = t('sb_studio', ...args); db.tasks.push(task); return task; };
    const cart = add(15, 'Cart totals', 'claude', 'chained', null, { noSteps: true, updated: 40 * MIN });
    const receipts = add(16, 'Receipt emails', 'claude', 'chained', null, { noSteps: true, updated: 35 * MIN });
    const release = add(17, 'Release checklist', 'claude', 'chained', null, { noSteps: true, updated: 30 * MIN });
    chainAfter(cart, find(12));
    chainAfter(receipts, cart);
    chainAfter(release, find(12), '#12 done and #16 done');
    // #14 ran after #13 and has started: its link is history, not a wait.
    chainAfter(find(14), find(13));
    add(18, 'Weekly cleanup', 'claude', 'prepared', null, { noSteps: true, updated: 25 * MIN }).start_at = isoIn(2 * DAY);
    add(19, 'Pricing experiment', 'claude', 'prepared', null, { noSteps: true, updated: 20 * MIN });
    const hardening = add(20, 'Security hardening', 'claude', 'queued', null, { noSteps: true, updated: 15 * MIN });
    hardening.supertask = true;
    hardening.heldBy = mockHeldBy(db, hardening);
    find(8).heldBy = [20];
    add(21, 'Translate the docs', 'claude', 'queued', null, { noSteps: true, updated: 10 * MIN }).heldBy = [20];
    chainAfter(add(22, 'Restore the old landing', 'claude', 'chained', null, { noSteps: true, updated: 5 * MIN }), find(15), '#1 done or #15 ended');
    if (params.get('graph') === 'cycle') {
      const a = add(23, 'Loop A', 'claude', 'chained', null, { noSteps: true, updated: 4 * MIN });
      const b = add(24, 'Loop B', 'claude', 'chained', null, { noSteps: true, updated: 3 * MIN });
      chainAfter(a, b);
      chainAfter(b, a);
    }
  }
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
  const finishedEffort = finished.reduce((sum, s) => sum + effort(s), 0);
  const percent = total ? Math.floor((finishedEffort * 100) / total) : 0;
  // The share in progress ends at the cumulative floor, as the server computes it (SPEC-159).
  const activeEffort = list.filter((s) => s.status === 'in_progress').reduce((sum, s) => sum + effort(s), 0);
  const active_percent = total ? Math.min(100, Math.floor(((finishedEffort + activeEffort) * 100) / total)) - percent : 0;
  return { done: finished.length, total: list.length, current: current ? current.title : null, percent, active_percent };
}

/** The mock's database, for views built without it (a subscription's email by its label). */
let seededDb = null;

/** The email of the subscription named `label` (SPEC-244), `null` while unknown. */
function accountEmailOf(label) {
  const account = label && seededDb ? seededDb.accounts.find((a) => a.label === label) : null;
  return account ? account.email ?? null : null;
}

function summary(task) {
  return {
    id: task.id, sandbox_id: task.sandbox_id, number: task.number, title: task.title, harness: task.harness,
    status: task.status, activity: task.status === 'running' || task.status === 'waiting_capacity' ? task.activity : null,
    branch: task.branch, origin: task.origin, created_by: task.created_by, created_at: task.created_at, updated_at: task.updated_at,
    // Since when it is in its state and working (SPEC-087).
    state_since: task.state_since || task.updated_at, working_since: task.working_since ?? null,
    // The owner, whose GitHub token the task uses, and a waiting request for control (SPEC-070).
    owner: { ...ownerOf(task) }, control_request: task.controlRequest ? { by: { ...task.controlRequest.by }, at: task.controlRequest.at } : null,
    // What the agent asked a person to do (`needs_you`, SPEC-134).
    needs_you: task.needs_you ? { ...task.needs_you } : null,
    steps: stepsSummary(task), archived: task.archived, account_label: task.account_label,
    resumes_at: task.status === 'waiting_capacity' ? task.resumes_at || null : null,
    // No Claude account usable (SPEC-246): it waits for one, not for capacity.
    waits_for_account: task.status === 'waiting_capacity' && Boolean(seededDb) && !claudeUsable(seededDb),
    // Chained tasks (TSK-012) and a held prompt (TSK-013).
    after: task.afterLink ? { ...task.afterLink } : null,
    next: (task.nextLinks || []).map((link) => ({ ...link })),
    held_prompt: task.heldPrompt ? { ...task.heldPrompt } : null,
    ...(task.mode ? { mode: task.mode } : {}),
    // How Auto chose the mode and workflow (TSK-015).
    auto: task.auto ? { ...task.auto } : null,
    // A supertask, and what a queued task waits for because of one (SPEC-071).
    supertask: Boolean(task.supertask),
    held_by: task.status === 'queued' ? [...(task.heldBy || [])] : [],
    // A prepared task's time (SPEC-105).
    start_at: task.status === 'prepared' ? task.start_at || null : null,
    // Background shells an idle agent left running (TSK-014).
    background_shells: task.status === 'running' ? task.background_shells ?? null : null,
    background_shells_since: task.status === 'running' && task.background_shells ? task.background_shells_since ?? null : null,
    // The idle agent waits in `yulius wait` (SPEC-085 STT-002).
    yulius_wait: task.status === 'running' && (task.activity === 'waiting' || task.activity === 'done') && task.yulius_wait === true,
    // A resting task that never ran, and the onboarding task a `pending` sandbox waits for (SPEC-153).
    not_started: task.status === 'closed' && task.neverRan === true,
    awaits_start_onboarding: task.status === 'closed' && task.neverRan === true && task.awaitsStart === true,
    // Why its live run is paused (SPEC-191 PAU-001): an update holds it, or a person paused it.
    pause: (task.status === 'running' || task.status === 'switching') && task.pause ? { ...task.pause } : null,
  };
}

/**
 * What a new queued task waits for because of a supertask (SPEC-071), by the server's rule: any
 * task waits for the project's pending supertasks (a supertask only for earlier ones), and a
 * supertask also for the project's working tasks. Done tasks hold nothing.
 */
function mockHeldBy(db, task) {
  const done = (t) => t.status === 'running' && t.activity === 'done';
  const pending = (t) => t.supertask && ['queued', 'starting', 'running', 'switching', 'waiting_capacity'].includes(t.status) && !done(t);
  const working = (t) => ['starting', 'running', 'switching', 'stopping'].includes(t.status) && !done(t);
  return db.tasks
    .filter((t) => t.sandbox_id === task.sandbox_id && t.id !== task.id && !t.archived)
    .filter((t) => (pending(t) && (!task.supertask || t.number < task.number)) || (task.supertask && working(t)))
    .map((t) => t.number)
    .sort((a, b) => a - b);
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

/** Waiting states in which a task that never started takes queued messages (SPEC-052). */
const WAITING = new Set(['queued', 'chained', 'prepared', 'waiting_capacity']);

/** Whether a task ever had a run: in the mock, every task past waiting in a queue or a chain. */
const hasStarted = (task) => task.started ?? !['queued', 'chained', 'prepared'].includes(task.status);

/** Whether the mock's person administers the instance: subscription emails are theirs (UI-022). */
const isAdmin = (db) => Boolean(db.me.user.is_instance_admin || db.me.user.is_superadmin);

function detail(db, task) {
  const live = LIVE.has(task.status);
  const ran = hasStarted(task) && (task.origin !== 'onboarding' || task.stepsList.some((s) => s.status === 'done'));
  return {
    ...summary(task),
    ...(isAdmin(db) && task.account_label && accountEmailOf(task.account_label) ? { account_email: accountEmailOf(task.account_label) } : {}),
    spec_path: task.stepsList.length && task.branch ? `docs/specs/2026-10-07-${task.branch.replace('feat/', '')}.md` : null,
    worktree: task.branch ? `/srv/yulius/sandboxes/${task.sandbox_id.slice(3)}/worktrees/${task.number}` : null,
    attempts: ran ? [{ seq: 1, harness: task.harness, account_label: task.account_label || 'Claude #1', ...(isAdmin(db) && accountEmailOf(task.account_label || 'Claude #1') ? { account_email: accountEmailOf(task.account_label || 'Claude #1') } : {}), launch_kind: 'new', state: live ? 'live' : 'exited', started_at: task.created_at, ended_at: live ? null : task.updated_at, exit_code: live || task.status === 'stopped_crashed' ? null : 0, exit_reason: live ? null : task.status === 'stopped_crashed' ? 'exited code=None signal=Some(6) limit=pids' : 'normal' }] : [],
    steps_list: task.stepsList,
    // Unknown usage stays null (task #6 never reported tokens).
    metrics: task.number === 6
      ? { wall_ms: Date.now() - Date.parse(task.created_at), active_ms: null, input_tokens: null, output_tokens: null, cache_read_tokens: null, cache_write_tokens: null, models: [], switches: 0, turns: null }
      : { wall_ms: Date.now() - Date.parse(task.created_at), active_ms: 41 * MIN, input_tokens: 182340, output_tokens: 23410, cache_read_tokens: 1204000, cache_write_tokens: 90210, models: [task.model], switches: task.number === 9 ? 1 : 0, turns: 17 },
    feedback: [],
    delivery: delivery(db, task),
    switches: task.number === 9 ? [{ at: iso(2 * MIN), reason: 'account exhausted', from: 'Claude Code (Claude #2)', to: 'Claude Code (Claude #4)' }] : [],
    request: task.prompt ?? task.title,
    started: hasStarted(task),
  };
}

/** A task's pull request as the Git overview shows it (`TaskBranch.pr`), or null. */
function pullOf(task) {
  const pr = task.branch && task.delivery && task.delivery.pr;
  return pr ? { ...pr, checks: pr.state === 'open' ? pr.checks : 'none' } : null;
}

/**
 * The sandbox counters: the sidebar's groups by its rule (status.js taskInfo and counter, the
 * server's `AttentionFacts::group`), so a card, a tab and the sidebar never disagree (#147, #170).
 */
function recount(db) {
  for (const sandbox of db.sandboxes) {
    const counts = { running: 0, needs_attention: 0, queued: 0 };
    for (const t of db.tasks.filter((task) => task.sandbox_id === sandbox.id && !task.archived)) {
      const task = summary(t);
      const kind = counter(task, taskInfo(task, null, pullOf(t)));
      if (kind === 'needs') counts.needs_attention += 1;
      else if (kind === 'working') counts.running += 1;
      else if (kind === 'queued') counts.queued += 1;
    }
    sandbox.counts = counts;
  }
}

function emit(db, event) {
  const data = JSON.stringify(event);
  for (const socket of db.eventSockets) socket.push(data);
}

/**
 * A sandbox's pull requests may have changed (a delivery stage moved, a CI run was queued or
 * ended): the server says so and watching browsers read the Git overview again (SPEC-108).
 */
function announcePulls(db, sandboxId) {
  emit(db, { type: 'pulls.updated', sandbox_id: sandboxId });
}

/**
 * A clock that moves only when a test says (`pullclock=manual`, SPEC-144): the store plans its pull
 * request reads on it, and `advanceClock(ms)` runs the timers that come due on the way, in order.
 */
function manualClock(start) {
  let now = start;
  let nextId = 1;
  const timers = new Map(); // id → { at, fn, every }
  const add = (fn, ms, every) => {
    const id = nextId;
    nextId += 1;
    timers.set(id, { at: now + (Number(ms) > 0 ? Number(ms) : 0), fn, every }); // like setTimeout: NaN or less is 0
    return id;
  };
  return {
    now: () => now,
    setTimeout: (fn, ms) => add(fn, ms, 0),
    clearTimeout: (id) => { timers.delete(id); },
    setInterval: (fn, ms) => add(fn, ms, Number(ms) > 1 ? Number(ms) : 1),
    clearInterval: (id) => { timers.delete(id); },
    advance(ms) {
      if (!(ms >= 0)) throw new Error(`advanceClock takes a time to move forward, not ${ms}`);
      const end = now + ms;
      for (;;) {
        let next = null;
        for (const [id, timer] of timers) if (timer.at <= end && (!next || timer.at < next[1].at)) next = [id, timer];
        if (!next) break;
        const [id, timer] = next;
        now = timer.at;
        if (timer.every) timer.at += timer.every;
        else timers.delete(id);
        try { timer.fn(); } catch (error) { now = end; throw error; }
      }
      now = end;
    },
  };
}

/** Sandbox changes: the server publishes `resync` (sandbox views differ per user). */
function resync(db) {
  emit(db, { type: 'resync' });
}

/**
 * The start of a task's working stretch after a change, as the server keeps it (SPEC-087 TIM-002):
 * it goes on while the task works, is on its way or is idle with background work that wakes it,
 * and ends when it waits for a person, is queued or has ended.
 */
function workingSince(task) {
  const idle = task.status === 'running' && (task.activity === 'waiting' || task.activity === 'done');
  const working = ['starting', 'switching', 'waiting_capacity'].includes(task.status)
    || (task.status === 'running' && (task.activity === 'working' || !task.activity))
    || (idle && task.background_shells > 0);
  if (working) return task.working_since || task.updated_at;
  return idle && task.background_shells == null ? task.working_since ?? null : null;
}

/**
 * Task change: `task.updated` only; sandbox counters are not pushed (clients refetch). It is dated
 * now, or `changes.updated_at`: the server dates to the second, so a change and a read answered
 * just before it may carry the same time (#506).
 */
function touch(db, task, changes = {}) {
  const [status, activity] = [task.status, task.activity];
  Object.assign(task, changes, { updated_at: changes.updated_at || new Date().toISOString() });
  // Out of `closed` it starts: it has had a run from then on, as on the server (SPEC-153).
  if (task.status !== 'closed') Object.assign(task, { neverRan: false, awaitsStart: false });
  if (task.status !== status || task.activity !== activity) task.state_since = task.updated_at;
  task.working_since = workingSince(task);
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

/** Who owns `task` (SPEC-070): its creator until a hand-over. */
function ownerOf(task) {
  return task.owner || task.created_by;
}

/** Changing a task needs write access and its ownership, like the server's `load_task`. */
function needTaskWrite(db, task) {
  need(sandboxOf(db, task.sandbox_id), 'write');
  const owner = ownerOf(task).login;
  if (owner !== db.me.user.login) fail(403, 'forbidden', `Task #${task.number} is @${owner}'s: it acts on GitHub as them. Ask them for control.`);
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

// The server's validate_new_password (SPEC-242 PWM-001): the repeat matches, 8 code points.
function checkNewPassword(body) {
  if (body.password !== body.password_confirm) fail(422, 'invalid', 'passwords do not match');
  if ([...(body.password || '')].length < 8) fail(422, 'invalid', 'the password needs at least 8 characters');
}
route('GET', '/api/setup/status', ({ db }) => ({ setup_required: db.setupRequired }));
route('POST', '/api/setup', ({ db, body }) => {
  if (!db.setupRequired) fail(403, 'forbidden', 'This instance is already set up.');
  if (body.token !== 'YUL-SETUP-1234') fail(403, 'forbidden', 'Wrong setup token. (Mock token: YUL-SETUP-1234)');
  if (!body.confirm_saved) fail(422, 'invalid', 'Confirm that the password is stored safely.');
  checkNewPassword(body);
  db.setupRequired = false;
  db.signedIn = true;
  db.me.user = { id: 'usr_root', login: 'superadmin', display_name: 'Superadmin', avatar_url: null, is_superadmin: true, is_instance_admin: true, github_linked: false };
  db.me.instance.github_configured = false;
  return json(201, db.setupRecoveryCode ? { ...db.me, recovery_code: db.setupRecoveryCode } : db.me);
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
// Whether agents can start (SPEC-246 CLA-001): an account signed in and `ok` or `limited`.
const claudeUsable = (db) => db.accounts.some((a) => a.status === 'ok' || a.status === 'limited');
// A person's start of an agent without one is refused, with where to connect one (CLA-002).
const needClaude = (db) => {
  if (!claudeUsable(db)) fail(409, 'claude_account_required', 'Connect a Claude account first: no Claude account is connected, so no agent can start. An instance administrator connects one in Subscriptions (/admin/accounts).');
};
route('GET', '/api/me', ({ db }) => ({ ...db.me, instance: { ...db.me.instance, claude_connected: claudeUsable(db) } }));
// Superadmin recovery (ACC-011): a code is set, three attempts left. A test can hold the answers
// (`__yuliusMock.holdRecovery = promise`; `recoveryWaiting` counts the reads waiting there), so a
// page that asked can be left before it answers (#581).
route('GET', '/api/auth/recovery', async ({ db }) => {
  if (db.holdRecovery) {
    db.recoveryWaiting = (db.recoveryWaiting || 0) + 1;
    try { await db.holdRecovery; } finally { db.recoveryWaiting -= 1; }
  }
  return clone(db.recovery);
});
route('POST', '/api/auth/recover', ({ db, body }) => {
  checkNewPassword(body || {});
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
// A test can hold the reads (`__yuliusMock.holdGithub = promise`) to act while one is on its way (SPEC-283).
route('GET', '/api/admin/github', async ({ db }) => {
  if (db.holdGithub) await db.holdGithub;
  return db.me.instance.github_configured
    ? { configured: true, app_slug: 'yulius-quartio-studio', owner_login: db.me.instance.owner_login, install_url: 'https://github.com/apps/yulius-quartio-studio/installations/new', installed: true }
    : { configured: false, app_slug: null, owner_login: null, install_url: null, installed: false };
});
route('POST', '/api/admin/github/manifest', ({ db, body }) => {
  if (db.me.instance.github_configured) fail(409, 'conflict', 'a GitHub App is already configured');
  const owner = (body && body.owner_login) || 'Quartio-Studio';
  return { post_url: `https://github.com/organizations/${owner}/settings/apps/new?state=mock`, manifest: { name: 'Yulius (mock)', url: ORIGIN } };
});
route('POST', '/api/device/approve', ({ body }) => {
  if (!/^[A-Z0-9]{4}-?[A-Z0-9]{4}$/.test(body.user_code || '')) fail(404, 'not_found', 'not found');
  return json(204);
});

// Personal API tokens (SPEC-247): one `yulius login` token to start with; a new token is shown once.
const apiTokens = (db) => {
  if (!db.apiTokens) db.apiTokens = [{ id: 'tok_cli', label: 'yulius CLI', scope: 'cli', created_at: new Date(Date.now() - 3 * 86400000).toISOString(), last_used_at: new Date(Date.now() - 3600000).toISOString() }];
  return db.apiTokens;
};
route('GET', '/api/me/tokens', ({ db }) => apiTokens(db));
route('POST', '/api/me/tokens', ({ db, body }) => {
  const label = String((body && body.label) || '').trim();
  if (!label || label.length > 80) fail(422, 'invalid', 'a label of 1-80 characters is required');
  if (!['operate', 'read'].includes(body.scope)) fail(422, 'invalid', 'scope must be operate or read');
  const tokens = apiTokens(db);
  const view = { id: `tok_${tokens.length + 1}`, label, scope: body.scope, created_at: new Date().toISOString(), last_used_at: null };
  tokens.unshift(view);
  const token = `yul_mock${String(tokens.length).padStart(4, '0')}`;
  return json(201, {
    token,
    view,
    mcp_url: `${ORIGIN}/mcp`,
    claude_command: `claude mcp add --transport http yulius ${ORIGIN}/mcp --header "Authorization: Bearer ${token}"`,
    cli_command: `yulius remote login ${ORIGIN}`,
  });
});
route('DELETE', `/api/me/tokens/${ID}`, ({ db, p }) => {
  const tokens = apiTokens(db);
  const at = tokens.findIndex((t) => t.id === p[0]);
  if (at < 0) fail(404, 'not_found', 'not found');
  tokens.splice(at, 1);
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
/** A playground as the server answers it (SPEC-099): no GitHub owner, its Git on the server. */
function addPlayground(db, rawName) {
  let slug = String(rawName || '').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40) || 'project';
  const base = slug;
  for (let n = 2; db.sandboxes.some((s) => s.slug === slug); n += 1) slug = `${base}-${n}`;
  db.nextId += 1;
  const sandbox = {
    id: `sb_${db.nextId}`, slug, name: slug, playground: true,
    repo: { owner: '', name: slug, url: '' },
    access: 'admin', default_branch: 'main', profile: clone(PRESETS.standard), onboarding: 'not_needed',
    port_base: 20000 + db.sandboxes.length * 20, port_count: 20, counts: { running: 0, needs_attention: 0, queued: 0 },
  };
  db.sandboxes.push(sandbox);
  recount(db);
  resync(db);
  return sandbox;
}

/** The entry names of a gzip tar (PAX `path` records applied), as the server reads a folder. */
async function tarNames(blob) {
  const data = new Uint8Array(await new Response(blob.stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());
  const text = (from, length) => new TextDecoder().decode(data.subarray(from, from + length)).replace(/\0.*$/s, '');
  const names = [];
  let pax = null;
  for (let at = 0; at + 512 <= data.length;) {
    if (data.subarray(at, at + 512).every((b) => b === 0)) break;
    const size = parseInt(text(at + 124, 12), 8) || 0;
    const type = text(at + 156, 1);
    const body = at + 512;
    if (type === 'x') {
      const record = text(body, size);
      const match = /\d+ path=([^\n]*)\n/.exec(record);
      pax = match ? match[1] : null;
    } else {
      names.push(pax || text(at, 100));
      pax = null;
    }
    at = body + Math.ceil(size / 512) * 512;
  }
  return names;
}

// A folder packed by the browser becomes a playground (SPEC-099); the mock keeps the names it got
// (`__yuliusMock.playgroundUploads`) so tests see what the browser sent.
route('POST', '/api/playgrounds', async ({ db, body, query }) => {
  if (!body.blob) fail(422, 'invalid', 'the request body is the packed folder');
  let names;
  try { names = await tarNames(body.blob); } catch { fail(422, 'invalid', 'unreadable folder archive: not a gzip tar'); }
  if (!names.some((name) => name.startsWith('files/'))) fail(422, 'invalid', 'the folder holds no files to start a playground with');
  (db.playgroundUploads = db.playgroundUploads || []).push({ name: query.get('name'), names });
  return json(201, addPlayground(db, query.get('name')));
});

/** The onboarding task's title and plan, as `onboarding.rs` creates them (`ONBOARDING_TITLE`, `onboarding_steps`; `ENVIRONMENT_TITLE`, `environment_steps`). */
const ONBOARDING_KINDS = {
  prepare: {
    title: 'Prepare this repository for Yulius',
    steps: [
      "Read the repository's instructions and conventions",
      'Install the toolchains and run the repository\'s own checks',
      'Reverse-engineer the product, architecture and decisions into the docs',
      'Map existing documents and commands in yulius.toml',
      'Move GitHub Actions into Yulius CI and delete them',
      'Add the missing documents and check the format',
    ],
  },
  environment: {
    title: "Set up this sandbox's environment",
    steps: [
      "Read the repository's instructions and conventions",
      'Install the toolchains and run the repository\'s own checks',
      'Record the working commands in yulius.toml [commands]',
    ],
  },
};
route('POST', '/api/sandboxes', ({ db, body }) => {
  let repoName;
  if (body.mode === 'playground') {
    if (!/[A-Za-z0-9]/.test(body.name || '')) fail(422, 'invalid', 'a playground needs a name');
    if (!['empty', 'web', 'python-data', 'rust-service'].includes(body.template)) fail(422, 'invalid', `unknown template ${body.template}`);
    return json(201, addPlayground(db, body.name));
  }
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
    fail(422, 'invalid', 'mode must be import, create or playground');
  }
  db.nextId += 1;
  const sandbox = {
    id: `sb_${db.nextId}`, slug: repoName.toLowerCase(), name: repoName,
    repo: { owner: 'Quartio-Studio', name: repoName, url: `https://github.com/Quartio-Studio/${repoName}` },
    access: 'admin', default_branch: 'main',
    profile: clone(PRESETS.standard),
    // An imported repository is onboarded; created projects start from a conforming template.
    // The real server starts a browser import's onboarding at once; the mock leaves it `pending`,
    // standing in for a repository added with `yulius onboard` (SPEC-051), so Start onboarding can
    // be tested. Like the server it has no task until onboarding starts (SPEC-097).
    onboarding: body.mode === 'import' ? 'pending' : 'not_needed',
    port_base: 20000 + db.sandboxes.length * 20, port_count: 20, counts: { running: 0, needs_attention: 0, queued: 0 },
  };
  db.sandboxes.push(sandbox);
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
/**
 * The server's two kinds of onboarding (onboarding.rs): `mobile-app` already follows the format, so
 * its onboarding only sets up the environment; any other repository gets the whole preparation.
 */
const onboardingKind = (sandbox) => ONBOARDING_KINDS[sandbox.repo.name === 'mobile-app' ? 'environment' : 'prepare'];
/** `GET /api/sandboxes/{sid}/onboarding/plan` (SPEC-097): its task's plan, or the one it would get. */
route('GET', `/api/sandboxes/${ID}/onboarding/plan`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  const task = db.tasks.find((t) => t.sandbox_id === sandbox.id && t.origin === 'onboarding');
  return task
    ? { title: task.title, steps: task.stepsList.map((step) => step.title), task_number: task.number }
    : { ...clone(onboardingKind(sandbox)), task_number: null };
});
/**
 * The preparation task of a sandbox, created resting when it has none yet: like the server, it is
 * created when onboarding starts or a new task waits after it, never at import (SPEC-097).
 */
function preparationTask(db, sandbox) {
  const found = db.tasks.find((t) => t.sandbox_id === sandbox.id && t.origin === 'onboarding');
  if (found) return found;
  const number = Math.max(0, ...db.tasks.filter((t) => t.sandbox_id === sandbox.id).map((t) => t.number)) + 1;
  const id = `tsk_${sandbox.id.slice(3)}_${number}`;
  const kind = onboardingKind(sandbox);
  const task = {
    id, sandbox_id: sandbox.id, number, title: kind.title, harness: 'claude', status: 'closed', activity: null,
    branch: null, origin: 'onboarding', created_by: { login: db.me.user.login }, created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    archived: false, account_label: null,
    stepsList: kind.steps.map((title, i) => ({ id: `${id}-s${i + 1}`, ordinal: i + 1, title, status: 'pending', note: '', updated_by: null, updated_at: null })),
    model: 'claude-opus-4', effort: 'high', delivery: null, prompt: 'Adapt this repository to the Yulius documentation format.',
    // Created resting: it has not run, and its sandbox waits for Start onboarding (SPEC-153).
    neverRan: true, awaitsStart: true,
  };
  db.tasks.push(task);
  recount(db);
  return task;
}
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
  // Its onboarding task, if it never ran, no longer waits for Start onboarding (SPEC-153).
  const preparation = db.tasks.find((t) => t.sandbox_id === sandbox.id && t.origin === 'onboarding');
  if (preparation) preparation.awaitsStart = false;
  releaseWaiting(db, sandbox);
  resync(db);
  return sandbox;
});
/** The onboarding task finished and the repository passes the format check: the sandbox is ready. */
function finishOnboarding(db, sandbox) {
  if (sandbox.onboarding !== 'preparing') return;
  const task = db.tasks.find((t) => t.sandbox_id === sandbox.id && t.origin === 'onboarding');
  if (task) {
    task.stepsList = task.stepsList.map((s) => ({ ...s, status: 'done', updated_by: `task #${task.number} (claude)`, updated_at: new Date().toISOString() }));
    touch(db, task, { status: 'closed', activity: null, account_label: null });
  }
  sandbox.onboarding = 'ready';
  releaseWaiting(db, sandbox);
  resync(db);
}
route('POST', `/api/sandboxes/${ID}/onboarding/approve`, async ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  if (sandbox.onboarding !== 'pending') fail(409, 'conflict', 'nothing awaits approval');
  needClaude(db);
  // In the server's order (`start_onboarding`): the task is created and its run started, and only
  // then is the sandbox `preparing`. A test can hold the start in between
  // (`__yuliusMock.holdApprove = promise`), while the task's events have already gone out.
  const task = preparationTask(db, sandbox);
  touch(db, task, { status: 'starting', branch: 'yulius/prepare-repository' });
  if (db.holdApprove) await db.holdApprove;
  // …and make the run's start fail then (`__yuliusMock.failApprove = true`), as `start_run` can.
  if (db.failApprove) fail(409, 'conflict', 'the onboarding task could not start');
  sandbox.onboarding = 'preparing';
  later(() => touch(db, task, { status: 'running', activity: 'working', account_label: 'Claude #1' }), 1200);
  // A test that sets `__yuliusMock.manualOnboarding` ends it itself (`finishOnboarding(slug)`).
  if (!db.manualOnboarding) later(() => finishOnboarding(db, sandbox), 9000);
  resync(db);
  return sandbox;
});
route('DELETE', `/api/sandboxes/${ID}`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'admin');
  db.sandboxes = db.sandboxes.filter((s) => s.id !== sandbox.id);
  // Removed with its files kept (`__yuliusMock.kept`, SPEC-276).
  (db.kept = db.kept || []).push(sandbox.slug);
  resync(db);
  return json(204);
});
// Purging a sandbox (SPEC-171, SPEC-276): removes it and deletes its files once nothing of it
// runs; the mock keeps the requests (`__yuliusMock.purges`), and `purgeWaitsFor` makes it wait.
route('POST', `/api/sandboxes/${ID}/purge`, ({ db, p }) => {
  // A removed sandbox with kept files: the superadmin deletes them (SPEC-276 RDA-012).
  const kept = (db.leftovers.items || []).find((i) => i.kind === 'removed-sandbox' && `removed-${i.sandbox}` === p[0]);
  if (kept) {
    if (!db.me.user.is_superadmin) fail(404, 'not_found', 'No such sandbox');
    db.leftovers.items = db.leftovers.items.filter((i) => i.sandbox !== kept.sandbox || !i.kind.startsWith('removed-'));
    (db.purges = db.purges || []).push(kept.sandbox);
    return { state: 'purged', waiting_for: [] };
  }
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'admin');
  db.sandboxes = db.sandboxes.filter((s) => s.id !== sandbox.id);
  (db.purges = db.purges || []).push(sandbox.slug);
  resync(db);
  const waiting = db.purgeWaitsFor || [];
  return { state: waiting.length ? 'waiting' : 'purged', waiting_for: waiting };
});

// Moving a playground to GitHub (SPEC-176): the mock keeps the moves (`__yuliusMock.moves`), and
// `moveRefusal` makes the server refuse with that sentence.
route('POST', `/api/sandboxes/${ID}/move-to-github`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'admin');
  if (!sandbox.playground) fail(409, 'conflict', 'only a playground moves to GitHub');
  if (!db.me.instance.github_configured) fail(409, 'conflict', 'connect this instance to GitHub first');
  const name = String((body && body.name) || '');
  if (!/^[A-Za-z0-9._-]{1,100}$/.test(name)) fail(422, 'invalid', `${name} is not a GitHub repository name`);
  if (db.moveRefusal) fail(409, 'conflict', db.moveRefusal);
  const owner = db.me.instance.owner_login;
  sandbox.playground = false;
  // The view names a GitHub sandbox by its repository; its slug stays.
  sandbox.name = name;
  sandbox.repo = { owner, name, url: `https://github.com/${owner}/${name}` };
  (db.moves = db.moves || []).push({ slug: sandbox.slug, name });
  resync(db);
  return sandbox;
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
  // A test can hold the creation (`__yuliusMock.holdCreate = promise`) to look at a starting task.
  if (db.holdCreate) await db.holdCreate;
  // Auto (TSK-015): the mock's stand-in for Jev answers after a moment, like the server.
  const choice = body.mode === 'auto' || body.workflow_id === 'auto' ? await mockAutoChoice(body, visible) : null;
  // A start rule (SPEC-017): the mock checks the shape the server checks, and anchors the chain
  // on the first task the rule names.
  if (body.supertask && (body.after_task_id || (body.start_rule && body.start_rule.trim()))) fail(422, 'invalid', 'a supertask starts after the running tasks: it cannot also run after a task or a rule');
  // A prepared task (SPEC-105) waits for Start now or its time, with the server's window.
  const prepared = body.prepared === true || Boolean(body.start_at);
  if (prepared && (body.after_task_id || body.supertask || (body.start_rule && body.start_rule.trim()))) fail(422, 'invalid', 'a prepared task starts when you decide or at its time: it cannot also run after a task, by a rule or as a supertask');
  const startAt = body.start_at ? mockStartAt(body.start_at) : null;
  if (!prepared) needClaude(db);
  let rule = null;
  if (body.start_rule && body.start_rule.trim()) {
    if (body.after_task_id) fail(422, 'invalid', 'give a task to run after or a start rule, not both');
    rule = body.start_rule.trim().replace(/\s+/g, ' ');
    if (!/^[#\w\s()!&|]+$/.test(rule) || !/#\d/.test(rule)) fail(422, 'invalid', 'the rule must name tasks like #12');
    const named = [...rule.matchAll(/#(\d+)/g)].map((m) => Number(m[1]));
    const missing = named.find((n) => !db.tasks.some((x) => x.sandbox_id === sandbox.id && x.number === n));
    if (missing) fail(422, 'invalid', `the rule names #${missing}, which does not exist in this sandbox`);
  }
  // While the sandbox is being prepared a new task waits after the preparation task (U-08),
  // created then, once the request is valid, when nobody has started onboarding yet (SPEC-097).
  const preparation = () => (sandbox.onboarding === 'pending' ? preparationTask(db, sandbox)
    : sandbox.onboarding === 'preparing' ? db.tasks.find((t) => t.sandbox_id === sandbox.id && t.origin === 'onboarding') : null);
  const before = rule ? db.tasks.find((x) => x.sandbox_id === sandbox.id && x.number === Number(rule.match(/#(\d+)/)[1])) : body.after_task_id ? db.tasks.find((x) => x.id === body.after_task_id && x.sandbox_id === sandbox.id) : null;
  // A waiting task may be run after, prepared ones too (SPEC-105 PRP-005).
  if (body.after_task_id && (!before || !(LIVE.has(before.status) || before.status === 'chained' || before.status === 'prepared'))) fail(422, 'invalid', 'the task to run after does not exist or has ended');
  // A prepared task waits for onboarding when it is started instead (SPEC-105): it creates no
  // preparation task, as on the server.
  const waitsFor = prepared ? null : before || preparation();
  const number = Math.max(0, ...db.tasks.filter((t) => t.sandbox_id === sandbox.id).map((t) => t.number)) + 1;
  const title = body.title || (parsePrompt(body.prompt).text.split('\n')[0] || 'Untitled task').trim().slice(0, 60);
  const wf = choice ? choice.workflow : db.workflows.find((w) => w.id === body.workflow_id);
  const id = `tsk_${sandbox.id.slice(3)}_${number}`;
  const task = {
    id, sandbox_id: sandbox.id, number, title, harness: 'claude', status: waitsFor ? 'chained' : 'queued', activity: null,
    branch: `feat/${title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 40) || `task-${number}`}`,
    origin: 'ui', created_by: { login: db.me.user.login, id: db.me.user.id }, created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    archived: false, account_label: null,
    stepsList: wf ? wf.steps.map((title2, i) => ({ id: `${id}-s${i + 1}`, ordinal: i + 1, title: title2, status: 'pending', note: '', updated_by: 'workflow', updated_at: new Date().toISOString() })) : [],
    model: body.model || 'claude-opus-4', effort: body.effort || 'default', prompt: body.prompt,
    delivery: null, mode: choice ? choice.mode : body.mode || 'goal', auto: choice ? choice.auto : null,
    supertask: body.supertask === true,
  };
  if (prepared) Object.assign(task, { status: 'prepared', start_at: startAt });
  db.tasks.push(task);
  recount(db);
  if (prepared) return json(201, summary(task));
  if (waitsFor) {
    chainAfter(task, waitsFor, rule);
    emit(db, { type: 'task.updated', task: summary(waitsFor) });
    return json(201, summary(task));
  }
  // A supertask holds the project's starts, and waits for its working tasks (SPEC-071).
  task.heldBy = mockHeldBy(db, task);
  if (task.heldBy.length) return json(201, summary(task));
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
// Work on "the whole product" is broad: a supertask is proposed unless it splits (SPEC-188).
const MOCK_BACKEND = /backend|api|server|database|pool|queue/i;
const MOCK_BROAD = /whole product/i;
route('POST', `/api/sandboxes/${ID}/intake/check`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  const text = String(body.prompt || '');
  const words = text.split(/\s+/).filter(Boolean).length;
  if (words < 3) return { verdict: 'one', checked: false, supertask: false };
  const lines = text.split('\n').filter((l) => /^\s*(\d+[.)]|[-*•])\s/.test(l));
  const several = words >= 40 && lines.some((l) => MOCK_BACKEND.test(l)) && lines.some((l) => !MOCK_BACKEND.test(l));
  return { verdict: several ? 'several' : 'one', checked: true, supertask: !several && MOCK_BROAD.test(text) };
});
route('POST', `/api/sandboxes/${ID}/intake/proposal`, async ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  // A test can hold the answer (`__yuliusMock.holdProposal = promise`) to look at the waiting card.
  await (db.holdProposal || delay(900));
  // Then refused, as the server refuses the proposal's Claude call without an account (SPEC-246).
  needClaude(db);
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
  if (task.status !== 'chained' && task.status !== 'prepared') fail(409, 'conflict', `the task is ${task.status}, not waiting in a chain or prepared`);
  needClaude(db);
  touch(db, task, { status: 'queued' });
  later(() => touch(db, task, { status: 'starting' }), 700);
  later(() => touch(db, task, { status: 'running', activity: 'working', account_label: 'Claude #1' }), 1800);
  return summary(task);
});
// A person pauses a running task and resumes it (SPEC-191 PER-001/002): its run is frozen.
route('POST', `/api/tasks/${ID}/pause`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (task.status !== 'running') fail(409, 'conflict', 'Only a running task can be paused.');
  if (!task.pause || task.pause.by !== 'person') {
    touch(db, task, { pause: { by: 'person', user: { login: db.me.user.login }, since: new Date().toISOString(), pending: false } });
  }
  return summary(task);
});
route('POST', `/api/tasks/${ID}/resume`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (task.pause && task.pause.by === 'person') touch(db, task, { pause: null });
  return summary(task);
});
route('PUT', `/api/tasks/${ID}/schedule`, ({ db, p, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (task.status !== 'prepared') fail(409, 'conflict', `the task is ${task.status}, not prepared`);
  touch(db, task, { start_at: body.start_at ? mockStartAt(body.start_at) : null });
  return summary(task);
});
// How a task that waits to start starts instead (SPEC-126): New task's choices and checks, a wait
// on itself refused; a started task keeps its start.
route('PUT', `/api/tasks/${ID}/start`, ({ db, p, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (task.status !== 'chained' && task.status !== 'prepared') fail(409, 'conflict', `the task has started (it is ${task.status}): only a task waiting to start changes how it starts`);
  const rule = body.start_rule && body.start_rule.trim() ? body.start_rule.trim().replace(/\s+/g, ' ') : null;
  if (body.after_task_id && rule) fail(422, 'invalid', 'give a task to run after or a start rule, not both');
  if (body.supertask && (body.after_task_id || rule)) fail(422, 'invalid', 'a supertask starts after the running tasks: it cannot also run after a task or a rule');
  const prepared = body.prepared === true || Boolean(body.start_at);
  if (prepared && (body.after_task_id || body.supertask || rule)) fail(422, 'invalid', 'a prepared task starts when you decide or at its time: it cannot also run after a task, by a rule or as a supertask');
  const startAt = body.start_at ? mockStartAt(body.start_at) : null;
  if (!prepared) needClaude(db);
  const tasks = db.tasks.filter((x) => x.sandbox_id === task.sandbox_id);
  const numbersOf = (text) => [...text.matchAll(/#(\d+)/g)].map((m) => Number(m[1]));
  let before = null;
  if (rule) {
    if (!/^[#\w\s()!&|]+$/.test(rule) || !/#\d/.test(rule)) fail(422, 'invalid', 'the rule must name tasks like #12');
    const missing = numbersOf(rule).find((n) => !tasks.some((x) => x.number === n));
    if (missing) fail(422, 'invalid', `the rule names #${missing}, which does not exist in this sandbox`);
    before = tasks.find((x) => x.number === numbersOf(rule)[0]);
  } else if (body.after_task_id) {
    before = tasks.find((x) => x.id === body.after_task_id);
    if (!before || !(LIVE.has(before.status) || before.status === 'chained' || before.status === 'prepared')) fail(422, 'invalid', 'the task to run after does not exist or has ended');
  }
  if (before) {
    // Through the predecessors and rules of the chained tasks it would wait for.
    const pending = rule ? numbersOf(rule).map((n) => tasks.find((x) => x.number === n)) : [before];
    const seen = new Set();
    while (pending.length) {
      const next = pending.pop();
      if (!next || seen.has(next.id)) continue;
      if (next.id === task.id) fail(422, 'invalid', `#${task.number} cannot wait for itself: what it would wait for already waits for it`);
      seen.add(next.id);
      const link = next.status === 'chained' && next.afterLink;
      if (link) pending.push(...(link.rule ? numbersOf(link.rule).map((n) => tasks.find((x) => x.number === n)) : [tasks.find((x) => x.id === link.task_id)]));
    }
  }
  // A chained task started now keeps its chain, as Start now does; otherwise the old
  // predecessor no longer lists it.
  const keeps = !before && !prepared && task.status === 'chained';
  const old = !keeps && task.afterLink && db.tasks.find((x) => x.id === task.afterLink.task_id);
  if (old) {
    old.nextLinks = (old.nextLinks || []).filter((l) => l.task_id !== task.id);
    emit(db, { type: 'task.updated', task: summary(old) });
  }
  if (!keeps) task.afterLink = null;
  if (before) {
    chainAfter(task, before, rule);
    touch(db, task, { status: 'chained', start_at: null, supertask: false });
    emit(db, { type: 'task.updated', task: summary(before) });
  } else if (prepared) {
    touch(db, task, { status: 'prepared', start_at: startAt, supertask: false });
  } else {
    touch(db, task, { status: 'queued', start_at: null, supertask: body.supertask === true });
    later(() => touch(db, task, { status: 'starting' }), 700);
    later(() => touch(db, task, { status: 'running', activity: 'working', account_label: 'Claude #1' }), 1800);
  }
  return summary(task);
});
/** A prepared task's start time (SPEC-105) checked like the server: at most a minute past, 366 days ahead. */
function mockStartAt(text) {
  const at = Date.parse(text);
  if (!Number.isFinite(at)) fail(422, 'invalid', '`start_at` must be an RFC 3339 time, e.g. 2026-10-15T09:00:00Z');
  if (at - Date.now() < -60e3) fail(422, 'invalid', 'that start time has passed: pick a later one, or start the task now');
  if (at - Date.now() > 366 * 86400e3) fail(422, 'invalid', 'a task can be scheduled at most 366 days ahead');
  return new Date(at).toISOString().replace(/\.\d{3}Z$/, 'Z');
}
route('POST', `/api/tasks/${ID}/queued-messages`, ({ db, p, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  const text = String(body.text || '').trim();
  if (!text) fail(422, 'invalid', 'the message is empty');
  if (hasStarted(task) || !WAITING.has(task.status)) fail(409, 'conflict', 'The task has started: type into its session instead');
  const at = `${new Date().toISOString().slice(0, 16).replace('T', ' ')} UTC`;
  touch(db, task, { prompt: `${task.prompt ?? task.title}\n\n**Added by ${db.me.user.login} before the task started (${at}):**\n\n${text}` });
  return { request: task.prompt };
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
// Reads of a task's detail, per task, so a test can wait for a view's read to land. A test can
// hold the next read (`__yuliusMock.holdDetail = promise`): it answers what the task was when
// asked, once released (`heldDetailAnswered`); `failNextDetail` fails the next one.
route('GET', `/api/tasks/${ID}`, async ({ db, p }) => {
  db.detailReads = db.detailReads || {};
  db.detailReads[p[0]] = (db.detailReads[p[0]] || 0) + 1;
  if (db.failNextDetail) { db.failNextDetail = false; fail(500, 'internal', 'the mock failed this read'); }
  const hold = db.holdDetail;
  if (!hold) return detail(db, taskOf(db, p[0]));
  db.holdDetail = null;
  const answer = clone(detail(db, taskOf(db, p[0])));
  await hold;
  db.heldDetailAnswered = true;
  return answer;
});
// The prompt a task started with (SPEC-043): its `@'path'` mentions and `@path` lines are its
// attached files, in order (SPEC-124), served only from the task's own attachment folders, named
// directly (SPEC-080).
const promptParts = (task) => parsePrompt(task.prompt ?? task.title);
function promptFileServed(task, path) {
  const base = `${MOCK_SANDBOXES}/${task.sandbox_id}/attachments`;
  const uploads = `${MOCK_UPLOADS}/${task.sandbox_id}/attachments`;
  const owned = [`${base}/drafts/usr_${task.created_by.login}`, `${base}/tasks/${task.id}`];
  // The task's own folder, where the upload route puts a file dropped on its terminal or queued
  // with a message (SPEC-168), and for a task created in this page its creator's composer uploads.
  owned.push(`${uploads}/tasks/${task.id}`);
  if (task.created_by.id) owned.push(`${uploads}/drafts/${task.created_by.id}`);
  const slash = path.lastIndexOf('/');
  const plain = path.split('/').slice(1).every((part) => part && part !== '.' && part !== '..');
  return plain && owned.includes(path.slice(0, slash)) && !MOCK_GONE.has(path);
}
route('GET', `/api/tasks/${ID}/prompt`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  const { text, paths, parts } = promptParts(task);
  return {
    text,
    attachments: paths.map((path) => ({ name: path.split('/').pop(), image: /\.(png|jpe?g|gif|webp)$/i.test(path), available: promptFileServed(task, path) })),
    parts: parts.map((part) => ('file' in part ? { attachment: part.file } : { text: part.text })),
  };
});
route('GET', `/api/tasks/${ID}/prompt/attachments/${ID}`, ({ db, p }) => {
  // Reads per `task/index`, so a test can tell a file was read once.
  db.promptFileReads = db.promptFileReads || {};
  db.promptFileReads[`${p[0]}/${p[1]}`] = (db.promptFileReads[`${p[0]}/${p[1]}`] || 0) + 1;
  const task = taskOf(db, p[0]);
  const path = promptParts(task).paths[Number(p[1])];
  if (!path || !promptFileServed(task, path)) fail(404, 'not_found', 'not found');
  if (/\.png$/i.test(path)) return new Response(Uint8Array.from(atob(MOCK_PNG), (c) => c.charCodeAt(0)), { status: 200, headers: { 'Content-Type': 'image/png' } });
  return new Response('Mock notes attached to the prompt.\n', { status: 200, headers: { 'Content-Type': 'application/octet-stream' } });
});
route('POST', `/api/tasks/${ID}/stop`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (task.status === 'chained' || task.status === 'prepared') { touch(db, task, { status: 'stopped_cancelled' }); return summary(task); }
  if (!LIVE.has(task.status)) fail(409, 'conflict', 'The task is not running.');
  touch(db, task, { status: 'stopping', activity: null });
  later(() => touch(db, task, { status: 'stopped_cancelled', account_label: null }), 1400);
  return summary(task);
});
const continueHandler = ({ db, p, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (LIVE.has(task.status)) fail(409, 'conflict', 'The task is already running.');
  needClaude(db);
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
route('DELETE', `/api/tasks/${ID}`, async ({ db, p }) => {
  // Tests count the requests and can hold the answer (`__yuliusMock.holdDelete = promise`).
  db.deleteRequests = (db.deleteRequests || 0) + 1;
  if (db.holdDelete) await db.holdDelete;
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  // A finished task is closed as it is deleted (SPEC-039); any other live task is stopped first.
  const finished = task.status === 'running' && taskInfo(summary(task), null, pullOf(task)).group === 'done';
  if (['starting', 'running', 'switching', 'stopping'].includes(task.status) && !finished) fail(409, 'conflict', 'stop the task before deleting it');
  db.tasks = db.tasks.filter((t) => t.id !== task.id);
  recount(db);
  emit(db, { type: 'task.removed', task_id: task.id });
  return json(204);
});
route('POST', `/api/tasks/${ID}/archive`, async ({ db, p }) => {
  // Tests can hold the answers (`__yuliusMock.holdArchive = promise`).
  if (db.holdArchive) await db.holdArchive;
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  // A finished task (Done by the sidebar's rule, the server's task_is_done) is closed as it is
  // archived (TSK-014); any other live task has to be stopped first.
  const finished = task.status === 'running' && taskInfo(summary(task), null, pullOf(task)).group === 'done';
  if (LIVE.has(task.status) && !finished) fail(409, 'conflict', 'Stop the task before archiving it.');
  touch(db, task, finished ? { archived: true, status: 'closed', activity: null } : { archived: true });
  return summary(task);
});
// Control of a task (SPEC-070): ask its owner, withdraw or decline, hand over.
route('POST', `/api/tasks/${ID}/control-request`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  need(sandboxOf(db, task.sandbox_id), 'write');
  const me = db.me.user.login;
  if (ownerOf(task).login === me) fail(409, 'conflict', 'you already own this task');
  const asked = task.controlRequest && task.controlRequest.by.login;
  if (asked && asked !== me) fail(409, 'conflict', `@${asked} already asked for control of this task; ask again once @${ownerOf(task).login} has answered`);
  if (!asked) touch(db, task, { controlRequest: { by: { login: me }, at: new Date().toISOString() } });
  return summary(task);
});
route('DELETE', `/api/tasks/${ID}/control-request`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  if (!task.controlRequest) fail(409, 'conflict', 'nobody asks for control of this task');
  if (task.controlRequest.by.login !== db.me.user.login) needTaskWrite(db, task);
  touch(db, task, { controlRequest: null });
  return summary(task);
});
route('POST', `/api/tasks/${ID}/control-request/accept`, ({ db, p }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  if (!task.controlRequest) fail(409, 'conflict', 'nobody asks for control of this task');
  touch(db, task, { owner: { ...task.controlRequest.by }, controlRequest: null });
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
route('GET', '/api/archived-tasks', ({ db }) => db.tasks.filter((t) => t.archived && db.sandboxes.some((s) => s.id === t.sandbox_id)).map(summary));

// ---- Git and delivery

route('GET', '/api/delivery/presets', () => [clone(PRESETS.standard), clone(PRESETS.product), clone(PRESETS.studio)]);
route('GET', `/api/sandboxes/${ID}/git`, async ({ db, p }) => {
  // A test can hold these reads, the page's first included (`window.__yuliusHoldGit = promise`,
  // set by an init script), to show a view drawn before its pull requests are known.
  if (window.__yuliusHoldGit) await window.__yuliusHoldGit;
  const sandbox = sandboxOf(db, p[0]);
  const tasks = db.tasks.filter((t) => t.sandbox_id === sandbox.id && !t.archived && t.branch);
  const pull = pullOf;
  const overview = {
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
  // `__yuliusMock.holdGitReads = true`: each read waits on its own until `releaseGit()`, answering
  // what it saw when it started, as a server read that began before a change (SPEC-108).
  if (db.holdGitReads) await new Promise((resolve) => db.gitHolds.push(resolve));
  return overview;
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
  later(() => { if (task.delivery && task.delivery.pr) { task.delivery.pr.checks = 'success'; announcePulls(db, task.sandbox_id); } }, 4000);
  announcePulls(db, task.sandbox_id);
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
  announcePulls(db, task.sandbox_id);
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
  announcePulls(db, sandbox.id);
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

function ciRun(id, reference, sha, state, minutesAgo, { failJob = null, skipJob = null, reason = null, by = 'yulius' } = {}) {
  const queued = Date.now() - minutesAgo * 60000;
  const started = queued + 4000;
  const ended = state === 'running' || state === 'queued' ? null : started + 3 * 60000 + 12000;
  const jobs = CI_JOBS.map((job, i) => {
    const failed = failJob === job.name;
    const jobState = skipJob === job.name ? 'skipped' : state === 'passed' ? 'passed' : state === 'failed' ? (failed ? 'failed' : 'passed') : state === 'running' ? (i === 0 ? 'passed' : 'running') : state;
    return {
      id: `${id}_j${i}`, name: job.name, command: job.command, matrix: job.matrix, state: jobState, reason: null,
      exit_code: jobState === 'passed' ? 0 : jobState === 'failed' ? 101 : jobState === 'skipped' ? 78 : null,
      duration_ms: jobState === 'passed' || jobState === 'failed' ? 120000 + i * 30000 : jobState === 'skipped' ? 2000 : null,
      output: jobState === 'queued' ? '' : jobState === 'running' ? `$ ${job.command}\n   Compiling yulius-app v0.1.0\n`
        : jobState === 'skipped' ? `$ ${job.command}\nSecurity scan not run: No Vidoc API key.\n` : ciOutput(job, !failed),
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
        // SPEC-162: a passed run whose "test (2/2)" exited with the skip code.
        ciRun('ci_s3', 'pr/221', '4be81c0d2a9f', 'passed', 22, { skipJob: 'test (2/2)' }),
        ciRun('ci_s2', 'pr/218', '0d7e5f3a1c2b', 'failed', 47, { failJob: 'test (2/2)' }),
        ciRun('ci_s1', 'pr/218', 'a1b2c3d4e5f6', 'cancelled', 63, { reason: 'superseded by 0d7e5f3' }),
      ],
      hygiene: {
        scanned_at: new Date(Date.now() - 2 * 3600000).toISOString(), notes: [],
        last_cleanup: null, next_cleanup_at: new Date(Date.now() + 4 * 3600000).toISOString(),
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

/** A running mock run finishes about 20 s after it was created (`__yuliusMock.ciRunMs` changes it). */
function progressCi(run, ms = 20000) {
  if (run.state === 'running' && Date.now() - run.created > ms) {
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
    gate: clone(ci.gate), gate_problem: ci.gate_problem, runs: ci.runs.map((r) => ciSummary(progressCi(r, db.ciRunMs))),
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
  return { ...progressCi(run, db.ciRunMs), created: undefined };
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
// ---- CD (deployment), SPEC-134: studio defines dev (a script with a secret) and production
// (promotion, then a script); product has dev empty and production not defined yet.
function cdRun(id, environment, branch, sha, state, minutesAgo, by, extra = {}) {
  const started = Date.now() - minutesAgo * 60000;
  const ended = state === 'running' ? null : started + 95000;
  return {
    run_id: id, environment, stage: extra.stage || (environment === 'dev' ? 'dev' : 'prod'), state, branch, commit: sha, requested_by: by,
    deploy: `scripts/cd/deploy.sh ${environment}`,
    verify: `scripts/cd/verify.sh ${environment}`,
    note: extra.note || null, reason: extra.reason || null, exit_code: state === 'passed' ? 0 : (state === 'failed' ? 3 : null),
    started_at: new Date(started).toISOString(), ended_at: ended ? new Date(ended).toISOString() : null, created: Date.now(),
    output: extra.output || `[yulius] deploying ${branch} at ${sha} to ${environment === 'prod' ? 'production' : environment}\n[yulius] deploy: scripts/cd/deploy.sh ${environment}\nrsync to app-dev… done\n[yulius] verify: scripts/cd/verify.sh ${environment}\nGET /health 200\n[yulius] done\n`,
  };
}

function seedCd() {
  return {
    sb_studio: {
      environments: [
        { name: 'dev', label: 'dev', stage: 'dev', state: 'ready', branch: 'quartio', head: '9f3c2a1b7e4d5f6a', problem: null, about: 'the team’s dev VM', deploy: 'scripts/cd/deploy.sh dev', verify: 'scripts/cd/verify.sh dev', promote: false, secrets: [{ name: 'DEV_SSH_KEY', state: 'ready' }, { name: 'DEV_HOST', state: 'not_allowed' }] },
        { name: 'prod', label: 'production', stage: 'prod', state: 'ready', branch: 'main', head: '4be81c0d2a9f1e2d', problem: null, about: 'customers', deploy: 'scripts/cd/deploy.sh prod', verify: 'scripts/cd/verify.sh prod', promote: true, promotes_from: '9f3c2a1b7e4d5f6a', secrets: [] },
      ],
      runs: [
        cdRun('cd_s2', 'dev', 'quartio', '7d1e0c9b8a7f6e5d', 'failed', 90, 'task #12', { reason: null, output: '[yulius] deploy: scripts/cd/deploy.sh dev\nssh: connect to host [secret:DEV_HOST] port 22: timed out\n[yulius] deploy failed with status 3; verify skipped\n' }),
        cdRun('cd_s1', 'prod', 'main', '4be81c0d2a9f1e2d', 'passed', 26 * 60, 'mkarp', { note: "promoted 'quartio' to 'main' at 4be81c0d2a9f1e2d" }),
      ],
    },
    sb_product: {
      environments: [
        { name: 'dev', label: 'dev', stage: 'dev', state: 'empty', branch: 'quartio', head: 'b7e9f01c2d3e4f5a', problem: null, about: 'no dev machine yet', deploy: null, verify: null, promote: false, secrets: [] },
        { name: 'prod', label: 'production', stage: 'prod', state: 'not_defined', branch: 'main', head: 'a0b1c2d3e4f5a6b7', problem: null, about: null, deploy: null, verify: null, promote: false, secrets: [] },
      ],
      runs: [],
    },
  };
}

function cdOf(db, sid) {
  sandboxOf(db, sid);
  if (!db.cd[sid]) {
    db.cd[sid] = { environments: ['dev', 'prod'].map((name) => ({ name, label: name === 'dev' ? 'dev' : 'production', stage: name, state: 'not_defined', branch: name === 'dev' ? 'dev' : 'main', head: null, problem: null, about: null, deploy: null, verify: null, promote: false, secrets: [] })), runs: [] };
  }
  return db.cd[sid];
}

/** A running mock deploy finishes about 15 s after it started (`__yuliusMock.cdRunMs` changes it). */
function progressCd(run, ms = 15000) {
  if (run.state === 'running' && Date.now() - run.created > ms) {
    run.state = 'passed';
    run.exit_code = 0;
    run.ended_at = new Date().toISOString();
  }
  return run;
}

const cdSummary = (run) => ({ ...run, created: undefined, output: undefined });

function cdOverview(db, sid) {
  const sandbox = sandboxOf(db, sid);
  const cd = cdOf(db, sid);
  const runs = cd.runs.map((r) => progressCd(r, db.cdRunMs));
  return {
    slug: sandbox.slug,
    environments: cd.environments.map((env) => {
      const mine = runs.filter((r) => r.environment === env.name);
      const deployed = mine.find((r) => r.state === 'passed');
      return { ...clone(env), deployed: deployed ? deployed.commit : null, last_run: mine.length ? cdSummary(mine[0]) : null };
    }),
    runs: runs.map(cdSummary),
    can_deploy_dev: (RANK[sandbox.access] || 0) >= RANK.write,
    can_deploy_prod: (RANK[sandbox.access] || 0) >= RANK.admin,
  };
}

route('GET', `/api/sandboxes/${ID}/cd`, ({ db, p }) => cdOverview(db, p[0]));
route('POST', `/api/sandboxes/${ID}/cd/${ID}/deploy`, async ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  const cd = cdOf(db, p[0]);
  const name = p[1] === 'production' ? 'prod' : p[1];
  const env = cd.environments.find((e) => e.name === name);
  if (!/^[a-z][a-z0-9-]{0,31}$/.test(name)) fail(422, 'invalid', "the environment is a [cd.<name>] of yulius.toml: dev, prod or another name of lowercase letters, digits and '-'");
  if (!env) fail(409, 'conflict', `CD ${name} is not defined: add [cd.${name}] to yulius.toml on the protected branch`);
  const prod = env.stage === 'prod';
  need(sandbox, prod ? 'admin' : 'write');
  db.lastCdDeploy = body || {}; // what the page sent (SPEC-137: `confirm_ms` of a production deploy)
  // A test can hold the answer (`__yuliusMock.holdCdDeploy = promise`): the checks and a promotion
  // take a while, and a refusal comes after them (SPEC-189).
  if (db.holdCdDeploy) { await db.holdCdDeploy; db.holdCdDeploy = null; }
  if (prod && String((body && body.confirm) || '').trim() !== sandbox.slug) fail(422, 'invalid', `type the project's slug '${sandbox.slug}' to confirm a production deploy`);
  if (env.state !== 'ready') fail(409, 'conflict', `CD ${env.label} is ${env.state === 'empty' ? 'empty' : 'not defined'}`);
  const used = env.secrets.filter((s) => s.state === 'agents_use').map((s) => s.name);
  const it = used.length === 1 ? 'it' : 'them';
  // The server names the others waiting for a person in the same refusal (SPEC-237 CAU-002).
  const others = env.secrets.filter((s) => s.state === 'not_allowed').map((s) => s.name);
  const also = others.length ? `. Also, a person who manages ${others.join(', ')} allows ${others.length === 1 ? 'it' : 'them'} for production deploys` : '';
  if (used.length) fail(409, 'conflict', `nothing ran: agents use ${used.join(', ')} (copies into .env files or hosts the proxy sends ${it} to), so production deploys never get ${it} and nobody can allow ${it} for them. Production names a separate secret of its own in [cd.${name}] secrets of yulius.toml, added on the Secrets page (${ORIGIN}/s/${encodeURIComponent(sandbox.slug)}/secrets?section=agents); or a person first revokes the .env copies and hosts of ${used.join(', ')}${also}`);
  if (env.secrets.some((s) => s.state !== 'ready')) fail(409, 'conflict', `nothing ran: a person who manages ${env.secrets.filter((s) => s.state !== 'ready').map((s) => s.name).join(', ')} allows using it outside the proxy on the Secrets page`);
  if (cd.runs.some((r) => r.environment === name && progressCd(r, db.cdRunMs).state === 'running')) fail(409, 'conflict', `a deploy to ${env.label} is running`);
  db.nextId += 1;
  const run = cdRun(`cd_${db.nextId}`, name, env.branch, env.head || '0000000', 'running', 0, db.me.user.login, { stage: env.stage, ...(prod && env.promote ? { note: `promoted the integration branch to '${env.branch}'` } : {}) });
  cd.runs.unshift(run);
  return cdSummary(run);
});
route('GET', `/api/sandboxes/${ID}/cd/runs/${ID}`, ({ db, p }) => {
  const run = cdOf(db, p[0]).runs.find((r) => r.run_id === p[1]);
  if (!run) fail(404, 'not_found', 'not found');
  return { ...progressCd(run, db.cdRunMs), created: undefined };
});
route('POST', `/api/sandboxes/${ID}/cd/runs/${ID}/cancel`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  const run = cdOf(db, p[0]).runs.find((r) => r.run_id === p[1]);
  if (!run) fail(404, 'not_found', 'not found');
  need(sandbox, run.stage === 'prod' ? 'admin' : 'write');
  if (progressCd(run, db.cdRunMs).state !== 'running') fail(409, 'conflict', 'the run already ended');
  run.state = 'cancelled';
  run.reason = `cancelled by ${db.me.user.login}`;
  run.ended_at = new Date().toISOString();
  return cdSummary(run);
});

route('GET', `/api/sandboxes/${ID}/hygiene`, ({ db, p }) => clone(ciOf(db, p[0]).hygiene));
route('POST', `/api/sandboxes/${ID}/hygiene/scan`, ({ db, p }) => {
  need(sandboxOf(db, p[0]), 'write');
  const hygiene = ciOf(db, p[0]).hygiene;
  hygiene.scanned_at = new Date().toISOString();
  return clone(hygiene);
});
route('POST', `/api/sandboxes/${ID}/hygiene/cleanup`, async ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  // A test can hold the answer (`__yuliusMock.holdHygieneCleanup = promise`): the scans, GitHub's
  // deletions and the refetch take a while (SPEC-269), and make it fail (`failHygieneCleanup`).
  if (db.holdHygieneCleanup) { await db.holdHygieneCleanup; db.holdHygieneCleanup = null; }
  if (db.failHygieneCleanup) fail(502, 'upstream', db.failHygieneCleanup);
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
  hygiene.last_cleanup = { at: hygiene.scanned_at, by: db.me.user.login, automatic: false, removed, skipped };
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
  const known = query.get('known');
  if (known !== null && !/^[0-9a-f]{64}$/.test(known)) fail(422, 'invalid', 'known is 64 lowercase hex digits');
  if (known === mockSha(content)) return json(204);
  if (tree.binary && tree.binary.has(path)) return { path, content: null, binary: true, too_large: false, size: content.length, sha256: mockSha(content) };
  if (tree.tooLarge && tree.tooLarge.has(path)) return { path, content: null, binary: false, too_large: true, size: content.length, sha256: mockSha(content) };
  return { path, content, binary: false, too_large: false, size: content.length, sha256: mockSha(content) };
});
route('PUT', `/api/sandboxes/${ID}/files/content`, async ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  const tree = repoTree(db, p[0], body.root || 'sandbox');
  const path = repoPath(body.path);
  if (typeof body.content !== 'string') fail(422, 'invalid', 'content is required');
  if (!isDir(tree, parentOf(path))) fail(404, 'not_found', 'the folder does not exist');
  // `holdSave = promise`: the save waits there, the file still as it was (SPEC-181).
  if (db.holdSave) await db.holdSave;
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
  return json(201, { path: `${MOCK_UPLOADS}/${sid}/attachments/${folder}/${name}`, name, size: body.size, sha256: mockSha(body.text || ''), replaced: false });
}
route('POST', `/api/tasks/${ID}/attachments`, async ({ db, p, query, body }) => {
  const task = taskOf(db, p[0]);
  needTaskWrite(db, task);
  // A test can hold the answer (`__yuliusMock.holdUpload = promise`), as for the composer's uploads.
  if (db.holdUpload) await db.holdUpload;
  return storeAttachment(db, task.sandbox_id, `tasks/${task.id}`, query, body);
});
route('POST', `/api/sandboxes/${ID}/attachments`, async ({ db, p, query, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  // A test can hold the answer (`__yuliusMock.holdUpload = promise`) to leave the composer meanwhile,
  // and make it fail then (`__yuliusMock.failUpload = true`).
  if (db.holdUpload) await db.holdUpload;
  if (db.failUpload) fail(500, 'internal', 'The upload failed on purpose.');
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

// Shipped (default) automations (SPEC-042, SPEC-197): the catalog, matched to a sandbox's automations by name.
const AUTOMATION_TEMPLATES = [
  { id: 'docs-up-to-date', name: 'Keep the docs up to date', description: 'Every night, the documentation is brought in line with what changed in the code during the last day.', cron: '0 3 * * *', utc_offset_minutes: 120, prompt: 'Bring this project’s documentation in line with the code, for the changes made since the last run of this automation.', workflow_id: 'wf-docs' },
  { id: 'open-issues', name: 'Work through open issues', description: 'Every night, the core team’s open GitHub issues are ranked and the 5 most important actionable ones get a task each.', cron: '0 3 * * *', utc_offset_minutes: 120, prompt: 'Get the 5 most important actionable open GitHub issues of this repository solved, among those written by the core team or their agents that no running task owns.', workflow_id: null },
  { id: 'nightly-dev-deploy', name: 'Deploy dev every night', description: 'Every night, dev is deployed from the integration branch with the project’s CD and tested; a failure becomes an issue.', cron: '0 3 * * *', utc_offset_minutes: 120, prompt: 'Deploy dev with `cd_deploy` and test it.', workflow_id: null },
];
const sameName = (a, b) => a.trim().toLowerCase() === b.trim().toLowerCase();
route('GET', `/api/sandboxes/${ID}/automation-templates`, ({ db, p }) => AUTOMATION_TEMPLATES.map((t) => {
  const own = db.automations.find((a) => a.sandbox === p[0] && sameName(a.name, t.name));
  return { ...t, automation_id: own ? own.id : null };
}));
route('POST', `/api/sandboxes/${ID}/automation-templates/${ID}/enable`, ({ db, p }) => {
  need(sandboxOf(db, p[0]), 'write');
  const t = AUTOMATION_TEMPLATES.find((x) => x.id === p[1]);
  if (!t) fail(404, 'not_found', 'not found');
  let a = db.automations.find((x) => x.sandbox === p[0] && sameName(x.name, t.name));
  const created = !a;
  if (created) {
    db.nextId += 1;
    const { id: _id, description: _d, ...fields } = t;
    a = { id: `aut_${db.nextId}`, sandbox: p[0], ...fields, harness: 'claude', created_via: 'ui', last_run: null };
    db.automations.push(a);
  }
  Object.assign(a, { enabled: true, next_run_at: isoIn(5 * HOUR) });
  const { sandbox: _s, ...out } = a;
  return json(created ? 201 : 200, out);
});
/** The start choice of an automation body, checked like the server (SPEC-078 AUT-014, SPEC-155). */
function automationStart(db, sandbox, body) {
  const after = body.after_task_id || null;
  const rule = (body.start_rule || '').trim() || null;
  if (body.prepared && (body.supertask || after || rule)) fail(422, 'invalid', 'a prepared task starts when you decide or at its time: it cannot also run after a task, by a rule or as a supertask');
  if (body.supertask && (after || rule)) fail(422, 'invalid', 'a supertask starts after the running tasks: it cannot also run after a task or a rule');
  if (after && rule) fail(422, 'invalid', 'give a task to run after or a start rule, not both');
  const task = after ? db.tasks.find((t) => t.id === after && t.sandbox_id === sandbox) : null;
  if (after && !task) fail(422, 'invalid', 'the task to run after does not exist');
  // The server parses the full rule language; the mock knows the editor's sentences.
  let canonical = null;
  if (rule) {
    const words = rule.toLowerCase().replace(/\s+/g, ' ');
    if (!/^#\d+ (done|merged|ended)( (and|or) #\d+ (done|merged|ended))*$/.test(words)) fail(422, 'invalid', `the rule cannot be read: ${rule}`);
    for (const [, n] of words.matchAll(/#(\d+)/g)) {
      if (!db.tasks.some((t) => t.sandbox_id === sandbox && t.number === Number(n))) fail(422, 'invalid', `the rule names #${n}, which does not exist in this project`);
    }
    canonical = words;
  }
  return { supertask: Boolean(body.supertask), after_task_id: after, after_task_number: task ? task.number : null, start_rule: canonical, prepared: Boolean(body.prepared) };
}
route('GET', `/api/sandboxes/${ID}/automations`, ({ db, p }) => db.automations.filter((a) => a.sandbox === p[0]).map(({ sandbox: _s, ...a }) => a));
route('POST', `/api/sandboxes/${ID}/automations`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  if (Math.abs(Number(body.utc_offset_minutes) || 0) > 1080) fail(422, 'invalid', 'utc_offset_minutes must be within ±1080');
  db.nextId += 1;
  const a = { id: `aut_${db.nextId}`, sandbox: p[0], ...body, ...automationStart(db, p[0], body), created_via: 'ui', next_run_at: body.enabled ? isoIn(5 * HOUR) : null, last_run: null };
  db.automations.push(a);
  const { sandbox: _s, ...out } = a;
  return json(201, out);
});
route('PUT', `/api/automations/${ID}`, ({ db, p, body }) => {
  const a = db.automations.find((x) => x.id === p[0]);
  if (!a) fail(404, 'not_found', 'not found');
  // The stored choice sent back unchanged is kept unchecked, as the server does (SPEC-078).
  const same = Boolean(body.supertask) === Boolean(a.supertask) && (body.after_task_id || null) === (a.after_task_id || null) && (body.start_rule || null) === (a.start_rule || null) && Boolean(body.prepared) === Boolean(a.prepared);
  const start = same ? { supertask: Boolean(a.supertask), after_task_id: a.after_task_id || null, after_task_number: a.after_task_number ?? null, start_rule: a.start_rule || null, prepared: Boolean(a.prepared) } : automationStart(db, a.sandbox, body);
  Object.assign(a, body, start, { next_run_at: body.enabled ? isoIn(5 * HOUR) : null });
  const { sandbox: _s, ...out } = a;
  return out;
});
route('DELETE', `/api/automations/${ID}`, ({ db, p }) => { db.automations = db.automations.filter((a) => a.id !== p[0]); return json(204); });
// Run now: one run, also when disabled; 200 with the AutomationRun. A body is this run's start
// choice, checked like a save and not stored (SPEC-093); without one, the stored choice.
route('POST', `/api/automations/${ID}/run`, ({ db, p, body }) => {
  const a = db.automations.find((x) => x.id === p[0]);
  if (!a) fail(404, 'not_found', 'not found');
  need(sandboxOf(db, a.sandbox), 'write');
  const stored = { supertask: Boolean(a.supertask), after_task_id: a.after_task_id || null, start_rule: a.start_rule || null, prepared: Boolean(a.prepared) };
  const same = !body || (Boolean(body.supertask) === stored.supertask && (body.after_task_id || null) === stored.after_task_id && (body.start_rule || null) === stored.start_rule && Boolean(body.prepared) === stored.prepared);
  const start = same ? stored : automationStart(db, a.sandbox, body);
  const previous = a.last_run && a.last_run.task_id ? db.tasks.find((t) => t.id === a.last_run.task_id) : null;
  const run = previous && (LIVE.has(previous.status) || previous.status === 'chained' || previous.status === 'prepared')
    ? { scheduled_for: new Date().toISOString(), state: 'skipped_previous_active', task_id: null, error: null }
    : { scheduled_for: new Date().toISOString(), state: 'task_created', task_id: automationTask(db, a, start).id, error: null };
  a.last_run = run;
  emit(db, { type: 'automation.run', automation_id: a.id, state: run.state, task_id: run.task_id });
  return run;
});
/** The task a run of automation `a` creates, started as `start` says (SPEC-078 AUT-015). */
function automationTask(db, a, start) {
  const number = Math.max(0, ...db.tasks.filter((t) => t.sandbox_id === a.sandbox).map((t) => t.number)) + 1;
  const id = `tsk_${a.sandbox.slice(3)}_${number}`;
  const rule = start.start_rule;
  const before = rule ? db.tasks.find((t) => t.sandbox_id === a.sandbox && t.number === Number(rule.match(/#(\d+)/)[1]))
    : start.after_task_id ? db.tasks.find((t) => t.id === start.after_task_id) : null;
  const now = new Date().toISOString();
  const task = {
    id, sandbox_id: a.sandbox, number, title: a.name, harness: 'claude', status: before ? 'chained' : start.prepared ? 'prepared' : 'queued', activity: null,
    branch: `feat/${number}-automation`, origin: 'automation', created_by: { login: db.me.user.login }, created_at: now, updated_at: now,
    archived: false, account_label: null, stepsList: [], model: 'claude-opus-4', effort: 'default', prompt: a.prompt,
    delivery: null, mode: 'goal', auto: null, supertask: start.supertask,
  };
  db.tasks.push(task);
  if (before) chainAfter(task, before, rule);
  else if (!start.prepared) task.heldBy = mockHeldBy(db, task);
  recount(db);
  emit(db, { type: 'task.updated', task: summary(task) });
  return task;
}
route('GET', `/api/automations/${ID}/runs`, ({ db, p }) => {
  const a = db.automations.find((x) => x.id === p[0]);
  if (!a) fail(404, 'not_found', 'not found');
  return [0, 1, 2, 3, 4].map((i) => ({ scheduled_for: iso((i + 1) * DAY - 10 * HOUR), state: i === 2 ? 'skipped_previous_active' : i === 4 ? 'failed' : 'task_created', task_id: i === 2 || i === 4 ? null : 'tsk_studio_8', error: i === 4 ? 'the task could not be created: capacity reached' : null }));
});

// ------------------------------------------------------------------ secrets vault (VLT-001..009, SPEC-040)
// Synthetic values only. Reveals need a two-factor code within 10 minutes (the mock accepts the
// code below); setting two-factor up needs a sign-in or confirmation within 10 minutes (the mock
// session signed in when the page loaded; mock password below).

const REAUTH_MS = 10 * MIN;
const MOCK_SUPERADMIN_PASSWORD = 'correct horse battery';
const MOCK_TWO_FACTOR_CODE = '123456';
const MOCK_TWO_FACTOR_SEED = 'JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP';

function vaultOf(db) {
  if (!db.vault) {
    db.vault = {
      reauthAt: Date.now(),
      unlockAt: 0,
      twoFactor: 'missing',
      team: [{ id: 'ts_gmail', title: 'Shared Gmail', value: 'mock-not-a-real-password', notes: 'User name: team@example.test\nRecovery codes: mock', created_by: 'mkarp', updated_by: 'mkarp', updated_at: iso(3 * DAY), value_updated_at: iso(3 * DAY) }],
      agent: [{ id: 'as_stripe', sandbox: 'sb_studio', scope: 'sandbox', name: 'STRIPE_KEY', description: 'Stripe test-mode key for the checkout', hosts: ['api.stripe.com'], injection: 'bearer', header_name: null, username: null, value: 'mock-agent-value-1', created_by: 'mkarp', updated_at: iso(DAY), value_updated_at: iso(DAY), last_used_at: iso(2 * HOUR), requested_hosts: [], requested_by: null, env_files: 'asked', env_requested_by: '#111' },
        // SPEC-121: added with a name, a value and a description; an agent asked for its host.
        { id: 'as_vidoc', sandbox: 'sb_studio', scope: 'sandbox', name: 'VIDOC_API_KEY', description: 'API key for Vidoc', hosts: [], injection: 'none', header_name: null, username: null, value: 'mock-agent-value-2', created_by: 'mkarp', updated_at: iso(HOUR), value_updated_at: iso(HOUR), last_used_at: null, requested_hosts: ['api.vidoc.test'], requested_by: '#107', env_files: 'no', env_requested_by: null },
        // SPEC-204: a production secret (SPEC-134) an agent asked to send to a host and copy into
        // .env before it became one; for every sandbox, so a member sees it without managing it.
        { id: 'as_prodtoken', sandbox: null, scope: 'instance', name: 'PROD_API_TOKEN', description: 'Token production deploys use', hosts: [], injection: 'none', header_name: null, username: null, value: 'mock-agent-value-4', created_by: 'mkarp', updated_at: iso(DAY), value_updated_at: iso(DAY), last_used_at: null, requested_hosts: ['api.prod.test'], requested_by: '#112', env_files: 'asked', env_requested_by: '#112', cd_prod: true, on: ['sb_studio'] },
        // SPEC-245: a global secret no project has switched on yet.
        { id: 'as_argo', sandbox: null, scope: 'instance', name: 'ARGO_TOKEN', description: 'Pulls private packages from Argo', hosts: ['pkg.argo.test'], injection: 'bearer', header_name: null, username: null, value: 'mock-agent-value-5', created_by: 'mkarp', updated_at: iso(DAY), value_updated_at: iso(DAY), last_used_at: null, requested_hosts: [], requested_by: null, env_files: 'no', env_requested_by: null, on: [] },
        // SPEC-245 MAN-003: another person's personal secret, which nobody else ever sees.
        { id: 'as_annas', sandbox: null, scope: 'personal', owner: 'anna', name: 'ANNAS_TOKEN', description: 'Anna’s own token', hosts: ['api.anna.test'], injection: 'bearer', header_name: null, username: null, value: 'mock-agent-value-6', created_by: 'anna', updated_at: iso(DAY), value_updated_at: iso(DAY), last_used_at: null, requested_hosts: [], requested_by: null, env_files: 'no', env_requested_by: null, on: ['sb_studio'] },
        // SPEC-193 MLK-003: a key pasted before #591 was fixed, its line breaks turned into spaces.
        { id: 'as_olddeploy', sandbox: 'sb_studio', scope: 'sandbox', name: 'OLD_DEPLOY_KEY', description: 'Deploy key for the staging server', hosts: [], injection: 'none', header_name: null, username: null, value: '-----BEGIN MOCK OPENSSH PRIVATE KEY----- bW9jay1mbGF0LTAx bW9jay1mbGF0LTAy -----END MOCK OPENSSH PRIVATE KEY-----', created_by: 'mkarp', updated_at: iso(2 * DAY), value_updated_at: iso(2 * DAY), last_used_at: null, requested_hosts: [], requested_by: null, env_files: 'no', env_requested_by: null }],
      activity: [{ at: iso(HOUR), actor: 'mkarp', action: 'vault.team.reveal', secret_id: 'ts_gmail', detail: 'Shared Gmail (value)' }],
    };
  }
  return db.vault;
}

// The server's refusal of an agents' use of a production secret (vault/mod.rs, SPEC-134).
const PRODUCTION_SECRET = 'this is a production secret: production deploys get it and agents never do. Add a separate secret for agents';
const INJECTION_TEXT = { bearer: 'Authorization: Bearer <value>', basic: 'Authorization: Basic <username:value>', none: 'no header (agents send the placeholder)' };

function teamView(s) {
  return { id: s.id, title: s.title, scope: s.sandbox ? 'sandbox' : 'instance', has_value: Boolean(s.value), has_notes: Boolean(s.notes), created_by: s.created_by, updated_by: s.updated_by, updated_at: s.updated_at, value_updated_at: s.value_updated_at, rotate_due: false };
}

/** Whether `s` is shown to the caller at all: another person's personal secret never is (SPEC-245). */
function agentVisible(db, s) {
  return s.scope !== 'personal' || s.owner === db.me.user.login;
}

/** The agent secret `id`, as the caller may see it (another person's answers 404). */
function agentSecret(db, id) {
  const secret = vaultOf(db).agent.find((s) => s.id === id && agentVisible(db, s));
  return secret || fail(404, 'not_found', 'not found');
}

/** Whether `s` reaches `sandbox`'s runs: its own, or switched on there. */
function agentOn(s, sandbox) {
  return s.scope === 'sandbox' ? s.sandbox === sandbox.id : (s.on || []).includes(sandbox.id);
}

/** The scope of the secret of `s`'s name that wins in `sandbox` for the caller, when not `s` (project > personal > global). */
function agentShadow(db, s, sandbox) {
  if (!agentOn(s, sandbox)) return null;
  const rank = { sandbox: 0, personal: 1, instance: 2 };
  const winner = vaultOf(db).agent
    .filter((o) => o.name === s.name && agentVisible(db, o) && agentOn(o, sandbox))
    .sort((a, b) => rank[a.scope] - rank[b.scope])[0];
  return winner && winner !== s ? winner.scope : null;
}

function agentView(db, s, sandbox) {
  const admin = Boolean(db.me.user.is_instance_admin || db.me.user.is_superadmin);
  const manage = s.scope === 'instance' ? admin : s.scope === 'personal' ? s.owner === db.me.user.login : RANK[sandbox.access] >= RANK.write;
  return { id: s.id, name: s.name, description: s.description, scope: s.scope, enabled: agentOn(s, sandbox), shadowed_by: agentShadow(db, s, sandbox), hosts: s.hosts, requested_hosts: s.requested_hosts, requested_by: s.requested_by, env_files: s.env_files || 'no', env_requested_by: s.env_requested_by || null, cd_prod: Boolean(s.cd_prod), auto_allow: Boolean(s.auto_allow), injection: s.injection, header_name: s.header_name, injection_text: s.injection === 'header' ? `${s.header_name}: <value>` : INJECTION_TEXT[s.injection], placeholder: `yulius-secret-${s.name}`, has_username: Boolean(s.username), has_totp: false, created_by: s.created_by, updated_at: s.updated_at, value_updated_at: s.value_updated_at, last_used_at: s.last_used_at, rotate_due: false, flattened: manage && flattenedKey(s.value), can_manage: manage };
}

function reauthStatus(db) {
  const vault = vaultOf(db);
  const fresh = Date.now() - vault.unlockAt < REAUTH_MS;
  const methods = [];
  if (db.me.user.is_superadmin) methods.push('password');
  if (db.me.user.github_linked) methods.push('github');
  return { fresh, fresh_until: fresh ? new Date(vault.unlockAt + REAUTH_MS).toISOString() : null, two_factor: vault.twoFactor, identity_fresh: Date.now() - vault.reauthAt < REAUTH_MS, methods };
}

function needFresh(db) {
  const status = reauthStatus(db);
  if (status.fresh) return null;
  return json(403, { error: { code: 'reauth_required', message: 'Enter your two-factor code to unlock the secrets for 10 minutes.' }, reauth: status });
}

/** The fields asked of a reveal (`value`, `notes`) that are stored. */
function revealed(body, value, notes) {
  const fields = body.fields || ['value', 'notes'];
  return { value: fields.includes('value') ? value || null : null, notes: fields.includes('notes') ? notes || null : null };
}

function audit(db, action, id, detail) {
  vaultOf(db).activity.unshift({ at: new Date().toISOString(), actor: db.me.user.login, action, secret_id: id, detail });
}

/** A team secret the person may see: a sandbox's one only through that sandbox (SPEC-216). */
function teamSecret(db, id, change = false) {
  const secret = vaultOf(db).team.find((s) => s.id === id) || fail(404, 'not_found', 'not found');
  if (secret.sandbox) {
    const sandbox = sandboxOf(db, secret.sandbox);
    need(sandbox, change ? 'write' : 'read');
  }
  return secret;
}

function createTeam(db, body, sandbox) {
  if (!String(body.title || '').trim()) fail(422, 'invalid', 'a title is required');
  for (const key of Object.keys(body)) if (!['title', 'value', 'notes', 'scope'].includes(key)) fail(422, 'invalid', `unknown field ${key}`);
  const scope = body.scope || (sandbox ? 'sandbox' : 'instance');
  if (!['sandbox', 'instance'].includes(scope) || (scope === 'sandbox' && !sandbox)) fail(422, 'invalid', 'scope must be sandbox or instance');
  if (scope === 'sandbox') need(sandbox, 'write');
  const secret = { id: `ts_${db.nextId++}`, sandbox: scope === 'sandbox' ? sandbox.id : null, title: body.title.trim(), value: body.value || null, notes: body.notes || null, created_by: db.me.user.login, updated_by: db.me.user.login, updated_at: new Date().toISOString(), value_updated_at: new Date().toISOString() };
  vaultOf(db).team.push(secret);
  audit(db, 'vault.team.create', secret.id, `${secret.title} (${secret.sandbox ? 'this sandbox' : 'team-wide'})`);
  return json(201, teamView(secret));
}

const AGENT_NAME_RE = /^[A-Z][A-Z0-9_]{0,63}$/;
/** A key whose line breaks were lost: `dotenv::flattened_key` (SPEC-193 MLK-002). */
function flattenedKey(value) {
  const match = /^-----BEGIN ([^-]+)-----(.*)-----END ([^-]+)-----$/s.exec(String(value || '').trim());
  return Boolean(match) && !value.includes('\\n') && match[2].trim() !== '';
}

function validateAgent(body, creating) {
  if (body.name !== undefined && !AGENT_NAME_RE.test(body.name)) fail(422, 'invalid', `invalid name "${body.name}"`);
  if (body.hosts !== undefined && (!Array.isArray(body.hosts) || body.hosts.length > 16)) fail(422, 'invalid', 'at most 16 hosts');
  if ((creating || body.value !== undefined) && [...(body.value || '')].length < 8) fail(422, 'invalid', 'an agent secret needs at least 8 characters');
  if (body.value !== undefined && flattenedKey(body.value)) fail(422, 'invalid', "this key's line breaks were lost (a one-line field turns them into spaces): paste it into the Secrets page's Value field, which keeps them as \\n, or write each line break as \\n");
}

route('GET', '/api/team-secrets', ({ db }) => vaultOf(db).team.filter((s) => !s.sandbox).map(teamView));
route('POST', '/api/team-secrets', ({ db, body }) => createTeam(db, body, null));
route('GET', `/api/sandboxes/${ID}/team-secrets`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  return vaultOf(db).team.filter((s) => !s.sandbox || s.sandbox === sandbox.id).map(teamView);
});
route('POST', `/api/sandboxes/${ID}/team-secrets`, ({ db, p, body }) => createTeam(db, body, sandboxOf(db, p[0])));
route('PATCH', `/api/team-secrets/${ID}`, ({ db, p, body }) => {
  const secret = teamSecret(db, p[0], true);
  for (const key of ['title', 'value', 'notes']) if (body[key] !== undefined) secret[key] = body[key] || (key === 'title' ? secret.title : null);
  secret.updated_at = new Date().toISOString();
  audit(db, 'vault.team.update', secret.id, secret.title);
  return teamView(secret);
});
route('DELETE', `/api/team-secrets/${ID}`, ({ db, p }) => {
  const secret = teamSecret(db, p[0], true);
  vaultOf(db).team = vaultOf(db).team.filter((s) => s !== secret);
  audit(db, 'vault.team.delete', secret.id, secret.title);
  return json(204);
});
route('POST', `/api/team-secrets/${ID}/reveal`, ({ db, p, body }) => {
  const secret = teamSecret(db, p[0]);
  const refused = needFresh(db);
  if (refused) return refused;
  audit(db, body.purpose === 'copy' ? 'vault.team.copy' : 'vault.team.reveal', secret.id, secret.title);
  return revealed(body, secret.value, secret.notes);
});
route('GET', '/api/auth/reauth', ({ db }) => reauthStatus(db));
route('POST', '/api/auth/reauth', ({ db, body }) => {
  if (!db.me.user.is_superadmin) fail(403, 'forbidden', 'this account has no password: sign in with GitHub again to confirm');
  if (body.password !== MOCK_SUPERADMIN_PASSWORD) { audit(db, 'vault.reauth.failed', null, 'password'); fail(403, 'forbidden', `wrong password (mock: ${MOCK_SUPERADMIN_PASSWORD})`); }
  vaultOf(db).reauthAt = Date.now();
  audit(db, 'vault.reauth', null, 'password');
  return reauthStatus(db);
});
route('POST', '/api/auth/two-factor', ({ db }) => {
  const vault = vaultOf(db);
  if (vault.twoFactor === 'on') fail(409, 'conflict', 'two-factor is already set up');
  const status = reauthStatus(db);
  if (!status.identity_fresh) return json(403, { error: { code: 'reauth_required', message: 'Confirm it is you (password or GitHub sign-in) to set up two-factor.' }, reauth: status });
  vault.twoFactor = 'pending';
  audit(db, 'vault.2fa.setup', null, 'authenticator');
  return json(201, { secret: MOCK_TWO_FACTOR_SEED, uri: `otpauth://totp/Yulius:${encodeURIComponent(db.me.user.login)}?secret=${MOCK_TWO_FACTOR_SEED}&issuer=Yulius&algorithm=SHA1&digits=6&period=30` });
});
route('POST', '/api/auth/unlock', ({ db, body }) => {
  const vault = vaultOf(db);
  if (vault.twoFactor === 'missing') fail(422, 'invalid', 'set up two-factor first');
  if (body.code !== MOCK_TWO_FACTOR_CODE) { audit(db, 'vault.unlock.failed', null, 'two-factor code'); fail(403, 'forbidden', `wrong code (mock: ${MOCK_TWO_FACTOR_CODE})`); }
  audit(db, vault.twoFactor === 'pending' ? 'vault.2fa.confirm' : 'vault.unlock', null, 'two-factor code');
  vault.twoFactor = 'on';
  vault.unlockAt = Date.now();
  return reauthStatus(db);
});
route('GET', '/api/secrets/activity', ({ db }) => vaultOf(db).activity.slice(0, 100));
route('GET', `/api/sandboxes/${ID}/agent-secrets`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  return vaultOf(db).agent.filter((s) => (s.scope === 'sandbox' ? s.sandbox === sandbox.id : agentVisible(db, s))).map((s) => agentView(db, s, sandbox));
});
route('PUT', `/api/sandboxes/${ID}/agent-secrets/${ID}/enabled`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  const secret = agentSecret(db, p[1]);
  if (secret.scope === 'sandbox') fail(422, 'invalid', 'a project secret always applies to its project; only global and personal secrets are switched on or off');
  if (secret.scope === 'instance' && !(db.me.user.is_instance_admin || db.me.user.is_superadmin)) fail(403, 'forbidden', 'global secrets are managed by instance administrators');
  if (secret.scope === 'personal') need(sandbox, 'write');
  const on = new Set(secret.on || []);
  if (body.enabled !== on.has(sandbox.id)) {
    if (body.enabled) on.add(sandbox.id);
    else on.delete(sandbox.id);
    secret.on = [...on];
    audit(db, body.enabled ? 'vault.agent.enable' : 'vault.agent.disable', secret.id, `${secret.name} → ${sandbox.slug}`);
  }
  return agentView(db, secret, sandbox);
});
route('POST', `/api/sandboxes/${ID}/agent-secrets`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  const scope = body.scope || 'sandbox';
  if (!['sandbox', 'instance', 'personal'].includes(scope)) fail(422, 'invalid', 'scope must be sandbox, instance or personal');
  if (scope === 'instance' && !(db.me.user.is_instance_admin || db.me.user.is_superadmin)) fail(403, 'forbidden', 'global secrets are managed by instance administrators');
  if (scope !== 'instance') need(sandbox, 'write');
  validateAgent(body, true);
  const me = db.me.user.login;
  const sameScope = (s) => s.scope === scope && (scope === 'sandbox' ? s.sandbox === sandbox.id : scope === 'personal' ? s.owner === me : true);
  if (vaultOf(db).agent.some((s) => s.name === body.name && sameScope(s))) fail(409, 'conflict', `an agent secret named ${body.name} already exists in this scope`);
  const secret = { id: `as_${db.nextId++}`, sandbox: scope === 'sandbox' ? sandbox.id : null, scope, owner: scope === 'personal' ? me : undefined, on: scope === 'sandbox' ? undefined : [sandbox.id], name: body.name, description: body.description || '', hosts: body.hosts || [], injection: body.injection || 'none', header_name: body.header_name || null, username: body.username || null, value: body.value, created_by: db.me.user.login, updated_at: new Date().toISOString(), value_updated_at: new Date().toISOString(), last_used_at: null, requested_hosts: [], requested_by: null, auto_allow: body.auto_allow !== false };
  vaultOf(db).agent.push(secret);
  audit(db, 'vault.agent.create', secret.id, `${secret.name} → ${secret.hosts.join(', ') || 'no hosts yet'}`);
  return json(201, agentView(db, secret, sandbox));
});
route('PATCH', `/api/agent-secrets/${ID}`, ({ db, p, body }) => {
  const secret = agentSecret(db, p[0]);
  const sandbox = secret.sandbox ? sandboxOf(db, secret.sandbox) : db.sandboxes[0];
  if (secret.scope === 'sandbox') need(sandbox, 'write');
  validateAgent(body, false);
  if (body.username && (body.injection || secret.injection) !== 'basic') fail(422, 'invalid', 'a user name is only for Basic credentials: put a login\'s user name in the notes');
  // A production secret is never set up by agents (SPEC-216 SSA-004).
  if (body.auto_allow === true && secret.cd_prod) fail(409, 'conflict', PRODUCTION_SECRET);
  // Nor sent to a host; clearing the hosts of an older one passes (update_agent_secret, SPEC-308).
  if (body.hosts && body.hosts.length && secret.cd_prod) fail(409, 'conflict', PRODUCTION_SECRET);
  const sameHosts = (a, b) => a.length === b.length && a.every((x) => b.includes(String(x).toLowerCase()));
  if (body.expect_hosts && !sameHosts(body.expect_hosts, secret.hosts)) fail(409, 'conflict', `its hosts changed since you opened it (now ${secret.hosts.join(', ') || 'none'}): reload and edit again`);
  delete body.expect_hosts;
  for (const key of ['name', 'description', 'hosts', 'injection', 'header_name', 'username', 'value', 'auto_allow']) if (body[key] !== undefined) secret[key] = body[key];
  secret.updated_at = new Date().toISOString();
  audit(db, 'vault.agent.update', secret.id, secret.name);
  return agentView(db, secret, sandbox);
});
route('POST', `/api/agent-secrets/${ID}/reveal`, ({ db, p, body }) => {
  const secret = agentSecret(db, p[0]);
  const sandbox = secret.sandbox ? sandboxOf(db, secret.sandbox) : db.sandboxes[0];
  if (!agentView(db, secret, sandbox).can_manage) fail(403, 'forbidden', 'only people who manage this secret read it');
  const refused = needFresh(db);
  if (refused) return refused;
  audit(db, body.purpose === 'copy' ? 'vault.agent.copy' : 'vault.agent.reveal', secret.id, secret.name);
  return revealed(body, secret.value, [secret.username && `User name: ${secret.username}`, secret.description].filter(Boolean).join('\n\n'));
});
route('POST', `/api/agent-secrets/${ID}/request`, ({ db, p, body }) => {
  const secret = agentSecret(db, p[0]);
  const sandbox = secret.sandbox ? sandboxOf(db, secret.sandbox) : db.sandboxes[0];
  if (!agentView(db, secret, sandbox).can_manage) fail(403, 'forbidden', 'only people who manage this secret answer its requests');
  if (!secret.requested_hosts.length) fail(409, 'conflict', 'no hosts wait to be allowed for this secret');
  const asked = secret.requested_hosts;
  const seen = (body.hosts || []).map((h) => String(h).toLowerCase());
  if (seen.length !== asked.length || !asked.every((h) => seen.includes(h))) fail(409, 'conflict', `an agent changed the request meanwhile: reload to see the hosts that wait (${asked.join(', ')})`);
  // In the server's order (requests.rs, env_files.rs): a production secret right after the conflict.
  if (body.allow && secret.cd_prod) fail(409, 'conflict', PRODUCTION_SECRET);
  const hosts = [...new Set([...secret.hosts, ...asked])];
  if (body.allow && hosts.length > 16) fail(422, 'invalid', 'at most 16 hosts');
  if (body.allow) secret.hosts = hosts;
  secret.requested_hosts = [];
  secret.requested_by = null;
  audit(db, body.allow ? 'vault.agent.allow' : 'vault.agent.dismiss', secret.id, `${secret.name} → ${asked.join(', ')}`);
  return agentView(db, secret, sandbox);
});
route('POST', `/api/agent-secrets/${ID}/env`, ({ db, p, body }) => {
  const secret = agentSecret(db, p[0]);
  const sandbox = secret.sandbox ? sandboxOf(db, secret.sandbox) : db.sandboxes[0];
  if (!agentView(db, secret, sandbox).can_manage) fail(403, 'forbidden', 'only people who manage this secret answer its requests');
  const state = secret.env_files || 'no';
  if (body.env_files !== state || (body.env_requested_by || null) !== (secret.env_requested_by || null)) fail(409, 'conflict', 'the secret changed since you looked: reload and answer again');
  if (body.allow && secret.cd_prod) fail(409, 'conflict', PRODUCTION_SECRET);
  if (state === 'no') fail(409, 'conflict', 'no agent asks to copy this secret into .env files');
  if (state === 'allowed' && body.allow) return agentView(db, secret, sandbox);
  audit(db, body.allow ? 'vault.agent.env_allow' : state === 'asked' ? 'vault.agent.env_dismiss' : 'vault.agent.env_revoke', secret.id, `${secret.name} → .env files`);
  secret.env_files = body.allow ? 'allowed' : 'no';
  secret.env_requested_by = null;
  return agentView(db, secret, sandbox);
});
route('POST', `/api/agent-secrets/${ID}/cd-prod`, ({ db, p, body }) => {
  const secret = agentSecret(db, p[0]);
  const sandbox = secret.sandbox ? sandboxOf(db, secret.sandbox) : db.sandboxes[0];
  if (!agentView(db, secret, sandbox).can_manage) fail(403, 'forbidden', 'only people who manage this secret allow it for production deploys');
  if (body.allow && secret.auto_allow) fail(409, 'conflict', 'agents set this secret up, so production deploys already get it: turn that off in Edit first to keep it for production deploys only');
  // A production secret is exclusive (ADR-0109): not while agents can use it.
  if (body.allow && ((secret.env_files || 'no') === 'allowed' || secret.hosts.length)) fail(409, 'conflict', 'agents can use this secret (copies into .env files or hosts the proxy sends it to): revoke those, or add a separate secret for production deploys');
  if (Boolean(secret.cd_prod) !== Boolean(body.allow)) {
    audit(db, body.allow ? 'vault.agent.cd_prod_allow' : 'vault.agent.cd_prod_revoke', secret.id, `${secret.name} → production CD runs`);
    secret.cd_prod = Boolean(body.allow);
  }
  return agentView(db, secret, sandbox);
});
route('DELETE', `/api/agent-secrets/${ID}`, ({ db, p }) => {
  const secret = agentSecret(db, p[0]);
  if (secret.scope === 'sandbox') need(sandboxOf(db, secret.sandbox), 'write');
  vaultOf(db).agent = vaultOf(db).agent.filter((s) => s !== secret);
  audit(db, 'vault.agent.delete', secret.id, secret.name);
  return json(204);
});

// Project variables (SPEC-275): non-secret values in plain.
const RESERVED_VARIABLE = /^(PATH|HOME|USER|LOGNAME|SHELL|TERM|COLORTERM|LANG|LANGUAGE|PWD|OLDPWD|TMPDIR|TMP|TEMP|PORT|HOSTNAME|EDITOR|VISUAL|PAGER|IFS|PS1|PS2|PROMPT_COMMAND|ENV|BASH_ENV|CDPATH|HTTP_PROXY|HTTPS_PROXY|ALL_PROXY|NO_PROXY|FTP_PROXY|REQUESTS_CA_BUNDLE|CURL_CA_BUNDLE|DISABLE_AUTOUPDATER)$|^(YULIUS_|CLAUDE_|ANTHROPIC_|GIT_|SSL_|NODE_|NPM_CONFIG_|LD_|DYLD_|XDG_|LC_|BASH_|ZSH_)/;
const variablesOf = (db, sandbox) => {
  // sandbox-3 (read access) starts with one, for the reader's view.
  db.variables = db.variables || { sb_three: [{ name: 'APP_REGION', value: 'eu-west-1', description: 'Where the app runs', updated_by: 'anna-q', updated_at: new Date().toISOString() }] };
  db.variables[sandbox.id] = db.variables[sandbox.id] || [];
  return db.variables[sandbox.id];
};
route('GET', `/api/sandboxes/${ID}/variables`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  const variables = [...variablesOf(db, sandbox)].sort((a, b) => a.name.localeCompare(b.name));
  return { variables, can_manage: (RANK[sandbox.access] || 0) >= RANK.write };
});
route('PUT', `/api/sandboxes/${ID}/variables`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  const held = variablesOf(db, sandbox);
  const out = { created: [], changed: [], unchanged: [], existing: [] };
  const inputs = body.variables || [];
  // The server's checks (crates/yulius-app/src/variables.rs), all before anything is written.
  if (inputs.length > 200) fail(422, 'invalid', 'at most 200 variables at once');
  const seen = new Set();
  for (const input of inputs) {
    if (!/^[A-Z][A-Z0-9_]{0,63}$/.test(input.name || '') || RESERVED_VARIABLE.test(input.name)) fail(422, 'invalid', `invalid name ${JSON.stringify(input.name)}: capital letters, digits and _, starting with a letter, not a reserved name`);
    if (seen.has(input.name)) fail(422, 'invalid', `${input.name} is given twice`);
    seen.add(input.name);
    if (new TextEncoder().encode(input.value || '').length > 4096) fail(422, 'invalid', `${input.name}: the value is longer than 4096 bytes`);
    if (/[\u0000-\u001f\u007f]/.test(input.value || '')) fail(422, 'invalid', `${input.name}: a variable is one line, without control characters`);
    if ([...(input.description || '').trim()].length > 1000) fail(422, 'invalid', `${input.name}: the description is longer than 1000 characters`);
    if (vaultOf(db).agent.some((s) => s.name === input.name && (s.scope === 'sandbox' ? s.sandbox === sandbox.id : s.scope === 'instance' && (s.on || []).includes(sandbox.id)))) fail(409, 'conflict', `${input.name} is an agent secret of this project: a name is either a secret or a variable; nothing was set`);
  }
  if (held.length + inputs.filter((i) => !held.some((v) => v.name === i.name)).length > 200) fail(422, 'invalid', 'a project holds at most 200 variables; nothing was set');
  for (const input of inputs) {
    const now = new Date().toISOString();
    const before = held.find((v) => v.name === input.name);
    if (before && body.only_new) { out.existing.push(input.name); continue; }
    const description = input.description != null ? input.description.trim() : (before ? before.description : '');
    if (!before) {
      held.push({ name: input.name, value: input.value, description, updated_by: db.me.user.login, updated_at: now });
      out.created.push(input.name);
      audit(db, 'variable.create', sandbox.id, input.name);
    } else if (before.value !== input.value || before.description !== description) {
      const fields = [before.value !== input.value && 'value', before.description !== description && 'description'].filter(Boolean);
      Object.assign(before, { value: input.value, description, updated_by: db.me.user.login, updated_at: now });
      out.changed.push(input.name);
      audit(db, 'variable.update', sandbox.id, `${input.name} (${fields.join(', ')})`);
    } else out.unchanged.push(input.name);
  }
  return out;
});
route('DELETE', `/api/sandboxes/${ID}/variables/${ID}`, ({ db, p }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  const held = variablesOf(db, sandbox);
  const at = held.findIndex((v) => v.name === p[1]);
  if (at < 0) fail(404, 'not_found', 'not found');
  held.splice(at, 1);
  audit(db, 'variable.delete', sandbox.id, p[1]);
  return json(204);
});

route('GET', `/api/sandboxes/${ID}/mcp`, ({ db, p }) => db.mcp.filter((m) => m.scope === 'instance' || m.sandbox === p[0]).map(({ sandbox: _s, ...m }) => m));
route('POST', `/api/sandboxes/${ID}/mcp`, ({ db, p, body }) => {
  need(sandboxOf(db, p[0]), 'write');
  if (!/^[A-Za-z0-9_-]{1,64}$/.test(body.name || '') || body.name.toLowerCase() === 'yulius') fail(422, 'invalid', 'invalid or reserved server name');
  if (body.scope === 'instance' && !db.me.user.is_instance_admin) fail(403, 'forbidden', 'instance servers need an instance administrator');
  db.nextId += 1;
  const m = { id: `mcp_${db.nextId}`, sandbox: p[0], scope: body.scope === 'instance' ? 'instance' : 'sandbox', name: body.name, transport: body.transport, command: body.command || null, args: body.args || [], url: body.url || null, env_keys: Object.keys(body.env || {}), header_keys: Object.keys(body.headers || {}), enabled: body.enabled !== false, origin: 'person' };
  db.mcp.push(m);
  const { sandbox: _s, ...out } = m;
  return json(201, out);
});
route('PUT', `/api/mcp/${ID}`, ({ db, p, body }) => {
  const m = db.mcp.find((x) => x.id === p[0]);
  if (!m) fail(404, 'not_found', 'not found');
  if (m.scope === 'instance' && !db.me.user.is_instance_admin) fail(403, 'forbidden', 'instance servers need an instance administrator');
  // A .mcp.json server becomes a person's when what it runs changes (SPEC-218 MOR-003): a stdio
  // command or arguments with its environment replaced, or an HTTP URL.
  if (m.origin === 'mcp_json') {
    const stdio = body.transport === 'stdio';
    const changed = body.transport !== m.transport || (stdio
      ? (body.command ?? '').trim() !== (m.command ?? '') || JSON.stringify(body.args || []) !== JSON.stringify(m.args)
      : (body.url ?? null) !== m.url);
    if (changed && (!stdio || body.env || !m.env_keys.length)) {
      m.origin = 'person';
      if (!body.env) m.env_keys = []; // the file's environment does not outlive the take-over
    }
  }
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
  // Tests see which servers were tested (`__yuliusMock.mcpTests`).
  (db.mcpTests ||= []).push(m.id);
  if (m.origin === 'mcp_json' && m.transport === 'stdio') return { ok: false, tools: [], error: 'this server came from the repository’s .mcp.json, which agents can write, so the Test does not run its command on the Yulius host; check the whole command line, change it and enter its environment again to test it' };
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
// `&cockpit=gone` answers the first call with a Cockpit whose process is already gone (a reboot);
// a test can hold the answer (`__yuliusMock.holdCockpit = promise`) to act while it starts;
// `__yuliusMock.rebootCockpits()` ends running ones abruptly; `switchCockpits()` moves them to
// another subscription (SPEC-074).
route('POST', `/api/sandboxes/${ID}/cockpit`, async ({ db, p }) => {
  need(sandboxOf(db, p[0]), 'write');
  needClaude(db);
  const scenario = new URLSearchParams(location.search).get('cockpit');
  if (scenario === 'fail') fail(429, 'capacity', 'no subscription has capacity right now');
  if (db.cockpitFailsOnce) { db.cockpitFailsOnce = false; fail(429, 'capacity', 'no subscription has capacity right now'); }
  if (db.holdCockpit) await db.holdCockpit;
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

// ---- Notes (SPEC-041): one notepad per sandbox, one for the team and one per person
// (`me:<user id>`, SPEC-103); a save names the revision it
// started from and a stale one is refused (409). Kept in sessionStorage so a reload finds them, as
// it would on the server. Tests edit as a teammate (`editNote`), fail saves (`failNoteSaves`) and
// loads (`failNoteLoads`), hold loads (`holdNoteLoad`), lose a stored save's answer (`loseNoteResponse`) and see which requests
// were sent to outlive the page (`keepaliveRequests`).

const NOTES_KEY = 'yulius.mock.notes';

function notesOf(db) {
  if (!db.notes) {
    let saved = null;
    try { saved = JSON.parse(sessionStorage.getItem(NOTES_KEY) || 'null'); } catch { /* none */ }
    db.notes = saved && typeof saved === 'object' ? saved : {};
  }
  return db.notes;
}

function noteView(db, scope) {
  const note = notesOf(db)[scope];
  return {
    scope: scope === 'team' ? 'team' : scope.startsWith('me:') ? 'me' : 'sandbox',
    body: note ? note.body : '',
    revision: note ? note.revision : 0,
    updated_at: note ? note.updated_at : null,
    updated_by: note ? note.updated_by : null,
  };
}

/** Stores `body` as the next revision of `scope` and announces it unless `announce` is false. */
function putNote(db, scope, body, by, { announce = true } = {}) {
  const notes = notesOf(db);
  const revision = (notes[scope] ? notes[scope].revision : 0) + 1;
  notes[scope] = { body, revision, updated_by: by, updated_at: new Date().toISOString() };
  try { sessionStorage.setItem(NOTES_KEY, JSON.stringify(notes)); } catch { /* memory only */ }
  if (announce) {
    const sandboxId = scope.startsWith('sandbox:') ? scope.slice('sandbox:'.length) : null;
    // The mock has one person: their own notepad's events reach them (on the server, only them).
    emit(db, { type: 'note.updated', scope: noteView(db, scope).scope, sandbox_id: sandboxId, revision, updated_by: by });
  }
  return noteView(db, scope);
}

function saveNote(db, scope, body) {
  if (db.failNoteSaves) fail(503, 'unavailable', 'The mock server is unavailable on purpose');
  if (typeof body.body !== 'string' || !Number.isInteger(body.revision)) fail(422, 'invalid', 'body and revision are required');
  if (new TextEncoder().encode(body.body).length > 256 * 1024) fail(422, 'invalid', 'notes hold at most 256 KiB');
  if (noteView(db, scope).revision !== body.revision) fail(409, 'conflict', 'someone else changed these notes; load them again');
  // Like the server: the GitHub login, else the display name.
  const note = putNote(db, scope, body.body, db.me.user.github_linked ? db.me.user.login : db.me.user.display_name);
  // Stored, but the answer does not arrive (a timeout or a dropped connection).
  if (db.loseNoteResponse) fail(503, 'unavailable', 'The answer was lost on purpose');
  return note;
}

function loadNote(db, scope) {
  db.noteLoads = (db.noteLoads || 0) + 1;
  if (db.failNoteLoads) fail(503, 'unavailable', 'The mock server is unavailable on purpose');
  // A test can hold reads (`__yuliusMock.holdNoteLoad = promise`): they answer what is stored then.
  if (db.holdNoteLoad) return db.holdNoteLoad.then(() => noteView(db, scope));
  return noteView(db, scope);
}

route('GET', '/api/notes', ({ db }) => loadNote(db, 'team'));
route('PUT', '/api/notes', ({ db, body }) => saveNote(db, 'team', body));
route('GET', '/api/me/notes', ({ db }) => loadNote(db, `me:${db.me.user.id}`));
route('PUT', '/api/me/notes', ({ db, body }) => saveNote(db, `me:${db.me.user.id}`, body));
route('GET', `/api/sandboxes/${ID}/notes`, ({ db, p }) => loadNote(db, `sandbox:${sandboxOf(db, p[0]).id}`));
route('PUT', `/api/sandboxes/${ID}/notes`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  return saveNote(db, `sandbox:${sandbox.id}`, body);
});

// Presence (SPEC-112): the browser's own selection is recorded (`presenceSent`) and relayed like
// the server does (its own echo included, which the page leaves out); tests act as a teammate with
// `notePresence(event)`, refuse updates (`failPresence`) and lose events (`resyncNow()`).
function relayPresence(db, scope, sandboxId, body) {
  for (const key of Object.keys(body)) if (!['revision', 'selection', 'hello', 'client', 'seq'].includes(key)) fail(422, 'invalid', `unknown field ${key}`);
  if (body.client !== undefined && (typeof body.client !== 'string' || !/^[A-Za-z0-9]{0,32}$/.test(body.client))) fail(422, 'invalid', 'a client is at most 32 letters and digits');
  if (body.seq !== undefined && !(Number.isInteger(body.seq) && body.seq >= 0)) fail(422, 'invalid', 'seq is a number from 0');
  // `failPresence`: the server refuses it as too many (429).
  if (db.failPresence) fail(429, 'capacity', 'too many presence updates; slow down');
  const selection = body.selection;
  const offset = (n) => Number.isInteger(n) && n >= 0 && n <= 1 << 20;
  if (!Number.isInteger(body.revision) || selection === undefined
    || (selection !== null && (typeof selection !== 'object' || Object.keys(selection).some((k) => k !== 'anchor' && k !== 'head') || !offset(selection.anchor) || !offset(selection.head)))) {
    fail(422, 'invalid', 'revision and selection are required');
  }
  const client = body.client || '';
  const seq = body.seq || 0;
  (db.presenceSent = db.presenceSent || []).push({ scope, sandbox_id: sandboxId, revision: body.revision, selection, hello: Boolean(body.hello), client, seq });
  const user = db.me.user;
  emit(db, { type: 'note.presence', scope, sandbox_id: sandboxId, user_id: user.id, client, name: user.github_linked ? user.login : user.display_name, revision: body.revision, selection, hello: Boolean(body.hello), seq });
  return json(204);
}

route('PUT', '/api/notes/presence', ({ db, body }) => relayPresence(db, 'team', null, body));

// File presence (SPEC-127): recorded (`filePresenceSent`) and relayed like the server does, to this
// page and to the other mock pages of this origin (a teammate's window, `as=<login>`); tests act
// as a teammate with `filePresence(event)`.
route('PUT', `/api/sandboxes/${ID}/files/presence`, ({ db, p, body }) => {
  const sandbox = sandboxOf(db, p[0]);
  need(sandbox, 'write');
  for (const key of Object.keys(body)) if (!['root', 'path', 'sha256', 'selection', 'hello', 'client', 'seq'].includes(key)) fail(422, 'invalid', `unknown field ${key}`);
  if (body.client !== undefined && (typeof body.client !== 'string' || !/^[A-Za-z0-9]{0,32}$/.test(body.client))) fail(422, 'invalid', 'a client is at most 32 letters and digits');
  if (body.seq !== undefined && !(Number.isInteger(body.seq) && body.seq >= 0)) fail(422, 'invalid', 'seq is a number from 0');
  if (body.sha256 !== undefined && !/^([0-9a-f]{64})?$/.test(body.sha256)) fail(422, 'invalid', 'sha256 is 64 lowercase hex digits');
  if (db.failPresence) fail(429, 'capacity', 'too many presence updates; slow down');
  const path = repoPath(body.path);
  const selection = body.selection;
  const offset = (n) => Number.isInteger(n) && n >= 0 && n <= 2 << 20;
  if (!path || selection === undefined
    || (selection !== null && (typeof selection !== 'object' || Object.keys(selection).some((k) => k !== 'anchor' && k !== 'head') || !offset(selection.anchor) || !offset(selection.head)))) {
    fail(422, 'invalid', 'path and selection are required');
  }
  const root = body.root || 'sandbox';
  if (String(body.path).length > 4096) fail(422, 'invalid', 'a path is at most 4096 bytes');
  // Someone is in a file that is there; leaving one may come after it was deleted.
  if (selection !== null && !repoTree(db, sandbox.id, root).files.has(path)) fail(404, 'not_found', 'not found');
  const client = body.client || '';
  const sha256 = body.sha256 || '';
  const seq = body.seq || 0;
  (db.filePresenceSent = db.filePresenceSent || []).push({ sandbox_id: sandbox.id, root, path, sha256, selection, hello: Boolean(body.hello), client, seq });
  const user = db.me.user;
  const event = { type: 'file.presence', sandbox_id: sandbox.id, root, path, user_id: user.id, client, name: user.github_linked ? user.login : user.display_name, sha256, selection, hello: Boolean(body.hello), seq };
  emit(db, event);
  if (db.presenceChannel) db.presenceChannel.postMessage(event);
  // `holdPresence = true`: answers wait for `releasePresence()` (an update still on its way).
  if (db.holdPresence) return new Promise((resolve) => { (db.presenceHolds = db.presenceHolds || []).push(() => resolve(json(204))); });
  return json(204);
});
route('PUT', `/api/sandboxes/${ID}/notes/presence`, ({ db, p, body }) => relayPresence(db, 'sandbox', sandboxOf(db, p[0]).id, body));

// ---- Administration

const adminOnly = (db) => { if (!db.me.user.is_instance_admin && !db.me.user.is_superadmin) fail(403, 'forbidden', 'instance administrators only'); };
const superOnly = (db) => { if (!db.me.user.is_superadmin) fail(403, 'forbidden', 'superadmin only'); };
// Instance administrators read the people and their two-factor (SPEC-061); changing them is the superadmin's.
route('GET', '/api/admin/users', ({ db }) => { adminOnly(db); return db.users; });
// SPEC-061: an administrator resets another person's two-factor after their own code.
route('DELETE', `/api/users/${ID}/two-factor`, ({ db, p }) => {
  adminOnly(db);
  const locked = needFresh(db);
  if (locked) return locked;
  const user = db.users.find((u) => u.id === p[0]);
  if (!user) fail(404, 'not_found', 'not found');
  if (user.id === db.me.user.id) fail(403, 'forbidden', 'another administrator resets your own two-factor');
  if (user.two_factor !== 'missing') audit(db, 'vault.2fa.reset', null, user.github_login);
  user.two_factor = 'missing';
  return json(204);
});
route('POST', '/api/admin/users', ({ db, body }) => {
  superOnly(db);
  if (!/^[A-Za-z0-9-]{1,39}$/.test(body.github_login || '')) fail(422, 'invalid', 'not a valid GitHub login');
  if (db.users.some((u) => u.github_login.toLowerCase() === String(body.github_login).toLowerCase())) fail(409, 'conflict', 'Already admitted.');
  db.nextId += 1;
  const user = { id: `usr_${db.nextId}`, github_login: body.github_login, display_name: body.display_name || body.github_login, is_instance_admin: Boolean(body.is_instance_admin), disabled: false, github_linked: false, two_factor: 'missing' };
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
// Each subscription's tokens Yulius observed, with their cost at API list prices (SPEC-110).
route('GET', '/api/admin/accounts', ({ db }) => {
  adminOnly(db);
  const totals = (k, runs) => ({ runs, tasks: Math.max(1, runs - 1), ...poolSpend(k) });
  return db.accounts.map((a, i) => (a.status === 'pending_login' ? a : { ...a, tokens_24h: totals(0.3 / (i + 1), 2), tokens_7d: totals(2 / (i + 1), 9) }));
});
// 201 {account, login_id}: the account waits in `pending_login` until the login terminal finishes.
route('POST', '/api/admin/accounts', ({ db, body }) => {
  adminOnly(db);
  if (body.provider !== 'anthropic') fail(422, 'invalid', 'provider must be anthropic (Claude)');
  if (!body.label || body.label.length > 80) fail(422, 'invalid', 'label must be 1–80 characters');
  db.nextId += 1;
  const account = { id: `acc_${db.nextId}`, provider: body.provider, label: body.label, email: null, status: 'pending_login', usage: { primary: null, secondary: null, observed_at: null }, last_error: null, in_use_by: 0 };
  db.accounts.push(account);
  db.unsignedAccounts.add(account.id);
  return json(201, { account, login_id: startLogin(db, account.id) });
});
route('PATCH', `/api/admin/accounts/${ID}`, ({ db, p, body }) => {
  adminOnly(db);
  const account = db.accounts.find((a) => a.id === p[0]);
  if (!account) fail(404, 'not_found', 'not found');
  // Only the name: the pool has no priorities or switches (F 2026-10-10).
  if (Object.keys(body).some((key) => key !== 'label')) fail(422, 'invalid', 'unknown field; only `label` can change');
  if (typeof body.label !== 'string' || !(body.label.trim().length >= 1 && body.label.length <= 80)) fail(422, 'invalid', 'label must be 1–80 characters');
  // Tasks name their subscription by label here (the server joins by id): a rename follows.
  const previous = account.label;
  account.label = body.label.trim();
  for (const task of db.tasks) if (task.account_label === previous) task.account_label = account.label;
  emit(db, { type: 'account.updated', account });
  return account;
});
route('DELETE', `/api/admin/accounts/${ID}`, ({ db, p }) => {
  adminOnly(db);
  const account = db.accounts.find((a) => a.id === p[0]);
  if (!account) fail(404, 'not_found', 'not found');
  if (account.in_use_by > 0) fail(409, 'conflict', `${account.label} is used by ${account.in_use_by} live task(s)`);
  for (const session of db.logins.values()) if (session.accountId === account.id && !session.done) session.end('cancelled', null);
  db.accounts = db.accounts.filter((a) => a.id !== p[0]);
  return json(204);
});
// Refreshes the provider's usage windows; an account that must sign in again gets the provider's 502.
route('POST', `/api/admin/accounts/${ID}/usage`, ({ db, p }) => {
  adminOnly(db);
  const account = db.accounts.find((a) => a.id === p[0]);
  if (!account) fail(404, 'not_found', 'not found');
  // Like the server: an account that never signed in has no credential to ask with (409), and
  // its sign-in error stays.
  if (db.unsignedAccounts.has(account.id)) fail(409, 'conflict', 'the account is not signed in yet');
  if (account.status === 'relogin' || account.status === 'pending_login') fail(502, 'upstream', account.last_error || 'the provider refused the usage request; sign in again');
  account.usage = { ...account.usage, observed_at: new Date().toISOString() };
  account.last_error = null;
  return account;
});
route('POST', `/api/admin/accounts/${ID}/relogin`, ({ db, p }) => {
  adminOnly(db);
  const account = db.accounts.find((a) => a.id === p[0]);
  if (!account) fail(404, 'not_found', 'not found');
  return { account, login_id: startLogin(db, account.id) };
});
// Stops a running sign-in and says how it ended (SPEC-243 LGN-002); `loginCancels` records the
// cancelled ids.
route('POST', `/api/admin/accounts/login/${ID}/cancel`, ({ db, p }) => {
  adminOnly(db);
  const session = db.logins.get(p[0]);
  if (!session) fail(404, 'not_found', 'not found');
  if (!session.done) {
    db.loginCancels.push(p[0]);
    session.end('cancelled', null);
  }
  return { status: session.status };
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
    const count = (hashString(date + query.get('sandbox')) % 5) + base;
    // The cohort's runs and switches that day (SPEC-110): about 31 min of agent work a task.
    const vary = (hashString(`${date}w`) % 7) / 10 + 0.7;
    perDay.push({ date, count, active_ms: count ? Math.round(count * 31 * MIN * vary) : null, median_active_ms: count ? Math.round(26 * MIN * vary) : null,
      input: count ? Math.round(count * 340e3 * vary) : null, output: count ? Math.round(count * 43e3 * vary) : null, switches: Math.round(count * 0.15 * vary * 2) });
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
      by_status: { closed: Math.round(total * 0.62), stopped_cancelled: Math.round(total * 0.12), running: 3, done: 2, failed_spawn_error: 1, stopped_crashed: 2 },
      by_harness: { claude: total },
      by_origin: { ui: Math.round(total * 0.8), automation: Math.round(total * 0.15), import: Math.round(total * 0.05) },
    },
    tasks_per_day: perDay,
    duration_ms: { median: 47 * MIN, p90: 3.2 * HOUR, total_wall: total * 58 * MIN, total_active: total * 31 * MIN, median_active: 26 * MIN, p90_active: 1.4 * HOUR },
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
      ...db.accounts.filter((a) => a.status !== 'pending_login').map((a, i) => ({ account_id: a.id, account_label: a.label, ...(isAdmin(db) && a.email ? { account_email: a.email } : {}), ...tokenTotals(Math.round(runs / (i + 2)), Math.round(total / (i + 2)), n(14e6 / (i + 1)), n(1.8e6 / (i + 1)), null, null, 0) })),
      { account_id: 'acc_deleted', account_label: null, ...tokenTotals(2, 1, n(1e6), n(0.1e6), null, null, 0) },
    ],
    switches: { total: Math.round(total * 0.15) },
    feedback: { correct: Math.round(total * 0.3), incorrect: Math.round(total * 0.06) },
    truncated: false,
  };
});
// ---- Stages (SPEC-137): the Pipeline page, waterfalls and the Dashboard's overview.

// One stage row: subject, kind, name, job, environment, ended, failed, p50 and p90 minutes, and for
// CI jobs and CD runs what they used (SPEC-154): CPU p50/p90 minutes and peak memory p50/p90 MiB.
const STAGE_ROWS = [
  ['ci_run', 'run', 'run', null, null, 46, 5, 14, 23],
  ['ci_run', 'queue', 'queue', null, null, 49, 0, 1.5, 9],
  ['ci_run', 'queue', 'queue · waiting for a lane', null, null, 12, 0, 4, 11],
  ['ci_run', 'queue', 'queue · waiting for disk', null, null, 3, 0, 7, 12],
  ['ci_run', 'job', 'quality', null, null, 46, 3, 12, 19, [31, 52, 2048, 3584]],
  ['ci_run', 'stage', 'lint', 'quality', null, 44, 1, 2.5, 3.5],
  ['ci_run', 'stage', 'unit', 'quality', null, 43, 2, 6, 9],
  ['ci_run', 'job', 'e2e', null, null, 46, 2, 9, 13, [14, 20, 1536, 2304]],
  ['task', 'state', 'queued', null, null, 31, 0, 6, 48],
  ['task', 'state', 'needs_you', null, null, 18, 0, 22, 190],
  ['task', 'step', 'step', null, null, 212, 4, 18, 55],
  ['task', 'delivery', 'pr_open', null, null, 27, 0, 52, 240],
  ['task', 'wait', 'merge_armed', null, null, 24, 2, 19, 41],
  ['cd_run', 'run', 'run', null, 'dev', 14, 3, 3.2, 6, [2.5, 4, 512, 900]],
  ['cd_run', 'deploy', 'deploy', null, 'dev', 14, 2, 2.4, 4.5],
  ['cd_run', 'verify', 'verify', null, 'dev', 12, 1, 0.4, 0.9],
  ['cd_run', 'run', 'run', null, 'prod', 4, 0, 5, 7],
  ['cd_run', 'wait', 'confirmation', null, 'prod', 4, 0, 0.3, 0.8],
];

function stageRows(query) {
  const want = (key, value) => !query.get(key) || query.get(key) === value;
  return STAGE_ROWS
    .filter(([subject, , , job, env]) => want('subject', subject) && want('environment', env) && want('job', job))
    .map(([subject, kind, name, job, env, count, failed, p50, p90, used]) => ({
      subject_kind: subject, kind, name, job, environment: env, count, failed, running: name === 'run' && subject === 'ci_run' ? 1 : 0,
      backfilled: subject === 'ci_run' ? 6 : 0, p50_ms: Math.round(p50 * MIN), p90_ms: Math.round(p90 * MIN), max_ms: Math.round(p90 * 1.6 * MIN), total_ms: Math.round(count * p50 * 1.2 * MIN),
      ...(used ? { cpu_p50_ms: Math.round(used[0] * MIN), cpu_p90_ms: Math.round(used[1] * MIN), memory_peak_p50_mb: used[2], memory_peak_p90_mb: used[3] } : {}),
    }))
    .sort((a, b) => b.total_ms - a.total_ms);
}

function stageDays(query) {
  const to = Date.parse(query.get('to') || new Date().toISOString().slice(0, 10));
  const from = Date.parse(query.get('from') || new Date(to - 29 * DAY).toISOString().slice(0, 10));
  const days = [];
  for (let at = from; at <= to; at += DAY) days.push(new Date(at).toISOString().slice(0, 10));
  return days;
}

route('GET', `/api/sandboxes/${ID}/stages/summary`, ({ db, p, query }) => {
  if (!db.sandboxes.some((s) => s.id === p[0])) fail(404, 'not_found', 'not found');
  const stages = stageRows(query);
  const [subject, kind, ...rest] = (query.get('trend') || 'ci_run:run:run').split(':');
  const name = rest.join(':');
  const row = STAGE_ROWS.find((r) => r[0] === subject && r[1] === kind && r[2] === name) || STAGE_ROWS[0];
  const days = stageDays(query).map((date) => {
    const weekday = new Date(date).getUTCDay();
    if (weekday === 0 || weekday === 6) return { date, count: 0, p50_ms: null, p90_ms: null };
    const vary = (hashString(date + name) % 9) / 10 + 0.6;
    return { date, count: 1 + (hashString(date) % 4), p50_ms: Math.round(row[7] * vary * MIN), p90_ms: Math.round(row[8] * vary * MIN) };
  });
  const delivery = [
    { environment: 'dev', deployments: 14, passed: 11, per_week: 2.6, lead_time_p50_ms: 5.5 * HOUR, lead_time_p90_ms: 19 * HOUR, lead_time_count: 23, change_failure_rate: 3 / 14, time_to_restore_p50_ms: 47 * MIN, restores: 3 },
    { environment: 'prod', deployments: 4, passed: 4, per_week: 0.9, lead_time_p50_ms: 2.1 * DAY, lead_time_p90_ms: 4.5 * DAY, lead_time_count: 23, change_failure_rate: 0, time_to_restore_p50_ms: null, restores: 0 },
  ];
  return {
    from: days[0], to: days[days.length - 1], stages, trend: { subject_kind: subject, kind, name, days }, delivery,
    options: { jobs: ['e2e', 'quality'], environments: ['dev', 'prod'], kinds: ['delivery', 'deploy', 'job', 'queue', 'run', 'stage', 'state', 'step', 'verify', 'wait'] },
    reused_runs: 3, truncated: false,
  };
});

function stageSpan(subjectKind, subjectId, key, kind, name, startMin, tookMin, extra = {}) {
  const start = Date.now() - startMin * MIN;
  const ended = tookMin === null ? null : start + tookMin * MIN;
  return {
    subject_kind: subjectKind, subject_id: subjectId, key, parent_key: null, kind, name, environment: null, job: null,
    started_at: new Date(start).toISOString(), ended_at: ended === null ? null : new Date(ended).toISOString(),
    duration_ms: ended === null ? null : Math.round(tookMin * MIN), outcome: ended === null ? null : 'passed', source: 'recorded', detail: {}, ...extra,
  };
}

// A busy running run (SPEC-165, the screenshot of task #155): eight jobs waited 6 to 9 min for a
// job slot (9.1 min in, so the 8 min tick would crowd "now"), as `ci::exec` records it (`wait:<job id>@…` beside the job's `job:<job id>@…`), each
// with its checkout; four are still running, one of them with a failed stage.
// Jobs are listed in the order they began waiting: prototypes began before gateway, started after.
const BUSY_JOBS = [
  ['shared', 6, 0.5], ['engine', 6, null], ['prototypes', 6.5, 0.15], ['package (gateway)', 6.2, null],
  ['package (broker)', 6.8, 0.7], ['package (deployer)', 7.5, null], ['package (mcp/browser)', 8.2, 0.7], ['package (mcp/email)', 8.6, null],
];

function busyTimeline(id) {
  const s = (...args) => stageSpan('ci_run', id, ...args);
  const spans = [
    s('queue@1', 'queue', 'queue', 9.1, 0.02, { detail: { reference: 'branch/quartio', reason: null } }),
    s('run@1', 'run', 'run', 9.08, null, { detail: { reference: 'branch/quartio', sha: '9f3c2a1b7e4d', requested_by: 'yulius' } }),
    s('prepare@1', 'prepare', 'commit', 9.08, 0.005),
  ];
  BUSY_JOBS.forEach(([name, waited, ran], i) => {
    const asked = 9.05 - i * 0.01;
    const at = asked - waited;
    const job = `job:j${i}@1`;
    spans.push(s(`wait:j${i}@1`, 'wait', 'job slot', asked, waited, { job: name, outcome: 'started', detail: { cpus: 4, memory_mb: 8192 } }));
    spans.push(s(job, 'job', name, at, ran, { job: name, detail: ran === null ? {} : { cpu_ms: Math.round(ran * 3 * MIN), memory_peak_mb: 900 } }));
    spans.push(s(`${job}/checkout`, 'prepare', 'checkout', at, 0.01, { parent_key: job, job: name }));
    if (name === 'engine') spans.push(s(`${job}/stage:1`, 'stage', 'build', at - 0.01, null, { parent_key: job, job: name }));
    // Still running, but a stage of it already failed.
    if (name === 'package (deployer)') spans.push(s(`${job}/stage:1`, 'stage', 'lint', at - 0.01, 0.5, { parent_key: job, job: name, outcome: 'failed' }));
  });
  return { spans, related: [] };
}

// A short run (about 2 min, SPEC-165 review): its axis steps by 30 s, "quality" was requeued after
// its process host was lost (two waits, two job spans), "e2e"'s wait has no job span and
// "security" was skipped.
function shortTimeline(id) {
  const s = (...args) => stageSpan('ci_run', id, ...args);
  return { spans: [
    s('queue@1', 'queue', 'queue', 2, 0.05, { detail: { reference: 'pr/221', reason: null } }),
    s('run@1', 'run', 'run', 1.95, 1.9, { detail: { reference: 'pr/221', sha: '4be81c0d2a9f', requested_by: 'yulius' } }),
    s('prepare@1', 'prepare', 'commit', 1.95, 0.01),
    s('wait:j0@1', 'wait', 'job slot', 1.94, 0.1, { job: 'quality', outcome: 'started' }),
    s('job:j0@1', 'job', 'quality', 1.84, 0.3, { job: 'quality', outcome: 'error', detail: { reason: 'the job’s process host was lost' } }),
    s('wait:j9@1', 'wait', 'job slot', 1.94, 0.2, { job: 'e2e', outcome: 'started' }),
    // Checked nothing (exit 78, ADR-0133): skipped, not passed.
    s('wait:j5@1', 'wait', 'job slot', 1.9, 0.05, { job: 'security', outcome: 'started' }),
    s('job:j5@1', 'job', 'security', 1.85, 0.1, { job: 'security', outcome: 'skipped', detail: { exit_code: 78 } }),
    s('wait:j0@2', 'wait', 'job slot', 1.54, 0.3, { job: 'quality', outcome: 'started' }),
    s('job:j0@2', 'job', 'quality', 1.24, 1.1, { job: 'quality' }),
  ], related: [] };
}

function mockTimeline(kind, id) {
  const s = (...args) => stageSpan(kind, id, ...args);
  if (kind === 'ci_run' && id === 'ci_s4') return busyTimeline(id);
  if (kind === 'ci_run' && id === 'ci_s3') return shortTimeline(id);
  if (kind === 'ci_run') {
    const failed = id === 'ci_s2';
    return { spans: [
      s('queue@1', 'queue', 'queue', 47, 2, { detail: { reference: 'pr/218', reason: null } }),
      s('run@1', 'run', 'run', 45, 14, { outcome: failed ? 'failed' : 'passed', detail: { reference: 'pr/218', sha: '0d7e5f3a1c2b', requested_by: 'yulius' } }),
      s('prepare@1', 'prepare', 'commit', 45, 0.2),
      // A slot free at once: `ci::exec` still records the (0 ms) wait, which says nothing.
      s('wait:j1@1', 'wait', 'job slot', 44.8, 0, { job: 'quality', outcome: 'started' }),
      s('wait:j2@1', 'wait', 'job slot', 44.8, 0, { job: 'test', outcome: 'started' }),
      s('job:j1@1', 'job', 'quality', 44.8, 12, { job: 'quality', detail: { cpu_ms: 31 * MIN, memory_peak_mb: 2048, disk_written_mb: 640 } }),
      s('job:j1@1/checkout', 'prepare', 'checkout', 44.8, 0.5, { parent_key: 'job:j1@1', job: 'quality' }),
      s('job:j1@1/stage:1', 'stage', 'lint', 44.3, 3, { parent_key: 'job:j1@1', job: 'quality' }),
      s('job:j1@1/stage:2', 'stage', 'unit', 41.3, 8.5, { parent_key: 'job:j1@1', job: 'quality', detail: { tests_passed: 1234, tests_flaky: 2, tests_skipped: 3 } }),
      s('job:j2@1', 'job', failed ? 'test (2/2)' : 'e2e', 44.8, 13.8, { job: 'test', outcome: failed ? 'failed' : 'passed', detail: failed ? { exit_code: 1 } : {} }),
      s('job:j2@1/stage:1', 'stage', 'integration', 44.6, 13.6, { parent_key: 'job:j2@1', job: 'test', outcome: failed ? 'failed' : 'passed', detail: failed ? { tests_passed: 310, tests_failed: 1 } : {} }),
    ], related: [] };
  }
  if (kind === 'cd_run') {
    const e = { environment: 'dev' };
    return { spans: [
      s('prepare', 'prepare', 'checks', 92, 0.4, e),
      s('run', 'run', 'run', 91.6, 3.4, { ...e, outcome: 'failed', detail: { branch: 'quartio', sha: '7d1e0c9b8a7f', requested_by: 'task #12' } }),
      s('run/deploy', 'deploy', 'deploy', 91.5, 3.2, { ...e, parent_key: 'run', outcome: 'failed' }),
      s('run/deploy/stage:1', 'stage', 'build', 91.5, 2.1, { ...e, parent_key: 'run/deploy' }),
      s('run/deploy/stage:2', 'stage', 'upload', 89.4, 1.1, { ...e, parent_key: 'run/deploy', outcome: 'failed' }),
    ], related: [] };
  }
  return { spans: [
    s('state@1', 'state', 'queued', 300, 12),
    s('state@2', 'state', 'working', 288, 40),
    s('step:a', 'step', 'Spec written and validated', 285, 20, { detail: { output_tokens: 18400, input_tokens: 220000, active_ms: 17 * MIN } }),
    s('state@3', 'state', 'needs_you', 248, 22, { outcome: null }),
    s('state@4', 'state', 'working', 226, null),
    s('step:b', 'step', 'Spans stored for tasks, CI and CD', 224, 60),
    s('delivery@1', 'delivery', 'pr_open', 150, 52, { outcome: 'merged_integration', detail: { pr: 221 } }),
    s('wait@1', 'wait', 'merge_armed', 120, 22, { outcome: 'merged' }),
    s('wait@2', 'wait', 'merge_armed', 10, null, { outcome: null }),
  ], related: [
    stageSpan('ci_run', 'ci_s3', 'queue@1', 'queue', 'queue', 140, 4),
    stageSpan('ci_run', 'ci_s3', 'run@1', 'run', 'run', 136, 16, { detail: { reference: 'pr/221', sha: '4be81c0d2a9f', requested_by: 'yulius' } }),
  ] };
}

route('GET', `/api/sandboxes/${ID}/stages/timeline`, ({ db, p, query }) => {
  if (!db.sandboxes.some((s) => s.id === p[0])) fail(404, 'not_found', 'not found');
  const [kind, ...rest] = (query.get('subject') || '').split(':');
  const id = rest.join(':');
  if (!['task', 'ci_run', 'cd_run'].includes(kind) || !id) fail(422, 'invalid', 'subject must be task:<id>, ci_run:<id> or cd_run:<id>');
  // A deploy still running (SPEC-238): its checks done, its deploy running its "build" stage.
  const deploying = kind === 'cd_run' && cdOf(db, p[0]).runs.find((r) => r.run_id === id && r.state === 'running');
  if (deploying) {
    const s = (...args) => stageSpan(kind, id, ...args);
    const e = { environment: deploying.environment };
    const since = (Date.now() - deploying.created) / MIN;
    return { subject_kind: kind, subject_id: id, related: [], spans: [
      s('prepare', 'prepare', 'checks', since, 0.01, e),
      s('run', 'run', 'run', since - 0.01, null, e),
      s('run/deploy', 'deploy', 'deploy', since - 0.01, null, { ...e, parent_key: 'run' }),
      s('run/deploy/stage:1', 'stage', 'build', since - 0.01, null, { ...e, parent_key: 'run/deploy' }),
    ] };
  }
  return { subject_kind: kind, subject_id: id, ...mockTimeline(kind, id) };
});

const SPAN_COLUMNS = ['subject_kind', 'subject_id', 'key', 'parent_key', 'kind', 'name', 'environment', 'job', 'started_at', 'ended_at', 'duration_ms', 'outcome', 'source', 'detail'];
route('GET', `/api/sandboxes/${ID}/stages/spans`, ({ db, p, query }) => {
  if (!db.sandboxes.some((s) => s.id === p[0])) fail(404, 'not_found', 'not found');
  const format = query.get('format') || 'json';
  if (format !== 'json' && format !== 'csv') fail(422, 'invalid', 'format must be json or csv');
  const rows = [...mockTimeline('ci_run', 'ci_s2').spans, ...mockTimeline('cd_run', 'cd_s2').spans];
  if (format === 'json') return rows;
  const cell = (v) => {
    const text = v === null || v === undefined ? '' : typeof v === 'object' ? JSON.stringify(v) : String(v);
    return /[",\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
  };
  const csv = [SPAN_COLUMNS.join(','), ...rows.map((r) => SPAN_COLUMNS.map((c) => cell(r[c])).join(','))].join('\r\n');
  return new Response(`${csv}\r\n`, { status: 200, headers: { 'Content-Type': 'text/csv; charset=utf-8', 'Content-Disposition': `attachment; filename="yulius-stages-${query.get('from') || 'start'}-${query.get('to') || 'now'}.csv"` } });
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
    // Tasks with a live process (TaskStatus::is_live), not the cards' "working" count (#170).
    sandboxes: db.sandboxes.map((s) => ({ id: s.id, slug: s.slug, name: s.name, live_sessions: db.tasks.filter((t) => t.sandbox_id === s.id && !t.archived && ['starting', 'running', 'switching', 'stopping'].includes(t.status)).length })),
    accounts: db.accounts.map((a) => (admin
      ? { label: a.label, ...(a.email ? { email: a.email } : {}), provider: a.provider, status: a.status, usage: a.usage, id: a.id, last_error: a.last_error, in_use_by: a.in_use_by }
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
    moveUpdate(db, 'pausing', { step: 'pausing', message: 'Pausing tasks: 2 of 3 paused.', tasks: { paused: 2, total: 3, waiting: [{ id: 'tsk_x', number: 4, title: 'Fix the flaky export test', sandbox: 'studio' }], deadline: new Date(Date.now() + 30 * MIN).toISOString() } });
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
      // The new version's re-check of sandboxes' Docker (SPEC-203 HST-012).
      const docker = db.docker.mode === 'rebuild'
        ? { ok: false, commit: db.update.current, checked_at: Date.now(), error: null, lines: [{ name: 'kernel modules', outcome: 'failed', detail: 'cannot load overlay: modprobe: FATAL: Module overlay not found' }, { name: 'sandbox Docker', outcome: 'failed', detail: 'the kernel lacks overlay, which a sandbox engine needs' }] }
        : db.docker.mode === 'kept'
          ? keptHostCheck(db.update.current, Date.now())
          : { ok: true, commit: db.update.current, checked_at: Date.now(), error: null, lines: [] };
      if (db.docker.mode === 'kept') db.docker.hostCheck = docker;
      moveUpdate(db, 'succeeded', { message: `Updated to ${short7(db.update.current)}.`, docker });
    }
  }, UPDATE_STEP_MS * 4);
}

// `__yuliusMock.holdUpdateReads = true`: each read answers what it saw when it arrived, once
// `releaseUpdate()` lets it go (the newest first); `updateReadsTaken` counts the held answers the
// page has read, after which it handles them at once (#502).
route('GET', '/api/update', async ({ db }) => {
  const view = updateView(db);
  if (!db.holdUpdateReads) return view;
  await new Promise((resolve) => db.updateHolds.push(resolve));
  const response = json(200, view);
  const text = response.text.bind(response);
  response.text = async () => { const body = await text(); db.updateReadsTaken += 1; return body; };
  return response;
});
route('GET', '/api/disk', ({ db }) => clone(db.disk));
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

const OWN_ENGINE_FIX = 'to use Docker’s packages, as the base does, run in that sandbox `sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin` (apt replaces docker.io; the engine switches when it next starts, after it was idle); never `sudo apt-get remove docker.io`, which hides the base’s Docker too';
const SMOKE_CHECKS = [
  ['a run older than the engine gets the socket', '/var/run/docker.sock'],
  ['its refusal names the reason', 'the sandbox’s Docker engine is not available: Docker is turned off'],
  ['docker run --rm hello-world in the old run', 'Hello from Docker!'],
  ['docker build in the old run', 'built-in-the-sandbox'],
  ['docker compose up (two services) in the old run', 'probe-1 exited with code 0'],
];

/** `docker=kept`: a re-check whose safe fixes failed (a helper unit from before SPEC-203 keeps
 * /etc read-only) while its own "sandbox Docker" verdict holds (#674). */
function keptHostCheck(commit, checkedAt) {
  const lines = [
    { name: 'needrestart', outcome: 'failed', detail: 'cannot write /etc/needrestart/conf.d/yulius.conf: Read-only file system (os error 30)' },
    { name: 'host Docker units', outcome: 'ok', detail: 'not installed' },
    { name: 'kernel modules', outcome: 'failed', detail: 'cannot write /etc/modules-load.d/yulius-docker.conf: Read-only file system (os error 30)' },
    { name: 'update helper', outcome: 'failed', detail: 'cannot write /etc/modules-load.d after an update; run `sudo yulius host setup`' },
    { name: 'sandbox Docker', outcome: 'ok', detail: 'each isolated sandbox gets its own engine at /var/run/docker.sock (DOCKER_HOST) on first use' },
  ];
  return { ok: false, commit, checked_at: checkedAt, error: null, lines };
}

function sandboxDockerView(db) {
  const d = db.docker;
  const base = { own_engine_fix: OWN_ENGINE_FIX, smoke: clone(d.smoke), host_check: null, own_engines: [], rebuild: null };
  if (d.mode === 'legacy') {
    return { ...base, ok: false, isolated: false, base: null, host_dockerd: false, detail: 'sandboxes are not isolated on this server, so they get no Docker: run Yulius contained (YULIUS_CONTAINED=true in yulius.service) with the isolation prerequisites, which `sudo yulius host setup` installs; `sudo -u yulius yulius doctor --contained` says what is missing' };
  }
  if (d.mode === 'rebuild') {
    return {
      ...base, ok: false, isolated: true, host_dockerd: true, base: { id: 'base-3', dockerd: false },
      detail: 'the current sandbox base (base-3) has no `dockerd` though the host has one: a new base is being built; until then only a sandbox with Docker installed in it gets an engine',
      rebuild: { building_since: iso(2 * MIN), pending_since: null, starts_at: null, ready_by: isoIn(MIN), last_build_secs: 190, error: null },
      own_engines: [{ sandbox: '6b76ccc7-0000-4000-8000-000000000001', name: 'quartio/product', dockerd: 'usr/sbin/dockerd' }],
      host_check: { ok: false, commit: db.update.current, checked_at: Date.now() - 20 * MIN, error: null, lines: [{ name: 'host Docker units', outcome: 'changed', detail: 'disabled and stopped docker.service' }, { name: 'kernel modules', outcome: 'failed', detail: 'cannot load overlay: modprobe: FATAL: Module overlay not found' }, { name: 'sandbox Docker', outcome: 'failed', detail: 'the kernel lacks overlay, which a sandbox engine needs' }] },
    };
  }
  if (d.mode === 'kept' && d.hostCheck === undefined) d.hostCheck = db.update.fail || db.update.offer ? null : keptHostCheck(db.update.current, Date.now() - 41 * MIN);
  const hostCheck = d.mode === 'kept' ? d.hostCheck : null;
  return { ...base, ok: true, isolated: true, host_dockerd: true, base: { id: 'base-9', dockerd: true }, host_check: hostCheck, detail: 'each isolated sandbox gets its own engine at /var/run/docker.sock (DOCKER_HOST) on first use; `sudo yulius doctor --docker-smoke` proves it on this host' };
}

route('GET', '/api/admin/sandbox-docker', ({ db }) => { superadminOnly(db); return sandboxDockerView(db); });
route('POST', '/api/admin/sandbox-docker/smoke', ({ db }) => {
  superadminOnly(db);
  const d = db.docker;
  if (d.mode === 'legacy') fail(409, 'conflict', sandboxDockerView(db).detail);
  if (d.smoke && d.smoke.state === 'running') fail(409, 'conflict', 'a Docker smoke test is running already');
  const run = { state: 'running', started_by: db.me.user.login, started_at: new Date().toISOString(), finished_at: null, checks: [], error: null };
  d.smoke = run;
  SMOKE_CHECKS.forEach(([name, detail], i) => {
    later(() => {
      if (d.smoke !== run || run.state !== 'running') return;
      const failed = d.smokeFails && i === 2;
      run.checks.push({ name, ok: !failed, detail: failed ? 'toomanyrequests: You have reached your pull rate limit' : detail });
      if (failed || i === SMOKE_CHECKS.length - 1) {
        run.state = failed ? 'failed' : 'passed';
        run.finished_at = new Date().toISOString();
      }
    }, 1000 * (i + 1));
  });
  return json(202, sandboxDockerView(db));
});

// Leftovers (SPEC-215): a removed product sandbox's parts, a done task's build cache, a worktree
// no task owns, everything else of the removed sandbox and the host's Docker data.
const GB = 1e9;
function leftoverItems() {
  const product = '/var/lib/yulius/sandboxes/6b76ccc7-0000-4000-8000-000000000001';
  const item = (id, kind, path, bytes, auto, reason, extra = {}) => ({ id, kind, path, bytes, auto, deletable: kind !== 'host-docker', reason, sandbox: null, task: null, ...extra });
  return [
    item('a1', 'removed-worktrees', `${product}/worktrees`, 48 * GB, true, 'the sandbox product was removed', { sandbox: 'product' }),
    item('a2', 'removed-system', '/var/lib/yulius/sysroots/6b76ccc7-0000-4000-8000-000000000001', 31 * GB, true, 'the sandbox product was removed', { sandbox: 'product' }),
    item('a3', 'task-cache', '/var/lib/yulius/sandboxes/d62be476/worktrees/de8d2402/target', 23 * GB, true, 'target of task #193: done, quiet for 30 minutes', { sandbox: 'yulius', task: 193 }),
    item('m1', 'removed-sandbox', product, 6 * GB, false, 'everything of the removed sandbox product: its clone, sessions, transcripts and export, kept unless a person deletes them', { sandbox: 'product' }),
    item('m2', 'orphan-worktree', '/var/lib/yulius/sandboxes/d62be476/worktrees/wt-206', 2.7 * GB, false, 'a worktree no task owns (it may hold uncommitted work)', { sandbox: 'yulius' }),
    item('h1', 'host-docker', '/var/lib/docker', null, false, 'the host’s own Docker data (docker-ce): Yulius does not use it; remove it on the host if nothing else does'),
  ];
}

function leftoversView(db) {
  const l = db.leftovers;
  const items = l.items || [];
  const deletable = items.filter((i) => i.deletable);
  return {
    items: clone(items), scanned_at: l.scannedAt, data_dir: '/var/lib/yulius',
    total_bytes: deletable.reduce((n, i) => n + (i.bytes || 0), 0),
    safe_bytes: deletable.filter((i) => i.auto).reduce((n, i) => n + (i.bytes || 0), 0),
    disk_used: 312 * 1024 ** 3, disk_total: 386 * 1024 ** 3,
    job: clone(l.job), next_auto_at: isoIn(40 * MIN),
    // Removed sandboxes whose files were kept (SPEC-276 RDA-012): one per removed-sandbox item.
    kept: items.filter((i) => i.kind === 'removed-sandbox').map((i) => ({
      id: `removed-${i.sandbox}`, slug: i.sandbox, removed_at: isoIn(-3 * 24 * 60 * MIN), bytes: i.bytes, purge_requested: false,
    })),
    // The automatic round 20 minutes ago (SPEC-276 RDA-009).
    last_auto: { action: 'delete', running: false, actor: 'yulius', total: 2, done: 2, deleted: 2, freed: 3 * 1024 ** 3, kept: [], failed: [], started_at: isoIn(-20 * MIN), ended_at: isoIn(-20 * MIN) },
  };
}

function leftoverJob(db, action, total) {
  const l = db.leftovers;
  if (l.job && l.job.running) fail(409, 'conflict', 'a leftovers scan or deletion runs');
  l.job = { action, running: true, actor: db.me.user.login, total, done: 0, deleted: 0, freed: 0, kept: [], failed: [], started_at: new Date().toISOString(), ended_at: null };
  return l.job;
}

route('GET', '/api/admin/leftovers', ({ db }) => { superadminOnly(db); return leftoversView(db); });
route('POST', '/api/admin/leftovers/scan', ({ db }) => {
  superadminOnly(db);
  const job = leftoverJob(db, 'scan', 0);
  later(() => {
    db.leftovers.items = db.leftovers.none ? [] : leftoverItems();
    db.leftovers.scannedAt = new Date().toISOString();
    Object.assign(job, { running: false, ended_at: new Date().toISOString() });
  }, 800);
  return leftoversView(db);
});
route('POST', '/api/admin/leftovers/delete', ({ db, body }) => {
  superadminOnly(db);
  const l = db.leftovers;
  const items = l.items || [];
  const chosen = body.all_safe ? items.filter((i) => i.auto) : items.filter((i) => (body.ids || []).includes(i.id));
  if (!body.all_safe && chosen.length !== (body.ids || []).length) fail(422, 'invalid', 'not in the last scan (scan again)');
  if (!chosen.length) fail(422, 'invalid', 'nothing to delete');
  const job = leftoverJob(db, 'delete', chosen.length);
  chosen.forEach((item, i) => {
    later(() => {
      job.done += 1;
      if (item.deletable) {
        job.deleted += 1;
        job.freed += item.bytes || 0;
        l.items = l.items.filter((x) => x.id !== item.id);
      } else {
        job.kept.push(`${item.path}: reported only: the host's own data`);
      }
      if (i === chosen.length - 1) Object.assign(job, { running: false, ended_at: new Date().toISOString() });
    }, 600 * (i + 1));
  });
  return leftoversView(db);
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

// The instance Vidoc key (SPEC-195 VDK-002): the Jev key's routes, for Vidoc.
function vidocView(db) {
  const v = db.vidoc;
  const source = v.key ? 'settings' : v.config ? 'config' : null;
  return { configured: Boolean(source), source, set_at: v.key ? v.setAt : null, set_by: v.key ? v.setBy : null, config_present: v.config, config_error: null };
}
route('GET', '/api/admin/vidoc', ({ db }) => { superadminOnly(db); return vidocView(db); });
route('PUT', '/api/admin/vidoc', ({ db, body }) => {
  superadminOnly(db);
  const key = String(body.key || '').trim();
  if (key.length < 8 || /\s/.test(key)) fail(422, 'invalid', 'the Vidoc key must be 8-4096 printable characters without spaces');
  Object.assign(db.vidoc, { key, setAt: new Date().toISOString(), setBy: db.me.user.login });
  return vidocView(db);
});
route('DELETE', '/api/admin/vidoc', ({ db }) => { superadminOnly(db); db.vidoc.key = null; return vidocView(db); });
route('POST', '/api/admin/vidoc/adopt', ({ db }) => {
  superadminOnly(db);
  if (!db.vidoc.config) fail(409, 'conflict', 'YULIUS_VIDOC_API_KEY_FILE holds no usable key');
  Object.assign(db.vidoc, { key: 'from-config', setAt: new Date().toISOString(), setBy: db.me.user.login });
  return vidocView(db);
});
route('POST', '/api/admin/vidoc/test', ({ db }) => {
  superadminOnly(db);
  const key = db.vidoc.key || (db.vidoc.config ? 'from-config' : null);
  if (!key) return { ok: false, result: 'none', message: 'No Vidoc key is set.', latency_ms: null };
  if (key.includes('bad')) return { ok: false, result: 'refused', message: 'Vidoc refused this key. Check it and save it again.', latency_ms: 180 };
  return { ok: true, result: 'ok', message: 'Vidoc answered: the key works and vidoc-1 is available.', latency_ms: 160 };
});

// The pool's tokens over 7 rolling days (SPEC-110), priced as Opus 5.5 ($4 in, $20 out, $0.20
// cache read, $5 cache write per million).
const poolSpend = (k) => {
  const t = { input: Math.round(2.1e6 * k), output: Math.round(1.4e6 * k), cache_read: Math.round(310e6 * k), cache_write: Math.round(9e6 * k) };
  return { ...t, cost_usd: (t.input * 4 + t.output * 20 + t.cache_read * 0.2 + t.cache_write * 5) / 1e6 };
};
function poolTokens() {
  const perDay = [0.6, 0.9, 0.4, 1.1, 1.3, 0.8, 1.2].map(poolSpend);
  const sum = (key) => perDay.reduce((a, d) => a + d[key], 0);
  return {
    last_24h: perDay[6],
    last_7d: { input: sum('input'), output: sum('output'), cache_read: sum('cache_read'), cache_write: sum('cache_write'), cost_usd: sum('cost_usd') },
    per_day: perDay,
    unknown_usage_runs: 1,
    unpriced_runs: 0,
  };
}

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
    burn_per_day: [1.2, 1.6, null, 1.9, 2.2, 2.0, 2.4],
    paused_hours_per_day: [0, 0, 1.5, 0, 2, 0.5, 2.5],
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
      ...(admin ? { id: a.id, in_use_by: a.in_use_by, ...(a.email ? { email: a.email } : {}) } : {}),
    })),
    tokens: poolTokens(),
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
    entry(1, 'assistant', [call('b1', 'Bash', 'npx playwright test login', { command: 'npx playwright test login' })]),
    entry(2, 'user', [result('b1', 'Error: Timed out 5000ms waiting for expect(locator).toBeVisible()\n  at e2e/login.spec.ts:15', true)]),
    entry(3, 'assistant', [call('g1', 'Grep', 'waitForTimeout in e2e', { pattern: 'waitForTimeout', path: 'e2e' })], { sidechain: true }),
    entry(4, 'user', [result('g1', 'e2e/login.spec.ts:14:  await page.waitForTimeout(500);\ne2e/signup.spec.ts:9:  await page.waitForTimeout(300);\ne2e/cart.spec.ts:21:  await page.waitForTimeout(250);\ne2e/admin.spec.ts:7:  await page.waitForTimeout(500);\ne2e/search.spec.ts:30:  await page.waitForTimeout(100);')]),
    entry(5, 'assistant', [call('e1', 'Edit', 'e2e/login.spec.ts', { file_path: 'e2e/login.spec.ts' }, { path: 'e2e/login.spec.ts', hunks: [{ old: '  await page.waitForTimeout(5000);', new: '  await expect(page.getByRole(\'heading\')).toBeVisible();' }] })]),
    entry(6, 'user', [result('e1', '<tool_use_error>String to replace not found in file.\nString:   await page.waitForTimeout(5000);</tool_use_error>', true)]),
    entry(7, 'assistant', [call('e2', 'Edit', 'e2e/cart.spec.ts', { file_path: 'e2e/cart.spec.ts' }, { path: 'e2e/cart.spec.ts', hunks: [{ old: '  await page.waitForTimeout(250);', new: '' }] })]),
    entry(8, 'user', [result('e2', 'The file e2e/cart.spec.ts has been updated.')]),
    entry(9, 'assistant', [call('w1', 'Write', 'e2e/wait.ts', { file_path: 'e2e/wait.ts' }, { path: 'e2e/wait.ts', hunks: [{ old: '', new: 'export async function signedIn(page) {\n  await page.waitForURL(/dashboard/);\n}' }] })]),
    entry(10, 'user', [result('w1', 'File created successfully at: e2e/wait.ts')]),
    entry(11, 'assistant', [text('The test sleeps 500 ms instead of waiting for the session cookie; on a slow runner the redirect is not done yet. Waiting for the dashboard heading fixes it.')]),
    // A write the server cut at its bound (#350): no line count is claimed.
    entry(12, 'assistant', [call('w2', 'Write', 'e2e/fixtures/users.json', { file_path: 'e2e/fixtures/users.json' }, { path: 'e2e/fixtures/users.json', hunks: [{ old: '', new: Array.from({ length: 400 }, (_, i) => `  { "id": ${i}, "email": "user${i}@example.test" },`).join('\n') }], truncated: true })]),
    entry(13, 'user', [result('w2', 'File created successfully at: e2e/fixtures/users.json')]),
    // 16 lines of 60 characters: it scrolls on a phone only (#363).
    entry(14, 'assistant', [call('b2', 'Bash', 'npx playwright test --list', { command: 'npx playwright test --list' })]),
    entry(15, 'user', [result('b2', Array.from({ length: 16 }, (_, i) => `[chromium] › e2e/login.spec.ts:${String(i + 1).padStart(2, '0')} › signs in with a link`.padEnd(60, '.')).join('\n'))]),
    // Hook context the server cut at its bound (#380).
    entry(16, 'user', [{ type: 'meta', text: ['Hook context: flaky test history', '', ...Array.from({ length: 400 }, (_, i) => `- run ${i + 1}: e2e/login.spec.ts timed out after 5000ms`)].join('\n').slice(0, 16384), truncated: true }]),
    // Review round 1 of SPEC-177: hunks, a result whose call is not on the page, a cut command
    // output and a command run twice are told apart.
    entry(17, 'assistant', [call('m1', 'MultiEdit', 'e2e/login.spec.ts', { file_path: 'e2e/login.spec.ts' }, { path: 'e2e/login.spec.ts', hunks: [1, 2].map((n) => ({ old: Array.from({ length: 30 }, (_, i) => `  await page.waitForTimeout(${n}${i});`).join('\n'), new: Array.from({ length: 30 }, (_, i) => `  await expect(page.getByRole('heading', { name: 'Step ${n}${i}' })).toBeVisible();`).join('\n') })) })]),
    entry(18, 'user', [result('m1', 'Applied 2 edits to e2e/login.spec.ts')]),
    entry(19, 'user', [result('gone1', Array.from({ length: 40 }, (_, i) => `e2e/report-${i}.json`).join('\n')), result('gone2', Array.from({ length: 40 }, (_, i) => `e2e/trace-${i}.zip`).join('\n'))]),
    entry(20, 'user', [{ type: 'meta', text: `<local-command-stdout>${Array.from({ length: 40 }, (_, i) => `Usage line ${i}`).join('\n')}`, truncated: true }]),
    entry(21, 'assistant', [call('b3', 'Bash', 'npx playwright test login', { command: 'npx playwright test login' })]),
    entry(22, 'user', [result('b3', Array.from({ length: 40 }, (_, i) => `  ✓ ${i + 1} [chromium] › e2e/login.spec.ts:${i + 1}`).join('\n'))]),
    // Long code blocks: one, then two in one message (review round 2).
    entry(23, 'assistant', [text(`Before:\n\`\`\`ts\n${Array.from({ length: 40 }, (_, i) => `await page.waitForTimeout(${i});`).join('\n')}\n\`\`\``)]),
    entry(24, 'assistant', [text(['After:', '```ts', ...Array.from({ length: 40 }, (_, i) => `await expect(page.getByText('${i}')).toBeVisible();`), '```', 'And the helper:', '```ts', ...Array.from({ length: 40 }, (_, i) => `export const step${i} = ${i};`), '```'].join('\n'))]),
    // A MultiEdit cut past its hunks names what it shows (review round 2).
    entry(25, 'assistant', [call('m2', 'MultiEdit', 'e2e/steps.ts', { file_path: 'e2e/steps.ts' }, { path: 'e2e/steps.ts', hunks: [{ old: 'a', new: 'b' }, { old: 'c', new: 'd' }], truncated: true })]),
    entry(26, 'user', [result('m2', 'Applied 21 edits to e2e/steps.ts')]),
  ];
  // Claude Code's wrappers as recorded (a slash command, its output, a `!` command, reminders).
  const meta = (t) => ({ type: 'meta', text: t });
  const webhook = [
    entry(0, 'user', [meta('Caveat: The messages below were generated by the user while running local commands. DO NOT respond to these messages or otherwise consider them in your response unless the user explicitly asks you to.')]),
    entry(1, 'user', [meta('<command-name>/goal</command-name>\n            <command-message>goal</command-message>\n            <command-args>Payment webhook ships on green CI</command-args>')], { at: iso(9 * HOUR + 2 * MIN) }),
    entry(2, 'user', [meta('<local-command-stdout>Goal set: Payment webhook ships on green CI</local-command-stdout>')]),
    entry(3, 'user', [meta('<system-reminder>\nThe date is 2026-10-13.\n</system-reminder>')]),
    entry(4, 'user', [text('Payment webhook: verify Stripe signatures and retry fulfilment with backoff.')], { at: iso(9 * HOUR) }),
    entry(5, 'assistant', [text('I’ll verify the signature with the webhook secret and queue fulfilment with three retries and exponential backoff.')]),
    entry(6, 'user', [meta('[Request interrupted by user]')]),
    entry(7, 'user', [meta('<bash-input>ls src/webhooks</bash-input>')]),
    entry(8, 'user', [meta('<bash-stdout>stripe.ts\nretry.ts</bash-stdout><bash-stderr></bash-stderr>')]),
    entry(9, 'system', [meta('<local-command-stderr>Error: Error during compaction: Conversation too long.</local-command-stderr>')]),
    entry(10, 'user', [meta('A session-scoped Stop hook is now active with condition: "Payment webhook ships on green CI".')]),
    entry(11, 'assistant', [call('w9', 'Write', 'src/webhooks/fixtures.ts', { file_path: 'src/webhooks/fixtures.ts' }, { path: 'src/webhooks/fixtures.ts', hunks: [{ old: '', new: Array.from({ length: 60 }, (_, i) => `export const event${i} = { id: 'evt_${i}', type: 'checkout.session.completed' };`).join('\n') }] })]),
    entry(12, 'user', [result('w9', 'File created successfully at: src/webhooks/fixtures.ts')]),
    // A skill's body, loaded as context (SPEC-090): the TUI does not print it.
    entry(13, 'user', [meta(['Base directory for this skill: /home/dev/.claude/skills/retry-policy', '', '# Retry policy — back-off for fulfilment', '─'.repeat(40), '', ...Array.from({ length: 36 }, (_, i) => `- Rule ${i + 1}: back off before attempt ${i + 2}.`)].join('\n'))]),
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
// A task's live session for the people watching it (SPEC-179 WAT-001): its session document's
// entries, grown by `appendTranscript` as the session writes. The size is the entries' JSON.
route('GET', `/api/tasks/${ID}/transcript`, ({ db, p, query }) => {
  const task = taskOf(db, p[0]);
  db.transcriptReads = (db.transcriptReads || 0) + 1;
  (db.transcriptQueries = db.transcriptQueries || []).push(query.toString());
  // Tests make the next reads fail as a server restarting would (`failTranscriptReads = 1`).
  if (db.failTranscriptReads > 0) {
    db.failTranscriptReads -= 1;
    fail(502, 'unavailable', 'The server is restarting.');
  }
  const doc = sessionDocs(db).find((d) => d.kind === 'task' && d.task_id === task.id);
  if (!doc) return { session_id: null, doc_id: null, entries: [], next: null, end: 0, bytes: 0, earlier: false, skipped_lines: 0 };
  const lines = doc.transcript;
  const end = lines.length ? lines[lines.length - 1].line + 1 : 0;
  const bytes = JSON.stringify(lines).length;
  const from = query.has('from') ? Number(query.get('from')) : null;
  const limit = Math.min(500, Math.max(1, Number(query.get('limit') || 200)));
  const base = { session_id: doc.session_id, doc_id: null, next: null, bytes, earlier: false, skipped_lines: 0 };
  if (from !== null && Number(query.get('seen')) === bytes && query.get('session') === doc.session_id) return { ...base, entries: [], end: from };
  if (from === null) return { ...base, doc_id: doc.id, entries: lines.slice(-limit), end, earlier: lines.length > limit };
  const entries = lines.filter((e) => e.line >= from).slice(0, limit);
  const more = entries.length === limit && lines.some((e) => e.line > entries[entries.length - 1].line);
  return { ...base, entries, next: more ? entries[entries.length - 1].line + 1 : null, end: more ? entries[entries.length - 1].line + 1 : end };
});
route('POST', `/api/sessions/${ID}/continue`, ({ db, p }) => {
  const doc = sessionDocOf(db, p[0]);
  const sandbox = sandboxOf(db, doc.sandbox_id);
  need(sandbox, 'write');
  if (doc.kind !== 'imported') fail(422, 'invalid', 'a task’s session is continued from its task');
  if (sandbox.onboarding === 'pending' || sandbox.onboarding === 'preparing') fail(409, 'conflict', 'this project is being prepared; tasks can start once the preparation task has made it ready (or an admin skips it)');
  const existing = doc.continued_in ? db.tasks.find((t) => t.id === doc.continued_in) : null;
  if (existing) return summary(existing);
  needClaude(db);
  const number = Math.max(0, ...db.tasks.filter((t) => t.sandbox_id === sandbox.id).map((t) => t.number)) + 1;
  const id = `tsk_${sandbox.id.slice(3)}_${number}`;
  const task = {
    id, sandbox_id: sandbox.id, number, title: doc.title, harness: 'claude', status: 'starting', activity: null,
    branch: `feat/${doc.title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 40)}`,
    origin: 'import', created_by: { login: db.me.user.login }, created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    archived: false, account_label: null, stepsList: [], model: 'claude-opus-4', effort: 'default', prompt: doc.title, delivery: null,
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

const PUBLIC = ['/api/setup/status', '/api/setup', '/api/auth/password', '/api/auth/mock-github', '/api/auth/recovery', '/api/auth/recover'];

async function handleFetch(db, url, init = {}) {
  // Counted as the page asks, before the simulated latency (SPEC-108 tests); `asked` counts every
  // GET by path (SPEC-197).
  if ((init.method || 'GET') === 'GET') {
    const asked = new URL(url, ORIGIN).pathname;
    db.asked = db.asked || {};
    db.asked[asked] = (db.asked[asked] || 0) + 1;
  } else {
    // `sent` counts the other requests by method and path (SPEC-209: nothing sent while settling).
    const sent = `${init.method} ${new URL(url, ORIGIN).pathname}`;
    db.sent = db.sent || {};
    db.sent[sent] = (db.sent[sent] || 0) + 1;
  }
  if ((init.method || 'GET') === 'GET' && /^\/api\/sandboxes\/[^/]+\/git$/.test(new URL(url, ORIGIN).pathname)) db.gitRequests += 1;
  // The Editor's file reads, as asked, and whether a save of a file was on its way (SPEC-181).
  const fileContent = /^\/api\/sandboxes\/[^/]+\/files\/content$/.test(new URL(url, ORIGIN).pathname);
  if (fileContent && (init.method || 'GET') === 'GET') {
    const asked = new URL(url, ORIGIN).searchParams;
    (db.fileReads = db.fileReads || []).push({ path: asked.get('path'), known: asked.get('known'), whileSaving: (db.savesOnTheirWay || 0) > 0 });
    // `holdReads = []`: each read waits there, as on a slow connection, until the test calls its
    // resolver (in the order asked), so the test says which answer comes first (#571).
    if (db.holdReads) await new Promise((resolve) => db.holdReads.push(resolve));
  }
  if (fileContent && init.method === 'PUT') {
    db.savesOnTheirWay = (db.savesOnTheirWay || 0) + 1;
    try { return await handleFetchAfterLatency(db, url, init); } finally { db.savesOnTheirWay -= 1; }
  }
  return handleFetchAfterLatency(db, url, init);
}

async function handleFetchAfterLatency(db, url, init) {
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
  // `hold=<path>` keeps GET requests of that path waiting, to look at a view while it loads (SPEC-092).
  // `heldWaiting`: the requests waiting at the hold now, so a test releases them once they are there.
  if (db.held && ((method === 'GET' && u.pathname === db.holdPath) || (method === 'POST' && u.pathname === db.holdPostPath))) {
    db.heldWaiting = (db.heldWaiting || 0) + 1;
    try { await db.held; } finally { db.heldWaiting -= 1; }
  }
  if (!PUBLIC.includes(u.pathname) && !db.signedIn) return json(401, { error: { code: 'unauthenticated', message: 'Sign in first.' } });
  if (db.failPath && method === 'GET' && u.pathname === db.failPath) {
    db.failedReads += 1;
    return json(500, { error: { code: 'internal', message: 'The mock server failed on purpose' } });
  }
  // Tests count archive requests (`__yuliusMock.archiveRequests`), also those failed on purpose.
  if (method === 'POST' && /^\/api\/tasks\/[^/]+\/archive$/.test(u.pathname)) db.archiveRequests = (db.archiveRequests || 0) + 1;
  if (db.failPostPath && method === 'POST' && u.pathname === db.failPostPath) {
    db.failedPosts += 1;
    return json(500, { error: { code: 'internal', message: 'The mock server failed on purpose' } });
  }
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

// A collapsed tool group as Claude Code's full-screen transcript shows it: a click on it expands
// the calls it groups, another click collapses them (task #25).
const GROUP_CLOSED = `${c(255, '⏺')} Searched for 2 patterns, read 1 file ${dim('(click to expand)')}`;
const GROUP_OPEN = `${c(255, '⏺')} Searched for 2 patterns, read 1 file`;
const GROUP_CALLS = [
  `  ${dim('⎿')}  ${bold('Search')}(pattern: "checkout", path: "src/")`,
  `  ${dim('⎿')}  ${bold('Search')}(pattern: "loyalty_points", path: "src/")`,
  `  ${dim('⎿')}  ${bold('Read')}(src/api/tax.rs)`,
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
      this.lines.push(GROUP_CLOSED);
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
    return (RANK[this.sandbox().access] || 0) >= RANK.write && LIVE.has(this.task.status)
      && ownerOf(this.task).login === this.db.me.user.login;
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
      // A test can hold a task terminal's first screen (`window.__yuliusHoldTerminal = promise`,
      // set by an init script) to act in the view before the terminal shows; once released, this
      // session stays open for good (`released` is per session).
      const hold = window.__yuliusHoldTerminal;
      if (hold && !this.released) { hold.then(() => { this.released = true; this.receive(ws, data); }); return; }
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

  /** A wheel report (SGR button 64 up, 65 down) scrolls the transcript by a line, as Claude Code does (SPEC-280). */
  wheel(button) {
    const most = Math.max(0, this.lines.length - this.regionBottom());
    const next = Math.min(most, Math.max(0, this.scroll + (button === 64 ? 1 : button === 65 ? -1 : 0)));
    if (next === this.scroll) return;
    this.scroll = next;
    this.broadcast(this.snapshot());
  }

  /** A primary click at 1-based `row` toggles the collapsed tool group drawn there, if any. */
  click(row) {
    const bottom = this.regionBottom();
    if (row < 1 || row > bottom) return;
    const index = this.lines.length - this.scroll - bottom + row - 1;
    const line = this.lines[index];
    if (line === GROUP_CLOSED) {
      this.lines.splice(index, 1, GROUP_OPEN, ...GROUP_CALLS);
    } else if (line === GROUP_OPEN) {
      this.lines.splice(index, 1 + GROUP_CALLS.length, GROUP_CLOSED);
    } else {
      return;
    }
    this.broadcast(this.snapshot());
  }

  /**
   * A primary release at another column of the press's row ends a drag: like Claude Code 2.1.292,
   * the program copies the text it selected with OSC 52 (task #149).
   */
  release(col, row) {
    const press = this.pressed;
    this.pressed = null;
    const bottom = this.regionBottom();
    if (!press || press[1] !== row || press[0] === col || row < 1 || row > bottom) return;
    const line = this.lines[this.lines.length - this.scroll - bottom + row - 1];
    if (!line) return;
    const text = line.replace(/\x1b\[[0-9;]*m/g, '').slice(Math.min(press[0], col) - 1, Math.max(press[0], col)).trim();
    const bytes = new TextEncoder().encode(text);
    this.broadcast(`\x1b]52;c;${btoa(String.fromCharCode(...bytes))}\x07`);
  }

  onInput(data) {
    // Mouse reports (the viewer's wheel and clicks) are not typed text: a wheel turn scrolls, a
    // primary press clicks, its release after a drag copies the dragged text.
    const typed = data.replace(/\x1b\[<(\d+);(\d+);(\d+)([Mm])/g, (_, button, col, row, kind) => {
      if (Number(button) !== 0) this.wheel(Number(button));
      else if (kind === 'M') { this.pressed = [Number(col), Number(row)]; this.click(Number(row)); }
      else this.release(Number(col), Number(row));
      return '';
    });
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
      // A test can hold a shell's first screen too (`window.__yuliusHoldShell = promise`); once
      // released, this session stays open for good (`released` is per session).
      const hold = window.__yuliusHoldShell;
      if (hold && !this.released) { hold.then(() => { this.released = true; this.receive(ws, data); }); return; }
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
  end(code, status = 'closed') {
    for (const ws of [...this.viewers]) {
      ws.push(JSON.stringify({ type: 'exit', status, code }));
      setTimeout(() => ws.serverClose(1000, 'shell ended'), 50);
    }
    this.viewers.clear();
    this.db.shells.delete(this.shell.id);
  }

  closed(ws) { this.viewers.delete(ws); }
}

/** Starts a sign-in for `accountId`, replacing (cancelling) one still running, as the server does. */
function startLogin(db, accountId) {
  for (const session of db.logins.values()) if (session.accountId === accountId && !session.done) session.end('cancelled', null);
  db.nextId += 1;
  const loginId = `login_${accountId}_${db.nextId}`;
  db.logins.set(loginId, new LoginSession(db, accountId));
  return loginId;
}

/**
 * Fake provider login for the account login terminal: a fixed 500×40 PTY; `state.status`
 * goes running → ok | error | cancelled and the exit frame carries the same status. Tokens are
 * masked. Enter signs in; the code `fail` makes the sign-in fail as the CLI's would (SPEC-243).
 */
class LoginSession {
  constructor(db, accountId) {
    this.db = db;
    this.accountId = accountId;
    this.seq = 0;
    this.input = '';
    this.ws = null;
    this.done = false;
    this.status = 'running';
    this.code = null;
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
        'Paste the authorization code here (any text in mock mode; "fail" fails), then press Enter:',
        '> ',
      ].join('\r\n')));
      if (this.done) ws.push(JSON.stringify({ type: 'exit', status: this.status, code: this.code }));
    } else if (msg.type === 'input' && !this.done) {
      for (const ch of String(msg.data || '')) {
        if (ch === '\r') { this.finish(); return; }
        if (ch === '\x7f') { if (this.input) { this.input = this.input.slice(0, -1); this.out('\b \b'); } } else if (ch >= ' ') { this.input += ch; this.out('*'); }
      }
    }
  }

  state() {
    return { type: 'state', attempt_seq: null, harness: 'claude', cols: 500, rows: 40, can_write: !this.done, controller: true, status: this.status };
  }

  finish() {
    const account = this.db.accounts.find((a) => a.id === this.accountId);
    if (this.input === 'fail') {
      this.out(`\r\n${c(203, 'OAuth error: the authorization code was not accepted.')}\r\n`);
      if (account) {
        // Like the server: a working account keeps its state and notes the failed re-login.
        const working = account.status === 'ok' || account.status === 'limited';
        Object.assign(account, working
          ? { last_error: 're-login failed: the sign-in ended without producing a token (exit code 1)' }
          : { status: 'error', last_error: 'the sign-in ended without producing a token (exit code 1)' });
        emit(this.db, { type: 'account.updated', account });
      }
      this.end('error', 1);
      return;
    }
    this.out(`\r\nToken: sk-ant-oat01-[REDACTED]\r\n${c(114, '✔')} Logged in. Credentials stored encrypted for this account.\r\n`);
    if (account) {
      this.db.unsignedAccounts.delete(account.id);
      Object.assign(account, { status: 'ok', last_error: null, usage: { primary: { used_percent: 3.0, resets_at: isoIn(5 * HOUR), window_minutes: 300 }, secondary: { used_percent: 1.0, resets_at: isoIn(7 * DAY), window_minutes: 10080 }, observed_at: new Date().toISOString() } });
      emit(this.db, { type: 'account.updated', account });
      // Like the server, the email of the Claude account is read right after the sign-in ended
      // and arrives as a later update (SPEC-244 SUB-003).
      setTimeout(() => {
        if (!account.email) account.email = 'signed-in@example.com';
        emit(this.db, { type: 'account.updated', account });
      }, 300);
    }
    this.end('ok', 0);
  }

  /** Ends the sign-in with `status` (ok | error | cancelled) and tells the viewer. */
  end(status, code) {
    if (this.done) return;
    this.done = true;
    this.status = status;
    this.code = code;
    if (this.ws) {
      this.ws.push(JSON.stringify(this.state()));
      this.ws.push(JSON.stringify({ type: 'exit', status, code }));
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
    if (this.db.holdPath === 'frames' && !this.db.released) {
      if (!this.heldFrame) this.heldFrame = this.db.held.then(() => this.render());
      return;
    }
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
  const fatal = (code, message, closeCode = 1008) => {
    ws.server = { open: () => { ws.push(JSON.stringify({ type: 'error', code, message })); ws.serverClose(closeCode, code); } };
  };
  if (!db.signedIn) { fatal('unauthenticated', 'Sign in first.'); return ws; }
  let m;
  // Terminal sockets may carry the viewer's size (`?cols=&rows=`), applied on hello here.
  const route_ = path.split('?')[0];
  if (route_ === '/api/events') {
    // Like the server, the stream beats every heartbeat (`heartbeat=<ms>`, SPEC-191 LIV-001).
    ws.server = {
      open: () => {
        db.eventSockets.add(ws);
        ws.beat = setInterval(() => { db.heartbeats += 1; ws.push(JSON.stringify({ type: 'heartbeat' })); }, db.heartbeatMs);
      },
      closed: () => { db.eventSockets.delete(ws); clearInterval(ws.beat); },
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
    const session = db.logins.get(decodeURIComponent(m[1]));
    if (!session) { fatal('not_found', 'this sign-in has ended or expired', 4003); return ws; }
    ws.server = session;
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

/**
 * Ambient activity so live updates are visible: one bounded timer for the whole mock. The browser
 * request about 5 s after the start is awaited by task-done.e2e.ts ("…its pull request is read
 * late"): keep its delay well under 15 s and its reason, or change that test with it.
 * `browserask=0` leaves it out: its toast stays while pointed at and can cover the tool panel's
 * top, which tests that point there would hit on a slow machine (#349). `ambient=0` leaves all of
 * it out: the request (#10 into "Needs you") and the task changes regroup the sidebar's rows under
 * a resting pointer, which a test that hovers a row would race on a slow machine (#515).
 */
function startAmbient(db) {
  if (db.ambient || db.noAmbient) return;
  db.ambient = true;
  let tick = 0;
  setTimeout(() => {
    // Like the server, events reach only viewers of the sandbox (none without GitHub).
    if (db.noBrowserAsk || !db.sandboxes.some((s) => s.id === 'sb_studio')) return;
    emit(db, { type: 'browser.action_requested', sandbox_id: 'sb_studio', tab_id: 'tab-aws', reason: 'Sign in to AWS so task #10 can deploy', task_id: 'tsk_studio_10' });
  }, 5000);
  setInterval(() => {
    if (db.eventSockets.size === 0) return;
    tick += 1;
    // A task a supertask holds does not start (`tasks=graph`).
    const queued = db.tasks.find((t) => t.status === 'queued' && t.sandbox_id === 'sb_studio' && !t.archived && t.number === 8 && !(t.heldBy && t.heldBy.length));
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
  // The GitHub App was created in the other tab (SPEC-283): the connection now reads configured.
  db.finishGithubApp = () => { db.me.instance.github_configured = true; };
  db.finishOnboarding = (slug) => finishOnboarding(db, db.sandboxes.find((s) => s.slug === slug));
  // Someone else presses Start onboarding (another person or tab): the approval, without its hold.
  db.startOnboarding = (slug) => {
    const sandbox = db.sandboxes.find((s) => s.slug === slug);
    touch(db, preparationTask(db, sandbox), { status: 'starting', branch: 'yulius/prepare-repository' });
    sandbox.onboarding = 'preparing';
    resync(db);
  };
  // A request from outside this page (the API, an agent's tool, an automation), as another client
  // sends it; the page hears of it as of any change the server makes (`request('POST', path, body)`).
  db.request = async (method, path, body = null) => {
    const response = await handleFetch(db, path, { method, body: body ? JSON.stringify(body) : undefined, headers: { 'X-Yulius-CSRF': db.me.csrf_token } });
    resync(db);
    return response.status;
  };
  // The session of a task writes more (SPEC-179): `appendTranscript('sb_studio', 12, [{ role,
  // text } | { role, blocks }])`, numbered on from its last line; a task without one gets one.
  db.appendTranscript = (sandboxId, number, items) => {
    const task = db.tasks.find((t) => t.sandbox_id === sandboxId && t.number === number);
    const docs = sessionDocs(db);
    let doc = docs.find((d) => d.kind === 'task' && d.task_id === task.id);
    if (!doc) {
      doc = { id: `ses_task_${number}`, sandbox_id: sandboxId, kind: 'task', task_id: task.id, session_id: `live-${number}`, title: task.title, author: task.created_by, git_branch: task.branch, started_at: new Date().toISOString(), ended_at: null, imported_at: null, bytes: 0, redactions: 0, continued_in: null, transcript: [] };
      docs.push(doc);
    }
    let line = doc.transcript.length ? doc.transcript[doc.transcript.length - 1].line + 1 : 0;
    for (const item of items) {
      const blocks = item.blocks || [{ type: 'text', text: item.text, truncated: false }];
      doc.transcript.push({ line, role: item.role, at: item.at || (item.role === 'user' ? new Date().toISOString() : null), sidechain: false, blocks });
      line += 1;
    }
  };
  // The task's session is another one now (a continue started it): its own lines from 0.
  db.newTranscriptSession = (sandboxId, number, sessionId, items) => {
    const task = db.tasks.find((t) => t.sandbox_id === sandboxId && t.number === number);
    const doc = sessionDocs(db).find((d) => d.kind === 'task' && d.task_id === task.id);
    doc.session_id = sessionId;
    doc.transcript = [];
    db.appendTranscript(sandboxId, number, items);
  };
  db.touchTask = (sandboxId, number, changes) => touch(db, db.tasks.find((t) => t.sandbox_id === sandboxId && t.number === number), changes);
  // An administrator connects a Claude account elsewhere (another tab, SPEC-246): it is signed in.
  db.connectClaude = () => {
    db.nextId += 1;
    const account = { id: `acc_${db.nextId}`, provider: 'anthropic', label: 'Claude', email: 'signed-in@example.com', status: 'ok', usage: { primary: null, secondary: null, observed_at: null }, last_error: null, in_use_by: 0 };
    db.accounts.push(account);
    emit(db, { type: 'account.updated', account });
  };
  // A pull request changed on the server (`changePull('sb_studio', 520, { state: 'merged' })`):
  // announced like the server does, unless `announce` is false (a change Yulius did not see).
  db.changePull = (sandboxId, number, changes, announce = true) => {
    const task = db.tasks.find((t) => t.sandbox_id === sandboxId && t.delivery && t.delivery.pr && t.delivery.pr.number === number);
    Object.assign(task.delivery.pr, changes);
    if (changes.state === 'merged') task.delivery.stage = 'merged_integration';
    recount(db);
    if (announce) announcePulls(db, sandboxId);
  };
  db.announcePulls = (sandboxId) => announcePulls(db, sandboxId);
  // An agent asks for hosts of an agent secret (`secret_request_hosts`, SPEC-121): announced as
  // the server does (SPEC-209), unless `announce` is false (a change the page missed).
  db.askHosts = (name, hosts, by, announce = true) => {
    const secret = vaultOf(db).agent.find((s) => s.name === name);
    if (secret.cd_prod) throw new Error(PRODUCTION_SECRET); // refused, as by the server
    secret.requested_hosts = [...new Set([...secret.requested_hosts, ...hosts])];
    secret.requested_by = by;
    audit(db, 'vault.agent.request', secret.id, `${secret.name} → ${hosts.join(', ')}`);
    if (announce) db.announceSecrets(secret.sandbox);
  };
  db.announceSecrets = (sandboxId) => emit(db, { type: 'secrets.updated', sandbox_id: sandboxId });
  db.updateHolds = [];
  db.updateReads = () => db.updateHolds.length;
  db.updateReadsTaken = 0;
  // Releases the newest held update read.
  db.releaseUpdate = () => { const resolve = db.updateHolds.pop(); if (resolve) resolve(); };
  // The server says the update view changed (`update.updated`), as after a check.
  db.announceUpdate = () => emit(db, { type: 'update.updated', phase: null });
  // The data disk changes level (`disk.updated`, SPEC-276).
  db.setDisk = (level, used) => {
    db.disk = { level, used, total: db.disk.total };
    emit(db, { type: 'disk.updated', level, used, total: db.disk.total });
  };
  db.gitHolds = [];
  db.gitReads = () => db.gitHolds.length;
  db.gitRequests = 0; // Git overview reads the page asked for
  // `pullclock=manual`: the store's pull request reads run on a clock only `advanceClock(ms)` moves.
  const pullClock = params.get('pullclock') === 'manual' ? manualClock(Date.now()) : undefined;
  db.advanceClock = (ms) => { if (!pullClock) throw new Error('advanceClock needs pullclock=manual'); pullClock.advance(ms); };
  // Releases the held Git overview reads, the newest first (an older read answering last).
  db.releaseGit = () => { for (const resolve of db.gitHolds.splice(0).reverse()) resolve(); };
  // Simulate an outage and its end: __yuliusMock.goOffline() / .goOnline().
  db.goOffline = () => {
    db.offline = true;
    for (const ws of [...db.eventSockets]) ws.serverClose(1006, '');
    for (const session of db.sessions.values()) for (const ws of [...(session.viewers || [])]) ws.serverClose(1006, '');
  };
  db.goOnline = () => { db.offline = false; };
  // The event streams die without a close (a laptop asleep, a dropped proxy connection): nothing
  // more reaches them, heartbeats included; a new connection works (SPEC-191 LIV-002).
  db.silenceEvents = () => {
    for (const ws of [...db.eventSockets]) { db.eventSockets.delete(ws); clearInterval(ws.beat); }
  };
  db.heartbeatMs = params.has('heartbeat') ? Number(params.get('heartbeat')) : 20000;
  db.heartbeats = 0; // beats sent, so a test can silence the stream right after one
  // The machine rebooted: every Cockpit's process is gone, its viewers lose the connection.
  db.rebootCockpits = () => {
    for (const shell of [...db.shells.values()]) {
      if (!shell.cockpit) continue;
      db.shells.delete(shell.id);
      for (const ws of [...((shell.session && shell.session.viewers) || [])]) ws.serverClose(1006, '');
    }
  };
  // A usage limit moved every Cockpit to another subscription (SPEC-074): each ends as `switched`
  // and a new one, resuming its conversation, is already running in its place.
  // `switchCockpits(true)`: the start on the other subscription failed, and the next start fails
  // once for lack of capacity.
  db.switchCockpits = (failed = false) => {
    for (const shell of [...db.shells.values()]) {
      if (!shell.cockpit) continue;
      db.nextId += 1;
      const next = { ...shell, id: `ck_${shell.sandbox_id}_${db.nextId}`, session: null, resumed: true };
      if (shell.session) shell.session.end(null, 'switched');
      db.shells.delete(shell.id);
      if (failed) db.cockpitFailsOnce = true;
      else db.shells.set(next.id, next);
    }
  };
  // A teammate's save of a notepad (`team` or `sandbox:<sid>`); `announce: false` holds back its
  // event, as if it were still on its way (SPEC-041).
  db.editNote = (scope, body, by = 'alice', options) => putNote(db, scope, body, by, options);
  db.notePresence = (event) => emit(db, { type: 'note.presence', sandbox_id: null, client: '', hello: false, seq: 0, ...event });
  db.releasePresence = () => { db.holdPresence = false; for (const resolve of (db.presenceHolds || []).splice(0)) resolve(); };
  db.filePresence = (event) => emit(db, { type: 'file.presence', sandbox_id: 'sb_studio', root: 'sandbox', client: '', hello: false, seq: 0, ...event });
  // The other mock windows of this origin are teammates on the same server (SPEC-127).
  db.presenceChannel = typeof BroadcastChannel === 'function' ? new BroadcastChannel('yulius-mock-presence') : null;
  if (db.presenceChannel) db.presenceChannel.onmessage = (message) => emit(db, message.data);
  // A file's text in the mock repository (`repoFile('sb_studio', 'long.md', text)`), before it is opened.
  db.repoFile = (sandboxId, path, text, root = 'sandbox') => { repoTree(db, sandboxId, root).files.set(path, text); };
  // Someone else deletes a file of the mock repository (#571).
  db.removeRepoFile = (sandboxId, path, root = 'sandbox') => { repoTree(db, sandboxId, root).files.delete(path); };
  // A file of the mock repository grows past the editor's limit: read without its text (#571).
  db.repoFileTooLarge = (sandboxId, path, root = 'sandbox') => { const tree = repoTree(db, sandboxId, root); (tree.tooLarge = tree.tooLarge || new Set()).add(path); };
  db.resyncNow = () => resync(db);
  // Another administrator signs a subscription in again: its running sign-in ends `cancelled`.
  db.startLogin = (accountId) => startLogin(db, accountId);
  // The server forgets the sign-ins of `accountId` (ended and expired, or a restart) and drops
  // their viewers, who reconnect to a terminal it no longer has.
  db.forgetLogins = (accountId) => {
    for (const [id, session] of db.logins) {
      if (session.accountId !== accountId) continue;
      db.logins.delete(id);
      if (session.ws) session.ws.serverClose(1001, 'going away');
    }
  };
  db.keepaliveRequests = [];
  // `noteloads=fail`: the notes cannot be loaded from the start (a server still restarting).
  db.failNoteLoads = params.get('noteloads') === 'fail';
  // `browserask=0`: no sign-in request from the sandbox browser 5 s after the start (startAmbient).
  db.noBrowserAsk = params.get('browserask') === '0';
  // `ambient=0`: no ambient activity at all (startAmbient): no task changes by themselves.
  db.noAmbient = params.get('ambient') === '0';
  notesOf(db);
  return {
    kind: 'mock',
    // How often the event stream beats (`heartbeat=<ms>`); the page's watchdog follows it.
    eventsHeartbeatMs: db.heartbeatMs,
    // `pullgap=<ms>`: the store's least time between announced pull request reads (SPEC-108).
    pullGapMs: params.has('pullgap') ? Number(params.get('pullgap')) : undefined,
    pullClock,
    // Like window.fetch, an aborted signal (a timeout, a view that went away) rejects the request.
    fetch: (url, init = {}) => {
      if (init.keepalive) db.keepaliveRequests.push(url);
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
