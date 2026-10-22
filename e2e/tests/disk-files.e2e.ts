import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The server's disk and a removed sandbox's files (SPEC-276). A warning pill shows in the tab row
// while the data disk is low or critical, for everyone, and follows `disk.updated`.

type Disky = { __yuliusMock: { setDisk(level: string, used: number): void } };

test('a low disk shows a warning pill that follows the level, for members too', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&disk=low'));
  const pill = browser.locator('.disk-alert');
  await expect(pill).toBeVisible();
  await expect(screen.getByText('Disk 88% full')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await browser.evaluate(() => (window as unknown as Disky).__yuliusMock.setDisk('critical', 370 * 1024 ** 3));
  await expect(screen.getByText('Disk almost full: 96%')).toBeVisible();
  expect(await browser.locator('.disk-alert.disk-critical').count()).toBe(1);
  // The superadmin's pill leads to the instance page, where kept files can go.
  expect(await browser.locator('.disk-alert a[href="/admin/instance"]').count()).toBe(1);

  await browser.evaluate(() => (window as unknown as Disky).__yuliusMock.setDisk('ok', 100 * 1024 ** 3));
  await expect(pill).toBeHidden();

  await app.open(mock('/', '&disk=critical&role=member'));
  await expect(screen.getByText('Disk almost full: 96%')).toBeVisible();
  expect(await browser.locator('.disk-alert a').count()).toBe(0);
});

type Asked = { __yuliusMock: { asked: Record<string, number> } };

test('a disk that is fine shows no pill', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  await expect(screen.getByRole('button', 'New task in studio')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => (window as unknown as Asked).__yuliusMock.asked['/api/disk'] || 0)).toBeGreaterThan(0);
  expect(await browser.locator('.disk-alert:not([hidden])').count()).toBe(0);
});
