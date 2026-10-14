import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

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

test('a malformed stored entry is ignored and the overview fills the rows', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const seen = await chipsAfterRead(browser);
  await browser.evaluate(() => {
    const key = Object.keys(localStorage).find((k) => k.startsWith('yulius.pulls.') && k.endsWith('.sb_studio')) as string;
    localStorage.setItem(key, '{"not":"rows"}');
  });

  await app.open(mock('/s/studio', `&hold=${GIT}`));
  await expect(browser.locator('.task-list .task-row').first()).toBeVisible();
  await expect(chips(browser)).toHaveCount(0);
  await release(browser);
  await expect(chips(browser)).toHaveText(seen);
});

test('signing out removes the stored pull requests', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await chipsAfterRead(browser);
  await screen.getByRole('button', /account menu$/).click();
  await screen.getByRole('menuitem', 'Sign out').click();
  await expect(browser).toHaveURL(/#\/signin/);
  expect(await storedPulls(browser)).toBeNull();
});
