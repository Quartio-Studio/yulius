import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

/** Imports Quartio-Studio/website from the home screen. The mock leaves it `pending`, as `yulius onboard` does. */
async function importWebsite({ app, screen }: { app: { open(url: string): Promise<unknown> }; screen: any }) {
  await app.open(mock('/'));
  await screen.getByRole('button', 'Import repository').click();
  const dialog = screen.getByRole('dialog', 'Add sandbox');
  await screen.getByRole('radio', /Quartio-Studio\/website/).check();
  await dialog.getByRole('button', 'Import').click();
  await expect(dialog).toBeHidden();
}

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
// task input, only a centred message saying what onboarding will do and its button; no Skip.
// SPEC-051: `yulius onboard` only uploads, so a `pending` sandbox waits for Start onboarding here.
test('a pending sandbox shows only what onboarding will do and Start onboarding, centred', async ({ app, browser, screen }) => {
  await importWebsite({ app, screen });
  await browser.evaluate(() => { location.hash = '#/'; });
  await expect(screen.getByText('Needs onboarding').first()).toBeVisible();

  await browser.evaluate(() => { location.hash = '#/s/website'; });
  const gate = screen.getByRole('region', 'Onboard website first');
  await expect(gate).toBeVisible();
  await expect(gate).toContainText('one task an agent runs before any other');
  await expect(gate).toContainText('Installs the toolchains and packages');
  await expect(gate).toContainText('Yulius standards');
  await expect(gate).toContainText('GitHub Actions');
  await expect(gate).toContainText('never application code');
  await expect(gate).toContainText('pull request into dev');
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

  await browser.setViewport({ width: 390, height: 844 });
  await expect(gate).toBeVisible();
  const phone = await offCentre(browser);
  expect(phone.x).toBeLessThanOrEqual(2);
  expect(phone.y).toBeLessThanOrEqual(2);
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
});

test('Start onboarding runs it, Follow onboarding opens it, and the task box comes back once it lands', async ({ app, browser, screen }) => {
  await importWebsite({ app, screen });
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  await screen.getByRole('region', 'Onboard website first').getByRole('button', 'Start onboarding').click();

  const running = screen.getByRole('region', 'Onboarding website');
  await expect(running).toBeVisible();
  await expect(running).toContainText('An agent is setting up this sandbox');
  await expect(running.getByRole('button', 'Start onboarding')).toHaveCount(0);
  // The keyboard moves on to Follow onboarding, and keeps it as the task's events arrive.
  const follow = running.getByRole('link', 'Follow onboarding');
  await expect(follow).toBeFocused();
  await browser.evaluate(() => {
    const db = (window as any).__yuliusMock;
    db.touchTask(db.sandboxes.find((s: { slug: string }) => s.slug === 'website').id, 1, { title: 'Prepare this repository (installing)' });
  });
  await expect(browser.locator('.sidebar')).toContainText('installing');
  await expect(follow).toBeFocused();
  await expect(screen.getByRole('button', 'Skip for now')).toHaveCount(0);
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeHidden();
  const centred = await offCentre(browser);
  expect(centred.x).toBeLessThanOrEqual(2);
  expect(centred.y).toBeLessThanOrEqual(2);
  expect(await axeViolations(browser)).toEqual([]);

  await running.getByRole('link', 'Follow onboarding').click();
  await expect(browser).toHaveURL(/#\/s\/website\/t\/1$/);

  // The mock marks the sandbox ready 9 s after Start onboarding: the composer shows by itself.
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await expect(prompt).toBeVisible({ timeout: 15_000 });
  await expect(browser.locator('.onboarding-gate')).toBeHidden();
  await expect(browser.locator('.composer-quote')).toBeVisible();
  await prompt.fill('Install the GitHub CLI');
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/website\/t\/2$/);
});
