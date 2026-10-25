import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Task #113 (SPEC-134): CD (deployment) next to CI. The mock's studio defines dev (a script with
// two secrets, one not allowed yet) and production (promotion, then a script); product has dev
// empty and production not defined; sandbox-3 is read-only (assets/mock.js, seedCd).

test('the CD page shows dev and production as their branches define them', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?section=cd'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('CI/CD');
  const dev = screen.getByRole('region', 'Dev environment');
  const prod = screen.getByRole('region', 'Production environment');
  await expect(dev.getByText('scripts/cd/deploy.sh dev')).toBeVisible();
  await expect(dev.getByText('waiting for a person to allow it')).toBeVisible();
  await expect(prod.getByText(/^quartio at 9f3c2a1 into main$/)).toBeVisible();
  await expect(prod.getByText(/Production is always deployed by a person/)).toBeVisible();
  await expect(screen.getByText('ssh: connect to host')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  // A failed dev deploy opens with its output, the secret's value never in it, in place of the
  // environments and the runs (SPEC-238): a view at its own address, not a dialog.
  await screen.getByRole('button', /Open the dev deploy at 7d1e0c9/).click();
  const view = screen.getByRole('region', 'Deploy');
  await expect(browser.locator('.run-output')).toHaveText(/\[secret:DEV_HOST\]/);
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd&run=cd_s2$/);
  await expect(dev).toBeHidden();
  expect(await browser.evaluate(() => document.querySelectorAll('dialog[open]').length)).toBe(0);
  // Ended: how long it took and when it started (review round 1).
  await expect(browser.locator('.run-view .run-facts')).toHaveText(/^took \S.* started .+ ago$/);
  expect(await axeViolations(browser)).toEqual([]);
  await view.getByRole('link', 'All runs').click();
  await expect(view).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd$/);
  // Back returns to the deploy, and the CD tab with it.
  await browser.evaluate(() => history.back());
  await expect(screen.getByRole('region', 'Deploy')).toBeVisible();
  await expect(screen.getByRole('radio', 'CD (deployment)')).toHaveAttribute('aria-checked', 'true');
  await screen.getByRole('region', 'Deploy').getByRole('link', 'All runs').click();

  // Dev needs a secret nobody allowed yet: nothing runs, and the page says why.
  await dev.getByRole('button', 'Deploy dev').click();
  await screen.getByRole('dialog', 'Deploy dev?').getByRole('button', 'Deploy dev').click();
  // SPEC-189 CDS-003: the refusal is said in the Deploy view the confirmation opened.
  const refused = screen.getByRole('region', 'Deploy');
  await expect(refused.getByText(/DEV_HOST allows using it outside the proxy/)).toBeVisible();
  await expect(refused.getByText('Not started')).toBeVisible();
  await expect(refused.getByText('Starting')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);
});

// #704 (SPEC-237 CAU-003): production names a secret agents use (its .env copies). Nobody can
// allow it, so the page says production never gets it, not that it waits for a person.
test('production says a secret agents use is never for production', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?section=cd'));
  const prod = screen.getByRole('region', 'Production environment');
  await expect(prod.getByText('Promotes')).toBeVisible();
  await browser.evaluate(() => {
    const w = window as unknown as { __yuliusMock: { cd: Record<string, { environments: { secrets: { name: string; state: string }[] }[] }> } };
    w.__yuliusMock.cd.sb_studio.environments[1].secrets.push({ name: 'PAY_KEY', state: 'agents_use' });
  });
  // Leaving and coming back reads the environments again.
  await screen.getByRole('radio', 'CI (tests)').click();
  await expect(screen.getByRole('heading', 'Repository hygiene')).toBeVisible();
  await screen.getByRole('radio', 'CD (deployment)').click();
  await expect(prod.getByText('agents use it: production never gets it')).toBeVisible();
  await expect(prod.getByText(/Give production a separate secret/)).toBeVisible();
  // The other way out, as the Secrets page and the server's refusal say.
  await expect(prod.getByText(/or remove its \.env copies and hosts first/)).toBeVisible();
  await expect(prod.getByText('waiting for a person to allow it')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  // The mock refuses a production deploy as the server does (cd::production_secrets_need_their_own_allowance).
  await prod.getByRole('button', 'Deploy production…').click();
  const confirm = screen.getByRole('dialog', 'Deploy production?');
  await confirm.getByRole('textbox', 'Type studio to confirm').fill('studio');
  await confirm.getByRole('button', 'Deploy production').click();
  const refused = screen.getByRole('region', 'Deploy');
  await expect(refused.getByText(/agents use PAY_KEY/)).toBeVisible();
  // The server's whole sentence, with the environment's own table (#788, SPEC-314).
  await expect(refused.getByText(/separate secret of its own in \[cd\.prod\] secrets of yulius\.toml, added on the Secrets page \(\S+\/s\/studio\/secrets\?section=agents\); or a person first revokes the \.env copies and hosts of PAY_KEY$/)).toBeVisible();
  await expect(refused.getByText('Not started')).toBeVisible();
});

// #788 (SPEC-314 RNE-002): with a secret agents use and one waiting for a person, the mock's
// refusal names both in one sentence, as the server's does
// (cd::production_never_gets_a_secret_bound_to_hosts).
test('a production refusal also names the secret waiting for a person, as the server does', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?section=cd'));
  const prod = screen.getByRole('region', 'Production environment');
  await expect(prod.getByText('Promotes')).toBeVisible();
  await browser.evaluate(() => {
    const w = window as unknown as { __yuliusMock: { cd: Record<string, { environments: { secrets: { name: string; state: string }[] }[] }> } };
    w.__yuliusMock.cd.sb_studio.environments[1].secrets.push({ name: 'PAY_KEY', state: 'agents_use' }, { name: 'MAIL_KEY', state: 'not_allowed' });
  });
  await screen.getByRole('radio', 'CI (tests)').click();
  await expect(screen.getByRole('heading', 'Repository hygiene')).toBeVisible();
  await screen.getByRole('radio', 'CD (deployment)').click();
  await expect(prod.getByText('agents use it: production never gets it')).toBeVisible();
  await prod.getByRole('button', 'Deploy production…').click();
  const confirm = screen.getByRole('dialog', 'Deploy production?');
  await confirm.getByRole('textbox', 'Type studio to confirm').fill('studio');
  await confirm.getByRole('button', 'Deploy production').click();
  const refused = screen.getByRole('region', 'Deploy');
  await expect(refused.getByText(/or a person first revokes the \.env copies and hosts of PAY_KEY\. Also, a person who manages MAIL_KEY allows it for production deploys$/)).toBeVisible();
  await expect(refused.getByText('Not started')).toBeVisible();
});

// Task #243 (SPEC-257): "dev/prod is default but we can [have a] different prod": a staging
// machine next to dev and the hosted platform next to production, each its own card, deployed by
// its stage's rules.
test('named environments show their stage and deploy by its rules', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?section=cd'));
  await expect(screen.getByRole('region', 'Production environment')).toBeVisible();
  await browser.evaluate(() => {
    type Env = Record<string, unknown>;
    const w = window as unknown as { __yuliusMock: { cd: Record<string, { environments: Env[] }> } };
    const envs = w.__yuliusMock.cd.sb_studio.environments;
    const base = { problem: null, promote: false, secrets: [], head: '4be81c0d2a9f1e2d' };
    envs.splice(1, 0, { ...base, name: 'staging', label: 'staging', stage: 'dev', state: 'ready', branch: 'quartio', head: '9f3c2a1b7e4d5f6a', about: 'the staging VM', deploy: 'scripts/cd/deploy.sh staging', verify: null });
    envs.push({ ...base, name: 'platform', label: 'platform', stage: 'prod', state: 'ready', branch: 'main', about: 'the hosted platform', deploy: 'scripts/cd/deploy.sh platform', verify: 'scripts/cd/verify.sh platform' });
  });
  // Leaving and coming back reads the environments again.
  await screen.getByRole('radio', 'CI (tests)').click();
  await expect(screen.getByRole('heading', 'Repository hygiene')).toBeVisible();
  await screen.getByRole('radio', 'CD (deployment)').click();
  const staging = screen.getByRole('region', 'Staging environment');
  const platform = screen.getByRole('region', 'Platform environment');
  await expect(staging.getByText('dev stage')).toBeVisible();
  await expect(platform.getByText('production stage')).toBeVisible();
  await expect(platform.getByText(/Platform, a production environment, is always deployed by a person/)).toBeVisible();
  await expect(screen.getByRole('region', 'Dev environment').getByText('dev stage')).toBeHidden();
  const names = await browser.evaluate(() => [...document.querySelectorAll('.cd-env h2')].map((e) => e.textContent));
  expect(names).toEqual(['Dev', 'Staging', 'Production', 'Platform']);
  expect(await axeViolations(browser)).toEqual([]);

  // Production's rules: the slug typed in the dialog the platform's button opens.
  await platform.getByRole('button', 'Deploy platform…').click();
  const confirm = screen.getByRole('dialog', 'Deploy platform?');
  await expect(confirm.getByText('Run the deploy command at the new main head: scripts/cd/deploy.sh platform')).toBeVisible();
  await confirm.getByRole('textbox', 'Type studio to confirm').fill('studio');
  await confirm.getByRole('button', 'Deploy platform').click();
  const view = screen.getByRole('region', 'Deploy');
  await expect(view.getByText('Platform', { exact: true })).toBeVisible();
  // The view opens before the request is answered (SPEC-189): wait until the mock got it.
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __yuliusMock: { lastCdDeploy?: { confirm?: string } } }).__yuliusMock.lastCdDeploy?.confirm ?? null)).toBe('studio');
  await view.getByRole('link', 'All runs').click();

  // Dev's rules: a plain confirmation.
  await staging.getByRole('button', 'Deploy staging').click();
  const dev = screen.getByRole('dialog', 'Deploy staging?');
  await expect(dev.getByText(/^Runs \[cd\.staging\] from quartio at 9f3c2a1/)).toBeVisible();
  await dev.getByRole('button', 'Deploy staging').click();
  await expect(screen.getByRole('region', 'Deploy').getByText('Staging', { exact: true })).toBeVisible();
});

// Task #145 (SPEC-151): "make CI/CD one tab with a tab like in secrets team/agent". One sidebar
// entry, one page; its sections are the Secrets page's tabs, kept in ?section=.
test('CI and CD are two tabs of the CI/CD page, like the Secrets sections', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci'));
  const nav = screen.getByRole('navigation', 'Sandbox');
  await expect(nav.getByRole('link', 'CI/CD')).toHaveAttribute('aria-current', 'page');
  await expect(screen.getByRole('radiogroup', 'Section')).toBeVisible();
  await expect(screen.getByRole('button', 'Check for commits')).toBeVisible();
  await expect(screen.getByRole('button', 'Set up CD with an agent')).toBeHidden();

  // A click shows CD in place, with its own header action; the address names the tab.
  await screen.getByRole('radio', 'CD (deployment)').click();
  await expect(screen.getByRole('radio', 'CD (deployment)')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('region', 'Dev environment')).toBeVisible();
  await expect(screen.getByRole('button', 'Set up CD with an agent')).toBeVisible();
  await expect(screen.getByRole('button', 'Check for commits')).toBeHidden();
  await expect(screen.getByRole('heading', 'Repository hygiene')).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd$/);
  await expect(nav.getByRole('link', 'CI/CD')).toHaveAttribute('aria-current', 'page');
  expect(await axeViolations(browser)).toEqual([]);

  // The tab survives a reload; an arrow key goes back to CI and keeps the focus on the tabs.
  await browser.reload();
  await expect(screen.getByRole('radio', 'CD (deployment)')).toHaveAttribute('aria-checked', 'true');
  await screen.getByRole('radio', 'CD (deployment)').press('ArrowLeft');
  await expect(screen.getByRole('radio', 'CI (tests)')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('radio', 'CI (tests)')).toBeFocused();
  await expect(screen.getByRole('heading', 'Repository hygiene')).toBeVisible();
  await expect(screen.getByRole('region', 'Dev environment')).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci$/);

  // The sidebar's link goes back to the tab last shown (SPEC-113), from another page.
  await screen.getByRole('radio', 'CI (tests)').press('ArrowRight');
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd$/);
  await nav.getByRole('link', 'Git').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  await nav.getByRole('link', 'CI/CD').click();
  await expect(screen.getByRole('region', 'Dev environment')).toBeVisible();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd$/);
});

// The CD page's own address, in agents' messages and older links, opens the CD tab (SPEC-151 CCD-004).
test('the earlier CD address opens the CD tab, and a deploy with it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/cd'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('CI/CD');
  await expect(screen.getByRole('radio', 'CD (deployment)')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('region', 'Dev environment')).toBeVisible();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd$/);
  await expect(screen.getByRole('navigation', 'Sandbox').getByRole('link', 'CI/CD')).toHaveAttribute('aria-current', 'page');

  await app.open(mock('/s/studio/cd?run=cd_s2'));
  const view = screen.getByRole('region', 'Deploy');
  await expect(browser.locator('.run-output')).toHaveText(/\[secret:DEV_HOST\]/);
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd&run=cd_s2$/);
  await view.getByRole('link', 'All runs').click();
  await expect(view).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd$/);
  await expect(screen.getByRole('radio', 'CD (deployment)')).toHaveAttribute('aria-checked', 'true');
});

// SPEC-151 CCD-003, review round 1: the tab left is gone. Its late answers draw nothing, and a
// deploy it started opens no view over the other tab nor names itself in the address.
test('a tab left behind draws nothing when its answers come late', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?section=cd', '&hold=/api/sandboxes/sb_studio/cd'));
  await screen.getByRole('radio', 'CI (tests)').click();
  await expect(screen.getByRole('heading', 'Repository hygiene')).toBeVisible();
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());
  await expect(screen.getByRole('heading', '3 jobs from yulius.toml on the integration branch')).toBeVisible();
  await expect(screen.getByRole('region', 'Dev environment')).toBeHidden();
  await expect(screen.getByRole('button', 'Check for commits')).toBeVisible();
  await expect(screen.getByRole('button', 'Set up CD with an agent')).toBeHidden();

  // A production deploy whose answer comes after the person went to CI.
  await screen.getByRole('radio', 'CD (deployment)').click();
  await browser.evaluate(() => {
    const w = window as unknown as { __yuliusMock: { holdCdDeploy?: Promise<void> }; __releaseDeploy?: () => void };
    w.__yuliusMock.holdCdDeploy = new Promise((resolve) => { w.__releaseDeploy = resolve; });
  });
  await screen.getByRole('region', 'Production environment').getByRole('button', 'Deploy production…').click();
  const confirm = screen.getByRole('dialog', 'Deploy production?');
  await confirm.getByRole('textbox', 'Type studio to confirm').fill('studio');
  await confirm.getByRole('button', 'Deploy production').click();
  await expect(confirm).toBeHidden();
  // The person leaves the starting deploy's view for the CI tab.
  await expect(screen.getByRole('region', 'Deploy')).toBeVisible();
  await screen.getByRole('radio', 'CI (tests)').click();
  await expect(screen.getByRole('region', 'Deploy')).toBeHidden();
  await expect(screen.getByRole('heading', 'Repository hygiene')).toBeVisible();
  await browser.evaluate(() => { (window as unknown as { __releaseDeploy: () => void }).__releaseDeploy(); });
  // The deploy itself went out: the CD tab lists it.
  await screen.getByRole('radio', 'CD (deployment)').click();
  await expect(browser.locator('.item-card:has-text("Production")').first()).toContainText('Deploying');
  await screen.getByRole('radio', 'CI (tests)').click();
  await expect(screen.getByRole('region', 'Deploy')).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci$/);
});

test('production deploys only after the project slug is typed', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/cd'));
  await screen.getByRole('region', 'Production environment').getByRole('button', 'Deploy production…').click();
  const confirm = screen.getByRole('dialog', 'Deploy production?');
  await expect(confirm.getByText(/Promote the integration branch at 9f3c2a1 into main/)).toBeVisible();
  const go = confirm.getByRole('button', 'Deploy production');
  await expect(go).toBeDisabled();
  await confirm.getByRole('textbox', 'Type studio to confirm').fill('studi');
  await expect(go).toBeDisabled();
  await confirm.getByRole('textbox', 'Type studio to confirm').fill('studio');
  expect(await axeViolations(browser)).toEqual([]);
  await go.click();
  const run = screen.getByRole('region', 'Deploy');
  await expect(run.getByText('Deploying')).toBeVisible();
  await expect(run.getByText(/promoted the integration branch|by mkarp/)).toBeVisible();
  // SPEC-238 CRV-003: its summary counts the time it has run, by itself, and names its stage.
  await expect(browser.locator('.run-view .run-time')).toHaveText(/^\d+ s so far$/);
  await expect(browser.locator('.run-view .run-summary')).toContainText('Now: build');
  // The time the person kept the confirmation open goes with it, as the run's confirmation wait
  // (SPEC-137 STG-302).
  const sent = await browser.evaluate(() => (window as unknown as { __yuliusMock: { lastCdDeploy?: { confirm_ms?: number } } }).__yuliusMock.lastCdDeploy?.confirm_ms ?? null);
  expect(typeof sent === 'number' && sent > 0).toBe(true);
});

type Held = Window & { __yuliusMock: { holdCdDeploy?: Promise<void> }; __releaseDeploy?: () => void; __startView?: Element | null };
const holdDeploy = (browser: { evaluate<T>(fn: () => T): Promise<T> }) => browser.evaluate(() => {
  const w = window as unknown as Held;
  w.__yuliusMock.holdCdDeploy = new Promise((resolve) => { w.__releaseDeploy = resolve; });
});
const releaseDeploy = (browser: { evaluate<T>(fn: () => T): Promise<T> }) => browser.evaluate(() => { (window as unknown as Held).__releaseDeploy!(); });
const elapsedSeconds = (browser: { evaluate<T>(fn: () => T): Promise<T> }) => browser.evaluate(() => {
  const text = document.querySelector('.cd-starting-elapsed')?.textContent || '';
  const m = /(\d+) s/.exec(text);
  return m ? Number(m[1]) : null;
});

// Task #174 (SPEC-189, #585): "when I clicked deploy nothing happened for a while and I thought it
// broke". The server answers a deploy after its checks and promotion (seconds to a minute); the
// page says at once that it is starting and what it does, then shows the run in the same view
// (SPEC-238: a view in the CD tab, no longer a dialog).
test('a confirmed deploy opens its view at once and says what is starting', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/cd'));
  await holdDeploy(browser);
  await screen.getByRole('region', 'Production environment').getByRole('button', 'Deploy production…').click();
  const confirm = screen.getByRole('dialog', 'Deploy production?');
  await confirm.getByRole('textbox', 'Type studio to confirm').fill('studio');
  await confirm.getByRole('button', 'Deploy production').click();

  // Before any answer: what is starting, what it does now, and time that moves.
  const dialog = screen.getByRole('region', 'Deploy');
  await expect(dialog.getByText('Starting')).toBeVisible();
  await expect(dialog.getByText(/Checking, then promoting quartio at 9f3c2a1 into main/)).toBeVisible();
  await expect(dialog.getByText(/stages and output appear here/)).toBeVisible();
  await expect(dialog.getByText('main · by mkarp')).toBeVisible();
  // Its state is said in a live region filled after it exists, so a screen reader hears it.
  expect(await browser.evaluate(() => document.querySelector('.run-view [role="status"]')?.textContent || '')).toMatch(/^Starting: Checking, then promoting/);
  expect(await elapsedSeconds(browser)).not.toBeNull();
  const first = (await elapsedSeconds(browser))!;
  await expect.poll(() => elapsedSeconds(browser), { timeout: 5000 }).toBeGreaterThan(first);
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd$/);
  expect(await axeViolations(browser)).toEqual([]);
  await browser.evaluate(() => { (window as unknown as Held).__startView = document.querySelector('.run-view'); });

  // The answer turns the same view into the run, without a toast over it.
  await releaseDeploy(browser);
  await expect(dialog.getByText('Deploying')).toBeVisible();
  await expect(browser.locator('.run-output')).toHaveText(/to production/);
  await expect(dialog.getByText('Starting')).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd&run=cd_\d+$/);
  expect(await browser.evaluate(() => {
    const kept = (window as unknown as Held).__startView;
    return Boolean(kept && kept.isConnected && kept.querySelector('.run-output'));
  })).toBe(true);
  await expect(screen.getByText('Deploying production.')).toBeHidden();
  expect(await browser.evaluate(() => document.querySelector('.cd-starting-elapsed') === null)).toBe(true);
});

test('a starting deploy left for all runs goes on, and its toast opens it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/cd'));
  await holdDeploy(browser);
  await screen.getByRole('region', 'Production environment').getByRole('button', 'Deploy production…').click();
  const confirm = screen.getByRole('dialog', 'Deploy production?');
  await confirm.getByRole('textbox', 'Type studio to confirm').fill('studio');
  await confirm.getByRole('button', 'Deploy production').click();
  const dialog = screen.getByRole('region', 'Deploy');
  await expect(dialog.getByText('Starting')).toBeVisible();
  await expect(dialog.getByText(/Going back to all runs does not stop it/)).toBeVisible();
  await dialog.getByRole('link', 'All runs').press('Enter');
  await expect(dialog).toBeHidden();
  await expect(screen.getByRole('region', 'Dev environment')).toBeVisible();
  // The keyboard goes back to the Deploy button that started it (review round 2).
  await expect(screen.getByRole('region', 'Production environment').getByRole('button', 'Deploy production…')).toBeFocused();

  await releaseDeploy(browser);
  await expect(screen.getByText('Deploying production.')).toBeVisible();
  await expect(dialog).toBeHidden();
  await expect(browser.locator('.item-card:has-text("Production")').first()).toContainText('Deploying');
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd$/);
  await browser.locator('.toast:has-text("Deploying production.") button:has-text("Open")').click();
  await expect(dialog.getByText('Deploying')).toBeVisible();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd&run=cd_\d+$/);

  // The toast's Open still opens the run once the person went to the CI tab.
  await app.open(mock('/s/studio/cd'));
  await holdDeploy(browser);
  await screen.getByRole('region', 'Production environment').getByRole('button', 'Deploy production…').click();
  await confirm.getByRole('textbox', 'Type studio to confirm').fill('studio');
  await confirm.getByRole('button', 'Deploy production').click();
  await expect(dialog.getByText('Starting')).toBeVisible();
  await dialog.getByRole('link', 'All runs').click();
  await expect(dialog).toBeHidden();
  await releaseDeploy(browser);
  await expect(screen.getByText('Deploying production.')).toBeVisible();
  await screen.getByRole('radio', 'CI (tests)').click();
  await expect(screen.getByRole('heading', 'Repository hygiene')).toBeVisible();
  await browser.locator('.toast:has-text("Deploying production.") button:has-text("Open")').click();
  await expect(dialog.getByText('Deploying')).toBeVisible();
  await expect(screen.getByRole('radio', 'CD (deployment)')).toHaveAttribute('aria-checked', 'true');
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd&run=cd_\d+$/);
});

test('a refusal after its view was left is a toast; a page left with it open draws nothing', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/cd'));
  // Dev is refused for a secret nobody allowed, after the server's checks took their time.
  await holdDeploy(browser);
  await screen.getByRole('region', 'Dev environment').getByRole('button', 'Deploy dev').click();
  await screen.getByRole('dialog', 'Deploy dev?').getByRole('button', 'Deploy dev').click();
  const dialog = screen.getByRole('region', 'Deploy');
  await expect(dialog.getByText(/Checking quartio at 9f3c2a1/)).toBeVisible();
  await dialog.getByRole('link', 'All runs').click();
  await expect(dialog).toBeHidden();
  await releaseDeploy(browser);
  await expect(screen.getByText(/Couldn’t deploy dev: nothing ran: a person who manages DEV_HOST/)).toBeVisible();
  await expect(dialog).toBeHidden();

  // Another page while production starts: its view goes with the CD tab, and nothing comes back.
  await holdDeploy(browser);
  await screen.getByRole('region', 'Production environment').getByRole('button', 'Deploy production…').click();
  const confirm = screen.getByRole('dialog', 'Deploy production?');
  await confirm.getByRole('textbox', 'Type studio to confirm').fill('studio');
  await confirm.getByRole('button', 'Deploy production').click();
  await expect(dialog.getByText('Starting')).toBeVisible();
  await browser.evaluate(() => { location.hash = '#/s/studio/git'; });
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  await expect(dialog).toBeHidden();
  expect(await browser.evaluate(() => document.querySelector('.cd-starting-elapsed') === null)).toBe(true);
  await releaseDeploy(browser);
  await screen.getByRole('navigation', 'Sandbox').getByRole('link', 'CI/CD').click();
  await expect(browser.locator('.item-card:has-text("Production")').first()).toContainText('Deploying');
  await expect(dialog).toBeHidden();
  await expect(screen.getByText('Deploying production.')).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd$/);
});

// SPEC-238 CRV-004: "go back easily after this is closed and we see some dynamic summary all the
// time". Above the tabs, on both, the page says what runs now, with a time that grows by itself,
// and each one opens its run; with nothing running, how the latest CI run and deploy ended.
const nowSeconds = (browser: { evaluate<T>(fn: () => T): Promise<T> }) => browser.evaluate(() => {
  const text = document.querySelector('.ci-now .run-time')?.textContent || '';
  const m = /^(?:(\d+) min )?(\d+) s so far$/.exec(text);
  return m ? Number(m[1] || 0) * 60 + Number(m[2]) : null;
});

test('the Now summary says what runs on both tabs and opens it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?section=cd'));
  // The deploy started below and the running CI run keep running for the whole test, however long
  // it takes (the CI run used to end 20 s in, before the phone-width check).
  await browser.evaluate(() => {
    const m = (window as unknown as { __yuliusMock: { cdRunMs: number; ciRunMs: number } }).__yuliusMock;
    m.cdRunMs = 600000;
    m.ciRunMs = 600000;
  });
  const now = screen.getByRole('region', 'Now');
  await expect(now.getByRole('link', 'Running CI quartio at 9f3c2a1')).toBeVisible();
  await expect(now).toContainText('3 jobs · 1 passed · 2 running');
  // Each link's name is its visible text (WCAG 2.5.3, review round 1).
  await expect(now.getByRole('link', 'Running CI quartio at 9f3c2a1')).toHaveText('Running CI quartio at 9f3c2a1');
  await expect.poll(() => nowSeconds(browser)).not.toBeNull();
  const first = (await nowSeconds(browser))!;
  await expect.poll(() => nowSeconds(browser), { timeout: 5000 }).toBeGreaterThan(first);
  expect(await axeViolations(browser)).toEqual([]);

  // A production deploy joins it, from the CD tab, and stays there on the CI tab.
  await screen.getByRole('region', 'Production environment').getByRole('button', 'Deploy production…').click();
  const confirm = screen.getByRole('dialog', 'Deploy production?');
  await confirm.getByRole('textbox', 'Type studio to confirm').fill('studio');
  await confirm.getByRole('button', 'Deploy production').click();
  await expect(screen.getByRole('region', 'Deploy').getByText('Deploying')).toBeVisible();
  await expect(now.getByRole('link', /^Deploying CD Production at 4be81c0$/)).toBeVisible();
  await screen.getByRole('radio', 'CI (tests)').click();
  await expect(now.getByRole('link', /^Deploying CD Production at 4be81c0$/)).toBeVisible();

  // A failed read of one overview keeps what it showed of that one (review round 1).
  type Mock = { __yuliusMock: { failPath: string | null; failedReads: number; holdPath: string | null; held: Promise<void> | null }; __releaseNow?: () => void };
  await browser.evaluate(() => { (window as unknown as Mock).__yuliusMock.failPath = '/api/sandboxes/sb_studio/cd'; });
  await expect.poll(() => browser.evaluate(() => (window as unknown as Mock).__yuliusMock.failedReads), { timeout: 10000 }).toBeGreaterThan(0);
  await expect(now.getByRole('link', /^Deploying CD Production at 4be81c0$/)).toBeVisible();
  await browser.evaluate(() => { (window as unknown as Mock).__yuliusMock.failPath = null; });

  // Its link opens the run, on its tab, the summary drawn at once as it was (its reads held) so
  // the tabs stay put; Back returns to where the person was.
  await browser.evaluate(() => {
    const w = window as unknown as Mock;
    w.__yuliusMock.holdPath = '/api/sandboxes/sb_studio/cd';
    w.__yuliusMock.held = new Promise((resolve) => { w.__releaseNow = resolve; });
  });
  await now.getByRole('link', 'Running CI quartio at 9f3c2a1').click();
  await expect(screen.getByRole('region', 'CI run').getByText('quartio')).toBeVisible();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?run=ci_s4$/);
  await expect(browser.locator('.ci-now .ci-now-row')).toHaveCount(2);
  await browser.evaluate(() => { const w = window as unknown as Mock; w.__yuliusMock.holdPath = null; w.__releaseNow!(); });
  // The link of the run in view adds no history entry.
  const entries = await browser.evaluate(() => history.length);
  await now.getByRole('link', 'Running CI quartio at 9f3c2a1').click();
  expect(await browser.evaluate(() => history.length)).toBe(entries);
  await expect(screen.getByRole('radio', 'CI (tests)')).toHaveAttribute('aria-checked', 'true');
  await now.getByRole('link', /^Deploying CD Production/).click();
  await expect(screen.getByRole('region', 'Deploy')).toBeVisible();
  await expect(screen.getByRole('radio', 'CD (deployment)')).toHaveAttribute('aria-checked', 'true');
  await browser.evaluate(() => history.back());
  await expect(screen.getByRole('region', 'CI run')).toBeVisible();

  // On a phone the summary and the run view wrap, the page never scrolls sideways.
  await browser.setViewport({ width: 390, height: 844 });
  await expect(now.getByRole('link', 'Running CI quartio at 9f3c2a1')).toBeVisible();
  expect(await browser.evaluate(() => { const page = document.querySelector('.page')!; return page.scrollWidth <= page.clientWidth; })).toBe(true);
});

// Review round 1 (CRV-004): the summary reads nothing while the page is hidden and nothing once it
// is gone; its timers are counted with stubbed setTimeout/setInterval (unit, in the page).
test('the Now summary stops its reads and clock when it goes and reads nothing while hidden (unit, in the page)', async ({ app, browser }) => {
  // A page with no poller of its own (review round 2): only the unit's timers land in the stubs.
  await app.open(mock('/s/studio/automations', '&ambient=0&browserask=0'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/pages/cinow.js';
    const { mountNowSummary } = await import(url);
    const timeouts = new Map<number, () => void>();
    const intervals = new Set<number>();
    let next = 1;
    const real = { st: window.setTimeout.bind(window), ct: window.clearTimeout.bind(window), si: window.setInterval.bind(window), ci: window.clearInterval.bind(window) };
    // Only the module's own timers are stubbed (review round 2): the page's other timers keep
    // running for real, so none of them lands in these maps. Stub ids are negative.
    const mine = () => (new Error().stack || '').includes('/pages/cinow.js');
    Object.assign(window, {
      setTimeout: (fn: () => void, ms?: number) => { if (!mine()) return real.st(fn, ms); const id = -next++; timeouts.set(id, fn); return id; },
      clearTimeout: (id: number) => { if (id < 0) timeouts.delete(id); else real.ct(id); },
      setInterval: (fn: () => void, ms?: number) => { if (!mine()) return real.si(fn, ms); const id = -next++; intervals.add(id); return id; },
      clearInterval: (id: number) => { if (id < 0) intervals.delete(id); else real.ci(id); },
    });
    const reads: string[] = [];
    const running = { runs: [{ id: 'r1', label: 'dev', sha: 'abcdef0', state: 'running', jobs: [], started_at: new Date().toISOString() }] };
    const ctx = { api: { get: async (path: string) => { reads.push(path); return path.endsWith('/ci') ? running : { runs: [] }; } } };
    const settle = () => new Promise((resolve) => real.st(resolve, 0));
    try {
      const now = mountNowSummary(ctx, { id: 'sb_x', slug: 'x' });
      await settle();
      const first = reads.length;
      // Hidden: the next read is skipped, and it waits again.
      Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
      const [id, fn] = [...timeouts][0];
      timeouts.delete(id);
      fn();
      await settle();
      const whileHidden = reads.length - first;
      const rescheduled = timeouts.size;
      Object.defineProperty(document, 'hidden', { configurable: true, get: () => false });
      now.destroy();
      const ended = { clock: intervals.size, pending: timeouts.size };
      // Mounted again, it draws its last reads at once; a minute later it waits for a fresh read
      // instead (review round 2: kept reads have an age limit).
      const again = mountNowSummary(ctx, { id: 'sb_x', slug: 'x' });
      const drawnAtOnce = Boolean(again.el.querySelector('.ci-now-list'));
      again.destroy();
      const realNow = Date.now;
      Date.now = () => realNow() + 61_000;
      let drawnWhenOld: boolean;
      try {
        const late = mountNowSummary(ctx, { id: 'sb_x', slug: 'x' });
        drawnWhenOld = Boolean(late.el.querySelector('.ci-now-list'));
        late.destroy();
      } finally {
        Date.now = realNow;
      }
      // A failed read does not make what was kept younger (review round 3): 50 s on, both reads
      // fail; 61 s after the last good read, a mount draws nothing of it.
      const failing = { api: { get: async () => { throw new Error('down'); } } };
      let drawnAfterFailures: boolean;
      try {
        Date.now = () => realNow() + 50_000;
        const down = mountNowSummary(failing, { id: 'sb_x', slug: 'x' });
        await settle();
        down.destroy();
        Date.now = () => realNow() + 61_000;
        const after = mountNowSummary(failing, { id: 'sb_x', slug: 'x' });
        drawnAfterFailures = Boolean(after.el.querySelector('.ci-now-list'));
        after.destroy();
      } finally {
        Date.now = realNow;
      }
      return { first, whileHidden, rescheduled, ...ended, drawnAtOnce, drawnWhenOld, drawnAfterFailures };
    } finally {
      Object.assign(window, { setTimeout: real.st, clearTimeout: real.ct, setInterval: real.si, clearInterval: real.ci });
      delete (document as unknown as { hidden?: boolean }).hidden;
    }
  });
  expect(result).toEqual({ first: 2, whileHidden: 0, rescheduled: 1, clock: 0, pending: 0, drawnAtOnce: true, drawnWhenOld: false, drawnAfterFailures: false });
});

test('with nothing running the Now summary says how the latest runs ended', async ({ app, screen }) => {
  await app.open(mock('/s/sandbox-3/ci'));
  const now = screen.getByRole('region', 'Now');
  await expect(now).toContainText('Nothing is running.');
  await expect(now.getByRole('link', /^Last CI run: dev at 77aa01b passed/)).toBeVisible();
  await app.open(mock('/s/product/ci?section=cd'));
  await expect(screen.getByRole('region', 'Now')).toHaveText('Nothing is running.');
});

test('members deploy dev but never production, and an empty dev says so', async ({ app, screen }) => {
  await app.open(mock('/s/studio/cd', '&role=member'));
  await expect(screen.getByRole('button', 'Deploy production…')).toBeDisabled();
  await expect(screen.getByRole('button', 'Deploy production…')).toHaveAttribute('title', 'Only an administrator deploys production');
  await expect(screen.getByRole('button', 'Deploy dev')).toBeEnabled();

  await app.open(mock('/s/product/cd'));
  const dev = screen.getByRole('region', 'Dev environment');
  await expect(dev.getByText('Nothing to deploy yet')).toBeVisible();
  await expect(dev.getByText('no dev machine yet')).toBeVisible();
  await expect(dev.getByRole('button', 'Deploy dev')).toBeDisabled();
  await expect(screen.getByRole('region', 'Production environment').getByText('Not defined')).toBeVisible();
  await expect(screen.getByText('No deploys yet')).toBeVisible();
});

test('read access watches; CD is set up with an agent through a task', async ({ app, browser, screen }) => {
  await app.open(mock('/s/sandbox-3/cd'));
  await expect(screen.getByRole('button', 'Set up CD with an agent')).toBeDisabled();

  await app.open(mock('/s/product/cd'));
  // The server ships the Infra bootstrap workflow (catalog/workflows); the shared mock lists fewer,
  // so the composer's menus elsewhere keep their size.
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { workflows: object[] } }).__yuliusMock.workflows.push({
    id: 'wf-infra-bootstrap', sandbox: null, name: 'Infra bootstrap', description: 'Foundations and CD (deployment).',
    when_to_use: 'CD must be set up or changed.', steps: ['Survey', 'CD dev and production defined'], instructions: 'Follow infra-bootstrap.',
    gates: { require_spec: true, require_tests: true }, scope: 'instance', builtin: true,
  }));
  await screen.getByRole('button', 'Set up CD with an agent').click();
  const setup = screen.getByRole('dialog', 'Set up CD with an agent');
  await expect(setup.getByRole('textbox', 'What the agent should do')).toHaveValue(/Define \[cd\.dev\] and \[cd\.prod\] explicitly/);
  expect(await axeViolations(browser)).toEqual([]);
  await setup.getByRole('button', 'Start task').click();
  await expect(browser).toHaveURL(/#\/s\/product\/t\/\d+$/);
});

// Task #146 (#501): the cards' layout. The deploy button stays at the top right of its card at
// every width (only the title and its badge wrap), a deploy that cannot run is not styled as the
// card's main action, and a promoting production names the branch and commit it promotes.
type DeployHead = { name: string; sameRow: boolean; fromRight: number; primary: boolean; disabled: boolean };
const deployHeads = (browser: { evaluate<T>(fn: () => T): Promise<T> }) => browser.evaluate((): DeployHead[] => [...document.querySelectorAll('.cd-env')].map((card) => {
  const title = card.querySelector('.card-title')!.getBoundingClientRect();
  const button = [...card.querySelectorAll('button')].find((b) => /^Deploy/.test(b.textContent || ''))!;
  const at = button.getBoundingClientRect();
  const style = getComputedStyle(card);
  const inner = card.getBoundingClientRect().right - parseFloat(style.borderRightWidth) - parseFloat(style.paddingRight);
  return {
    name: card.getAttribute('aria-label') || '',
    sameRow: at.top < title.bottom && title.top < at.bottom,
    fromRight: Math.round(inner - at.right),
    primary: button.classList.contains('btn-primary'),
    disabled: button.disabled,
  };
}));

test('each card keeps its deploy button top right, never styles a dead deploy as the main action', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/cd'));
  await expect(screen.getByRole('region', 'Production environment')).toBeVisible();
  expect(await deployHeads(browser)).toEqual([
    { name: 'Dev environment', sameRow: true, fromRight: 0, primary: true, disabled: false },
    { name: 'Production environment', sameRow: true, fromRight: 0, primary: false, disabled: false },
  ]);
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(await axeViolations(browser)).toEqual([]);

  // "Production · Not defined" is the widest head: its button fell under the title, left-aligned.
  await app.open(mock('/s/product/cd'));
  await expect(screen.getByRole('region', 'Production environment').getByText('Not defined')).toBeVisible();
  expect(await deployHeads(browser)).toEqual([
    { name: 'Dev environment', sameRow: true, fromRight: 0, primary: false, disabled: true },
    { name: 'Production environment', sameRow: true, fromRight: 0, primary: false, disabled: true },
  ]);
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(await axeViolations(browser)).toEqual([]);
});

test('a promoting production names the branch and the commit it promotes', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/cd'));
  await expect(screen.getByRole('region', 'Production environment')).toBeVisible();
  const facts = browser.locator('[aria-label="Production environment"] .cd-facts');
  await expect(facts).toHaveText(/Promotes\s*quartio at 9f3c2a1 into main/);
  await expect(facts).not.toHaveText(/First/);
});

type Touching = Window & { __yuliusMock: { touchTask(sandbox: string, number: number, changes: object): void } };

// SPEC-134 CDT-004 (ADR-0111): an agent asks a person explicitly with needs_you; the task reads
// "Needs you: <action>" with the link, until the request is gone.
test('a needs_you request puts the task in Needs you with what to do', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await browser.evaluate(() => (window as unknown as Touching).__yuliusMock.touchTask('sb_studio', 12, {
    needs_you: { action: 'Add DEV_SSH_KEY on the Secrets page', why: 'the dev deploy reaches the VM over SSH', link: `${location.origin}/#/s/studio/secrets?section=agents`, at: new Date().toISOString() },
  }));
  const bar = browser.locator('.attention-bar');
  await expect(bar).toHaveText(/Needs you: Add DEV_SSH_KEY on the Secrets page · the dev deploy reaches the VM over SSH/);
  await expect(bar.getByRole('link', 'Open')).toBeVisible();
  await expect(browser.locator('.task-row', { hasText: 'Needs you' }).first()).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await browser.evaluate(() => (window as unknown as Touching).__yuliusMock.touchTask('sb_studio', 12, { needs_you: null }));
  await expect(bar).not.toHaveText(/Needs you: Add DEV_SSH_KEY/);
});
