import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Settings → Instance, "Leftovers" (SPEC-215 LFT-011): the disk's usage and what can be
// reclaimed, each leftover with its kind, path, owner, reason and size, and whether Yulius deletes
// it by itself. Refresh rescans, Delete all safe and Delete of the selected items confirm with
// the size, and the list follows the background deletion. The mock scans in 0.8 s and deletes one
// item every 0.6 s.

const bodyText = (browser: { evaluate: (fn: () => string) => Promise<string> }) => browser.evaluate(() => document.body.innerText);

test('a scan lists the leftovers with the disk and what is safe to delete', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  await expect(screen.getByRole('heading', { level: 2, name: 'Leftovers' })).toBeVisible();
  await expect(screen.getByText(/Not scanned yet/)).toBeVisible();
  await expect(screen.getByText(/of the disk used \(312 GiB of 386 GiB\)/)).toBeVisible();
  await expect(screen.getByRole('button', 'Delete all safe')).toBeDisabled();

  await screen.getByRole('button', 'Refresh').click();
  await expect.poll(() => browser.locator('.leftovers-table tbody tr').count(), { timeout: 8000 }).toBe(6);
  const text = await bodyText(browser);
  expect(text).toMatch(/System layer of a removed sandbox/);
  expect(text).toMatch(/target of task #193: done, quiet for 30 minutes/);
  expect(text).toMatch(/Everything else of a removed sandbox/);
  expect(text).toMatch(/Host Docker data/);
  expect(text).toMatch(/reclaimable/);
  expect(await browser.locator('.leftovers-table tr[data-kind="host-docker"] input:disabled').count()).toBe(1);
  expect(await axeViolations(browser)).toEqual([]);
});

test('Delete all safe confirms with the size and leaves the items that need a person', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  await screen.getByRole('button', 'Refresh').click();
  await expect.poll(() => browser.locator('.leftovers-table tbody tr').count(), { timeout: 8000 }).toBe(6);

  await screen.getByRole('button', /^Delete all safe/).click();
  const dialog = screen.getByRole('dialog', /^Delete every safe leftover \(/);
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', /^Delete \d/).click();
  await expect(screen.getByText(/Deleting \d of 3/)).toBeVisible({ timeout: 4000 });
  await expect.poll(() => bodyText(browser), { timeout: 10000 }).toMatch(/3 items deleted, .* freed/);
  expect(await browser.locator('.leftovers-table tbody tr').count()).toBe(3);
  expect(await browser.locator('.leftovers-table tr[data-kind="removed-sandbox"]').count()).toBe(1);
  expect(await axeViolations(browser)).toEqual([]);
});

test('Delete takes the selected items after naming their size', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  await screen.getByRole('button', 'Refresh').click();
  await expect.poll(() => browser.locator('.leftovers-table tbody tr').count(), { timeout: 8000 }).toBe(6);
  await browser.locator('.leftovers-table tr[data-kind="removed-sandbox"] input').click();
  await screen.getByRole('button', /^Delete 1 item/).click();
  const dialog = screen.getByRole('dialog', /^Delete 1 leftover \(/);
  await expect(dialog.getByText(/1 of them needs a person: Everything else of a removed sandbox/)).toBeVisible();
  await dialog.getByRole('button', /^Delete \d/).click();
  await expect.poll(() => browser.locator('.leftovers-table tr[data-kind="removed-sandbox"]').count(), { timeout: 8000 }).toBe(0);
  expect(await browser.locator('.leftovers-table tbody tr').count()).toBe(5);
});

test('an empty scan says nothing is left over, and members never see the card', async ({ app, screen }) => {
  await app.open(mock('/admin/instance', '&leftovers=none'));
  await screen.getByRole('button', 'Refresh').click();
  await expect(screen.getByText(/Nothing left over/)).toBeVisible({ timeout: 8000 });
  await app.open(mock('/admin/instance', '&role=member'));
  await expect(screen.getByRole('heading', { level: 2, name: 'Leftovers' })).toBeHidden();
});
