import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock } from './support.ts';

failOnPageErrors();

// Tasks hand work to another sandbox (SPEC-512 part 5, #1106): the instance switch in Settings
// (HOF-001), a sandbox's opt-out on its Git page (HOF-002), the owner approving a stored request
// on the task's page (HOF-004) and a handed-off task showing where it came from (HOF-005).

type Mocked = Window & { __yuliusMock: { handoffs?: { setting: unknown; accept: Record<string, boolean> }; handoffAnswers?: number } };

test('an instance administrator turns handoffs on, off by default (SPEC-512 HOF-001)', async ({ app, browser, screen }) => {
  await app.open(mock('/settings', '&role=admin'));
  const card = screen.getByRole('region', 'Handoffs between sandboxes');
  await firstShown(card.getByRole('button', 'Save'));
  expect(await browser.locator('#handoffs-mode').inputValue()).toBe('off');
  await card.getByRole('combobox', 'Mode').selectOption({ label: 'Ask: the task’s owner approves each one' });
  await card.getByRole('button', 'Save').click();
  await expect(screen.getByText('Handoffs: ask')).toBeVisible();
  const setting = await browser.evaluate(() => JSON.parse(JSON.stringify((window as unknown as Mocked).__yuliusMock.handoffs?.setting)));
  expect(setting).toEqual({ mode: 'ask', max_depth: 3, per_day: 20 });
  expect(await axeViolations(browser)).toEqual([]);
});

test("the task's owner approves a handoff its agent asked for, exactly as shown (SPEC-512 HOF-004)", async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0&handoff=1'));
  const request = screen.getByRole('region', 'Handoff to quartio/web');
  await firstShown(request);
  await expect(request.getByText('Add GET /v1/items returning the catalog, with tests.')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await request.getByRole('button', 'Approve').click();
  await expect(screen.getByText('Handed off to quartio/web')).toBeVisible();
  await expect(screen.getByText(/Created: quartio\/web · Add the items endpoint/)).toBeVisible();
  expect(await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.handoffAnswers ?? 0)).toBe(1);
});

test('a handed-off task shows where it came from (SPEC-512 HOF-005)', async ({ app, screen }) => {
  await app.open(mock('/s/studio/t/11', '&ambient=0&handoff=1'));
  await firstShown(screen.getByText('Handed off from'));
  await expect(screen.getByRole('link', 'quartio/web #4')).toBeVisible();
});

test("a sandbox's writers close it to handoffs (SPEC-512 HOF-002)", async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/git'));
  const box = browser.locator('#sandbox-handoffs');
  await firstShown(screen.getByRole('region', 'Handoffs from other sandboxes'));
  await expect.poll(() => box.isChecked()).toBe(true);
  await box.uncheck();
  await expect(screen.getByText('This sandbox refuses handoffs')).toBeVisible();
  const accept = await browser.evaluate(() => JSON.parse(JSON.stringify((window as unknown as Mocked).__yuliusMock.handoffs?.accept ?? {})));
  expect(Object.values(accept)).toEqual([false]);
});
