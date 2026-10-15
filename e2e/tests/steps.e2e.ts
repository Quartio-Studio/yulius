import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// F 2026-10-10 (STP-006, #139): "make sure todos declared by every task takes similar amount of
// time: 9/10 should mean 90%". Steps are of similar effort; the one that cannot be split declares
// a larger effort, and the progress bar is weighted by it.

test('the task progress bar is weighted by the effort of its steps', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  // Two of seven steps are done; the open "Implement checkout API" step carries effort 3,
  // so the plan is 2 of 9 effort units: 22 %, not 28 %.
  const bar = browser.locator('.step-progress');
  await expect(bar).toHaveAttribute('aria-valuenow', '22');
  await expect(bar).toHaveAttribute('aria-label', '2 of 7 steps done, 22% of the plan, 33% in progress');
});

// #150: "currently running task should be recorded in progress in different color. 2/4 and only
// 1/4 colored seems weird". The step in progress follows the finished fill in the working colour.
test('the step in progress shows on the bar in the working colour', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await expect(browser.locator('.step-progress')).toHaveAttribute('aria-valuenow', '22');

  const bar = await browser.evaluate(() => {
    const track = document.querySelector('.step-progress');
    const done = track?.querySelector('.step-progress-fill');
    const active = track?.querySelector('.step-progress-active');
    if (!track || !done || !active) return null;
    const box = (el: Element) => el.getBoundingClientRect();
    const probe = document.createElement('span');
    probe.style.color = 'var(--ok)';
    document.body.append(probe);
    const ok = getComputedStyle(probe).color;
    probe.remove();
    return {
      done: Math.round((box(done).width / box(track).width) * 100),
      active: Math.round((box(active).width / box(track).width) * 100),
      after: Math.abs(box(active).left - box(done).right) < 1,
      hidden: (active as HTMLElement).hidden,
      colour: getComputedStyle(active).backgroundColor === ok,
      distinct: getComputedStyle(active).backgroundColor !== getComputedStyle(done).backgroundColor,
      // The two segments meet square: no rounded notch where the finished fill ends.
      join: parseFloat(getComputedStyle(done).borderTopRightRadius) === 0 && parseFloat(getComputedStyle(active).borderTopLeftRadius) === 0,
    };
  });
  // 22 % finished, then the open step's 3 of 9 effort units (33 %) right after it.
  expect(bar).toEqual({ done: 22, active: 33, after: true, hidden: false, colour: true, distinct: true, join: true });
});

test('a plan with no step in progress shows no in-progress segment', async ({ app, browser, screen }) => {
  // #3 "Image pipeline" crashed with 2 of 7 steps done and none in progress.
  await app.open(mock('/s/studio/t/3'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#3 Image pipeline');
  const bar = browser.locator('.step-progress');
  await expect(bar).toHaveAttribute('aria-label', '2 of 7 steps done, 22% of the plan');

  const shown = await browser.evaluate(() => {
    const active = document.querySelector('.step-progress-active') as HTMLElement | null;
    const done = document.querySelector('.step-progress-fill');
    if (!active || !done) return null;
    return {
      hidden: active.hidden,
      width: active.getBoundingClientRect().width,
      // The finished fill keeps its rounded end when nothing follows it.
      rounded: parseFloat(getComputedStyle(done).borderTopRightRadius) > 0,
    };
  });
  expect(shown).toEqual({ hidden: true, width: 0, rounded: true });
});
