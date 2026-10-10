import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

type MockBrowser = { typed: Map<string, string> };
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
