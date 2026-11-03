import { test, type Browser, type Screen } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// A playground's guests (SPEC-410 GST-007, issue #412, decided 2026-11-03): "teammates can try a
// playground without signing in with GitHub". Its admin creates an expiring link from the
// playground's menu and revokes it; the link opens a join page; a guest works in that playground
// and sees nothing else of Yulius. The mock applies the server's guest fence, so a call the guest
// shell must not make fails the test as a page error.

type Invite = { id: string; code: string; revoked?: boolean; guests: { name: string }[] };
type Mocked = Window & { __yuliusMock: { guestInvites: Invite[]; guestRefusals?: string[] } };

const CODE = 'c0ffee'.repeat(10) + 'c0de';

/** Opens the context menu of the playground tab `lab` and picks `item`. */
async function labMenu(browser: Browser, screen: Screen, item: RegExp): Promise<void> {
  await expect(browser.locator('.tabrow .tab:has-text("lab")')).toBeVisible();
  await browser.evaluate(() => {
    const tabEl = [...document.querySelectorAll('.tabrow .tab')].find((el) => el.querySelector('.tab-name')?.textContent === 'lab')!;
    const box = tabEl.getBoundingClientRect();
    tabEl.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: box.left + 10, clientY: box.top + 10 }));
  });
  await screen.getByRole('menuitem', item).click();
}

test('a playground admin creates a guest link and revokes it', async ({ app, browser, screen }) => {
  // The superadmin, with the playground `lab` and its seeded link.
  await app.open(mock('/s/lab', '&guestlink=mine&github=0'));
  await browser.evaluate(() => { (window as unknown as Mocked).__yuliusMock.guestInvites = []; });
  await labMenu(browser, screen, /^Invite guests/);
  const dialog = screen.getByRole('dialog', 'Invite guests to lab');
  await expect(dialog).toContainText('Teammates without GitHub try this playground');
  await expect(dialog).toContainText('No live guest link.');
  expect(await axeViolations(browser)).toEqual([]);

  await dialog.getByRole('combobox', 'Link lasts').selectOption({ label: '1 day' });
  await dialog.getByRole('button', 'Create link').click();
  const link = dialog.getByRole('textbox', 'Guest link');
  await expect(link).toHaveValue(/\/join\?code=[0-9a-f]{64}$/);
  await expect(dialog).toContainText('Shown only now.');
  const list = dialog.getByRole('list', 'Live guest links');
  await expect(list).toContainText('No guest yet · made by mkarp');

  // A guest joins meanwhile; the list names them, and revoking signs them out.
  await browser.evaluate(() => { (window as unknown as Mocked).__yuliusMock.guestInvites[0].guests.push({ name: 'Dana' }); });
  await browser.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await labMenu(browser, screen, /^Invite guests/);
  const reopened = screen.getByRole('dialog', 'Invite guests to lab');
  await expect(reopened.getByRole('list', 'Live guest links')).toContainText('1 guest: Dana');
  await reopened.getByRole('button', /^Revoke the link ending/).click();
  await expect(browser.locator('.toast:has-text("Link revoked: Dana is signed out.")')).toBeVisible();
  await expect(reopened).toContainText('No live guest link.');
  const revoked = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.guestInvites.map((i) => Boolean(i.revoked)));
  expect(revoked).toEqual([true]);
});

test('a guest link opens a join page and lands the guest in the playground', async ({ app, browser, screen }) => {
  await app.open(mock(`/join?code=${CODE}`, '&guestlink=1'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Try lab');
  await expect(browser.locator('.join-card')).toContainText('mkarp invited you to try the playground lab');
  await expect(browser.locator('.join-card')).toContainText('No GitHub account needed');
  expect(await axeViolations(browser)).toEqual([]);

  // A name is needed.
  await screen.getByRole('button', 'Join as guest').click();
  await expect(screen.getByRole('alert')).toHaveText('Type your name to join.');
  await screen.getByRole('textbox', /^Your name/).fill('Carol');
  await screen.getByRole('button', 'Join as guest').click();
  await expect.poll(() => browser.evaluate(() => location.hash)).toBe('#/s/lab');
  await expect(browser.locator('.chip-guest')).toHaveText('Guest');
  const joined = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.guestInvites[0].guests.map((g) => g.name));
  expect(joined).toEqual(['Carol']);
});

test('an ended guest link says so', async ({ app, screen }) => {
  await app.open(mock(`/join?code=${CODE}`, '&guestlink=dead'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('This guest link has ended');
  await expect(screen.getByRole('button', 'Join as guest')).toHaveCount(0);
});

test('a signed-in browser is told to sign out before joining', async ({ app, browser, screen }) => {
  // The superadmin's browser opens a link it was sent.
  await app.open(mock(`/join?code=${CODE}`, '&guestlink=mine'));
  await expect(browser.locator('.join-card')).toContainText('This browser is signed in as Michal Karp');
  await screen.getByRole('button', 'Sign out to join').click();
  await expect(screen.getByRole('button', 'Join as guest')).toBeVisible();
});

test('a guest sees their playground and nothing else', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&guest=1'));
  // Home is not theirs: the playground opens.
  await expect.poll(() => browser.evaluate(() => location.hash)).toBe('#/s/lab');
  await expect(browser.locator('.chip-guest')).toHaveText('Guest');
  await expect(browser.locator('.tabrow .tab-name')).toHaveText(['lab']);
  // No sandbox to add, no instance status.
  await expect(browser.locator('.tab-add')).toHaveCount(0);
  await expect(browser.locator('.btn-status')).toHaveCount(0);
  // Their playground's work only.
  const nav = screen.getByRole('navigation', 'Sandbox');
  await expect(nav.getByRole('link', 'Tasks')).toBeVisible();
  await expect(nav.getByRole('link', 'Git')).toBeVisible();
  for (const page of ['CI/CD', 'Secrets', 'Dashboard', 'Workflows', 'Skills', 'Automations', 'MCP']) {
    await expect(nav.getByRole('link', page)).toHaveCount(0);
  }
  // Settings, secrets and other sandboxes send them back to the playground.
  for (const path of ['/settings', '/s/lab/secrets', '/s/studio', '/admin/users']) {
    await browser.evaluate((to) => { location.hash = `#${to}`; }, path);
    await expect.poll(() => browser.evaluate(() => location.hash)).toBe('#/s/lab');
  }
  // Its pages and tools ask for nothing a guest is refused.
  for (const page of ['tasks', 'git', 'sessions']) {
    await browser.evaluate((to) => { location.hash = `#/s/lab/${to}`; }, page);
    await expect.poll(() => browser.evaluate(() => location.hash)).toBe(`#/s/lab/${page}`);
    await expect(browser.locator('main h1')).toBeVisible();
  }
  await browser.evaluate(() => { location.hash = '#/s/lab'; });
  await screen.getByRole('button', /^Show tools/).click();
  await expect(browser.locator('.rail')).toBeVisible();
  expect(await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.guestRefusals ?? [])).toEqual([]);
  // The account menu says who and until when, and offers only signing out.
  await screen.getByRole('button', /account menu$/).click();
  const menu = screen.getByRole('menu');
  await expect(menu).toContainText(/Carol · guest until/i);
  await expect(menu.getByRole('menuitem')).toHaveText(['Sign out']);
  expect(await axeViolations(browser)).toEqual([]);
});
