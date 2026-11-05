import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// SPEC-423 (task #405): "we want be able to explicitly disable this kind of protection for users
// with no access to github pro. It should be done in Git section". The Git page's Production
// protection card switches off the requirement of a GitHub ruleset on the production branch.

type Mocked = { __yuliusMock: { sandboxes: { slug: string; profile: { protect_production?: boolean } }[] } };

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

  await required.click();
  await expect(screen.getByText(/Production protection is off/)).toBeVisible();
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
