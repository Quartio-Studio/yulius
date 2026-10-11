import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// F 2026-10-10: "Those Claude sessions should go on server as well. I want to be able to see them
// in read-only mode as any other session. It should be accessible from the panel." Sessions lists
// task sessions and imported ones together, with filters and search; one opens read-only.
test('Sessions lists task and imported sessions, filters and searches them', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('navigation', 'Sandbox').getByRole('link', 'Sessions').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/sessions$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sessions');

  const list = screen.getByRole('list', 'Sessions');
  await expect(list.getByRole('link', /Checkout API/)).toBeVisible();
  await expect(list.getByRole('link', /Add Stripe checkout to the cart page/)).toContainText('Imported by @mkarp');
  await expect(list.getByRole('link', /Add Stripe checkout/)).toContainText('3 secrets redacted');
  expect(await axeViolations(browser)).toEqual([]);

  // Only the uploaded ones.
  await screen.getByRole('radiogroup', 'Kind of session').getByRole('radio', /^Imported/).click();
  await expect(list.getByRole('link')).toHaveCount(2);
  await expect(list.getByRole('link', /Checkout API/)).toHaveCount(0);
  await expect(browser).toHaveURL(/kind=imported/);

  // Full-text search marks what matched.
  await screen.getByRole('radiogroup', 'Kind of session').getByRole('radio', /^All/).click();
  await screen.getByRole('searchbox', 'Search sessions').fill('stripe webhook');
  await expect(list.getByRole('link')).toHaveCount(2);
  await expect(browser.locator('.ss-rows mark').first()).toBeVisible();
  await expect(screen.getByText('2 sessions match')).toBeVisible();
  await screen.getByRole('searchbox', 'Search sessions').fill('no such words anywhere');
  await expect(screen.getByText('Nothing found')).toBeVisible();
});

test('a session opens read-only with messages, tool calls and diffs, and continues in a task', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/sessions'));
  await screen.getByRole('list', 'Sessions').getByRole('link', /Add Stripe checkout/).click();
  await expect(browser).toHaveURL(/#\/s\/studio\/sessions\/ses_imp_checkout/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Add Stripe checkout to the cart page');
  await expect(screen.getByText('Read-only. Secrets are redacted.')).toBeVisible();

  const transcript = screen.getByRole('list', 'Transcript');
  await expect(transcript.getByText('Add Stripe checkout to the cart page. Use the test keys from .env.')).toBeVisible();
  // An edit shows as a diff, with its result inside the call.
  const diff = transcript.getByRole('group', 'Changes to src/api/checkout.ts');
  await expect(diff).toContainText('+ export async function checkout(cart, key) {');
  await expect(diff).toContainText('- export async function checkout(cart) {');
  await expect(transcript.getByText('The file src/api/checkout.ts has been updated.')).toBeVisible();
  // A redacted dotenv read stays redacted; folded calls open on demand.
  await transcript.getByText('/repo/.env').click();
  await expect(transcript.getByText(/STRIPE_SECRET_KEY=\[REDACTED:env-value\]/)).toBeVisible();
  await expect(screen.getByText('End of the session.')).toBeVisible();
  // No text box: nothing can be typed into a read-only session.
  expect(await browser.evaluate(() => document.querySelectorAll('#main textarea, #main input[type="text"]').length)).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);

  // Continuing creates a task from the session and opens it.
  await screen.getByRole('button', 'Continue in a new task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
});

test('a search match opens the transcript at its line', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/sessions?q=fulfilling'));
  await screen.getByRole('list', 'Sessions').getByRole('link', /Add Stripe checkout/).click();
  await expect(browser).toHaveURL(/sessions\/ses_imp_checkout\?line=7/);
  await expect(browser.locator('.ss-target')).toContainText('verifies signatures before fulfilling orders');
  await expect(screen.getByRole('link', 'Show from the start')).toBeVisible();
});
