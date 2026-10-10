import { test, type Browser } from '@e2e-dev/web';
import { expect, type Locator } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

/** An element's rendered width in whole pixels. */
const width = (browser: Browser, selector: string) =>
  browser.evaluate((s: string) => Math.round(document.querySelector(s)!.getBoundingClientRect().width), selector);

/** Drags a splitter `dx` pixels sideways with the pointer. */
async function drag(browser: Browser, splitter: Locator, dx: number) {
  const box = await splitter.boundingBox();
  if (!box) throw new Error('the splitter is not on screen');
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await browser.mouse.move(x, y);
  await browser.mouse.down();
  await browser.mouse.move(x + dx / 2, y);
  await browser.mouse.move(x + dx, y);
  await browser.mouse.up();
}

// SPEC-001 UI-044 (F 2026-10-10): "make the bars resizable horizontally".
test('the sidebar resizes by its edge and by keyboard, keeps its width after a reload, and resets on double-click', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  const splitter = screen.getByRole('separator', 'Resize sidebar');
  await expect(splitter).toHaveAttribute('aria-valuenow', '300');

  await drag(browser, splitter, 80);
  await expect.poll(() => width(browser, '.sidebar')).toBe(380);
  await expect(splitter).toHaveAttribute('aria-valuenow', '380');

  // ←/→ move the edge 16 px (Shift: 64), Home/End go to the narrowest and widest.
  await splitter.press('ArrowLeft');
  await expect.poll(() => width(browser, '.sidebar')).toBe(364);
  await splitter.press('Shift+ArrowLeft');
  await expect.poll(() => width(browser, '.sidebar')).toBe(300);
  await splitter.press('ArrowRight');
  await expect.poll(() => width(browser, '.sidebar')).toBe(316);
  await splitter.press('Home');
  await expect(splitter).toHaveAttribute('aria-valuenow', '200');
  await splitter.press('End');
  await expect(splitter).toHaveAttribute('aria-valuenow', '480');
  await splitter.press('Shift+ArrowLeft');
  await expect.poll(() => width(browser, '.sidebar')).toBe(416);

  // Kept for this user in this browser.
  await browser.reload();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  await expect.poll(() => width(browser, '.sidebar')).toBe(416);

  await screen.getByRole('separator', 'Resize sidebar').dblclick();
  await expect.poll(() => width(browser, '.sidebar')).toBe(300);
  await browser.reload();
  await expect.poll(() => width(browser, '.sidebar')).toBe(300);
  expect(await axeViolations(browser)).toEqual([]);
});

test('the editor explorer and the tool panel resize by their edges and keep their widths after a reload', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await expect(screen.getByRole('tree', 'Files')).toBeVisible();
  const explorer = await width(browser, '.ed-side');
  const panel = await width(browser, '.panel');

  await drag(browser, screen.getByRole('separator', 'Resize explorer'), 60);
  await expect.poll(() => width(browser, '.ed-side')).toBe(explorer + 60);
  // The panel's splitter is on its left edge: to the right makes it narrower.
  await drag(browser, screen.getByRole('separator', 'Resize panel'), 40);
  await expect.poll(() => width(browser, '.panel')).toBe(panel - 40);
  await expect.poll(() => width(browser, '.ed-side')).toBe(explorer + 60);

  await browser.reload();
  await screen.getByRole('button', 'Editor').click();
  await expect(screen.getByRole('tree', 'Files')).toBeVisible();
  await expect.poll(() => width(browser, '.panel')).toBe(panel - 40);
  await expect.poll(() => width(browser, '.ed-side')).toBe(explorer + 60);

  // A double-click on either edge brings its default back.
  await screen.getByRole('separator', 'Resize explorer').dblclick();
  await expect.poll(() => width(browser, '.ed-side')).toBe(explorer);
  await screen.getByRole('separator', 'Resize panel').dblclick();
  await expect.poll(() => width(browser, '.panel')).toBe(panel);
});

// The rail and the centre stay in the window (UI-042): with a panel open the sidebar gives way first.
test('in a 1024 px window with a panel open the sidebar gives way, so the centre keeps 360 px', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Shell').click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();

  await browser.setViewport({ width: 1024, height: 768 });
  const fit = () => browser.evaluate(() => {
    const rail = document.querySelector('nav.rail')!.getBoundingClientRect();
    return {
      centre: Math.round(document.querySelector('.center')!.getBoundingClientRect().width) >= 360,
      sidebar: Math.round(document.querySelector('.sidebar')!.getBoundingClientRect().width) < 300,
      railInWindow: rail.right <= window.innerWidth + 0.5,
      pageScrollsSideways: document.documentElement.scrollWidth > window.innerWidth,
    };
  });
  await expect.poll(fit).toEqual({ centre: true, sidebar: true, railInWindow: true, pageScrollsSideways: false });

  // Closing the panel gives the sidebar its width back.
  await screen.getByRole('button', 'Close panel').click();
  await expect.poll(() => width(browser, '.sidebar')).toBe(300);
});

test('on a 390 px phone there are no splitters and the navigation stays a drawer', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  await expect(screen.getByRole('separator', 'Resize sidebar')).toBeHidden();

  const menu = screen.getByRole('button', 'Open navigation');
  await menu.click();
  await expect(menu).toHaveAttribute('aria-expanded', 'true');
  await expect(screen.getByRole('separator', 'Resize sidebar')).toBeHidden();
  await browser.keyboard.press('Escape');
  await expect(menu).toHaveAttribute('aria-expanded', 'false');
});
