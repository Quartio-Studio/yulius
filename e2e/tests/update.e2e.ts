import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Instance updates and the Jev key (SPEC-001 OPS-006..OPS-009, ADM-001, ADM-002; SPEC-002 ADR-18;
// SPEC-160 OPS-014).
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
  // Tasks are frozen at once, mid-turn, not at their next step (SPEC-160 OPS-014).
  await expect(dialog.getByText(/Tasks then pause at once, mid-step/)).toBeVisible();
  await dialog.getByRole('button', 'Update to 9c1e7d3').click();
  await expect(dialog).toBeHidden();

  // The steps show while it runs, then the outcome; the banner offers a reload of the new code.
  await expect.poll(() => browser.evaluate(() => document.body.innerText), { timeout: 5000 }).toMatch(/Pause tasks/);
  await expect(screen.getByText('Tasks pause at once, mid-step. The agents don’t notice.')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => document.body.innerText), { timeout: 5000 }).toMatch(/Pausing tasks: 2 of 3 paused\./);
  expect(await browser.evaluate(() => document.body.innerText)).not.toMatch(/next step/);
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
  await expect(browser.locator('dialog[open]')).toHaveCount(0);
  await expect(footButton(browser, 'Update Yulius to 9c1e7d3')).toHaveAttribute('title', /needs the update helper/);
});

test('with nothing on offer the button and the account menu lead to the instance page', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  const quiet = footButton(browser, 'Version and updates');
  await expect(quiet).toBeVisible();
  await expect(quiet).toHaveText('');
  await quiet.click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Instance');

  await browser.evaluate(() => { window.location.hash = '#/'; });
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  await screen.getByRole('button', /account menu$/).click();
  await screen.getByRole('menuitem', 'Version and updates').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Instance');
});

test('at its narrowest the sidebar keeps the account name and the Update button', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&update=1'));
  await screen.getByRole('separator', 'Resize sidebar').press('Home');
  await expect.poll(() => browser.evaluate(() => Math.round(document.querySelector('.sidebar')!.getBoundingClientRect().width))).toBe(200);
  await expect(footButton(browser, 'Update Yulius to 9c1e7d3')).toHaveText('Update');
  const fit = await browser.evaluate(() => {
    const foot = document.querySelector('.sidebar-foot')!.getBoundingClientRect();
    const button = document.querySelector('.sidebar-foot .update-btn')!.getBoundingClientRect();
    const name = document.querySelector('.sidebar-foot .user-name')!.getBoundingClientRect();
    return { inside: button.left >= foot.left && button.right <= foot.right, name: name.width > 8 };
  });
  expect(fit).toEqual({ inside: true, name: true });
  expect(await axeViolations(browser)).toEqual([]);
});

test('instance administrators get neither the button nor the menu entry', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&update=1&role=admin'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  await screen.getByRole('button', /account menu$/).click();
  await expect(screen.getByRole('menuitem', 'Users')).toBeVisible();
  expect(await browser.evaluate(() => document.body.innerText)).not.toMatch(/Version and updates/);
  expect(await browser.locator('.sidebar-foot .update-btn:not([hidden])').count()).toBe(0);
});

test('in the phone drawer the account name and the Update button fit', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/', '&update=1'));
  await screen.getByRole('button', 'Open navigation').click();
  const button = footButton(browser, 'Update Yulius to 9c1e7d3');
  await expect(button).toBeVisible();
  await expect(button).toHaveText('Update');
  const fit = await browser.evaluate(() => {
    const foot = document.querySelector('.sidebar-foot')!.getBoundingClientRect();
    const button = document.querySelector('.sidebar-foot .update-btn')!.getBoundingClientRect();
    const name = document.querySelector('.sidebar-foot .user-name')!.getBoundingClientRect();
    return { inside: button.left >= foot.left && button.right <= foot.right, name: name.width > 8 };
  });
  expect(fit).toEqual({ inside: true, name: true });
  expect(await axeViolations(browser)).toEqual([]);
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
  // No Update button and no menu entry: updates are the superadmin's (UPB-001, UPB-007).
  expect(await browser.locator('.sidebar-foot .update-btn:not([hidden])').count()).toBe(0);
  await screen.getByRole('button', /account menu$/).click();
  await expect(screen.getByRole('menuitem', 'Sign out')).toBeVisible();
  expect(await browser.evaluate(() => document.body.innerText)).not.toMatch(/Version and updates/);
  await app.open(mock('/admin/instance', '&update=1&role=member'));
  await expect(screen.getByText(/Only the superadmin manages updates/)).toBeVisible();
});

test('setup asks for the Jev key, then the Vidoc key, both optional', async ({ app, browser, screen }) => {
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

  // SPEC-195 VDK-004: the Vidoc key is the last optional step, and skipping it changes nothing.
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Security scans with Vidoc');
  await expect(screen.getByText(/The code being scanned is sent to Vidoc/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Skip for now').click();
  // SPEC-283: the first Claude subscription is the last step, skippable too.
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Add your first Claude subscription');
  await screen.getByRole('button', 'Skip for now').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  await app.open(mock('/admin/instance'));
  await expect(screen.getByText(/No key: security scans are off/)).toBeVisible();
});

test('the Vidoc setup step saves and tests a key', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&setup=1&signedout=1'));
  await screen.getByLabel('Setup token').fill('YUL-SETUP-1234');
  await browser.locator('input[autocomplete="new-password"]').nth(0).fill('a long enough password');
  await browser.locator('input[autocomplete="new-password"]').nth(1).fill('a long enough password');
  await browser.locator('#ack-recover').check();
  await browser.locator('#ack-stored').check();
  await screen.getByRole('button', 'Create superadmin').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Task intake with Jev');
  await screen.getByRole('button', 'Skip for now').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Security scans with Vidoc');
  await browser.locator('#setup-vidoc').fill('vidoc-good-key-0001');
  await screen.getByRole('button', 'Save and test').click();
  await expect(screen.getByText(/vidoc-1 is available/)).toBeVisible();
  await screen.getByRole('button', 'Continue').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Add your first Claude subscription');
  await screen.getByRole('button', 'Skip for now').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
});

test('the superadmin sets, tests, replaces and removes the Jev key', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  await expect(screen.getByText(/No key: task intake checks nothing/)).toBeVisible();
  await browser.locator('#settings-jev').fill('jev-bad-key-0001');
  await screen.getByRole('region', 'Jev key').getByRole('button', 'Save').click();
  await expect(screen.getByText(/Jev refused this key/)).toBeVisible();
  await browser.locator('#settings-jev').fill('jev-good-key-0002');
  await screen.getByRole('region', 'Jev key').getByRole('button', 'Replace').click();
  await expect(screen.getByText(/the key works/)).toBeVisible();
  await expect(screen.getByText(/In use: saved here/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('region', 'Jev key').getByRole('button', 'Remove').click();
  await screen.getByRole('dialog', 'Remove the Jev key?').getByRole('button', 'Remove').click();
  await expect(screen.getByText(/No key: task intake checks nothing/)).toBeVisible();
});

test('the superadmin sets, tests, replaces and removes the Vidoc key', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  await expect(screen.getByText(/No key: security scans are off/)).toBeVisible();
  await browser.locator('#settings-vidoc').fill('vidoc-bad-key-0001');
  await screen.getByRole('region', 'Vidoc key').getByRole('button', 'Save').click();
  await expect(screen.getByText(/Vidoc refused this key/)).toBeVisible();
  await browser.locator('#settings-vidoc').fill('vidoc-good-key-0002');
  await screen.getByRole('region', 'Vidoc key').getByRole('button', 'Replace').click();
  await expect(screen.getByText(/vidoc-1 is available/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('region', 'Vidoc key').getByRole('button', 'Remove').click();
  await screen.getByRole('dialog', 'Remove the Vidoc key?').getByRole('button', 'Remove').click();
  await expect(screen.getByText(/No key: security scans are off/)).toBeVisible();
});

test('a Vidoc key file shows as config and moves into settings in one click', async ({ app, screen }) => {
  await app.open(mock('/admin/instance', '&vidoc=config'));
  await expect(screen.getByText(/YULIUS_VIDOC_API_KEY_FILE/)).toBeVisible();
  await screen.getByRole('region', 'Vidoc key').getByRole('button', 'Save it here').click();
  await expect(screen.getByRole('region', 'Vidoc key').getByText(/In use: saved here/)).toBeVisible();
});

test('a key file shows as config and moves into settings in one click', async ({ app, screen }) => {
  await app.open(mock('/admin/instance', '&jev=config'));
  await expect(screen.getByText(/the key from the server’s configuration/)).toBeVisible();
  await screen.getByRole('button', 'Save it here').click();
  await expect(screen.getByText(/In use: saved here/)).toBeVisible();
});

// Opening the instance page checks main by itself; "Check now" stays for a later look (SPEC-149,
// task #143).
test('opening the instance page checks main without a click', async ({ app, screen }) => {
  // The mock's last check is 10 minutes old.
  await app.open(mock('/admin/instance', '&update=1'));
  await expect(screen.getByText(/^Checked just now with the GitHub App\.$/)).toBeVisible();
  await expect(screen.getByRole('button', 'Check now')).toBeEnabled();
});

test('an automatic check that fails keeps the last result and raises no error', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&update=1&failpost=/api/admin/update/check'));
  await expect(screen.getByText('Update to 9c1e7d3')).toBeVisible();
  // The automatic check was sent and failed; it has settled once the button is enabled again.
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __yuliusMock: { failedPosts: number } }).__yuliusMock.failedPosts)).toBe(1);
  await expect(screen.getByRole('button', 'Check now')).toBeEnabled();
  await expect(screen.getByText(/^Checked 10 min ago with the GitHub App\.$/)).toBeVisible();
  expect(await browser.locator('.toast').count()).toBe(0);
  // Asked for, a failed check is said.
  await screen.getByRole('button', 'Check now').click();
  await expect(screen.getByText(/Couldn’t check for updates/)).toBeVisible();
});

// #502 (SPEC-226 UPC-002): reads of the update view overlap (events, the reconnect, the poll); an
// older read answering after a newer one must not bring its older view back.
type UpdateMock = {
  update: { offer: string | null };
  holdUpdateReads: boolean;
  updateReads: () => number;
  updateReadsTaken: number;
  releaseUpdate: () => void;
  announceUpdate: () => void;
};
const updateMock = <T>(browser: Browser, run: (m: UpdateMock) => T) => browser.evaluate(`(${run.toString()})(window.__yuliusMock)`) as Promise<T>;

test('an older update read answering last does not replace a newer one', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&update=1'));
  const offer = screen.getByRole('link', /^Update available:/);
  await expect(offer).toBeVisible();
  // A read starts while the update is on offer; the offer goes (installed elsewhere) and a second
  // read starts. Each answers what it saw when it arrived.
  await updateMock(browser, (m) => { m.holdUpdateReads = true; m.announceUpdate(); });
  await expect.poll(() => updateMock(browser, (m) => m.updateReads())).toBe(1);
  await updateMock(browser, (m) => { m.update.offer = null; m.announceUpdate(); });
  await expect.poll(() => updateMock(browser, (m) => m.updateReads())).toBe(2);
  // The newer answer first: the offer is gone.
  await updateMock(browser, (m) => m.releaseUpdate());
  await expect.poll(() => updateMock(browser, (m) => m.updateReadsTaken)).toBe(1);
  await expect(offer).toBeHidden();
  // The older answer last: once read it is handled at once, and dropped.
  await updateMock(browser, (m) => m.releaseUpdate());
  await expect.poll(() => updateMock(browser, (m) => m.updateReadsTaken)).toBe(2);
  await expect(offer).toBeHidden();
});
