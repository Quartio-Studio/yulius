import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, touchTask } from './support.ts';

failOnPageErrors();

// A task is owned by one person, whose GitHub credentials it uses (SPEC-070). Someone else's task
// is read-only with a way to ask for control; the owner hands it over or declines. The mock signs
// in as mkarp: product #3 is anna-q's, studio #12 mkarp's.

test("someone else's task is read-only, and its owner can hand it over", async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/3'));
  await expect(screen.getByText("You are watching @anna-q's task.")).toBeVisible();
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  // Disabled, saying why.
  await expect(screen.getByRole('menuitem', 'Continue @anna-q owns this task')).toBeDisabled();
  await browser.keyboard.press('Escape');
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Request control').click();
  await expect(screen.getByText('You asked @anna-q for control.')).toBeVisible();
  await expect(screen.getByRole('button', 'Withdraw')).toBeVisible();
  // Asking again takes control once the owner can no longer act.
  await expect(screen.getByRole('button', 'Ask again')).toBeVisible();

  // anna-q hands it over: the view follows the event, without a reload.
  await touchTask(browser, 'sb_product', 3, { owner: { login: 'mkarp' }, controlRequest: null });
  await expect(screen.getByText('You asked @anna-q for control.')).toBeHidden();
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await expect(screen.getByRole('menuitem', /^Continue/)).toBeEnabled();
});

test('the owner is asked for control and hands the task over', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  // xterm marks a terminal whose mouse it reports to the program (SPEC-098): the owner's is.
  const programMouse = () => browser.evaluate(() => Boolean(document.querySelector('.taskview .xterm.enable-mouse-events')));
  await expect.poll(programMouse, { message: 'the writer hands the program its mouse' }).toBe(true);
  await touchTask(browser, 'sb_studio', 12, { controlRequest: { by: { login: 'anna-q' }, at: new Date().toISOString() } });
  await expect(screen.getByText('@anna-q asks to take control of this task:')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Hand over').click();
  await expect(screen.getByText("You are watching @anna-q's task.")).toBeVisible();
  await expect(screen.getByRole('button', 'Request control')).toBeVisible();
  // mkarp now watches it: the session as its transcript, no terminal to send anything to (SPEC-179).
  await expect(browser.locator('.taskview .watch .ss-tui')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => document.querySelectorAll('.taskview .xterm').length), { message: 'a watcher has no terminal' }).toBe(0);
  // anna-q hands it back: mkarp writes again and the program gets the mouse again.
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'mkarp' }, controlRequest: null });
  await expect(browser.locator('.taskview .watch')).toBeHidden();
  await expect.poll(programMouse, { message: 'the writer hands the program its mouse again' }).toBe(true);
});

test('the owner declines a request for control', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await touchTask(browser, 'sb_studio', 12, { controlRequest: { by: { login: 'anna-q' }, at: new Date().toISOString() } });
  await screen.getByRole('button', 'Decline').click();
  await expect(screen.getByText('@anna-q asks to take control of this task:')).toBeHidden();
  await expect(screen.getByText("You are watching @anna-q's task.")).toBeHidden();
});

test('a request for control leaves the way out of a chain', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  await touchTask(browser, 'sb_product', 7, { controlRequest: { by: { login: 'anna-q' }, at: new Date().toISOString() } });
  await expect(screen.getByText('@anna-q asks to take control of this task:')).toBeVisible();
  await expect(screen.getByRole('button', 'Hand over')).toBeVisible();
  await expect(screen.getByRole('button', 'Start now')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
});

// #506: the view's read of the task is answered before a hand-over and arrives after the hand-over
// was announced (the server answered, then the change came first). The server dates to the second,
// so both may carry the same time. The older answer does not take the view back to its older owner.
test('a read answered before a hand-over and arriving after it changes nothing back', async ({ app, browser, screen }) => {
  type Holding = { __yuliusMock: { holdDetail?: Promise<void> | null; detailReads?: Record<string, number>; tasks: { id: string; updated_at: string }[] }; __releaseDetail?: () => void };
  await app.open(mock('/s/studio', '&ambient=0'));
  await browser.evaluate(() => {
    const w = window as unknown as Holding;
    w.__yuliusMock.holdDetail = new Promise<void>((resolve) => { w.__releaseDetail = resolve; });
    location.hash = '#/s/studio/t/12';
  });
  // The view's read is answered (mkarp owns it) and the answer waits on its way.
  await expect.poll(() => browser.evaluate(() => {
    const db = (window as unknown as Holding).__yuliusMock;
    return Boolean(db.detailReads?.tsk_studio_12) && !db.holdDetail;
  }), { message: 'the view never read the task' }).toBe(true);
  await browser.locator('.taskview .stepstrip-toggle').click();
  await expect(browser.locator('.taskview .steplist')).toHaveText(/Loading steps/);

  const answered = await browser.evaluate(() => (window as unknown as Holding).__yuliusMock.tasks.find((t) => t.id === 'tsk_studio_12')!.updated_at);
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'anna-q' }, updated_at: answered });
  await expect(screen.getByText("You are watching @anna-q's task.")).toBeVisible();
  // The answer arrives: its steps show and the owner stays anna-q.
  await browser.evaluate(() => (window as unknown as Holding).__releaseDetail?.());
  await expect(browser.locator('.taskview .step-item').first()).toBeVisible();
  await expect(screen.getByText("You are watching @anna-q's task.")).toBeVisible();
  await expect(screen.getByRole('button', 'Request control')).toBeVisible();
});
