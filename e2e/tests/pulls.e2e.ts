import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock, touchTask } from './support.ts';

failOnPageErrors();

// Task #89 (SPEC-104): "after reloading prs are not visible at the beginning". The sidebar's pull
// request chips came only from the sandbox's Git overview, which the server reads from GitHub and
// Git for every task, so after a reload the rows showed no pull request for seconds. The chips this
// browser last saw show at once; the fresh read replaces them.

const GIT = '/api/sandboxes/sb_studio/git';
const chips = (browser: Browser) => browser.locator('.task-list .task-chips .tl-pr-chip');
const release = (browser: Browser) => browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());

/** The pull requests this browser keeps for the studio sandbox, as stored. */
function storedPulls(browser: Browser): Promise<Array<[string, { number: number }]> | null> {
  return browser.evaluate(() => {
    const key = Object.keys(localStorage).find((k) => k.startsWith('yulius.pulls.') && k.endsWith('.sb_studio'));
    return key ? JSON.parse(localStorage.getItem(key) as string) : null;
  });
}

/** Replaces the stored pull requests of the studio sandbox with `rows` (any JSON). */
function storePulls(browser: Browser, rows: unknown): Promise<void> {
  return browser.evaluate((value) => {
    const key = Object.keys(localStorage).find((k) => k.startsWith('yulius.pulls.') && k.endsWith('.sb_studio')) as string;
    localStorage.setItem(key, JSON.stringify(value));
  }, rows);
}

/** The sidebar's chips once the Git overview answered, as their texts ("#41"). */
async function chipsAfterRead(browser: Browser): Promise<string[]> {
  await expect(chips(browser).first()).toBeVisible();
  return chips(browser).allTextContents();
}

test('after a reload the sidebar shows the pull requests at once, before the Git overview answers', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const seen = await chipsAfterRead(browser);
  expect(seen.length).toBeGreaterThan(0);

  // The reload's overview read is held: the rows already show what was last seen.
  await app.open(mock('/s/studio', `&hold=${GIT}`));
  await expect(chips(browser)).toHaveText(seen);
  await release(browser);
  await expect(chips(browser)).toHaveText(seen);
});

test('the fresh read replaces the pull requests remembered from before', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const seen = await chipsAfterRead(browser);
  // What an older read said: the first pull request had another number then.
  await browser.evaluate(() => {
    const key = Object.keys(localStorage).find((k) => k.startsWith('yulius.pulls.') && k.endsWith('.sb_studio')) as string;
    const rows = JSON.parse(localStorage.getItem(key) as string);
    rows[0][1].number = 999;
    localStorage.setItem(key, JSON.stringify(rows));
  });

  await app.open(mock('/s/studio', `&hold=${GIT}`));
  await expect(chips(browser).filter({ hasText: '#999' })).toHaveCount(1);
  await release(browser);
  await expect(chips(browser).filter({ hasText: '#999' })).toHaveCount(0);
  await expect(chips(browser)).toHaveText(seen);
  expect((await storedPulls(browser))!.some(([, pr]) => pr.number === 999)).toBe(false);
});

// Rows of the wrong shape name the sidebar's own tasks, so a row let through would show a chip.
const wrongRows = (ids: string[]) => [[ids[0], null], [ids[1], { number: 'x', state: 'open' }], [ids[2], { number: 7 }], [ids[3], 'pr']];
for (const [what, value] of [
  ['an object', () => ({ not: 'rows' })],
  ['rows of the wrong shape', wrongRows],
] as const) {
  test(`a stored value that is ${what} is ignored and the overview fills the rows`, async ({ app, browser }) => {
    await app.open(mock('/s/studio'));
    const seen = await chipsAfterRead(browser);
    const ids = (await storedPulls(browser))!.map(([id]) => id);
    expect(ids.length).toBeGreaterThanOrEqual(4);
    await storePulls(browser, value(ids));

    await app.open(mock('/s/studio', `&hold=${GIT}`));
    await expect(browser.locator('.task-list .task-row').first()).toBeVisible();
    await expect(chips(browser)).toHaveCount(0);
    await release(browser);
    await expect(chips(browser)).toHaveText(seen);
  });
}

test('a sandbox no view watches does not use the remembered pull requests', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  await chipsAfterRead(browser);
  // A failing pull request remembered for #12, which has none: in another sandbox nobody reads
  // studio's overview, so a notification about #12 must not say what that old read said.
  const id = await browser.evaluate(() => (window as unknown as { __yuliusMock: { tasks: Array<{ id: string; sandbox_id: string; number: number }> } }).__yuliusMock.tasks.find((t) => t.sandbox_id === 'sb_studio' && t.number === 12)!.id);
  const rows = (await storedPulls(browser))!;
  await storePulls(browser, [...rows, [id, { number: 999, url: 'https://github.com/x/y/pull/999', title: 'Old', state: 'open', base: 'dev', head: 'feat/old', checks: 'failure' }]]);

  // Only the notification this change brings counts: the studio page may have stored one about #12.
  await browser.evaluate(() => { for (const k of Object.keys(localStorage)) if (k.startsWith('yulius.notifications.')) localStorage.removeItem(k); });

  // A new page load (another query, not only another hash): nothing read in memory yet.
  await app.open(mock('/s/product', '&page=2'));
  await expect(browser.locator('.task-list .task-row').first()).toBeVisible();
  await touchTask(browser, 'sb_studio', 12, { status: 'running', activity: 'waiting' });
  const detail = () => browser.evaluate(() => {
    const key = Object.keys(localStorage).find((k) => k.startsWith('yulius.notifications.') && k !== 'yulius.notifications.filter');
    const list = key ? JSON.parse(localStorage.getItem(key) as string) : [];
    const found = (Array.isArray(list) ? list : []).find((n: { title: string }) => n.title.startsWith('#12 '));
    return found ? found.detail as string : null;
  });
  await expect.poll(detail).not.toBeNull();
  expect(await detail()).not.toContain('#999');
});

test('a failed first read drops the remembered pull requests instead of keeping them', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  await chipsAfterRead(browser);
  const ids = (await storedPulls(browser))!.map(([id]) => id);
  await storePulls(browser, [[ids[0], { number: 999, url: 'https://github.com/x/y/pull/999', title: 'Old', state: 'open', base: 'dev', head: 'feat/old', checks: 'success' }]]);

  // The overview fails (a deploy restarting the server): an old read must not pass for a current one.
  await app.open(mock('/s/studio', `&fail=${GIT}`));
  await expect(browser.locator('.task-list .task-row').first()).toBeVisible();
  await expect(chips(browser)).toHaveCount(0);
});

test('a session found expired when the page loads removes the stored pull requests', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await chipsAfterRead(browser);
  expect(await storedPulls(browser)).not.toBeNull();
  await app.open(mock('/s/studio', '&signedout=1'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sign in');
  expect(await storedPulls(browser)).toBeNull();
});

test('a session that expires removes the stored pull requests', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await chipsAfterRead(browser);
  expect(await storedPulls(browser)).not.toBeNull();
  await browser.evaluate(() => { (window as unknown as { __yuliusMock: { signedIn: boolean } }).__yuliusMock.signedIn = false; });
  await screen.getByRole('link', /^Git/).first().click();
  await expect(screen.getByText('You were signed out. Please sign in again.')).toBeVisible();
  expect(await storedPulls(browser)).toBeNull();
});

test('signing out removes the stored pull requests', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await chipsAfterRead(browser);
  expect(await storedPulls(browser)).not.toBeNull();
  await screen.getByRole('button', /account menu$/).click();
  await screen.getByRole('menuitem', 'Sign out').click();
  await expect(browser).toHaveURL(/#\/signin/);
  expect(await storedPulls(browser)).toBeNull();
});

// Task #93 (SPEC-108): "we got wrong status again". Task #56 read Done with PR #340 "checks
// running" 4.5 minutes after Yulius had merged it: the task's state comes live, its pull request
// only from the overview read, every minute in view and never while hidden. The server now says
// when a sandbox's pull requests changed (`pulls.updated`) and the browser reads them again.
type Mocked = {
  __yuliusMock: {
    changePull: (sandbox: string, number: number, changes: Record<string, unknown>, announce?: boolean) => void;
    announcePulls: (sandbox: string) => void;
    holdGitReads: boolean;
    gitReads: () => number;
    gitRequests: number;
    releaseGit: () => void;
  };
};
const mocked = <T>(browser: Browser, run: (m: Mocked['__yuliusMock']) => T) => browser.evaluate(`(${run.toString()})(window.__yuliusMock)`) as Promise<T>;
// #13's goal is achieved (Done) and its pull request #520 is open with its checks running: the
// row of the screenshot.
const chip520 = (browser: Browser) => browser.locator('.task-list .tl-pr-chip').filter({ hasText: '#520' });

/**
 * Events arrive in order and the page asks for a read as it handles one: once a rename announced
 * after them shows, every event before it was handled. Returns the Git reads asked for by then.
 */
async function handledSoFar(browser: Browser, title: string): Promise<number> {
  await touchTask(browser, 'sb_studio', 13, { title });
  await expect(browser.locator('.task-list .task-row').filter({ hasText: title })).toBeVisible();
  return mocked(browser, (m) => m.gitRequests);
}

test('a merge Yulius announces reaches the row at once', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Improve the code/)).toBeVisible();
  await expect(chip520(browser)).toHaveAttribute('title', 'Pull request #520 · open · checks running');
  await mocked(browser, (m) => m.changePull('sb_studio', 520, { state: 'merged' }));
  // Within the 3 s between announced reads, well before the next minute's read.
  await expect(chip520(browser)).toHaveAttribute('title', 'Pull request #520 · merged', { timeout: 10_000 });
});

test('an announcement during a read is read again after it', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&pullgap=0'));
  await expect(chip520(browser)).toHaveAttribute('title', /checks running/);
  // A read starts before the merge and answers after it; the merge's announcement comes meanwhile.
  const before = await handledSoFar(browser, 'Improve the code, read');
  await mocked(browser, (m) => { m.holdGitReads = true; m.announcePulls('sb_studio'); });
  await expect.poll(() => mocked(browser, (m) => m.gitReads())).toBe(1);
  await mocked(browser, (m) => m.changePull('sb_studio', 520, { state: 'merged' }));
  // Reads do not overlap: the announcement waits for the read in flight.
  expect(await handledSoFar(browser, 'Improve the code, merged')).toBe(before + 1);
  // Every held read answers, the newest first: the one from before the merge must not have the
  // last word.
  await expect.poll(async () => {
    await mocked(browser, (m) => m.releaseGit());
    return chip520(browser).getAttribute('title');
  }, { timeout: 3_000 }).toBe('Pull request #520 · merged');
});

test('a hidden tab reads nothing on an announcement and catches up when shown', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&pullgap=0'));
  await expect(chip520(browser)).toHaveAttribute('title', /checks running/);
  const before = await handledSoFar(browser, 'Improve the code, shown');
  await browser.evaluate(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await mocked(browser, (m) => m.changePull('sb_studio', 520, { state: 'merged' }));
  expect(await handledSoFar(browser, 'Improve the code, hidden')).toBe(before);
  await expect(chip520(browser)).toHaveAttribute('title', /checks running/);

  // Shown again seconds after the last read: the announcement it missed is read at once.
  await browser.evaluate(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await expect(chip520(browser)).toHaveAttribute('title', 'Pull request #520 · merged', { timeout: 3_000 });
});

test('announced reads of a sandbox keep their gap', async ({ app, browser }) => {
  // A minute between announced reads: what this test sees in seconds stays apart.
  await app.open(mock('/s/studio', '&pullgap=60000'));
  await expect(chip520(browser)).toHaveAttribute('title', /checks running/);
  const before = await handledSoFar(browser, 'Improve the code, before');
  // A push: its run queued, started and ended, a superseded run cancelled.
  await mocked(browser, (m) => { for (let i = 0; i < 4; i += 1) m.announcePulls('sb_studio'); });
  expect(await handledSoFar(browser, 'Improve the code, after')).toBe(before);
});

test('watching a sandbox again during its read starts no second read', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&pullgap=0'));
  await expect(chip520(browser)).toHaveAttribute('title', /checks running/);
  const before = await handledSoFar(browser, 'Improve the code, before');
  await mocked(browser, (m) => { m.holdGitReads = true; m.announcePulls('sb_studio'); });
  await expect.poll(() => mocked(browser, (m) => m.gitReads())).toBe(1);
  // To another sandbox and back while studio's read is held: product reads once, studio waits for
  // its read in flight.
  await browser.evaluate(() => { location.hash = '#/s/product'; });
  await expect(browser.locator('.task-list .task-row').filter({ hasText: 'Invoice PDF export' })).toBeVisible();
  await browser.evaluate(() => { location.hash = '#/s/studio'; });
  expect(await handledSoFar(browser, 'Improve the code, back')).toBe(before + 2);
  await mocked(browser, (m) => { m.holdGitReads = false; m.releaseGit(); });
  await expect(chip520(browser)).toHaveAttribute('title', /checks running/);
});
