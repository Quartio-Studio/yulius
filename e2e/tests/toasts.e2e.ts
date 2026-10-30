import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Toasts (#954): a toast stays while the pointer is on it (WCAG 2.2.1), but only once the pointer
// moves there. One that appears or moves under a pointer resting where it was closes on time, or
// it would cover the page header's buttons for good.

/** In the page: raise a toast with dom.js's `toast()`, its message also its `data-probe`. */
const raise = (browser: Browser, message: string, timeout = 500) => browser.evaluate(async ({ message, timeout }) => {
  const url = '/assets/dom.js';
  const { toast } = await import(url);
  toast(message, { timeout });
  const el = [...document.querySelectorAll<HTMLElement>('#toasts .toast')].pop()!;
  el.dataset.probe = message;
  return el.parentElement!.classList.contains('flipped');
}, { message, timeout });

/** The centre of a raised toast, where it is now. */
const centre = (browser: Browser, message: string) => browser.evaluate((message) => {
  const box = document.querySelector(`[data-probe="${message}"]`)!.getBoundingClientRect();
  return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
}, message);

/** A toast is hovered: the pointer is on it although it never moved there. */
const hovered = (browser: Browser, message: string) => expect(browser.locator(`[data-probe="${message}"]:hover`)).toHaveCount(1);

const open = async (app: { open: (url: string) => Promise<unknown> }, browser: Browser) => {
  await app.open(mock('/s/studio', '&ambient=0&browserask=0'));
  // `top`: where the flipped stack starts (styles.css `.toasts.flipped`).
  return browser.evaluate(() => ({ w: innerWidth, h: innerHeight, top: parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--tabrow-h')) + 8 }));
};

test('a toast that appears under a resting pointer closes on time', async ({ app, browser }) => {
  const size = await open(app, browser);
  // Where the stack shows at the bottom right.
  await browser.mouse.move(size.w - 100, size.h - 30);
  expect(await raise(browser, 'Under a still pointer')).toBe(false);
  await hovered(browser, 'Under a still pointer');
  await expect(browser.locator('[data-probe="Under a still pointer"]')).toHaveCount(0);
});

test('a toast that flips under a resting pointer closes on time', async ({ app, browser }) => {
  const size = await open(app, browser);
  // The focused control sits where the stack shows, so the stack moves to the top right, where the
  // pointer rests (the page header's add buttons are there; #954 clicked one and the toast took it).
  await browser.evaluate(() => {
    const button = document.createElement('button');
    button.textContent = 'Covered';
    button.style.cssText = 'position: fixed; right: 12px; bottom: 12px; width: 380px; height: 80px; z-index: 1';
    document.body.append(button);
    button.focus();
  });
  await browser.mouse.move(size.w - 100, size.top + 20);
  expect(await raise(browser, 'Flipped under a still pointer')).toBe(true);
  await hovered(browser, 'Flipped under a still pointer');
  await expect(browser.locator('[data-probe="Flipped under a still pointer"]')).toHaveCount(0);
});

test('a toast the pointer moves onto stays until the pointer leaves it', async ({ app, browser }) => {
  const size = await open(app, browser);
  await browser.mouse.move(8, size.h / 2);
  await raise(browser, 'Pointed at');
  await raise(browser, 'Beside it');
  const kept = await centre(browser, 'Pointed at');
  await browser.mouse.move(kept.x, kept.y);
  // The other toast of the same timeout closes; the one pointed at stays.
  await expect(browser.locator('[data-probe="Beside it"]')).toHaveCount(0);
  await expect(browser.locator('[data-probe="Pointed at"]')).toHaveCount(1);
  await browser.mouse.move(8, size.h / 2);
  await expect(browser.locator('[data-probe="Pointed at"]')).toHaveCount(0);
});
