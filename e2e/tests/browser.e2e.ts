import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, DRAG_GRIP, DRAG_HINT, dragTab, failOnPageErrors, mock, pressAndMove, tabGrips, tabMenu, tabNames, tabsInPlace } from './support.ts';

failOnPageErrors();

type MockBrowser = { typed: Map<string, string>; acked: number };
type WithMock = Window & { __yuliusMock: { browsers: Map<string, MockBrowser> } };

// The browser panel (SPEC-002 ADR-15): the sandbox Chrome — the agents' browser — streamed onto a
// canvas (in mock mode a page drawn by assets/mock.js, sent as video where the browser's
// WebCodecs can encode it, as JPEG frames otherwise), its tabs in the strip, the address bar;
// clicks and keys reach the page; the tab an agent asks the user to act in is marked and its
// sign-in is done right in the panel.
test('the sandbox Chrome streams into the panel and takes clicks and keys', async ({ app, browser, screen }) => {
  // The page is shown 1:1 once the shared viewport is this panel's size; right after the panel
  // resizes (a banner appears) the old picture is scaled for a moment. Clicks at fixed page
  // positions wait for that.
  const atPanelSize = () => expect
    .poll(() => browser.evaluate(() => {
      const canvas = document.querySelector('.bp-canvas') as HTMLCanvasElement | null;
      if (!canvas || canvas.width < 10) return false;
      const rect = canvas.getBoundingClientRect();
      return Math.abs(rect.width - canvas.width) < 1 && Math.abs(rect.height - canvas.height) < 1;
    }), { timeout: 10000 })
    .toBe(true);

  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Browser/).click();

  const checkout = screen.getByRole('tab', /^Checkout/);
  await expect(checkout).toHaveAttribute('aria-selected', 'true');
  await expect(screen.getByRole('textbox', 'Address')).toHaveValue(/^localhost:\d+\/$/);
  // A picture arrives: the canvas is painted (the mock's page has a dark header).
  await expect
    .poll(() => browser.evaluate(() => {
      const canvas = document.querySelector('.bp-canvas') as HTMLCanvasElement | null;
      const context = canvas && canvas.getContext('2d');
      if (!canvas || !context || canvas.width < 10) return false;
      const [r, g, b] = context.getImageData(10, 10, 1, 1).data;
      return r + g + b < 200;
    }), { timeout: 15000 })
    .toBe(true);
  expect(await axeViolations(browser)).toEqual([]);
  // Video frames are acknowledged as they arrive (the server paces the stream by them).
  const video = await browser.evaluate(() => (document.querySelector('.bp-viewchip') as HTMLElement | null)?.hidden ?? false);
  if (video) {
    await expect
      .poll(() => browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.browsers.get('sb_studio')?.acked ?? 0))
      .toBeGreaterThan(0);
  }

  // Keys typed after a click in the page go to the page.
  await atPanelSize();
  await browser.locator('.bp-surface').click({ position: { x: 120, y: 270 } });
  await browser.keyboard.type('mug');
  await expect
    .poll(() => browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.browsers.get('sb_studio')?.typed.get('tab-app') ?? ''))
    .toBe('mug');

  // The agent's sign-in tab asks for the user's action; the user signs in in the panel.
  const aws = screen.getByRole('tab', /Amazon Web Services Sign-In/);
  await aws.click();
  await expect(aws).toHaveAttribute('aria-selected', 'true');
  await expect(screen.getByText(/An agent asks for your action here/)).toBeVisible();
  // The banner made the panel shorter: its own new size is not "another viewer's".
  await atPanelSize();
  await expect(screen.getByText('Sized for another viewer')).toBeHidden();
  await browser.locator('.bp-surface').click({ position: { x: 200, y: 355 } });
  await expect(screen.getByRole('tab', /AWS Management Console/)).toBeVisible();
  await expect(screen.getByText(/An agent asks for your action here/)).toBeHidden();

  // A new tab opens the sandbox's welcome page.
  await screen.getByRole('button', 'New tab').click();
  await expect(screen.getByRole('tab', /^New tab/)).toHaveAttribute('aria-selected', 'true');
  await expect(screen.getByRole('textbox', 'Address')).toHaveValue('');
});

// SPEC-434 MTB-003/004: the Browser's tabs move as the sandbox and Notes tabs do, and keep their order.
const bpStrip = '[role="tablist"][aria-label="Browser tabs"]';

test('browser tabs move by drag, keyboard and menu and stay so', async ({ app, browser, screen }) => {
  // product: no tab asks for an action (its attention pulse is studio's).
  await app.open(mock('/s/product', '&ambient=0&browserask=0'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Browser/).click();
  const app_ = screen.getByRole('tab', /^Checkout/);
  const aws = screen.getByRole('tab', /^Amazon/);
  await expect(app_).toHaveAttribute('aria-selected', 'true');
  const names = async () => (await tabNames(browser, bpStrip)).map((name) => name.split(' ')[0]);
  expect(await names()).toEqual(['Checkout', 'Amazon']);
  const grips = await tabGrips(browser, bpStrip);
  expect(grips.map((g) => g.grip)).toEqual([DRAG_GRIP, DRAG_GRIP]);
  for (const g of grips) expect(g.title.endsWith(`\n${DRAG_HINT}`)).toBe(true);
  await expect(aws).toHaveAttribute('aria-keyshortcuts', 'Delete Backspace Alt+ArrowLeft Alt+ArrowRight');

  // A drag moves Amazon first and chooses nothing.
  await dragTab(browser, aws, app_);
  await expect.poll(names).toEqual(['Amazon', 'Checkout']);
  await expect(app_).toHaveAttribute('aria-selected', 'true');

  // Alt+←/→ moves the focused tab and keeps the keyboard on it.
  await aws.focus();
  await browser.keyboard.press('Alt+ArrowRight');
  expect(await names()).toEqual(['Checkout', 'Amazon']);
  await expect(aws).toBeFocused();
  await browser.keyboard.press('Alt+ArrowRight');
  expect(await names()).toEqual(['Checkout', 'Amazon']);

  // The menu moves it without dragging (WCAG 2.5.7), next to Close.
  await tabMenu(browser, `${bpStrip} [data-tab-id="tab-app"]`);
  await expect(screen.getByRole('menuitem', 'Move left')).toHaveAttribute('aria-disabled', 'true');
  await expect(screen.getByRole('menuitem', 'Close')).toBeVisible();
  await screen.getByRole('menuitem', 'Move right').click();
  expect(await names()).toEqual(['Amazon', 'Checkout']);
  expect(await axeViolations(browser)).toEqual([]);

  // A tab opened since comes last; a press on its × closes it and moves nothing.
  await screen.getByRole('button', 'New tab').click();
  await expect.poll(names).toEqual(['Amazon', 'Checkout', 'New']);
  expect(await pressAndMove(browser, `${bpStrip} .bp-tab:last-child .bp-tab-close`, -80)).toBe(false);
  expect(await names()).toEqual(['Amazon', 'Checkout', 'New']);
  await browser.locator(`${bpStrip} .bp-tab:last-child .bp-tab-close`).click();
  await expect.poll(names).toEqual(['Amazon', 'Checkout']);

  // A tab list arriving during a drag (a page's new title) is drawn once the tab is let go, in the
  // new order (review round 1).
  const chosen = await browser.evaluate((selector) => document.querySelector(`${selector} [aria-selected="true"]`)?.getAttribute('data-tab-id'), bpStrip);
  const from = await app_.boundingBox();
  const to = await aws.boundingBox();
  if (!from || !to) throw new Error('tabs not laid out');
  await browser.mouse.move(from.x + 12, from.y + from.height / 2);
  await browser.mouse.down();
  await browser.mouse.move(from.x - 20, from.y + from.height / 2);
  await browser.evaluate(() => {
    const session = (window as unknown as { __yuliusMock: { browsers: Map<string, { tabs: { id: string; title: string }[]; sendTabs: () => void }> } }).__yuliusMock.browsers.get('sb_product')!;
    session.tabs.find((tab) => tab.id === 'tab-aws')!.title = 'Console Home';
    session.sendTabs();
  });
  // The mock delivers a socket message a timer tick later: let it arrive while the tab is held.
  await browser.evaluate(() => new Promise((resolve) => { setTimeout(() => resolve(true), 0); }));
  // Still pressed: the strip is not drawn again under the pointer (#477), the old title stays.
  await expect.poll(() => browser.evaluate((selector) => {
    const held = document.querySelector<HTMLElement>(`${selector} .bp-tab.dragging`);
    return held ? { id: held.dataset.tabId, others: [...document.querySelectorAll(`${selector} .bp-tab-title`)].map((t) => t.textContent) } : null;
  }, bpStrip)).toEqual({ id: 'tab-app', others: [expect.stringMatching(/^Amazon/), expect.stringMatching(/^Checkout/)] });
  await browser.mouse.move(to.x + 4 - (from.width / 2 - 12), from.y + from.height / 2);
  await browser.mouse.up();
  await expect.poll(names).toEqual(['Checkout', 'Console']);
  await expect.poll(() => tabsInPlace(browser, bpStrip)).toBe(true);
  // The drag chose nothing.
  expect(await browser.evaluate((selector) => document.querySelector(`${selector} [aria-selected="true"]`)?.getAttribute('data-tab-id'), bpStrip)).toBe(chosen);
  await screen.getByRole('tab', /^Checkout/).focus();
  await browser.keyboard.press('Alt+ArrowRight');
  expect(await names()).toEqual(['Console', 'Checkout']);

  // Kept after a reload.
  await browser.reload();
  // The rail comes back after a reload, without its panel.
  await expect(screen.getByRole('button', 'Hide tools')).toBeVisible();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Browser/).click();
  await expect(screen.getByRole('tab', /^Checkout/)).toBeVisible();
  expect(await names()).toEqual(['Amazon', 'Checkout']);
});
