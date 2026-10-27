import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// A project's Vidoc switch (SPEC-338): the superadmin turns a project's scans off and on in the
// Instance page's Vidoc card (VOF-005), and the Add sandbox dialog offers it when the instance has
// a Vidoc key (VOF-004).

type Mocked = { __yuliusMock: { sandboxes: { slug: string; profile: { vidoc?: boolean } }[] } };

/** The mock's stored switch of the sandbox `slug` (`null`: not read through the API yet). */
const switchOf = (browser: Browser, slug: string) => browser.evaluate((s: string) => {
  const sandbox = (window as unknown as Mocked).__yuliusMock.sandboxes.find((sb) => sb.slug === s);
  return sandbox ? (sandbox.profile.vidoc ?? null) : 'missing';
}, slug);

test('the superadmin switches a project’s Vidoc scans off and on in the Vidoc card', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&vidoc=config'));
  const card = screen.getByRole('region', 'Vidoc key');
  await expect(card.getByText(/A project switched off scans nothing/)).toBeVisible();
  const studio = screen.getByRole('switch', 'Vidoc scans for studio');
  await expect(studio).toBeChecked();
  await expect(screen.getByRole('switch', 'Vidoc scans for product')).toBeChecked();
  expect(await axeViolations(browser)).toEqual([]);

  await studio.click();
  await expect(screen.getByText(/Vidoc scans are off for studio/)).toBeVisible();
  await expect(studio).not.toBeChecked();
  expect(await switchOf(browser, 'studio')).toBe(false);
  expect(await switchOf(browser, 'product')).toBe(true);

  await studio.click();
  await expect(screen.getByText(/Vidoc scans are on for studio/)).toBeVisible();
  await expect(studio).toBeChecked();
  expect(await switchOf(browser, 'studio')).toBe(true);
});

test('a project imported with the Vidoc box unchecked starts switched off', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&vidoc=config'));
  await screen.getByRole('button', 'Import repository').click();
  const dialog = screen.getByRole('dialog', 'Add sandbox');
  const scan = dialog.getByRole('checkbox', 'Scan pull requests with Vidoc');
  await expect(scan).toBeChecked();
  expect(await axeViolations(browser)).toEqual([]);
  await scan.click();
  await expect(scan).not.toBeChecked();
  await screen.getByRole('radio', /Quartio-Studio\/website/).check();
  await dialog.getByRole('button', 'Import').click();
  await expect(dialog).toBeHidden();
  expect(await switchOf(browser, 'website')).toBe(false);
});

test('a project imported as offered keeps scanning, and without a key nothing is offered', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&vidoc=config'));
  await screen.getByRole('button', 'Import repository').click();
  let dialog = screen.getByRole('dialog', 'Add sandbox');
  await expect(dialog.getByRole('checkbox', 'Scan pull requests with Vidoc')).toBeChecked();
  await screen.getByRole('radio', /Quartio-Studio\/website/).check();
  await dialog.getByRole('button', 'Import').click();
  await expect(dialog).toBeHidden();
  expect(await switchOf(browser, 'website')).toBe(true);

  await app.open(mock('/'));
  await screen.getByRole('button', 'Import repository').click();
  dialog = screen.getByRole('dialog', 'Add sandbox');
  await expect(screen.getByRole('radio', /Quartio-Studio\/website/)).toBeVisible();
  await expect(dialog.getByRole('checkbox', 'Scan pull requests with Vidoc')).toHaveCount(0);
});
