import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, eventsConnected, failOnPageErrors, firstShown, mock } from './support.ts';

failOnPageErrors();

// The end-of-day digest (SPEC-512 part 3, #1413): an instance administrator turns it on in
// Settings → Administration (DIG-001); a person's digest reaches the bell, also one made while no
// page was open, and opens /digest (DIG-005).

type Mocked = Window & { __yuliusMock: { digest: { setting: unknown }; sendDigest: (id?: string) => void } };

const setting = (browser: Browser) => browser.evaluate(() => JSON.parse(JSON.stringify((window as unknown as Mocked).__yuliusMock.digest.setting)));

test('an instance administrator turns the digest on, off by default (SPEC-512 DIG-001)', async ({ app, browser, screen }) => {
  await app.open(mock('/settings', '&role=admin'));
  const card = screen.getByRole('region', 'End-of-day digest');
  await firstShown(card.getByRole('button', 'Save'));
  expect(await browser.locator('#digest-enabled').isChecked()).toBe(false);
  await browser.locator('#digest-enabled').check();
  await browser.locator('#digest-at').fill('07:30');
  await browser.locator('#digest-park-hours').fill('0');
  await card.getByRole('button', 'Save').click();
  await expect(screen.getByText(/1 to 720 hours/)).toBeVisible();
  expect(await setting(browser)).toEqual({ enabled: false, at: '18:00', park_idle_hours: null });
  await browser.locator('#digest-park-hours').fill('24');
  await card.getByRole('button', 'Save').click();
  await expect(screen.getByText('Digest on, at 07:30 UTC')).toBeVisible();
  expect(await setting(browser)).toEqual({ enabled: true, at: '07:30', park_idle_hours: 24 });
  expect(await axeViolations(browser)).toEqual([]);
});

test('a member sees no digest setting (SPEC-512 DIG-001)', async ({ app, screen }) => {
  await app.open(mock('/settings', '&role=member&telegram=1'));
  await firstShown(screen.getByRole('button', 'Link Telegram'));
  await expect(screen.getByRole('region', 'End-of-day digest')).toHaveCount(0);
});

test('a digest made while no page was open reaches the bell and opens /digest (SPEC-512 DIG-005)', async ({ app, browser, screen }) => {
  await app.open(mock('/settings', '&digest=1'));
  const bell = screen.getByRole('button', /^Notifications \(1 unread\)/);
  await firstShown(bell);
  await bell.click();
  const entry = screen.getByRole('dialog', 'Notifications').getByRole('button', /^Unread: Your end-of-day digest/);
  await expect(entry).toBeVisible();
  await expect(screen.getByText('1 waits for you · 1 parked · 2 still working')).toBeVisible();
  await entry.click();
  await expect(screen.getByRole('heading', 'Waiting for you (1)')).toBeVisible();
  await expect(screen.getByText('Add a ruleset on main')).toBeVisible();
  await expect(screen.getByText(/Last message: Please promote dev to main\./)).toBeVisible();
  await expect(screen.getByRole('heading', 'Still working (2)')).toBeVisible();
  expect(await screen.getByRole('link', 'quartio/studio #12 Ruleset for main').getAttribute('href')).toMatch(/\/s\/studio\/t\/12$/);
  expect(await axeViolations(browser)).toEqual([]);
});

test("the day's digest arrives while the page is open (SPEC-512 DIG-005)", async ({ app, browser, screen }) => {
  await app.open(mock('/settings'));
  await eventsConnected(browser);
  await expect(screen.getByRole('button', /^Notifications \(\d+ unread\)/)).toHaveCount(0);
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.sendDigest('dg_live'));
  await screen.getByRole('button', /^Notifications \(1 unread\)/).click();
  await expect(screen.getByRole('dialog', 'Notifications').getByRole('button', /^Unread: Your end-of-day digest/)).toBeVisible();
});

test('a cleared digest entry does not come back on the next load (SPEC-512 DIG-005, review)', async ({ app, browser, screen }) => {
  await app.open(mock('/settings', '&digest=1'));
  const bell = screen.getByRole('button', /^Notifications \(1 unread\)/);
  await firstShown(bell);
  await bell.click();
  await screen.getByRole('dialog', 'Notifications').getByRole('button', 'Clear').click();
  // Another address, so the page really loads again (the same one is not reloaded).
  await app.open(mock('/settings', '&digest=1&again=1'));
  // The bell reads the digest when the page opens: wait until it has, then look once.
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __yuliusMock: { digest: { reads?: number } } }).__yuliusMock.digest.reads ?? 0)).toBeGreaterThan(0);
  await firstShown(screen.getByRole('button', /^Notifications$/));
  await expect(screen.getByRole('button', /^Notifications \(\d+ unread\)/)).toHaveCount(0);
});

test('a digest arriving live reaches every open tab, also one whose browser already saw it (SPEC-512 DIG-005, review round 2)', async ({ app, browser, screen }) => {
  await app.open(mock('/settings'));
  await eventsConnected(browser);
  // Another tab of the same browser heard this digest first and remembered it.
  await browser.evaluate(() => { const id = (window as unknown as { __yuliusMock: { me: { user: { id: string } } } }).__yuliusMock.me.user.id; localStorage.setItem(`yulius.digest.seen.${id}`, JSON.stringify('dg_both')); });
  await expect(screen.getByRole('button', /^Notifications \(\d+ unread\)/)).toHaveCount(0);
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.sendDigest('dg_both'));
  await screen.getByRole('button', /^Notifications \(1 unread\)/).click();
  await expect(screen.getByRole('dialog', 'Notifications').getByRole('button', /^Unread: Your end-of-day digest/)).toBeVisible();
});
