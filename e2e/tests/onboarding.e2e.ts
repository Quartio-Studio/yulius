import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// F 2026-10-11: while a sandbox was being prepared nothing else could start ("Couldn't handle the
// held prompt: this project is being prepared…", and a disabled composer). A new task now waits
// after the preparation task and starts by itself once the sandbox is ready or an admin skips
// preparation (SPEC-001 IMP-010, U-08).
test('a task started while the sandbox is being prepared waits for it, then starts', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  await screen.getByRole('button', 'Import repository').click();
  const dialog = screen.getByRole('dialog', 'Add sandbox');
  await screen.getByRole('radio', /Quartio-Studio\/website/).check();
  await dialog.getByRole('button', 'Import').click();
  await expect(dialog).toBeHidden();

  await browser.evaluate(() => { location.hash = '#/s/website'; });
  await expect(browser.locator('.banner-info')).toContainText('New tasks wait for it and start by themselves');
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await expect(prompt).toBeEnabled();
  await expect(screen.getByRole('button', 'Start task')).toBeEnabled();
  expect(await axeViolations(browser)).toEqual([]);

  await prompt.fill('Install the GitHub CLI');
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/website\/t\/2$/);
  await expect(screen.getByText('Queued after #1').first()).toBeVisible();

  // Skipping preparation starts the task that waited.
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  await browser.locator('.banner-info').getByRole('button', 'Skip for now').click();
  await screen.getByRole('dialog', 'Skip sandbox preparation?').getByRole('button', 'Skip for now').click();
  await expect(browser.locator('.banner-info')).toHaveCount(0);
  await browser.evaluate(() => { location.hash = '#/s/website/t/2'; });
  await expect(screen.getByText('Queued after #1')).toHaveCount(0);
});

// SPEC-051 (task #43): `yulius onboard` only uploads; adapting the repository to the Yulius
// standards waits for a person to click Start onboarding on the sandbox page. The mock's import
// leaves the sandbox `pending`, standing in for a repository added with `yulius onboard`.
test('a pending sandbox waits for Start onboarding in the browser', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  await screen.getByRole('button', 'Import repository').click();
  const dialog = screen.getByRole('dialog', 'Add sandbox');
  await screen.getByRole('radio', /Quartio-Studio\/website/).check();
  await dialog.getByRole('button', 'Import').click();
  await expect(dialog).toBeHidden();
  await browser.evaluate(() => { location.hash = '#/'; });
  await expect(screen.getByText('Needs onboarding').first()).toBeVisible();

  // SPEC-097 (task #82): nothing has started, so there is no task, not even a Closed one.
  await browser.evaluate(() => { location.hash = '#/s/website/tasks'; });
  await expect(browser.locator('.tl .empty-state')).toBeVisible();
  await expect(screen.getByText('Prepare repository for Yulius')).toHaveCount(0);

  await browser.evaluate(() => { location.hash = '#/s/website'; });
  const banner = browser.locator('.banner-info');
  await expect(banner).toContainText('This sandbox needs onboarding.');
  await expect(banner).toContainText('Yulius standards');
  expect(await axeViolations(browser)).toEqual([]);
  await banner.getByRole('button', 'Start onboarding').click();
  await expect(banner).toContainText('Preparing this sandbox.');
  await expect(banner.getByRole('button', 'Start onboarding')).toHaveCount(0);
  await browser.evaluate(() => { location.hash = '#/s/website/tasks'; });
  await expect(screen.getByText('Prepare repository for Yulius').first()).toBeVisible();
});
