import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, DRAG_GRIP, DRAG_HINT, dragTab, failOnPageErrors, mock, pressAndMove, tabGrips, tabMenu, tabNames, tabsInPlace, terminalShown } from './support.ts';

failOnPageErrors();

type MockBrowser = { typed: Map<string, string>; acked: number };
type WithMock = Window & { __yuliusMock: { browsers: Map<string, MockBrowser> } };
type WithLock = Window & { __yuliusMock: { browserLockMessage?: string; browserRestartFails?: boolean } };

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

// Advance just a clipboard offer's lifetime, while keeping the browser's normal UI timers.
function controlCopyExpiry() {
  const schedule = window.setTimeout.bind(window);
  const cancel = window.clearTimeout.bind(window);
  const offers = new Map<number, () => void>();
  window.setTimeout = ((run: () => void, delay?: number, ...args: unknown[]) => {
    const id = schedule(run, delay, ...args);
    if (delay === 15000) offers.set(id, run);
    return id;
  }) as typeof window.setTimeout;
  window.clearTimeout = (id?: number) => {
    if (id !== undefined) offers.delete(id);
    cancel(id);
  };
  window.addEventListener('expire-clipboard', () => {
    for (const [id, run] of offers) { cancel(id); offers.delete(id); run(); }
  });
}

test('a blocked Browser clipboard write can be completed with an explicit click', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Browser/).click();
  await expect(screen.getByRole('tab', /^Checkout/)).toHaveAttribute('aria-selected', 'true');
  await browser.evaluate(controlCopyExpiry);
  await browser.evaluate(() => {
    type Socket = { push(message: string): void; send(message: string): void };
    type Mock = { browsers: Map<string, { viewers: Map<Socket, string> }> };
    const session = (window as unknown as { __yuliusMock: Mock }).__yuliusMock.browsers.get('sb_studio')!;
    navigator.clipboard.writeText = async () => { throw new DOMException('Blocked', 'NotAllowedError'); };
    document.addEventListener('copy', event => {
      document.body.dataset.copiedText = event.clipboardData?.getData('text/plain');
    });
    for (const socket of session.viewers.keys()) {
      const send = socket.send.bind(socket);
      socket.send = message => {
        const request = JSON.parse(message);
        if (request.type === 'accept_page_copy' && request.tab_id === 'tab-app' && request.offer === 7) {
          window.dispatchEvent(new Event('expire-clipboard'));
          socket.push(JSON.stringify({ type: 'page_clipboard', tab_id: 'tab-app', offer: 7, text: 'first line\nzażółć' }));
        } else { send(message); }
      };
      socket.push(JSON.stringify({ type: 'page_copy_available', tab_id: 'tab-app', offer: 7 }));
    }
  });
  await screen.getByRole('button', 'Copy to my clipboard').click();
  await screen.getByRole('button', 'Retry copy').click();
  expect(await browser.evaluate(() => document.body.dataset.copiedText)).toBe('first line\nzażółć');
});

/**
 * Holds the task terminal until the test releases it (SPEC-641): its first screen (`screen`, the
 * mock's hold) or its code (`code`, the lazily loaded terminal-core.js), as a slow host does.
 * Returns whether it is still held, and the release.
 */
async function holdTaskTerminal(browser: Browser, late: 'screen' | 'code'): Promise<{ held(): Promise<boolean>; release(): Promise<void> }> {
  if (late === 'screen') {
    await browser.addInitScript(() => {
      const w = window as unknown as { __yuliusHoldTerminal?: Promise<void>; __releaseTerminal?: () => void };
      w.__yuliusHoldTerminal = new Promise<void>((resolve) => { w.__releaseTerminal = resolve; });
    });
    return {
      // Not shown yet, whether or not its code has loaded (the inverse of `terminalShown`).
      held: () => browser.evaluate(() => document.querySelector('.taskview .term-pending, .taskview .term-placeholder') !== null),
      release: () => browser.evaluate(() => (window as unknown as { __releaseTerminal: () => void }).__releaseTerminal()),
    };
  }
  let release = () => {};
  const code = new Promise<void>((resolve) => { release = resolve; });
  await browser.route(/\/assets\/terminal-core\.js(\?|$)/, async (route) => { await code; await route.continue(); });
  return {
    held: () => browser.evaluate(() => document.querySelector('.taskview .term-lazy') !== null && document.querySelector('.taskview .term-host') === null),
    release: async () => release(),
  };
}

// SPEC-641: the task terminal shows only after the person focused the toast's button, as on a slow
// host (CI run be25f19b): the terminal shown late leaves the keyboard there.
for (const late of ['screen', 'code'] as const) {
  test(late === 'screen'
    ? 'expired clipboard actions preserve focus and explain how to copy again'
    : 'the clipboard action keeps the focus when the terminal code loads after it', async ({ app, browser, screen }) => {
    const terminal = await holdTaskTerminal(browser, late);
    await app.open(mock('/s/studio/t/12'));
    await screen.getByRole('button', /^Show tools/).click();
    await screen.getByRole('navigation', 'Tools').getByRole('button', /^Browser/).click();
    await expect(screen.getByRole('tab', /^Checkout/)).toHaveAttribute('aria-selected', 'true');
    await browser.evaluate(controlCopyExpiry);
    await browser.evaluate(() => {
      type Socket = { push(message: string): void; send(message: string): void };
      type Mock = { browsers: Map<string, { viewers: Map<Socket, string> }> };
      const session = (window as unknown as { __yuliusMock: Mock }).__yuliusMock.browsers.get('sb_studio')!;
      navigator.clipboard.writeText = async () => { throw new DOMException('Blocked', 'NotAllowedError'); };
      for (const socket of session.viewers.keys()) {
        const offer = (id: number) => socket.push(JSON.stringify({ type: 'page_copy_available', tab_id: 'tab-app', offer: id }));
        window.addEventListener('offer-copy-again', () => offer(9));
        const send = socket.send.bind(socket);
        socket.send = message => {
          const request = JSON.parse(message);
          if (request.type === 'accept_page_copy') {
            socket.push(JSON.stringify({ type: 'page_clipboard', tab_id: 'tab-app', offer: request.offer, text: 'temporary text' }));
          } else { send(message); }
        };
        offer(8);
      }
      document.addEventListener('copy', () => { document.body.dataset.unexpectedCopy = 'yes'; });
    });
    const copy = screen.getByRole('button', 'Copy to my clipboard');
    await copy.focus();
    expect(await terminal.held()).toBe(true);
    await terminal.release();
    await terminalShown(browser);
    await expect(copy).toBeFocused();
    await browser.evaluate(() => window.dispatchEvent(new Event('expire-clipboard')));
    await expect(copy).toBeFocused();
    await expect(screen.getByText('Copy expired. Click Copy on the webpage again.')).toBeVisible();
    await copy.click();
    await browser.evaluate(() => window.dispatchEvent(new Event('offer-copy-again')));
    await copy.click();
    const retry = screen.getByRole('button', 'Retry copy');
    await retry.focus();
    await browser.evaluate(() => window.dispatchEvent(new Event('expire-clipboard')));
    await expect(retry).toBeFocused();
    await retry.click();
    expect(await browser.evaluate(() => document.body.dataset.unexpectedCopy)).toBeUndefined();
  });
}

// SPEC-434 MTB-003/004: the Browser's tabs move as the sandbox and Notes tabs do, and keep their order.
const bpStrip = '[role="tablist"][aria-label="Browser tabs"]';

// SPEC-483 E1 (ADR-0318): on a host whose browser policy is not in force the sandbox browser
// shows, but refuses clicks and keys (they would reach Chrome's own file dialogs on the host).
// The viewer is told why and what an administrator runs, once, not at every key.
// SPEC-639: a paste in the panel goes to the sandbox page whole, as one `paste` message (the
// server makes it a real paste there); too long, not taken or not connected is said, never lost.
test('a paste in the browser panel is sent whole and a lost one is said', async ({ app, browser, screen }) => {
  type Pasting = Window & { __yuliusMock: { browserPasteUnread?: boolean; browsers: Map<string, { pastes?: string[] }> } };
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Browser/).click();
  await expect(screen.getByRole('tab', /^Checkout/)).toHaveAttribute('aria-selected', 'true');
  await expect
    .poll(() => browser.evaluate(() => ((document.querySelector('.bp-canvas') as HTMLCanvasElement | null)?.width ?? 0) > 10), { timeout: 15000 })
    .toBe(true);
  await browser.locator('.bp-surface').click({ position: { x: 120, y: 270 } });
  const paste = (text: string) => browser.evaluate((value: string) => {
    const data = new DataTransfer();
    data.setData('text/plain', value);
    const target = document.activeElement as HTMLElement;
    target.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
    return target.className;
  }, text);
  const pastes = () => browser.evaluate(() => (window as unknown as Pasting).__yuliusMock.browsers.get('sb_studio')?.pastes ?? []);
  const code = 'sk-ant-oauth-'.padEnd(100, 'x');
  const long = 'ab\n'.repeat(1667).slice(0, 5000);
  expect(await paste(code)).toContain('bp-keys');
  await paste(long);
  await expect.poll(pastes).toEqual([code, long]);

  // Over 32,000 characters or 32 KiB is not cut: nothing is sent, and the panel says why.
  await paste('x'.repeat(32001));
  await expect(browser.locator('.toast:has-text("Nothing was pasted: the text has 32,001 characters")')).toBeVisible();
  await paste('€'.repeat(12000));
  await expect(browser.locator('.toast:has-text("Nothing was pasted: the text is 36 KiB")')).toBeVisible();
  expect(await pastes()).toHaveLength(2);
  // A paste the page did not take comes back, and the panel says so.
  await browser.evaluate(() => { (window as unknown as Pasting).__yuliusMock.browserPasteUnread = true; });
  await paste('lost code');
  await expect(browser.locator('.toast:has-text("the page did not take it")')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  // With no tab to paste into the panel says what to do.
  type Tabbed = { browsers: Map<string, { tabs: unknown[]; saved?: unknown[]; sendTabs(): void }> };
  await browser.evaluate(() => {
    const session = (window as unknown as { __yuliusMock: Tabbed }).__yuliusMock.browsers.get('sb_studio')!;
    session.saved = session.tabs;
    session.tabs = [];
    session.sendTabs();
  });
  await expect(screen.getByRole('tab', /^Checkout/)).toBeHidden();
  await browser.evaluate(() => {
    const data = new DataTransfer();
    data.setData('text/plain', 'nowhere');
    document.querySelector('.bp-keys')!.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  });
  await expect(browser.locator('.toast:has-text("Open a tab in the browser first")')).toBeVisible();
  await browser.evaluate(() => {
    const session = (window as unknown as { __yuliusMock: Tabbed }).__yuliusMock.browsers.get('sb_studio')!;
    session.tabs = session.saved!;
    session.sendTabs();
  });
  await expect(screen.getByRole('tab', /^Checkout/)).toHaveAttribute('aria-selected', 'true');
  // The person's own Shift+Insert is their browser's paste (with their clipboard), never an
  // Insert key that would paste the sandbox's clipboard.
  await browser.evaluate(() => {
    type Sent = { browsers: Map<string, { viewers: Map<{ send(message: string): void }, string> }> };
    const session = (window as unknown as { __yuliusMock: Sent }).__yuliusMock.browsers.get('sb_studio')!;
    const keys: string[] = [];
    (window as unknown as { sentKeys: string[] }).sentKeys = keys;
    for (const socket of session.viewers.keys()) {
      const send = socket.send.bind(socket);
      socket.send = (message: string) => {
        const request = JSON.parse(message);
        if (request.type === 'key') keys.push(`${request.event} ${request.key}`);
        send(message);
      };
    }
  });
  await browser.keyboard.press('Shift+Insert');
  await browser.keyboard.press('q');
  await expect.poll(() => browser.evaluate(() => (window as unknown as { sentKeys: string[] }).sentKeys)).toContain('down q');
  const sentKeys = () => browser.evaluate(() => (window as unknown as { sentKeys: string[] }).sentKeys);
  expect((await sentKeys()).filter((k) => k.endsWith('Insert'))).toEqual([]);
  // An Insert pressed before Shift was sent, and so is its release: it never stays held there.
  await browser.evaluate(() => {
    const keys = document.querySelector('.bp-keys')!;
    const press = (type: string, key: string, code: string, shiftKey: boolean) =>
      keys.dispatchEvent(new KeyboardEvent(type, { key, code, shiftKey, bubbles: true, cancelable: true }));
    press('keydown', 'Insert', 'Insert', false);
    press('keydown', 'Shift', 'ShiftLeft', true);
    press('keyup', 'Insert', 'Insert', true);
    press('keyup', 'Shift', 'ShiftLeft', false);
  });
  await expect.poll(async () => (await sentKeys()).filter((k) => k.endsWith('Insert'))).toEqual(['down Insert', 'up Insert']);
  // A connection that died unnoticed sends nothing, and the panel says so (the mock got all
  // that was sent before: the keys typed above reached it).
  await expect.poll(() => browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.browsers.get('sb_studio')?.typed.get('tab-app') ?? '')).toContain('q');
  const before = (await pastes()).length;
  await browser.evaluate(() => {
    type Viewers = { browsers: Map<string, { viewers: Map<{ readyState: number }, string> }> };
    const session = (window as unknown as { __yuliusMock: Viewers }).__yuliusMock.browsers.get('sb_studio')!;
    for (const socket of session.viewers.keys()) socket.readyState = 3;
  });
  await paste('offline code');
  await expect(browser.locator('.toast:has-text("the browser is not connected")')).toBeVisible();
  expect(await pastes()).toHaveLength(before);
});

test('a sandbox browser without the host policy says once that clicks and keys are off', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&browser=locked'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Browser/).click();
  await expect(screen.getByRole('tab', /^Checkout/)).toHaveAttribute('aria-selected', 'true');
  await expect
    .poll(() => browser.evaluate(() => ((document.querySelector('.bp-canvas') as HTMLCanvasElement | null)?.width ?? 0) > 10), { timeout: 15000 })
    .toBe(true);
  await browser.locator('.bp-surface').click({ position: { x: 120, y: 270 } });
  const told = browser.locator('.toast:has-text("clicks and keys are off in this browser")');
  await expect(told).toBeVisible();
  await expect(told).toContainText('sudo yulius host setup');
  await expect(screen.getByRole('button', 'Restart browser')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);
  await browser.keyboard.type('mug');
  await browser.locator('.bp-surface').click({ position: { x: 200, y: 355 } });
  // Nothing was typed into the page, and the notice did not multiply.
  expect(await browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.browsers.get('sb_studio')?.typed.get('tab-app') ?? '')).toBe('');
  expect(await browser.evaluate(() => [...document.querySelectorAll('.toast')].filter((t) => (t.textContent || '').includes('clicks and keys are off')).length)).toBe(1);
});

// SPEC-493 fix 10b (review round 1): a browser that ran without the policy while an agent
// clicked keeps clicks and keys off after host setup, until it restarts. That new reason is
// shown at once, though the first notice was less than a minute ago; the same one is not.
test('a locked browser offers restart when policy arrives and accepts input after a retry', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&browser=locked'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Browser/).click();
  await expect
    .poll(() => browser.evaluate(() => ((document.querySelector('.bp-canvas') as HTMLCanvasElement | null)?.width ?? 0) > 10), { timeout: 15000 })
    .toBe(true);
  await browser.locator('.bp-surface').click({ position: { x: 120, y: 270 } });
  await expect(browser.locator('.toast:has-text("sudo yulius host setup")')).toBeVisible();
  await browser.evaluate(() => {
    (window as unknown as WithLock).__yuliusMock.browserLockMessage =
      'clicks and keys are still off in this browser: a file dialog may still be on its display. They come back once the browser restarts';
  });
  await browser.locator('.bp-surface').click({ position: { x: 200, y: 355 } });
  const restart = screen.getByRole('button', 'Restart browser');
  await expect(restart).toBeVisible();
  await expect(screen.getByText(/a file dialog may still be on its display/)).toBeVisible();
  await browser.evaluate(() => { (window as unknown as WithLock).__yuliusMock.browserRestartFails = true; });
  await restart.click();
  await expect(screen.getByText('Chrome could not restart.')).toBeVisible();
  await expect(restart).toBeEnabled();
  await browser.evaluate(() => { (window as unknown as WithLock).__yuliusMock.browserRestartFails = false; });
  await restart.click();
  await expect(restart).toBeHidden();
  await expect(browser.locator('.bp-overlay')).toBeHidden();
  await browser.locator('.bp-surface').click({ position: { x: 120, y: 270 } });
  await browser.keyboard.type('recovered');
  await expect.poll(() => browser.evaluate(() =>
    (window as unknown as WithMock).__yuliusMock.browsers.get('sb_studio')?.typed.get('tab-app') ?? ''
  )).toBe('recovered');
});

test('the panel reconnects when Chrome restarts but its HTTP reply times out', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&browser=locked'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Browser/).click();
  await expect(screen.getByRole('tab', /^Checkout/)).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator('.bp-overlay')).toBeHidden();
  await browser.evaluate(() => {
    (window as unknown as WithLock).__yuliusMock.browserLockMessage = 'A file dialog may still be open.';
  });
  await browser.locator('.bp-surface').click({ position: { x: 120, y: 270 } });
  const restart = screen.getByRole('button', 'Restart browser');
  await expect(restart).toBeVisible();
  // Capture the client's request deadline and fire it once the old socket has closed.
  type DelayedReply = Window & { __yuliusMock: {
    holdBrowserRestartReply: Promise<void>; finishRestartReply: () => void; expireRestart: () => void
  } };
  await browser.evaluate(() => {
    const db = (window as unknown as DelayedReply).__yuliusMock;
    db.holdBrowserRestartReply = new Promise<void>((resolve) => { db.finishRestartReply = resolve; });
  });
  await restart.click();
  await expect(screen.getByRole('button', 'Retry')).toBeVisible();
  await browser.evaluate(() => {
    const db = (window as unknown as DelayedReply).__yuliusMock;
    db.expireRestart();
    db.finishRestartReply();
  });
  await expect(screen.getByRole('button', 'Retry')).toBeHidden();
  // Retry disappears when reconnection starts. The overlay covers the page until its first frame.
  await expect(browser.locator('.bp-overlay')).toBeHidden();
  await browser.locator('.bp-surface').click({ position: { x: 120, y: 270 } });
  await browser.keyboard.type('resumed');
  await expect.poll(() => browser.evaluate(() =>
    (window as unknown as WithMock).__yuliusMock.browsers.get('sb_studio')?.typed.get('tab-app') ?? ''
  )).toBe('resumed');
});

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
