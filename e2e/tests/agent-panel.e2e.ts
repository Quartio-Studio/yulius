import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, openSuperagent } from './support.ts';

failOnPageErrors();

// The Superagent (SPEC-074 SUP-007; the Agent of SPEC-035, formerly the Cockpit of SPEC-021): every
// user's own plain Claude Code session per sandbox, in the right tool panel beside the page, for
// quick work that should not become a task.

// The shown Superagent: another sandbox's stays alive, hidden, after a switch (SPEC-113).
const agentTerminal = '.panel-view:not([hidden]) > .agent .xterm-rows';

// SPEC-132 NSB-001: the Superagent sits first in the tools rail, right under the tools toggle, so
// the top bar has no button of its own for it, on a desktop or a phone.
for (const viewport of [{ width: 1280, height: 800 }, { width: 390, height: 844 }]) {
  test(`the top bar has no Superagent button at ${viewport.width} px; the rail opens it`, async ({ app, browser, screen }) => {
    await browser.setViewport(viewport);
    await app.open(mock('/s/studio/t/12'));
    const topBar = browser.locator('header.tabrow');
    // The toggle above the rail names the Superagent among the tools it shows.
    await expect(topBar.getByRole('button', /^Show tools \(Superagent, /)).toBeVisible();
    expect(await browser.evaluate(() => [...document.querySelectorAll('header.tabrow :is(button, a, [role="button"]):not(.tools-toggle)')]
      .map((b) => `${b.textContent} ${b.getAttribute('aria-label') ?? ''} ${b.getAttribute('title') ?? ''}`)
      .filter((text) => /Superagent/i.test(text)))).toEqual([]);
    expect(await browser.locator('.agent-toggle, .btn-agent').count()).toBe(0);

    await openSuperagent(browser, screen);
    await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
    await expect(screen.getByRole('navigation', 'Tools').getByRole('button', 'Superagent')).toHaveAttribute('aria-pressed', 'true');
  });
}

test('the Superagent opens on the right beside the page, says what it is for and keeps its session', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  await openSuperagent(browser, screen);
  const panel = screen.getByRole('region', 'Tool panel');
  await expect(panel).toBeVisible();
  // The task stays in the centre: the Superagent is beside it, not instead of it.
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await expect(screen.getByRole('navigation', 'Tools').getByRole('button', 'Superagent')).toHaveAttribute('aria-pressed', 'true');

  // The notice: quick work here, anything bigger (writing code) in a task.
  const note = screen.getByRole('note', 'About the Superagent');
  await expect(note).toHaveText(/quick work.*automations.*quick question/i);
  await expect(note).toHaveText(/writing code.*task/i);

  const terminal = browser.locator(agentTerminal);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  // The keyboard is in the session: a quick question gets an answer.
  await expect(browser.locator('.agent-term .xterm-helper-textarea')).toBeFocused();
  await browser.keyboard.type('why?');
  await browser.keyboard.press('Enter');
  await expect(terminal).toHaveText(/Good question/);
  expect(await axeViolations(browser)).toEqual([]);

  // Nothing calls it the Cockpit or the Agent any more (SPEC-074 SUP-007).
  expect(await browser.evaluate(() => document.body.innerText)).not.toMatch(/Cockpit/);
  expect(await browser.evaluate(() => [...document.querySelectorAll('.agent, nav[aria-label="Tools"]')]
    .map((el) => `${el.textContent} ${el.getAttribute('aria-label') ?? ''}`).join(' '))).not.toMatch(/\bAgent\b/);

  // Hidden and shown again, and after another page, it is the same session.
  const railAgent = screen.getByRole('navigation', 'Tools').getByRole('button', 'Superagent');
  await railAgent.click();
  await expect(panel).toBeHidden();
  await screen.getByRole('navigation', 'Sandbox').getByRole('link', 'Tasks').click();
  await railAgent.click();
  await expect(terminal).toHaveText(/Good question/);
});

test('the Superagent notice opens a new task for bigger work', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  await screen.getByRole('note', 'About the Superagent').getByRole('link', 'New task').click();
  await expect(browser).toHaveURL(/#\/s\/studio$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
});

test('the Superagent is not in the left navigation', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('navigation', 'Sandbox').getByRole('link', 'Tasks')).toBeVisible();
  const items = await browser.evaluate(() => [...document.querySelectorAll('nav.nav a')].map((a) => a.textContent));
  expect(items.filter((text) => /Cockpit|Agent/.test(text ?? ''))).toEqual([]);
});

for (const path of ['superagent', 'agent', 'cockpit']) {
  test(`/s/<slug>/${path} opens the sandbox with the Superagent`, async ({ app, browser, screen }) => {
    await app.open(mock(`/s/studio/${path}`));
    await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
    await expect(browser).toHaveURL(/#\/s\/studio$/);
    await expect(screen.getByRole('navigation', 'Tools').getByRole('button', 'Superagent')).toHaveAttribute('aria-pressed', 'true');
  });
}

test('a stopped Superagent starts again with its conversation', async ({ app, browser, screen }) => {
  // Wide enough that the panel's terminal does not wrap the welcome line.
  await browser.setViewport({ width: 1920, height: 1000 });
  await app.open(mock('/s/studio/agent'));
  const terminal = browser.locator(agentTerminal);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  await browser.locator('.agent-term .xterm-helper-textarea').focus();
  await browser.keyboard.type('/exit');
  await browser.keyboard.press('Enter');
  await expect(screen.getByText('The Superagent stopped. Its conversation is kept.')).toBeVisible();

  await screen.getByRole('button', 'Start again').click();
  await expect(terminal).toHaveText(/resumed your last conversation/);
});

test('a Superagent whose process is gone is started again by itself', async ({ app, browser }) => {
  // The panel got a Superagent that no longer runs (the machine rebooted): it does not stay stuck on it.
  await app.open(mock('/s/studio/agent', '&cockpit=gone'));
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
});

test('a Superagent that cannot start says why and offers to try again', async ({ app, screen }) => {
  await app.open(mock('/s/studio/agent', '&cockpit=fail'));
  await expect(screen.getByRole('alert')).toHaveText(/Couldn’t start the Superagent: no subscription has capacity right now/);
  await expect(screen.getByRole('button', 'Try again')).toBeVisible();
});

test('read access explains why the Superagent is not available', async ({ app, browser, screen }) => {
  await app.open(mock('/s/sandbox-3/agent'));
  await expect(screen.getByText('The Superagent needs write access to this sandbox.')).toBeVisible();
  await expect(browser.locator(agentTerminal)).toBeHidden();
});

// Review round 1: New task from a task page hands the keyboard to the composer, not the Superagent.
test('New task from the Superagent beside a task focuses the composer', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openSuperagent(browser, screen);
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
  await screen.getByRole('note', 'About the Superagent').getByRole('link', 'New task').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
  // Typed text must become a task, never a message to the Superagent.
  await expect.poll(() => browser.evaluate(() => Boolean(document.activeElement?.closest('.agent-term')))).toBe(false);
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeFocused();
});

test('on a phone, New task closes the Superagent so the composer shows', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/agent'));
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
  await screen.getByRole('note', 'About the Superagent').getByRole('link', 'New task').click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
});

test('the Superagent opens from the rail and keeps its session across tools', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await screen.getByRole('button', /^Show tools/).click();
  const rail = screen.getByRole('navigation', 'Tools');
  await rail.getByRole('button', 'Superagent').click();
  await expect(rail.getByRole('button', 'Superagent')).toHaveAttribute('aria-pressed', 'true');
  const terminal = browser.locator(agentTerminal);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  await browser.keyboard.type('why?');
  await browser.keyboard.press('Enter');
  await expect(terminal).toHaveText(/Good question/);

  // Another tool in the panel: the button is released; back to the Superagent, the same session.
  await rail.getByRole('button', 'Shell').click();
  await expect(rail.getByRole('button', 'Superagent')).toHaveAttribute('aria-pressed', 'false');
  await rail.getByRole('button', 'Superagent').click();
  await expect(terminal).toHaveText(/Good question/);
});

test('a Superagent link does not leave the tools rail on for good', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/agent'));
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  await expect(screen.getByRole('navigation', 'Tools')).toBeVisible();
  await screen.getByRole('button', 'Close panel').click();
  await expect(screen.getByRole('navigation', 'Tools')).toBeHidden();
  await browser.reload();
  await expect(screen.getByRole('button', /^Show tools/)).toBeVisible();
  await expect(screen.getByRole('navigation', 'Tools')).toBeHidden();
});

test('switching sandbox with the Superagent open shows that sandbox’s Superagent', async ({ app, screen }) => {
  await app.open(mock('/s/studio/agent'));
  await expect(screen.getByRole('navigation', 'Tools').getByRole('button', 'Superagent')).toHaveAttribute('aria-pressed', 'true');
  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^sandbox-3\b/).click();
  await expect(screen.getByText('The Superagent needs write access to this sandbox.')).toBeVisible();
  // Read users get no New task link they cannot use.
  await expect(screen.getByRole('note', 'About the Superagent').getByRole('link', 'New task')).toBeHidden();
});

// Review round 2: the new sandbox's Superagent starts beside the page without taking the keyboard.
test('switching to a sandbox whose Superagent starts keeps the keyboard on the page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openSuperagent(browser, screen);
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^product\b/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
  await expect(browser.locator(agentTerminal)).toHaveText(/cwd: .*product/);
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeFocused();
});

// Review round 3: the rail shown for a Superagent link becomes the person's when they use it for
// another tool, and goes again when the Superagent is closed from the rail.
test('a tool picked from the rail after a Superagent link keeps the rail', async ({ app, screen }) => {
  await app.open(mock('/s/studio/agent'));
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  const rail = screen.getByRole('navigation', 'Tools');
  await rail.getByRole('button', 'Shell').click();
  await rail.getByRole('button', 'Shell').click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await expect(rail).toBeVisible();
});

test('closing a linked Superagent from the rail hides the rail again', async ({ app, screen }) => {
  await app.open(mock('/s/studio/agent'));
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Superagent').click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await expect(screen.getByRole('navigation', 'Tools')).toBeHidden();
});

test('a Superagent recovered right after opening it has the keyboard', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks', '&cockpit=gone'));
  await openSuperagent(browser, screen);
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
  await expect(browser.locator('.agent-term .xterm-helper-textarea')).toBeFocused();
});

// Review round 4: a slow start leaves the keyboard where the person put it in the meantime.
type Holding = Window & { __yuliusMock: { holdCockpit?: Promise<void>; rebootCockpits(): void; switchCockpits(failed?: boolean): void }; __release?: () => void };

test('a Superagent that starts slowly does not take the keyboard from the page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await browser.evaluate(() => {
    const w = window as unknown as Holding;
    w.__yuliusMock.holdCockpit = new Promise<void>((resolve) => { w.__release = resolve; });
  });
  await openSuperagent(browser, screen);
  // Shimmering fields while it starts (SPEC-092), what starts said to screen readers.
  await expect(browser.locator('.skeleton[role="status"]').filter({ hasText: 'Starting your Superagent…' })).toBeVisible();
  // While it starts, the person goes back to the page.
  await screen.getByRole('textbox', 'Task prompt').click();
  await browser.evaluate(() => (window as unknown as Holding).__release?.());
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
  // Shown (revealed) is when a terminal asked to focus would take the keyboard.
  await expect(browser.locator(agentTerminal)).toBeVisible();
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeFocused();
});

// Review round 4: the wish to focus on opening does not outlive the opening.
test('a Superagent recovered long after it was opened leaves the keyboard on the page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openSuperagent(browser, screen);
  const terminal = browser.locator(agentTerminal);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  await browser.keyboard.type('why?');
  await browser.keyboard.press('Enter');
  await expect(terminal).toHaveText(/Good question/);
  await browser.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());

  await browser.evaluate(() => (window as unknown as Holding).__yuliusMock.rebootCockpits());
  // A new session (the old conversation's output is gone from the screen), shown.
  await expect(terminal).not.toHaveText(/Good question/);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  await expect(terminal).toBeVisible();
  await expect.poll(() => browser.evaluate(() => Boolean(document.activeElement?.closest('.agent-term')))).toBe(false);
});

// SPEC-074 SUP-006: a usage limit moved the session to another subscription; the panel follows it,
// with no "stopped" banner or Start again, and the keyboard stays in it.
test('a Superagent moved to another subscription after a usage limit goes on in the panel', async ({ app, browser, screen }) => {
  // Wide enough that the panel's terminal does not wrap the welcome line.
  await browser.setViewport({ width: 1920, height: 1000 });
  await app.open(mock('/s/studio'));
  await openSuperagent(browser, screen);
  const terminal = browser.locator(agentTerminal);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  await expect(browser.locator('.agent-term .xterm-helper-textarea')).toBeFocused();

  await browser.evaluate(() => (window as unknown as Holding).__yuliusMock.switchCockpits());
  await expect(terminal).toHaveText(/resumed your last conversation/);
  await expect(screen.getByRole('button', 'Start again')).toBeHidden();
  expect(await browser.evaluate(() => document.querySelector('.agent')?.textContent ?? '')).not.toMatch(/stopped/);
  await expect(browser.locator('.agent-term .xterm-helper-textarea')).toBeFocused();
  await browser.keyboard.type('why?');
  await browser.keyboard.press('Enter');
  await expect(terminal).toHaveText(/Good question/);
});

// Review round 1 (m1): the move failed and the Superagent cannot start right now; the panel says
// why and offers to try again instead of staying on "Moving…".
test('a Superagent whose move to another subscription failed says why and starts again', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1920, height: 1000 });
  await app.open(mock('/s/studio'));
  await openSuperagent(browser, screen);
  const terminal = browser.locator(agentTerminal);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);

  await browser.evaluate(() => (window as unknown as Holding).__yuliusMock.switchCockpits(true));
  await expect(screen.getByRole('alert')).toHaveText(/Couldn’t start the Superagent: no subscription has capacity right now/);
  await screen.getByRole('button', 'Try again').click();
  await expect(terminal).toHaveText(/resumed your last conversation/);
});

// SPEC-331 (#918): a terminal that is not shown measured its box as 100 px (the hidden box's
// `100%` read as pixels) and squeezed its session to 10×5. The Superagent then drew in ten columns
// for a tab that still showed it, and everything it wrote while hidden stayed wrapped.
test('a hidden Superagent or Shell terminal keeps its session’s size', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1400, height: 900 });
  await app.open(mock('/s/studio/agent'));
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
  /** The mock session's size (what the page last sent), of the Superagent or the Shell's terminal;
   * null until the page has sent one (the session's starting size is the mock's, #924). */
  const size = (cockpit: boolean) => browser.evaluate((agent) => {
    type Shells = { __yuliusMock: { shells: Map<string, { cockpit?: boolean; session?: { cols: number; rows: number; sized?: boolean } }> } };
    const shell = [...(window as unknown as Shells).__yuliusMock.shells.values()].find((s) => Boolean(s.cockpit) === agent && s.session);
    return shell && shell.session && shell.session.sized ? `${shell.session.cols}x${shell.session.rows}` : null;
  }, cockpit);
  // The terminal reacts to its box in the frame after the change is observed: two frames and a spare.
  const settle = () => browser.evaluate(() => new Promise<boolean>((done) => { requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(() => done(true)))); }));
  // The page's size, not the mock session's 100×30 from before the terminal's hello reached it
  // (under load the hello lands after a first look; #923's CI run).
  const greeted = (cockpit: boolean) => browser.evaluate((agent) => {
    type Shells = { __yuliusMock: { shells: Map<string, { cockpit?: boolean; session?: { viewers: Set<unknown> } }> } };
    const shell = [...(window as unknown as Shells).__yuliusMock.shells.values()].find((s) => Boolean(s.cockpit) === agent && s.session);
    return Boolean(shell && shell.session && shell.session.viewers.size > 0);
  }, cockpit);
  const fitted = async (cockpit: boolean) => {
    await expect.poll(() => greeted(cockpit)).toBe(true);
    await expect.poll(async () => Number(((await size(cockpit)) || '0x0').split('x')[0])).toBeGreaterThan(20);
    // A fit after the hello sends a resize: the size counts once it holds across settled frames.
    await expect.poll(async () => {
      const before = await size(cockpit);
      await settle();
      return before === (await size(cockpit));
    }).toBe(true);
    return size(cockpit);
  };
  const shown = await fitted(true);

  await screen.getByRole('button', 'Close panel').click();
  await settle();
  expect(await size(true)).toBe(shown);

  // AC-2: shown again it still fits its box, larger and back.
  await openSuperagent(browser, screen);
  await screen.getByRole('button', 'Maximize panel').click();
  await expect.poll(() => size(true)).not.toBe(shown);
  await screen.getByRole('button', 'Restore panel size').click();
  await expect.poll(() => size(true)).toBe(shown);
  // Review round 1: another window of the same Superagent (a phone) set its size meanwhile; shown
  // again here, it takes this window's size back, as before.
  await browser.evaluate(() => {
    type Shells = { __yuliusMock: { shells: Map<string, { cockpit?: boolean; session?: { cols: number; rows: number } }> } };
    const shell = [...(window as unknown as Shells).__yuliusMock.shells.values()].find((s) => s.cockpit && s.session);
    if (shell && shell.session) { shell.session.cols = 30; shell.session.rows = 20; }
    return true;
  });
  await screen.getByRole('button', 'Close panel').click();
  await settle();
  await openSuperagent(browser, screen);
  await expect.poll(() => size(true)).toBe(shown);

  // Another tool in front hides the Superagent; the Superagent back in front hides the Shell's.
  // #924: the Shell's session exists at the mock's starting size before the page's hello says its
  // own; held there, the reference must still be the size the page gives.
  await browser.evaluate(() => {
    const w = window as unknown as { __yuliusHoldShell?: Promise<void>; __releaseShell?: () => void };
    w.__yuliusHoldShell = new Promise<void>((done) => { w.__releaseShell = done; });
    return true;
  });
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Shell').click();
  await expect.poll(() => browser.evaluate(() => {
    type Shells = { __yuliusMock: { shells: Map<string, { cockpit?: boolean; session?: object }> } };
    return [...(window as unknown as Shells).__yuliusMock.shells.values()].some((s) => !s.cockpit && s.session);
  })).toBe(true);
  const fitting = fitted(false);
  const early = await Promise.race([
    fitting.then(() => 'fitted'),
    (async () => { for (let i = 0; i < 5; i += 1) await settle(); return 'waiting'; })(),
  ]);
  expect(early).toBe('waiting');
  await browser.evaluate(() => {
    const w = window as unknown as { __yuliusHoldShell?: Promise<void>; __releaseShell?: () => void };
    delete w.__yuliusHoldShell;
    if (w.__releaseShell) w.__releaseShell();
    return true;
  });
  const shell = await fitting;
  await settle();
  expect(await size(true)).toBe(shown);
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Superagent').click();
  await settle();
  expect(await size(false)).toBe(shell);
  expect(await size(true)).toBe(shown);
});

// SPEC-337 (#933): a sign-in in another tab gives the browser a new session and CSRF token; a page
// open since then rereads its token on the first refusal instead of failing every write.
type CsrfMock = { __yuliusMock: { me: { csrf_token: string; user: { id: string } }; refuseCsrf?: boolean; csrfRefusals?: number } };

test('the Superagent starts after another tab signed in again (a new CSRF token)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  // Another tab's sign-in: the session, and its token, change under this page.
  await browser.evaluate(() => { (window as unknown as CsrfMock).__yuliusMock.me.csrf_token = 'csrf-of-the-new-session'; return null; });

  await openSuperagent(browser, screen);
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
  expect(await browser.locator('.agent .load-error').count()).toBe(0);
  // One refusal, then the retry with the new token went through.
  expect(await browser.evaluate(() => (window as unknown as CsrfMock).__yuliusMock.csrfRefusals ?? 0)).toBe(1);
});

test('a page reloads, retrying nothing, when another person signed in in another tab', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await browser.evaluate(() => {
    const db = (window as unknown as CsrfMock).__yuliusMock;
    db.me = { ...db.me, csrf_token: 'csrf-of-someone-else', user: { ...db.me.user, id: 'usr_2' } };
    (window as unknown as { __beforeReload?: boolean }).__beforeReload = true;
    return null;
  });

  await openSuperagent(browser, screen);
  // The page reloads (its marker is gone) and shows the person now signed in; the write was not
  // sent again as them.
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __beforeReload?: boolean }).__beforeReload ?? null), { timeout: 15_000 }).toBeNull();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
});

test('a write refused again after its retry says so, once per attempt', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await browser.evaluate(() => { (window as unknown as CsrfMock).__yuliusMock.refuseCsrf = true; return null; });

  await openSuperagent(browser, screen);
  const error = browser.locator('.agent .load-error');
  await expect(error).toHaveText(/Couldn’t start the Superagent: missing or invalid CSRF token/);
  // The first try and its one retry; no loop.
  expect(await browser.evaluate(() => (window as unknown as CsrfMock).__yuliusMock.csrfRefusals ?? 0)).toBe(2);

  // Fixed meanwhile: Try again starts it.
  await browser.evaluate(() => { (window as unknown as CsrfMock).__yuliusMock.refuseCsrf = false; return null; });
  await screen.getByRole('button', 'Try again').click();
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
});
