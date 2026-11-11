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
