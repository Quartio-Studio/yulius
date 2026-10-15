import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Task #113 (SPEC-134): CD (deployment) next to CI. The mock's studio defines dev (a script with
// two secrets, one not allowed yet) and production (promotion, then a script); product has dev
// empty and production not defined; sandbox-3 is read-only (assets/mock.js, seedCd).

test('the CD page shows dev and production as their branches define them', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/cd'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('CD');
  const dev = screen.getByRole('region', 'Dev environment');
  const prod = screen.getByRole('region', 'Production environment');
  await expect(dev.getByText('scripts/cd/deploy.sh dev')).toBeVisible();
  await expect(dev.getByText('waiting for a person to allow it')).toBeVisible();
  await expect(prod.getByText(/^quartio at 9f3c2a1 into main$/)).toBeVisible();
  await expect(prod.getByText(/Production is always deployed by a person/)).toBeVisible();
  await expect(screen.getByText('ssh: connect to host')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  // A failed dev deploy opens with its output, the secret's value never in it.
  await screen.getByRole('button', /Open the dev deploy at 7d1e0c9/).click();
  const dialog = screen.getByRole('dialog', 'Deploy');
  await expect(browser.locator('.run-output')).toHaveText(/\[secret:DEV_HOST\]/);
  await expect(browser).toHaveURL(/#\/s\/studio\/cd\?run=cd_s2$/);
  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();

  // Dev needs a secret nobody allowed yet: nothing runs, and the page says why.
  await dev.getByRole('button', 'Deploy dev').click();
  await screen.getByRole('dialog', 'Deploy dev?').getByRole('button', 'Deploy dev').click();
  await expect(screen.getByText(/DEV_HOST allows using it outside the proxy/)).toBeVisible();
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
