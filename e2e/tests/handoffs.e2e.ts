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
  // Answered, it floats no more (review round 2): its history is on the task's info page.
  await expect(screen.getByRole('region', 'Handoff to quartio/web')).toHaveCount(0);
  expect(await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.handoffAnswers ?? 0)).toBe(1);
});

test('a handed-off task shows where it came from on its info page (SPEC-512 HOF-005)', async ({ app, screen }) => {
  await app.open(mock('/s/studio/tasks/11', '&ambient=0&handoff=1'));
  await firstShown(screen.getByText(/^Handed off from/));
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

test('a long handoff floats over the terminal and its answer stays in view (SPEC-512 HOF-004, review)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0&handoff=long'));
  const request = screen.getByRole('region', 'Handoff to quartio/web');
  await firstShown(request);
  const placed = await browser.evaluate(() => {
    const panel = document.querySelector('.handoffs');
    const approve = [...document.querySelectorAll('.handoff-request button')].find((b) => b.textContent === 'Approve');
    if (!panel || !approve) return null;
    const box = approve.getBoundingClientRect();
    return { position: getComputedStyle(panel).position, inView: box.top >= 0 && box.bottom <= window.innerHeight };
  });
  expect(placed).toEqual({ position: 'absolute', inView: true });
});
