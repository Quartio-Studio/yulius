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
  await expect(prod.getByText('promotes the integration branch into main')).toBeVisible();
  await expect(prod.getByText(/Production is always deployed by a person/)).toBeVisible();
  await expect(screen.getByText('ssh: connect to host')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  // A failed dev deploy opens with its output, the secret's value never in it.
  await screen.getByRole('button', /Open the dev deploy at 7d1e0c9/).click();
  const dialog = screen.getByRole('dialog', 'Deploy');
  await expect(browser.locator('.run-output')).toHaveText(/\[secret:DEV_HOST\]/);
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd&run=cd_s2$/);
  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd$/);

  // Dev needs a secret nobody allowed yet: nothing runs, and the page says why.
  await dev.getByRole('button', 'Deploy dev').click();
  await screen.getByRole('dialog', 'Deploy dev?').getByRole('button', 'Deploy dev').click();
  await expect(screen.getByText(/DEV_HOST allows using it outside the proxy/)).toBeVisible();
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
  const dialog = screen.getByRole('dialog', 'Deploy');
  await expect(browser.locator('.run-output')).toHaveText(/\[secret:DEV_HOST\]/);
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd&run=cd_s2$/);
  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?section=cd$/);
  await expect(screen.getByRole('radio', 'CD (deployment)')).toHaveAttribute('aria-checked', 'true');
});

// SPEC-151 CCD-003, review round 1: the tab left is gone. Its late answers draw nothing, and a
// deploy it started opens no dialog over the other tab nor names itself in the address.
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
  await screen.getByRole('radio', 'CI (tests)').click();
  await expect(screen.getByRole('heading', 'Repository hygiene')).toBeVisible();
  await browser.evaluate(() => { (window as unknown as { __releaseDeploy: () => void }).__releaseDeploy(); });
  // The deploy itself went out: the CD tab lists it.
  await screen.getByRole('radio', 'CD (deployment)').click();
  await expect(browser.locator('.item-card:has-text("Production")').first()).toContainText('Deploying');
  await screen.getByRole('radio', 'CI (tests)').click();
  await expect(screen.getByRole('dialog', 'Deploy')).toBeHidden();
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
  await expect(screen.getByText('Deploying production.')).toBeVisible();
  // The time the person kept the confirmation open goes with it, as the run's confirmation wait
  // (SPEC-137 STG-302).
  const sent = await browser.evaluate(() => (window as unknown as { __yuliusMock: { lastCdDeploy?: { confirm_ms?: number } } }).__yuliusMock.lastCdDeploy?.confirm_ms ?? null);
  expect(typeof sent === 'number' && sent > 0).toBe(true);
  const run = screen.getByRole('dialog', 'Deploy');
  await expect(run.getByText('Deploying')).toBeVisible();
  await expect(run.getByText(/promoted the integration branch|by mkarp/)).toBeVisible();
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
