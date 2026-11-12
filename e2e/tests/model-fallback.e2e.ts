import { type Browser, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock, touchTask } from './support.ts';

failOnPageErrors();

// A task of a Fable model runs Opus 5.5 while no subscription can run Fable (SPEC-482): the pages
// say which model a session runs and why, and count the subscriptions that run the fallback.

const LINE = 'Fable 5.2 is used up on every subscription: tasks run on Opus 5.5 until it can run again';

type Account = { label: string; status: string; usage: Record<string, unknown> };

/** Every subscription of the mock is out of Fable's allowance, with room in its own windows; with `opusToo`, out of Opus as well. */
async function fableOutEverywhere(browser: Browser, opusToo = false): Promise<void> {
  await browser.evaluate((opus) => {
    const db = (window as unknown as { __yuliusMock: { accounts: Account[] } }).__yuliusMock;
    const resets_at = new Date(Date.now() + 40 * 3600e3).toISOString();
    const w = (used_percent: number) => ({ used_percent, resets_at, window_minutes: 10080 });
    db.accounts.forEach((account) => Object.assign(account, {
      status: 'limited',
      usage: { primary: w(10), secondary: w(20), weekly_fable: w(100), ...(opus ? { weekly_opus: w(100) } : {}), observed_at: new Date().toISOString() },
    }));
  }, opusToo);
}

test('a task’s details say that its session runs on the fallback and why (SPEC-482 FBK-009)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks/11'));
  await firstShown(screen.getByRole('heading', { level: 2, name: 'Details' }));
  const model = () => browser.evaluate(() => {
    const dt = [...document.querySelectorAll('.ti-kv dt')].find((el) => (el as HTMLElement).innerText === 'Model');
    return dt ? (dt.nextElementSibling as HTMLElement).innerText : null;
  });
  // A task that names no model runs the default one. Its session is refused Fable everywhere
  // and starts again on the same subscription: the details are read again and name the fallback.
  await touchTask(browser, 'sb_studio', 11, { model: null, status: 'switching' });
  await expect.poll(model).toBe('Fable 5.2 (default) · High effort');
  await touchTask(browser, 'sb_studio', 11, { fallback_model: 'claude-opus-5-5', status: 'running' });
  await expect.poll(model).toBe('Fable 5.2 (default) · High effort · runs on Opus 5.5 while Fable 5.2 is used up');
  expect(await axeViolations(browser)).toEqual([]);
});

test('the Capacity panel says that tasks run on the fallback and counts the subscriptions that run it (SPEC-482 FBK-010)', async ({ app, browser, screen }) => {
  const tile = () => browser.evaluate(() => {
    const tiles = [...document.querySelectorAll('.capacity-panel .stat-tile')] as HTMLElement[];
    const found = tiles.find((el) => el.innerText.includes('Available now'));
    return found ? found.innerText.replace(/\s+/g, ' ').trim() : null;
  });
  await app.open(mock('/admin/accounts'));
  await expect(screen.getByRole('region', 'Capacity').getByText('Available now')).toBeVisible();
  expect(await browser.locator('.capacity-fallback').count()).toBe(0);
  expect(await tile()).toMatch(/below 98% in every window$/);
  expect(await tile()).not.toMatch(/Opus/);

  // The page reads the pool again when it is opened again.
  await browser.evaluate(() => { location.hash = '#/s/studio'; });
  await expect(browser.locator('.capacity-panel')).toHaveCount(0);
  await fableOutEverywhere(browser);
  await browser.evaluate(() => { location.hash = '#/admin/accounts'; });
  await expect(browser.locator('.capacity-fallback')).toHaveText(LINE);
  // The subscriptions that run the fallback are the ones available.
  await expect.poll(tile).toMatch(/Available now [1-9]\d* of \d+ for Opus 5\.5, below 98% in every window$/);
  expect(await axeViolations(browser)).toEqual([]);
});

test('the status line of a pool out of Fable says that tasks run on the fallback (SPEC-482 FBK-010)', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await fableOutEverywhere(browser);
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .status-health')).toHaveText(LINE);
  // Out of Opus too, nothing runs until a window resets: the line says that, as before.
  await browser.locator('.btn-status').click();
  await fableOutEverywhere(browser, true);
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .status-health')).toHaveText('Every subscription is at a usage limit: tasks continue when a window resets');
});

test('the model the pool places a task with, and which subscription takes it (unit, in the page; SPEC-482)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const got = await browser.evaluate(async () => {
    const [usage, actions, models] = await Promise.all(['/assets/usage.js', '/assets/actions.js', '/assets/models.js'].map((url) => import(url)));
    const ahead = new Date(Date.now() + 3600e3).toISOString();
    const past = new Date(Date.now() - 3600e3).toISOString();
    const w = (used: number, resets_at = ahead) => ({ used_percent: used, resets_at, window_minutes: 10080 });
    const fableOut = { status: 'limited', usage: { primary: w(10), secondary: w(20), weekly_fable: w(100) } };
    const room = { status: 'ok', usage: { primary: w(40), secondary: w(12) } };
    const full = { status: 'limited', usage: { primary: w(100), weekly_fable: w(100) } };
    const bothOut = { status: 'limited', usage: { primary: w(10), weekly_fable: w(100), weekly_opus: w(99) } };
    const fable = { model: 'claude-fable-5-2' };
    return {
      fallback: [models.fallbackOf('claude-fable-5-2'), models.fallbackOf('claude-fable-5-1'), models.fallbackOf('claude-opus-5-5'), models.fallbackOf('claude-sonnet-5-5'), models.fallbackOf(null)],
      // No model: the default one.
      allOut: usage.placingModel([fableOut, fableOut], null),
      oneRuns: usage.placingModel([fableOut, room], 'claude-fable-5-2'),
      // A window past its reset is a new window: Fable runs again.
      reset: usage.placingModel([{ status: 'limited', usage: { weekly_fable: w(100, past) } }], 'claude-fable-5-2'),
      neither: usage.placingModel([full, bothOut], 'claude-fable-5-2'),
      needsSignIn: usage.placingModel([{ ...fableOut, status: 'relogin' }], 'claude-fable-5-2'),
      reported: usage.placingModel([{ status: 'limited', usage: { limit_reached: true, limit_resets_at: null, weekly_fable: w(100) } }], 'claude-fable-5-2'),
      noFallback: usage.placingModel([{ status: 'ok', usage: { weekly_opus: w(100) } }], 'claude-opus-5-5'),
      // "Run #N on…": with Fable used up everywhere, a subscription with room takes the task.
      movedInFallback: actions.cannotTake(fableOut, fable, [fableOut, fableOut]),
      movedWhileOneRunsFable: actions.cannotTake(fableOut, fable, [fableOut, room]),
      movedOntoOneOutOfBoth: actions.cannotTake(bothOut, fable, [fableOut, bothOut]),
      judgedAlone: actions.cannotTake(fableOut, fable),
    };
  });
  expect(got).toEqual({
    fallback: ['claude-opus-5-5', 'claude-opus-5-5', null, null, null],
    allOut: 'claude-opus-5-5',
    oneRuns: 'claude-fable-5-2',
    reset: 'claude-fable-5-2',
    neither: 'claude-fable-5-2',
    needsSignIn: 'claude-fable-5-2',
    reported: 'claude-fable-5-2',
    noFallback: 'claude-opus-5-5',
    movedInFallback: null,
    movedWhileOneRunsFable: 'At its usage limit',
    movedOntoOneOutOfBoth: null,
    judgedAlone: 'At its usage limit',
  });
});
