import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Task #401 (SPEC-419, #1060): a project whose deploys print long release ids (quartio-v2's
// engine and infra parts) kept the CD cards within their bounds only with short ids like v41.

/** quartio-v2's release ids: a commit, then a content hash. */
const LONG = [
  '0e36f03a3bba-67f8230233522c3c4db7df093ecd6648aa6cf8f23746ebb84444c2d4d5adfb1c',
  '0731fe929a36-ca825cf952fe2db0b884816f7bc1dbbd352efb1bc205f3b47e89a8a7da902ff6',
  '5d1c0b2a9e8f-1f2e3d4c5b6a79881726354453627180918a7b6c5d4e3f2a1b0c9d8e7f6a5b4c',
  '4c3b2a190807-2e3d4c5b6a798817263544536271809a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e',
  '3b2a19080706-3d4c5b6a798817263544536271809a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f',
];
type Mock = { __yuliusMock: { cd: Record<string, { environments: { name: string; rollback: { parts: unknown[] } }[] }> } };

/** Gives dev the parts engine and infra, each with `count` kept releases with long ids, newest first. */
async function longReleases(browser: { evaluate<T, A>(fn: (a: A) => T, a: A): Promise<T> }, count: number) {
  await browser.evaluate(({ ids, count }) => {
    const dev = (window as unknown as Mock).__yuliusMock.cd.sb_studio.environments.find((e) => e.name === 'dev')!;
    const releases = ids.slice(0, count).map((release, i) => ({
      release, commit: `${release.slice(0, 12)}0000`, deployed_at: new Date(Date.now() - (33 + i * 150) * 60000).toISOString(),
      requested_by: 'task #77', current: i === 0, blocked: null, barrier: null,
    }));
    dev.rollback.parts = ['engine', 'infra'].map((name) => ({ name, current: ids[0], previous: ids[1], releases: releases.map((r) => ({ ...r })) }));
  }, { ids: LONG, count });
}

/** Opens the CD tab again, so it reads the mock's environments again. */
async function reread(screen: { getByRole(role: string, name: string): { click(): Promise<void> } }) {
  await screen.getByRole('radio', 'CI (tests)').click();
  await screen.getByRole('radio', 'CD (deployment)').click();
}

/** Every element of a CD card that lies outside the card, by a whole pixel. */
const outside = (browser: { evaluate<T>(fn: () => T): Promise<T> }) => browser.evaluate(() => {
  const out: string[] = [];
  for (const card of document.querySelectorAll('.cd-env')) {
    const box = card.getBoundingClientRect();
    for (const el of card.querySelectorAll('*')) {
      const r = el.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue;
      if (r.left < box.left - 1 || r.right > box.right + 1) out.push(`${el.tagName.toLowerCase()} ${(el.textContent || '').trim().slice(0, 40)}`);
    }
  }
  return out;
});

test('long release ids keep every CD card within its bounds, with one action per release', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1440, height: 900 });
  await app.open(mock('/s/studio/ci?section=cd'));
  await expect(screen.getByRole('region', 'Dev rollback')).toBeVisible();
  await longReleases(browser, 2);
  await reread(screen);
  const dev = screen.getByRole('region', 'Dev rollback');
  await expect(dev.getByRole('heading', 'engine')).toBeVisible();
  expect(await outside(browser)).toEqual([]);

  // The previous release's own button is the one-click default; the current one has none.
  const back = dev.getByRole('button', `Roll back to previous (${LONG[1]}) of dev engine`);
  await expect(back).toBeVisible();
  await expect(back).toHaveText('Roll back to previous');
  expect(await browser.evaluate(() => [...document.querySelectorAll('.cd-rollback[aria-label="Dev rollback"] .cd-releases > li')]
    .map((li) => li.querySelectorAll('button').length))).toEqual([0, 1, 0, 1]);
  await expect(dev.getByRole('button', /^Roll back to this/)).toBeHidden();
  // The full id is in the row's tooltip.
  expect(await browser.evaluate((id) => document.querySelector(`.cd-releases li[data-release="${id}"] code`)?.getAttribute('title') ?? null, LONG[1])).toBe(LONG[1]);
  expect(await axeViolations(browser)).toEqual([]);

  // The confirmation names the whole id; its button a short one.
  await back.click();
  const confirm = screen.getByRole('dialog', 'Roll back dev engine?');
  await expect(confirm.getByText(new RegExp(`^Redeploys ${LONG[1]} \\(from `))).toBeVisible();
  expect(await browser.evaluate(() => { const b = document.querySelector('dialog[open] .modal-body')!; return b.scrollWidth - b.clientWidth; })).toBeLessThanOrEqual(0);
  await confirm.getByRole('button', 'Roll back to 0731fe929a36…').click();
  // A shortened id's own ellipsis ends the starting toast: never two.
  await expect(screen.getByText('Starting the rollback of dev engine to 0731fe929a36…')).toBeVisible();
  await expect(screen.getByText('Rolling dev engine back to 0731fe929a36….')).toBeVisible();

  // A phone: one column, nothing outside its card, nothing scrolls sideways.
  await browser.setViewport({ width: 390, height: 844 });
  await expect.poll(() => outside(browser)).toEqual([]);
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
});

/** The ids of the engine part's release rows a person sees, in order. */
const shownEngine = (browser: { evaluate<T>(fn: () => T): Promise<T> }) => browser.evaluate(() =>
  [...document.querySelectorAll('.cd-rollback[aria-label="Dev rollback"] .cd-part')]
    .filter((p) => p.querySelector('h4')?.textContent === 'engine')
    .flatMap((p) => [...p.querySelectorAll('li[data-release]')])
    .filter((li) => li.checkVisibility())
    .map((li) => (li as HTMLElement).dataset.release!.slice(0, 4)));

test('a part shows its three newest releases and folds the older ones', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?section=cd'));
  await expect(screen.getByRole('region', 'Dev rollback')).toBeVisible();
  await longReleases(browser, 5);
  await reread(screen);
  const dev = screen.getByRole('region', 'Dev rollback');
  await expect(dev.getByRole('heading', 'engine')).toBeVisible();
  expect(await shownEngine(browser)).toEqual(['0e36', '0731', '5d1c']);
  await browser.locator('.cd-rollback[aria-label="Dev rollback"] .cd-part:has-text("engine") summary').click();
  await expect.poll(() => shownEngine(browser)).toEqual(['0e36', '0731', '5d1c', '4c3b', '3b2a']);
  expect(await browser.evaluate(() => [...document.querySelectorAll('.cd-rollback[aria-label="Dev rollback"] summary')].map((s) => [s.textContent, s.getAttribute('aria-label')]))).toEqual([
    ['2 older releases', '2 older releases of dev engine'], ['2 older releases', '2 older releases of dev infra']]);
  expect(await axeViolations(browser)).toEqual([]);

  // Walked back: the release deployed now and the previous one are never folded.
  await browser.evaluate((ids) => {
    const dev = (window as unknown as Mock).__yuliusMock.cd.sb_studio.environments.find((e) => e.name === 'dev')!;
    const engine = dev.rollback.parts[0] as { current: string; previous: string; releases: { release: string; current: boolean }[] };
    Object.assign(engine, { current: ids[2], previous: ids[3] });
    for (const r of engine.releases) r.current = r.release === ids[2];
  }, LONG);
  await reread(screen);
  await expect(dev.getByRole('heading', 'engine')).toBeVisible();
  expect(await shownEngine(browser)).toEqual(['0e36', '0731', '5d1c', '4c3b']);
  await expect(dev.getByRole('button', `Roll back to previous (${LONG[3]}) of dev engine`)).toBeVisible();
});
