import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

/**
 * Where the tools rail's items sit: items (and their count badges) reaching outside the rail,
 * the pressed (open) tools, and whether the rail or the page reaches past the window's edge.
 */
function railLayout(browser: Browser) {
  return browser.evaluate(() => {
    const rail = document.querySelector('nav.rail')!;
    const box = rail.getBoundingClientRect();
    // The rail's content box: inside its edge line.
    const left = box.left + rail.clientLeft;
    const right = left + rail.clientWidth;
    const outside: string[] = [];
    const pressed: string[] = [];
    for (const item of rail.querySelectorAll('button')) {
      const name = item.getAttribute('aria-label') ?? '';
      const r = item.getBoundingClientRect();
      if (r.left < left - 0.5 || r.right > right + 0.5 || item.scrollWidth > item.clientWidth) outside.push(name);
      for (const mark of item.querySelectorAll('.rail-badge:not([hidden]), .rail-dot:not([hidden])')) {
        const m = mark.getBoundingClientRect();
        if (m.left < r.left - 0.5 || m.right > r.right + 0.5) outside.push(`${name} (count)`);
      }
      if (item.getAttribute('aria-pressed') === 'true') pressed.push(name);
    }
    return {
      outside,
      pressed,
      railInWindow: box.right <= window.innerWidth + 0.5,
      pageScrollsSideways: document.documentElement.scrollWidth > window.innerWidth,
    };
  });
}

// F 2026-10-09: the rail looked "a little broken": highlighted items ran past its right edge.
test('the tools rail keeps its items inside it, with one open tool', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  const rail = screen.getByRole('navigation', 'Tools');
  await expect(rail).toBeVisible();

  // SPEC-028 (MSR-005): there are no Messages; SPEC-035 (AGT-001): the Agent comes first; then
  // Browser, Editor, Shell and Notes (SPEC-041).
  expect(await browser.evaluate(() => [...document.querySelectorAll('nav.rail button')].map((b) => b.getAttribute('aria-label'))))
    .toEqual(['Agent', 'Browser', 'Editor', 'Shell', 'Notes']);
  await rail.getByRole('button', 'Editor').click();
  await expect(rail.getByRole('button', 'Editor')).toHaveAttribute('aria-pressed', 'true');
  expect(await railLayout(browser)).toEqual({ outside: [], pressed: ['Editor'], railInWindow: true, pageScrollsSideways: false });

  await rail.getByRole('button', 'Shell').click();
  await expect(rail.getByRole('button', 'Shell')).toHaveAttribute('aria-pressed', 'true');
  expect(await railLayout(browser)).toEqual({ outside: [], pressed: ['Shell'], railInWindow: true, pageScrollsSideways: false });

  expect(await axeViolations(browser)).toEqual([]);
});

// The cause found for F 2026-10-09: a panel opened in a wide window kept its width when the window
// narrowed, so the grid grew wider than the window and cut the rail's right side off.
test('the rail stays in the window when the window narrows with a panel open', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  const rail = screen.getByRole('navigation', 'Tools');
  await rail.getByRole('button', 'Editor').click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();

  // Before the fix the rail's right 20 px were cut off here.
  await browser.setViewport({ width: 1040, height: 800 });
  await expect.poll(() => railLayout(browser)).toEqual({ outside: [], pressed: ['Editor'], railInWindow: true, pageScrollsSideways: false });

  // Past the breakpoint where the sidebar narrows, the panel has given way and the centre keeps room.
  await browser.setViewport({ width: 1000, height: 800 });
  await expect.poll(() => railLayout(browser)).toEqual({ outside: [], pressed: ['Editor'], railInWindow: true, pageScrollsSideways: false });
  await expect.poll(() => browser.evaluate(() => document.querySelector('.center')!.getBoundingClientRect().width)).toBeGreaterThanOrEqual(300);
});

test('on a 390 px phone the rail sits in the top bar without covering it', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await expect(screen.getByRole('navigation', 'Tools')).toBeVisible();

  const layout = await browser.evaluate(() => {
    const rail = document.querySelector('nav.rail')!.getBoundingClientRect();
    const bar = document.querySelector('.tabrow')!;
    const reach = Math.max(...[...bar.querySelectorAll('button, select')].filter((e) => (e as HTMLElement).offsetParent).map((e) => e.getBoundingClientRect().right));
    const items = [...document.querySelectorAll('nav.rail button')].map((b) => b.getBoundingClientRect());
    return {
      topBarClearOfRail: reach <= rail.left + 0.5,
      railInWindow: rail.right <= window.innerWidth + 0.5,
      itemsCentred: items.every((b) => Math.abs(b.top + b.height / 2 - (rail.top + rail.height / 2)) <= 1),
      pageScrollsSideways: document.documentElement.scrollWidth > window.innerWidth,
    };
  });
  expect(layout).toEqual({ topBarClearOfRail: true, railInWindow: true, itemsCentred: true, pageScrollsSideways: false });
});
