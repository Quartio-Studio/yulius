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

// SPEC-484 PLN-010 (task #465 on 2026-11-13: "You say 5 of 15 but I see only 6 steps … the number
// of steps grows in the time"): a plan holds all the work its agent knows of, and one that grew
// since its first step was finished says what it had then, so a person can tell.
test('a plan that grew says how many steps it had at first', async ({ app, browser, screen }) => {
  // #12's plan has seven steps; it had four when its first step was finished.
  await app.open(mock('/s/studio/t/12', '&planfirst=4'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  const count = browser.locator('.stepstrip .step-count');
  await expect(count).toHaveText('Step 3/7 · 4 at first');
  await expect(browser.locator('.stepstrip .step-first')).toHaveAttribute('title', 'The plan had 4 steps when its first step was finished; 3 were added since.');
  // The strip still fits its row: the count is one part that gives way whole.
  const fits = await browser.evaluate(() => {
    const strip = document.querySelector('.stepstrip-main');
    const part = document.querySelector('.stepstrip .step-count');
    if (!strip || !part) return null;
    return { inside: part.getBoundingClientRect().right <= strip.getBoundingClientRect().right + 0.5, oneLine: part.getBoundingClientRect().height < 30 };
  });
  expect(fits).toEqual({ inside: true, oneLine: true });

  // The task page says it beside the count.
  await app.open(mock('/s/studio/tasks/12', '&ambient=0&planfirst=4'));
  await expect(browser.locator('.ti-facts .stat:has-text("Plan") .stat-value')).toHaveText('2/7 · 4 at first');
});

test('a plan that did not grow says nothing more', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await expect(browser.locator('.stepstrip .step-count')).toHaveText('Step 3/7');
  await expect(browser.locator('.stepstrip .step-first')).toHaveCount(0);
  // A first size that is not below the plan's (an older plan, or one that shrank) is not a growth.
  await app.open(mock('/s/studio/t/12', '&planfirst=7'));
  await expect(browser.locator('.stepstrip .step-count')).toHaveText('Step 3/7');
  await expect(browser.locator('.stepstrip .step-first')).toHaveCount(0);
});
