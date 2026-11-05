import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// SPEC-423 (task #405): "we want be able to explicitly disable this kind of protection for users
// with no access to github pro. It should be done in Git section". The Git page's Production
// protection card switches off the requirement of a GitHub ruleset on the production branch.

type Mocked = { __yuliusMock: { sandboxes: { slug: string; profile: { protect_production?: boolean } }[]; release: () => void; resyncNow: () => void } };

/** The mock's stored switch of the sandbox `slug`. */
const switchOf = (browser: Browser, slug: string) => browser.evaluate((s: string) => {
  const sandbox = (window as unknown as Mocked).__yuliusMock.sandboxes.find((sb) => sb.slug === s);
  return sandbox ? (sandbox.profile.protect_production ?? null) : 'missing';
}, slug);

test('an admin turns the production ruleset requirement off and on in the Git page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/git'));
  const card = screen.getByRole('region', 'Production protection');
  const required = card.getByRole('switch', 'Require a GitHub ruleset on main');
  await expect(required).toBeChecked();
  await expect(required).toBeEnabled();
  await expect(card.getByText(/cannot have rulesets: switch this off to accept the risk/)).toBeVisible();
  await expect(card.getByRole('link', 'Rulesets on GitHub')).toHaveAttribute('href', 'https://github.com/Quartio-Studio/studio/settings/rules');
  expect(await axeViolations(browser)).toEqual([]);

  // From the keyboard: the keyboard stays on the switch after the change (review round 1).
  await required.focus();
  await browser.keyboard.press('Space');
  await expect(screen.getByText(/Production protection is off/)).toBeVisible();
  await expect(card.getByRole('switch', 'Require a GitHub ruleset on main')).toBeFocused();
  await expect(card.getByText(/accepted that agents could push to or merge into main/)).toBeVisible();
  await expect(card.getByRole('switch', 'Require a GitHub ruleset on main')).not.toBeChecked();
  expect(await switchOf(browser, 'studio')).toBe(false);
  expect(await switchOf(browser, 'product')).toBe(true);

  await card.getByRole('switch', 'Require a GitHub ruleset on main').click();
  await expect(card.getByRole('switch', 'Require a GitHub ruleset on main')).toBeChecked();
  expect(await switchOf(browser, 'studio')).toBe(true);
});

test('a member sees the production protection switch but cannot change it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/git', '&role=member'));
  const card = screen.getByRole('region', 'Production protection');
  const required = card.getByRole('switch', 'Require a GitHub ruleset on main');
  await expect(required).toBeChecked();
  await expect(required).toBeDisabled();
  await expect(card.getByText(/Only this sandbox’s admins/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
});

test('a refused change shows the stored switch again, also after a redraw while it was sent', async ({ app, browser, screen }) => {
  // Review round 1: the page redraws on every sandbox or task event; the card must not keep the
  // switch it drew while the change was on its way.
  await app.open(mock('/s/studio/git', '&ambient=0&protect_patch_fail=1&holdpost=/api/sandboxes/sb_studio'));
  const card = screen.getByRole('region', 'Production protection');
  const required = card.getByRole('switch', 'Require a GitHub ruleset on main');
  await expect(required).toBeChecked();
  await required.click();
  // A live update redraws the page while the change is held: the switch drawn then is a new one.
  await browser.evaluate(() => {
    document.querySelector('#production-protection input[role="switch"]')!.setAttribute('data-before', '1');
    const db = (window as unknown as Mocked).__yuliusMock;
    (db.sandboxes.find((sb) => sb.slug === 'studio') as unknown as { counts: { running: number } }).counts.running += 1;
    db.resyncNow();
    return null;
  });
  await expect.poll(() => browser.evaluate(() => document.querySelectorAll('#production-protection input[data-before]').length)).toBe(0);
  await expect(card.getByRole('switch', 'Require a GitHub ruleset on main')).toBeDisabled();
  await browser.evaluate(() => { (window as unknown as Mocked).__yuliusMock.release(); return null; });
  await expect(screen.getByText(/Couldn’t change the production protection/)).toBeVisible();
  // At once with the failure, not at some later live update.
  expect(await browser.evaluate(() => {
    const input = document.querySelector<HTMLInputElement>('#production-protection input[role="switch"]')!;
    return { checked: input.checked, disabled: input.disabled };
  })).toEqual({ checked: true, disabled: false });
  await expect(card.getByRole('switch', 'Require a GitHub ruleset on main')).toBeChecked();
  await expect(card.getByRole('switch', 'Require a GitHub ruleset on main')).toBeEnabled();
  expect(await switchOf(browser, 'studio')).toBe(true);
});

/** Whether the card's top is in the window. */
const cardInView = (browser: Browser) => browser.evaluate(() => {
  const card = document.getElementById('production-protection');
  if (!card) return false;
  const box = card.getBoundingClientRect();
  return box.top >= 0 && box.top < window.innerHeight;
});

test('a link to the card scrolls to it below the board', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/git'));
  await expect(screen.getByRole('heading', /^Activity/)).toBeVisible();
  await expect(screen.getByRole('region', 'Production protection')).toBeVisible();
  expect(await cardInView(browser)).toBe(false);
  await app.open(mock('/s/studio/git?show=production-protection'));
  await expect(screen.getByRole('heading', /^Activity/)).toBeVisible();
  await expect.poll(() => cardInView(browser)).toBe(true);
});

test('the card is there when the delivery overview cannot be read', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/git?show=production-protection', '&fail=/api/sandboxes/sb_studio/git'));
  await expect(screen.getByText(/Couldn’t load the delivery overview/)).toBeVisible();
  const card = screen.getByRole('region', 'Production protection');
  await expect(card.getByRole('switch', 'Require a GitHub ruleset on main')).toBeChecked();
  await expect.poll(() => cardInView(browser)).toBe(true);
});
