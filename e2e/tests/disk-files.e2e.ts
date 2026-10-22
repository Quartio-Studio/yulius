import { test, type Browser, type Screen } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The server's disk and a removed sandbox's files (SPEC-276). A warning pill shows in the tab row
// while the data disk is low or critical, for everyone, and follows `disk.updated`. Removing a
// sandbox asks, with no default, whether to delete its files permanently or keep them.

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

type Removals = { __yuliusMock: { purges?: string[]; kept?: string[] } };

/** The purges and the removals that kept the files, as the mock received them. */
const removals = (browser: Browser) => browser.evaluate(() => {
  const mocked = (window as unknown as Removals).__yuliusMock;
  return [mocked.purges || null, mocked.kept || null];
});

/** Opens the Remove dialog of the GitHub sandbox `name` from its tab's menu. */
async function removeDialog(browser: Browser, screen: Screen, name: string) {
  await expect(screen.getByRole('navigation', 'Sandboxes').getByRole('link', new RegExp(`^${name}\\b`))).toBeVisible();
  await browser.evaluate((name) => {
    const tabEl = [...document.querySelectorAll('.tabrow .tab')].find((el) => el.querySelector('.tab-name')?.textContent === name)!;
    const box = tabEl.getBoundingClientRect();
    tabEl.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: box.left + 10, clientY: box.top + 10 }));
  }, name);
  await screen.getByRole('menuitem', /^Remove from/).click();
  return screen.getByRole('dialog', new RegExp(`^Remove ${name}`));
}

test('removing a sandbox waits for a choice about its files and deletes them when asked', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const dialog = await removeDialog(browser, screen, 'studio');
  await expect(dialog).toContainText('Its files on this server');
  await expect(dialog).toContainText('with any uncommitted or unpushed work');
  await expect(dialog).toContainText('The GitHub repository, task records and the audit trail stay either way.');
  await dialog.getByRole('textbox', /^Type/).fill('studio');
  const remove = dialog.getByRole('button', 'Remove sandbox');
  await expect(remove).toBeDisabled();
  await dialog.getByRole('radio', /^Delete its files permanently/).click();
  await expect(remove).toBeEnabled();
  expect(await axeViolations(browser)).toEqual([]);
  await remove.click();
  await expect(browser.locator('.toast:has-text("Sandbox studio removed and its files deleted.")')).toBeVisible();
  expect(await removals(browser)).toEqual([['studio'], null]);
});

test('keeping a removed sandbox’s files removes it without deleting them', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const dialog = await removeDialog(browser, screen, 'studio');
  await dialog.getByRole('radio', /^Keep its files/).click();
  await dialog.getByRole('textbox', /^Type/).fill('studio');
  await dialog.getByRole('button', 'Remove sandbox').click();
  await expect(browser.locator('.toast:has-text("Sandbox studio removed. Its files are kept on this server.")')).toBeVisible();
  expect(await removals(browser)).toEqual([null, ['studio']]);
});
