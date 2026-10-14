import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, mockReady } from './support.ts';

failOnPageErrors();

type Mocked = Window & { __yuliusMock: {
  manualOnboarding?: boolean;
  finishOnboarding(slug: string): void;
  touchTask(sandboxId: string, number: number, changes: Record<string, unknown>): void;
  sandboxes: { id: string; slug: string }[];
} };

/**
 * Imports Quartio-Studio/`name` from the home screen. The mock leaves it `pending`, as `yulius
 * onboard` does; `mobile-app` already follows the format, so its onboarding is the environment
 * kind. The mock's onboarding then waits for the test to finish it (`finishOnboarding`).
 */
async function importRepository({ app, browser, screen }: { app: { open(url: string): Promise<unknown> }; browser: Browser; screen: any }, name: string) {
  await app.open(mock('/'));
  await mockReady(browser);
  await browser.evaluate(() => { (window as unknown as Mocked).__yuliusMock.manualOnboarding = true; });
  await screen.getByRole('button', 'Import repository').click();
  const dialog = screen.getByRole('dialog', 'Add sandbox');
  await screen.getByRole('radio', new RegExp(`Quartio-Studio/${name}`)).check();
  await dialog.getByRole('button', 'Import').click();
  await expect(dialog).toBeHidden();
}

/** Changes the onboarding task (#1) of the `website` sandbox as the server would announce it. */
const touchOnboarding = (browser: Browser, changes: Record<string, unknown>) => browser.evaluate((c: Record<string, unknown>) => {
  const db = (window as unknown as Mocked).__yuliusMock;
  db.touchTask(db.sandboxes.find((s) => s.slug === 'website')!.id, 1, c);
}, changes);

/**
 * How far the onboarding region sits from the centre of the free page area (pixels, x and y). The
 * free area is the composer's box, below its top bar when that bar is in the flow (phones).
 */
const offCentre = (browser: Browser) => browser.evaluate(() => {
  const page = document.querySelector('.composer')!.getBoundingClientRect();
  const bar = document.querySelector('.composer-top')!;
  const top = getComputedStyle(bar).position === 'absolute' ? page.top : bar.getBoundingClientRect().bottom;
  const gate = document.querySelector('.onboarding-gate')!.getBoundingClientRect();
  return {
    x: Math.round(Math.abs((gate.left + gate.right) / 2 - (page.left + page.right) / 2)),
    y: Math.round(Math.abs((gate.top + gate.bottom) / 2 - (top + page.bottom) / 2)),
  };
});

// SPEC-096 (task #81): onboarding is mandatory. Until it has finished, the new-task page shows no
// task input, only a centred message saying what onboarding will do (its task's own plan) and its
// button; no Skip. SPEC-051: `yulius onboard` only uploads, so a `pending` sandbox waits for Start
// onboarding here.
test('a pending sandbox shows only what onboarding will do and Start onboarding, centred', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'website');
  await browser.evaluate(() => { location.hash = '#/'; });
  await expect(screen.getByText('Needs onboarding').first()).toBeVisible();

  // SPEC-097 (task #82): nothing has started, so there is no task, not even a Closed one.
  await browser.evaluate(() => { location.hash = '#/s/website/tasks'; });
  await expect(browser.locator('.tl .empty-state')).toBeVisible();
  await expect(screen.getByText('Prepare this repository for Yulius')).toHaveCount(0);

  await browser.evaluate(() => { location.hash = '#/s/website'; });
  const gate = screen.getByRole('region', 'Onboard website first');
  await expect(gate).toBeVisible();
  await expect(gate).toContainText('one task an agent runs before any other');
  // The task does not exist until Start onboarding (SPEC-097): its title, not a link to it.
  await expect(gate).toContainText('Prepare this repository for Yulius');
  await expect(gate.getByRole('link')).toHaveCount(0);
  // Its plan, step by step, as the server wrote it for this repository.
  const plan = gate.getByRole('list');
  await expect(plan.getByRole('listitem')).toHaveCount(6);
  await expect(plan).toContainText('Install the toolchains and run the repository\'s own checks');
  await expect(plan).toContainText('Move GitHub Actions into Yulius CI and delete them');
  await expect(gate).toContainText('never application code, and any change reaches dev through a pull request');
  await expect(gate).toContainText('passes the Yulius format check');
  await expect(gate.getByRole('button', 'Start onboarding')).toBeVisible();
  // No task input and no way around onboarding.
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeHidden();
  await expect(screen.getByRole('button', 'Start task')).toBeHidden();
  await expect(browser.locator('.composer-tools')).toBeHidden();
  await expect(screen.getByRole('button', 'Skip for now')).toHaveCount(0);
  await expect(browser.locator('.composer-quote')).toBeHidden();
  await expect(browser.locator('.banner-info')).toHaveCount(0);
  const desktop = await offCentre(browser);
  expect(desktop.x).toBeLessThanOrEqual(2);
  expect(desktop.y).toBeLessThanOrEqual(2);
  expect(await axeViolations(browser)).toEqual([]);

  // The new-task shortcut puts the keyboard on Start onboarding: there is no prompt to type in.
  await browser.keyboard.press('ControlOrMeta+Shift+O');
  await expect(gate.getByRole('button', 'Start onboarding')).toBeFocused();

  await browser.setViewport({ width: 390, height: 844 });
  await expect(gate).toBeVisible();
  const phone = await offCentre(browser);
  expect(phone.x).toBeLessThanOrEqual(2);
  expect(phone.y).toBeLessThanOrEqual(2);
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(await axeViolations(browser)).toEqual([]);
});

// The review of SPEC-096: a repository that already follows the format gets the environment-only
// onboarding, which neither moves GitHub Actions nor adds documents; the page must not say it does.
test('a repository already in the Yulius format is told only what its onboarding will do', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'mobile-app');
  await browser.evaluate(() => { location.hash = '#/s/mobile-app'; });
  const gate = screen.getByRole('region', 'Onboard mobile-app first');
  await expect(gate).toContainText('Set up this sandbox\'s environment');
  const plan = gate.getByRole('list');
  await expect(plan.getByRole('listitem')).toHaveCount(3);
  await expect(plan).toContainText('Record the working commands in yulius.toml [commands]');
  await expect(gate).not.toContainText('GitHub Actions');
  await expect(gate).not.toContainText('missing documents');
});

test('Start onboarding runs it, Follow onboarding opens it, and the task box comes back once it lands', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'website');
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  await screen.getByRole('region', 'Onboard website first').getByRole('button', 'Start onboarding').click();

  const running = screen.getByRole('region', 'Onboarding website');
  await expect(running).toBeVisible();
  await expect(running).toContainText('An agent is setting up this sandbox');
  await expect(running.getByRole('button', 'Start onboarding')).toHaveCount(0);
  // The keyboard moves on to Follow onboarding, and keeps it as the task's events arrive.
  const follow = running.getByRole('link', 'Follow onboarding');
  await expect(follow).toBeFocused();
  // Start onboarding created the task, so it now has a number (SPEC-097).
  await expect(follow).toHaveAttribute('href', /\/s\/website\/t\/1$/);
  await touchOnboarding(browser, { title: 'Prepare this repository for Yulius (installing)' });
  await expect(browser.locator('.sidebar')).toContainText('installing');
  await expect(follow).toBeFocused();
  await expect(screen.getByRole('button', 'Skip for now')).toHaveCount(0);
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeHidden();
  const centred = await offCentre(browser);
  expect(centred.x).toBeLessThanOrEqual(2);
  expect(centred.y).toBeLessThanOrEqual(2);
  expect(await axeViolations(browser)).toEqual([]);

  // What the server reads in a preparing sandbox (`onboarding_status`), in the page's words.
  await touchOnboarding(browser, { status: 'waiting_capacity', activity: null });
  await expect(running).toContainText('#1 waits for a Claude subscription with capacity');
  await touchOnboarding(browser, { status: 'closed', activity: null });
  await expect(running).toContainText('#1 stopped before the repository passed the Yulius format check');
  await expect(running.getByRole('link', 'Open onboarding')).toBeVisible();
  await touchOnboarding(browser, { status: 'running', activity: 'working' });
  await running.getByRole('link', 'Follow onboarding').click();
  await expect(browser).toHaveURL(/#\/s\/website\/t\/1$/);

  // Onboarding lands while the page is open: the composer shows by itself, takes the keyboard,
  // and a screen reader hears so.
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  await expect(screen.getByRole('region', 'Onboarding website')).toBeVisible();
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.finishOnboarding('website'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await expect(prompt).toBeVisible();
  await expect(prompt).toBeFocused();
  await expect(browser.locator('.composer > [aria-live="polite"]')).toHaveText('Onboarding finished: the task box is ready.');
  await expect(browser.locator('.onboarding-gate')).toBeHidden();
  await expect(browser.locator('.composer-quote')).toBeVisible();
  await prompt.fill('Install the GitHub CLI');
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/website\/t\/2$/);
});

// The review of SPEC-096: until the sandbox's task list is read (or when reading it failed), the
// page cannot know whether the onboarding task exists, so it must not call it missing. Mock
// `onboarding=preparing` puts `product` in preparation; `fail=` makes its task list fail.
test('a preparing sandbox whose tasks are not known yet does not call its onboarding task missing', async ({ app, screen }) => {
  await app.open(mock('/s/product', '&onboarding=preparing&fail=/api/sandboxes/sb_product/tasks'));
  const gate = screen.getByRole('region', 'Onboarding product');
  await expect(gate).toContainText('An agent is setting up this sandbox');
  await expect(gate).not.toContainText('missing');
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeHidden();
});

test('a preparing sandbox without its onboarding task tells an administrator the way out', async ({ app, screen }) => {
  await app.open(mock('/s/product', '&onboarding=preparing&tasks=0'));
  const gate = screen.getByRole('region', 'Onboarding product');
  await expect(gate).toContainText('The onboarding task is missing');
  await expect(gate).toContainText('POST /api/sandboxes/sb_product/onboarding/skip');
});
