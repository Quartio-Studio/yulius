import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Settings → Instance, "Address" (SPEC-466 MVC-011): whether GitHub accepts this instance's
// sign-in address. After a move to another URL it does not, until a person adds the new callback
// URL to the GitHub App: the card gives the one link and the two values, reads the answer again
// until it is registered, and says where the instance came from and what the first start did with
// the tasks. The mock: `callback=missing|unknown` (GitHub's answer), `moved=1` (a move's report).

type AddressMock = { __yuliusMock: { address: { state: string }; sent?: Record<string, number> } };

const answers = (browser: Browser, state: string) => browser.evaluate((s: string) => {
  (window as unknown as AddressMock).__yuliusMock.address.state = s;
}, state);

const MISSING = /GitHub sign-in doesn’t work at this address yet/;
const REGISTERED = /GitHub accepts this address for sign-in/;

test('a missing callback is said with the App’s link and both values, until GitHub accepts it', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&callback=missing'));
  await expect(screen.getByRole('heading', { level: 2, name: 'Address' })).toBeVisible();
  const card = screen.getByRole('region', 'Address');
  await expect(card.getByText(MISSING)).toBeVisible();
  await expect(card.getByText(/password sign-in works meanwhile/)).toBeVisible();

  const said = await browser.evaluate(() => {
    const el = document.querySelector<HTMLElement>('section.address');
    const link = el ? el.querySelector<HTMLAnchorElement>('a.btn') : null;
    const value = (name: string) => {
      const box = el ? el.querySelector<HTMLElement>(`[data-address-value="${name}"]`) : null;
      return box ? box.innerText.trim() : 'missing';
    };
    return {
      origin: location.origin,
      link: link ? { text: link.innerText.trim(), href: link.href, target: link.target, rel: link.rel } : null,
      callback: value('callback'),
      homepage: value('homepage'),
    };
  });
  expect(said.link).toEqual({
    text: 'Open the App’s settings on GitHub',
    href: 'https://github.com/organizations/Quartio-Studio/settings/apps/yulius-quartio-studio',
    target: '_blank',
    rel: 'noopener noreferrer',
  });
  expect(said.callback).toBe(`${said.origin}/api/auth/github/callback`);
  expect(said.homepage).toBe(said.origin);
  await expect(card.getByRole('button', 'Copy the callback URL')).toBeVisible();
  await expect(card.getByRole('button', 'Copy the homepage URL')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  // The person adds the callback on GitHub: the card says so by itself, without a reload.
  await answers(browser, 'registered');
  await expect(card.getByText(REGISTERED)).toBeVisible({ timeout: 15000 });
  await expect(card.getByText(MISSING)).toBeHidden();
  expect(await browser.evaluate(() => ((window as unknown as AddressMock).__yuliusMock.sent || {})['POST /api/admin/instance/address/check'] || 0)).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);
});

test('"Check again" asks GitHub at once, and an answer GitHub did not give says why', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&callback=unknown'));
  const card = screen.getByRole('region', 'Address');
  await expect(card.getByText(/GitHub didn’t say whether it accepts this address: GitHub: timed out/)).toBeVisible();
  await expect(card.getByText(MISSING)).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  await answers(browser, 'registered');
  await card.getByRole('button', 'Check again').click();
  await expect(card.getByText(REGISTERED)).toBeVisible();
  expect(await browser.evaluate(() => ((window as unknown as AddressMock).__yuliusMock.sent || {})['POST /api/admin/instance/address/check'] || 0)).toBe(1);
});

test('after a move the card says where the instance came from and what happened to its tasks', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&moved=1'));
  const card = screen.getByRole('region', 'Address');
  await expect(card.getByText(/Moved here from https:\/\/old\.yulius\.example/)).toBeVisible();
  await expect(card.getByText(/Its first start put 7 tasks back in the queue, left 2 stopped that a person had paused or was stopping, and could not queue 1: its owner finds it under Needs you\./)).toBeVisible();
  await expect(card.getByText(REGISTERED)).toBeVisible();
  // Nothing to paste when GitHub accepts the address.
  await expect(browser.locator('section.address [data-address-value]')).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);
});

test('without a GitHub App the card says that nothing is to register', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&github=0'));
  const card = screen.getByRole('region', 'Address');
  await expect(card.getByText(/No GitHub App is connected/)).toBeVisible();
  await expect(card.getByText(MISSING)).toBeHidden();
  await expect(browser.locator('section.address button:visible')).toHaveCount(0);
});

test('a load that fails says so and can be tried again', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&fail=/api/admin/instance/address'));
  const card = screen.getByRole('region', 'Address');
  await expect(card.getByText(/Couldn’t load the address/)).toBeVisible();
  await browser.evaluate(() => { (window as unknown as { __yuliusMock: { failPath: string | null } }).__yuliusMock.failPath = null; });
  await card.getByRole('button', 'Try again').click();
  await expect(card.getByText(REGISTERED)).toBeVisible();
});

type HoldMock = { __yuliusMock: { holdPath: string | null; held: Promise<void> | null; release(): void; heldWaiting?: number; address: { state: string; answered: number } } };
const ADDRESS_API = '/api/admin/instance/address';

test('"Check again" stays the person’s while the card reads the answer again, and a late read does not undo it', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&callback=missing'));
  const card = screen.getByRole('region', 'Address');
  await expect(card.getByText(MISSING)).toBeVisible();
  // The card's next read of the answer stays on its way.
  await browser.evaluate((path: string) => {
    const m = (window as unknown as HoldMock).__yuliusMock;
    m.holdPath = path;
    m.held = new Promise<void>((resolve) => { m.release = resolve; });
  }, ADDRESS_API);
  await expect.poll(() => browser.evaluate(() => (window as unknown as HoldMock).__yuliusMock.heldWaiting ?? 0), { timeout: 15000 }).toBe(1);
  const answered = await browser.evaluate(() => (window as unknown as HoldMock).__yuliusMock.address.answered);

  // Meanwhile the person adds the callback on GitHub and presses Check again.
  await answers(browser, 'registered');
  const check = card.getByRole('button', 'Check again');
  await expect(check).toBeEnabled();
  await check.click();
  await expect(card.getByText(REGISTERED)).toBeVisible();

  // The read that was on its way answers now, with what an earlier look would have said: the
  // card keeps the later answer and reads nothing more.
  await answers(browser, 'missing');
  await browser.evaluate(() => { (window as unknown as HoldMock).__yuliusMock.release(); });
  await expect.poll(() => browser.evaluate(() => (window as unknown as HoldMock).__yuliusMock.address.answered)).toBe(answered + 2);
  await expect(card.getByText(REGISTERED)).toBeVisible();
  await expect(card.getByText(MISSING)).toBeHidden();
});

/** The page goes to the background or comes back, as when the person switches tabs. */
const setHidden = (browser: Browser, hidden: boolean) => browser.evaluate((h: boolean) => {
  Object.defineProperty(document, 'hidden', { configurable: true, get: () => h });
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (h ? 'hidden' : 'visible') });
  document.dispatchEvent(new Event('visibilitychange'));
}, hidden);

test('coming back to the page asks GitHub at once', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&callback=missing'));
  const card = screen.getByRole('region', 'Address');
  await expect(card.getByText(MISSING)).toBeVisible();
  const checks = () => browser.evaluate(() => ((window as unknown as AddressMock).__yuliusMock.sent || {})['POST /api/admin/instance/address/check'] || 0);

  // The person goes to GitHub in another tab, adds the callback and comes back.
  await setHidden(browser, true);
  await answers(browser, 'registered');
  expect(await checks()).toBe(0);
  await setHidden(browser, false);
  await expect.poll(checks).toBe(1);
  await expect(card.getByText(REGISTERED)).toBeVisible();

  // Nothing is left to ask about: coming back again asks nothing.
  await setHidden(browser, true);
  await setHidden(browser, false);
  await expect(card.getByText(REGISTERED)).toBeVisible();
  expect(await checks()).toBe(1);
});

test('a read that fails while the card is shown keeps what it shows and is tried again', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&callback=missing'));
  const card = screen.getByRole('region', 'Address');
  await expect(card.getByText(MISSING)).toBeVisible();
  // The server is away for a moment (a restart): the card's next read fails.
  type FailMock = { __yuliusMock: { failPath: string | null; failedReads: number } };
  const failed = () => browser.evaluate(() => (window as unknown as FailMock).__yuliusMock.failedReads);
  const before = await failed();
  await browser.evaluate((path: string) => { (window as unknown as FailMock).__yuliusMock.failPath = path; }, ADDRESS_API);
  await expect.poll(failed, { timeout: 15000 }).toBe(before + 1);
  await expect(card.getByText(MISSING)).toBeVisible();
  await expect(card.getByText(/Couldn’t load the address/)).toBeHidden();
  // It is back, and GitHub accepts the callback meanwhile: the card finds out by itself.
  await browser.evaluate(() => { (window as unknown as FailMock).__yuliusMock.failPath = null; });
  await answers(browser, 'registered');
  await expect(card.getByText(REGISTERED)).toBeVisible({ timeout: 15000 });
});
