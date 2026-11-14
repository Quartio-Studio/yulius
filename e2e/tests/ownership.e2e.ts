import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock, terminalShown, touchTask } from './support.ts';

failOnPageErrors();

// A task is owned by one person, whose GitHub credentials it uses (SPEC-070). Someone else's task
// can be stopped, archived and deleted by anyone who may work in its project, and is otherwise
// read-only with a way to ask for control (SPEC-490); the owner hands it over or declines. The
// mock signs in as mkarp: product #3 is anna-q's, studio #12 mkarp's.

test("someone else's task is read-only, and its owner can hand it over", async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/3'));
  await firstShown(screen.getByText("You are watching @anna-q's task."));
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  // Steering is disabled, saying why and how to get it; putting the task away is not.
  await expect(screen.getByRole('menuitem', 'Continue @anna-q owns this task: request control to do this')).toBeDisabled();
  await expect(screen.getByRole('menuitem', /^Archive/)).toBeEnabled();
  await expect(screen.getByRole('menuitem', /^Delete permanently/)).toBeEnabled();
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

const taskMenu = (browser: Browser) => browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();

// SPEC-490 OWN-006, OWN-007 (AC-7): someone else's running task is stopped from its menu, after a
// confirmation that names its owner; the bar says what one may do and how to get the rest.
test("someone else's running task is stopped after a confirmation naming its owner", async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/4', '&ambient=0'));
  await touchTask(browser, 'sb_product', 4, { owner: { login: 'anna-q' } });
  await firstShown(screen.getByText("You are watching @anna-q's task."));
  await expect(browser.locator('.attention-bar .attention-text').first()).toHaveText("You are watching @anna-q's task. You can stop, archive or delete it; to type in it or change it, ask @anna-q for control.");
  await taskMenu(browser);
  await expect(screen.getByRole('menuitem', 'Pause @anna-q owns this task: request control to do this')).toBeDisabled();
  await screen.getByRole('menuitem', /^Stop/).click();
  const dialog = screen.getByRole('dialog', "Stop @anna-q's task #4?");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/@anna-q can continue it later\.$/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await dialog.getByRole('button', 'Stop task').click();
  // Stopped, and still anna-q's: mkarp cannot continue it.
  await expect(browser.locator('.taskview .stepstrip-main')).toHaveText(/Stopped/);
  await taskMenu(browser);
  await expect(screen.getByRole('menuitem', 'Continue @anna-q owns this task: request control to do this')).toBeDisabled();
});

// SPEC-490 OWN-006: the sidebar's quick archive and the menu's Delete work on someone else's
// task; the deletion names its owner. Studio #13 is Done and #4 stopped; both are made anna-q's.
test("someone else's task is archived from the sidebar and deleted from its menu", async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await touchTask(browser, 'sb_studio', 13, { owner: { login: 'anna-q' } });
  await touchTask(browser, 'sb_studio', 4, { owner: { login: 'anna-q' } });
  const done = browser.locator('.task-row:has-text("Improve the code")');
  await firstShown(done);
  await done.hover();
  await browser.locator('.task-row:has-text("Improve the code") [aria-label="Archive task #13"]').click();
  await expect(screen.getByText('Task #13 archived.')).toBeVisible();
  await expect(done).toBeHidden();
  const stopped = browser.locator('.task-row:has-text("Payment webhook")');
  await stopped.hover();
  await stopped.getByRole('button', 'Task actions').click();
  await expect(screen.getByRole('menuitem', 'Continue @anna-q owns this task: request control to do this')).toBeDisabled();
  await screen.getByRole('menuitem', /^Delete permanently/).click();
  const dialog = screen.getByRole('dialog', "Delete @anna-q's task #4 permanently?");
  await expect(dialog).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await dialog.getByRole('button', 'Delete permanently').click();
  await expect(screen.getByText('Task #4 deleted.')).toBeVisible();
  await expect(stopped).toBeHidden();
});

// SPEC-490 OWN-003: Done's "Archive all" takes the viewer's own tasks and says that other
// people's stay.
test("Archive all leaves other people's Done tasks", async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await touchTask(browser, 'sb_studio', 13, { owner: { login: 'anna-q' } });
  const theirs = browser.locator('.task-row:has-text("Improve the code")');
  await firstShown(theirs);
  const all = screen.getByRole('button', 'Archive all');
  await expect(all).toHaveAttribute('title', 'Archive your Done tasks');
  await all.click();
  await expect(screen.getByText('Archived 1 Done task.')).toBeVisible();
  await expect(theirs).toBeVisible();
});

// SPEC-490 OWN-007: control is asked for from the task's menu wherever it opens, here the sidebar.
test('control is asked for and the request withdrawn from the task menu in the sidebar', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'anna-q' } });
  const row = browser.locator('.task-row:has-text("Checkout API")');
  await firstShown(row);
  const menu = async () => { await row.hover(); await row.getByRole('button', 'Task actions').click(); };
  await menu();
  await screen.getByRole('menuitem', /^Request control/).click();
  await expect(screen.getByText('Asked for control of #12')).toBeVisible();
  await menu();
  await expect(screen.getByRole('menuitem', /^Request control/)).toHaveCount(0);
  await screen.getByRole('menuitem', /^Withdraw request/).click();
  await menu();
  await expect(screen.getByRole('menuitem', /^Request control/)).toBeEnabled();
  // One's own task has no such item.
  await browser.keyboard.press('Escape');
  const own = browser.locator('.task-row:has-text("Search indexing")');
  await own.hover();
  await own.getByRole('button', 'Task actions').click();
  await expect(screen.getByRole('menuitem', /^Stop/)).toBeVisible();
  await expect(screen.getByRole('menuitem', /control/)).toHaveCount(0);
});

test('the owner is asked for control and hands the task over', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
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
