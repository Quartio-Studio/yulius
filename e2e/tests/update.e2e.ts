import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Instance updates and the Jev key (SPEC-001 OPS-006..OPS-009, ADM-001, ADM-002; SPEC-002 ADR-18).
// The mock walks an update through its phases in about six seconds (mock.js UPDATE_STEP_MS).

test('an update on offer is announced to the superadmin and runs to the end', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&update=1'));
  // A calm pill in the tab row: the summary of the new commits, leading to the instance page.
  const offer = screen.getByRole('link', /^Update available: An update banner/);
  await expect(offer).toBeVisible();
  await offer.click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Instance');
  await expect(screen.getByText('Update to 9c1e7d3')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Update now…').click();
  const dialog = screen.getByRole('dialog', /to 9c1e7d3\?$/);
  await expect(dialog.getByText(/The agents don’t notice/)).toBeVisible();
  await dialog.getByRole('button', 'Update to 9c1e7d3').click();
  await expect(dialog).toBeHidden();

  // The steps show while it runs, then the outcome; the banner offers a reload of the new code.
  await expect.poll(() => browser.evaluate(() => document.body.innerText), { timeout: 5000 }).toMatch(/Pause tasks/);
  await expect.poll(() => browser.evaluate(() => document.body.innerText), { timeout: 15000 }).toMatch(/Updated to 9c1e7d3/);
  // The instance page says it all; elsewhere the banner offers a reload of the new code.
  await browser.evaluate(() => { window.location.hash = '#/'; });
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  await expect(screen.getByRole('button', 'Reload')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
});

// The superadmin's Update button beside the account button (SPEC-133, task #121).
const footButton = (browser: Browser, name: string | RegExp) => browser.locator('.sidebar-foot').getByRole('button', name);

test('the sidebar foot offers Update and starts it after one confirmation', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&update=1'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  // Dismissing the tab-row pill hides the announcement, never the button (UPB-006).
  await screen.getByRole('button', 'Dismiss until the next update').click();
  await expect(screen.getByRole('link', /^Update available:/)).toBeHidden();
  const button = footButton(browser, 'Update Yulius to 9c1e7d3');
  await expect(button).toBeVisible();
  await expect(button).toHaveText('Update');
  await expect(button).toHaveAttribute('title', /^Update available: An update banner/);
  expect(await axeViolations(browser)).toEqual([]);

  // One click: the instance page's confirmation, on the page in view (UPB-003).
  await button.click();
  const dialog = screen.getByRole('dialog', 'Update Yulius to 9c1e7d3?');
  await expect(dialog.getByText(/The agents don’t notice/)).toBeVisible();
  await dialog.getByRole('button', 'Cancel').click();
  await expect(dialog).toBeHidden();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');

  await button.click();
  await screen.getByRole('dialog', 'Update Yulius to 9c1e7d3?').getByRole('button', 'Update to 9c1e7d3').click();
  await expect(screen.getByText('Update started. Tasks keep working while it builds.')).toBeVisible();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  // While it runs the button says so and leads to the page with the steps (UPB-004).
  const running = footButton(browser, /^Updating/);
  await expect(running).toBeVisible();
  await running.click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Instance');
  await expect.poll(() => browser.evaluate(() => document.body.innerText), { timeout: 15000 }).toMatch(/Updated to 9c1e7d3/);
  await expect(footButton(browser, 'Version and updates')).toBeVisible();
});

test('without the update helper the button opens the instance page', async ({ app, screen, browser }) => {
  await app.open(mock('/', '&update=1&helper=0'));
  await footButton(browser, 'Update Yulius to 9c1e7d3').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Instance');
  await expect(screen.getByText(/Updates need the update helper/)).toBeVisible();
  expect(await browser.locator('[role="dialog"]').count()).toBe(0);
});

test('a rolled-back update is said plainly', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&update=fail'));
  await screen.getByRole('button', 'Update now…').click();
  await screen.getByRole('dialog', /to 9c1e7d3\?$/).getByRole('button', 'Update to 9c1e7d3').click();
  await expect.poll(() => browser.evaluate(() => document.body.innerText), { timeout: 15000 }).toMatch(/The last update was rolled back/);
  await expect(screen.getByText(/did not become healthy/)).toBeVisible();
  await browser.evaluate(() => { window.location.hash = '#/'; });
  await expect.poll(() => browser.evaluate(() => document.body.innerText), { timeout: 5000 }).toMatch(/The update was rolled back; the previous version runs/);
});

test('members see no offer and no instance page', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&update=1&role=member'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  expect(await browser.evaluate(() => document.body.innerText)).not.toMatch(/Update available/);
  await app.open(mock('/admin/instance', '&update=1&role=member'));
  await expect(screen.getByText(/Only the superadmin manages updates/)).toBeVisible();
});

test('setup asks for the Jev key as an optional last step', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&setup=1&signedout=1'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Set up this instance');
  await screen.getByLabel('Setup token').fill('YUL-SETUP-1234');
  await browser.locator('input[autocomplete="new-password"]').nth(0).fill('a long enough password');
  await browser.locator('input[autocomplete="new-password"]').nth(1).fill('a long enough password');
  await browser.locator('#ack-recover').check();
  await browser.locator('#ack-stored').check();
  await screen.getByRole('button', 'Create superadmin').click();

  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Task intake with Jev');
  await expect(screen.getByRole('button', 'Skip for now')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await browser.locator('#setup-jev').fill('jev-good-key-0001');
  await screen.getByRole('button', 'Save and test').click();
  await expect(screen.getByText(/the key works/)).toBeVisible();
  await screen.getByRole('button', 'Continue').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
});

test('the superadmin sets, tests, replaces and removes the Jev key', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  await expect(screen.getByText(/No key: task intake checks nothing/)).toBeVisible();
  await browser.locator('#settings-jev').fill('jev-bad-key-0001');
  await screen.getByRole('button', 'Save').click();
  await expect(screen.getByText(/Jev refused this key/)).toBeVisible();
  await browser.locator('#settings-jev').fill('jev-good-key-0002');
  await screen.getByRole('button', 'Replace').click();
  await expect(screen.getByText(/the key works/)).toBeVisible();
  await expect(screen.getByText(/In use: saved here/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Remove').click();
  await screen.getByRole('dialog', 'Remove the Jev key?').getByRole('button', 'Remove').click();
  await expect(screen.getByText(/No key: task intake checks nothing/)).toBeVisible();
});

test('a key file shows as config and moves into settings in one click', async ({ app, screen }) => {
  await app.open(mock('/admin/instance', '&jev=config'));
  await expect(screen.getByText(/the key from the server’s configuration/)).toBeVisible();
  await screen.getByRole('button', 'Save it here').click();
  await expect(screen.getByText(/In use: saved here/)).toBeVisible();
});
