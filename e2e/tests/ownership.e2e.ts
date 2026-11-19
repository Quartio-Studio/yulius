import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, eventsConnected, failOnPageErrors, firstShown, mock, terminalShown, touchTask } from './support.ts';

failOnPageErrors();

// A task is owned by one person, whose GitHub credentials it uses (SPEC-070). Someone else's task
// can be stopped, archived and deleted by anyone who may work in its project, and is otherwise
// read-only with a way to take control (SPEC-510), or to ask an owner who chose to be asked
// (SPEC-490); they hand it over or decline. The mock signs in as mkarp: product #3 is anna-q's,
// studio #12 mkarp's.

// SPEC-510 TKC-001, TKC-005 (AC5): control of someone else's task is taken after a confirmation
// naming its owner; the task is the viewer's at once.
test("someone else's task is taken after a confirmation naming its owner", async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/3'));
  await firstShown(screen.getByText("You are watching @anna-q's task."));
  await expect(browser.locator('.attention-bar .attention-text').first()).toHaveText("You are watching @anna-q's task. You can stop, archive or delete it; to type in it or change it, take control.");
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await expect(screen.getByRole('menuitem', 'Continue @anna-q owns this task: take control to do this')).toBeDisabled();
  await expect(screen.getByRole('menuitem', /^Take control/)).toBeEnabled();
  await browser.keyboard.press('Escape');
  await screen.getByRole('button', 'Take control').click();
  const dialog = screen.getByRole('dialog', "Take control of @anna-q's task #3?");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/@anna-q is told and can take it back\.$/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await dialog.getByRole('button', 'Take control').click();
  await expect(screen.getByText('#3 is yours now')).toBeVisible();
  await expect(screen.getByText("You are watching @anna-q's task.")).toBeHidden();
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await expect(screen.getByRole('menuitem', /^Continue/)).toBeEnabled();
});

// SPEC-510 review round 1: the owner chose to be asked after the page read the task; the take is
// refused with why, and the page then offers Request control.
test('a take refused because the owner now asks first leaves Request control', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/3'));
  await firstShown(screen.getByRole('button', 'Take control'));
  // Changed on the server without an event, as for a page that read the task before.
  await browser.evaluate(() => { (window as unknown as { __yuliusMock: { tasks: { number: number; sandbox_id: string; ownerAsksFirst?: boolean }[] } }).__yuliusMock.tasks.find((t) => t.sandbox_id === 'sb_product' && t.number === 3)!.ownerAsksFirst = true; });
  await screen.getByRole('button', 'Take control').click();
  await screen.getByRole('dialog', "Take control of @anna-q's task #3?").getByRole('button', 'Take control').click();
  await expect(browser.locator('.toast').filter({ hasText: 'wants to be asked' })).toBeVisible();
  await expect(screen.getByRole('button', 'Request control')).toBeVisible();
  await expect(screen.getByRole('button', 'Take control')).toHaveCount(0);
});

test("an owner who asks first is asked, and can hand the task over", async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/3'));
  await touchTask(browser, 'sb_product', 3, { ownerAsksFirst: true });
  await firstShown(screen.getByText("You are watching @anna-q's task."));
  await expect(browser.locator('.attention-bar .attention-text').first()).toHaveText(/ask @anna-q for control\.$/);
  await expect(screen.getByRole('button', 'Take control')).toHaveCount(0);
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  // Steering is disabled, saying why and how to get it; putting the task away is not.
  await expect(screen.getByRole('menuitem', 'Continue @anna-q owns this task: request control to do this')).toBeDisabled();
  await expect(screen.getByRole('menuitem', /^Take control/)).toHaveCount(0);
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
  await expect(browser.locator('.attention-bar .attention-text').first()).toHaveText("You are watching @anna-q's task. You can stop, archive or delete it; to type in it or change it, take control.");
  await taskMenu(browser);
  await expect(screen.getByRole('menuitem', 'Pause @anna-q owns this task: take control to do this')).toBeDisabled();
  await screen.getByRole('menuitem', /^Stop/).click();
  const dialog = screen.getByRole('dialog', "Stop @anna-q's task #4?");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/@anna-q can continue it later\.$/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await dialog.getByRole('button', 'Stop task').click();
  // Stopped, and still anna-q's: mkarp cannot continue it.
  await expect(browser.locator('.taskview .stepstrip-main')).toHaveText(/Stopped/);
  await taskMenu(browser);
  await expect(screen.getByRole('menuitem', 'Continue @anna-q owns this task: take control to do this')).toBeDisabled();
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
  // Its menu is built from the task the page holds: wait until it shows this one as @anna-q's.
  await expect(browser.locator('.task-row:has-text("Payment webhook") .task-owner')).toBeVisible();
  await stopped.hover();
  await stopped.getByRole('button', 'Task actions').click();
  await expect(screen.getByRole('menuitem', 'Continue @anna-q owns this task: take control to do this')).toBeDisabled();
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
  // The page takes the Done tasks it shows as someone else's when the button is pressed: wait until
  // it shows this one as @anna-q's (it was archived when the change came after the press).
  await expect(browser.locator('.task-row:has-text("Improve the code") .task-owner')).toBeVisible();
  const all = screen.getByRole('button', 'Archive all');
  await expect(all).toHaveAttribute('title', 'Archive your Done tasks');
  await all.click();
  // mkarp's own Done tasks go (how many depends on the mock's clock); anna-q's stays.
  await expect(screen.getByText(/^Archived \d+ Done tasks?\.$/)).toBeVisible();
  await expect(theirs).toBeVisible();
});

// SPEC-490 OWN-007, SPEC-510 TKC-005: control is taken, or asked for from an owner who asks first,
// from the task's menu wherever it opens, here the sidebar.
test('control is asked for and the request withdrawn from the task menu in the sidebar', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'anna-q' } });
  const row = browser.locator('.task-row:has-text("Checkout API")');
  await firstShown(row);
  // The menu is built from the task the page holds: wait until it shows this one as @anna-q's.
  await expect(browser.locator('.task-row:has-text("Checkout API") .task-owner')).toBeVisible();
  const menu = async () => { await row.hover(); await row.getByRole('button', 'Task actions').click(); };
  await menu();
  await expect(screen.getByRole('menuitem', /^Take control/)).toBeEnabled();
  await expect(screen.getByRole('menuitem', /^Request control/)).toHaveCount(0);
  await browser.keyboard.press('Escape');
  await touchTask(browser, 'sb_studio', 12, { ownerAsksFirst: true });
  await expect.poll(async () => { await menu(); const n = await screen.getByRole('menuitem', /^Request control/).count(); await browser.keyboard.press('Escape'); return n; }, { message: 'the menu offers Request control' }).toBe(1);
  await menu();
  await screen.getByRole('menuitem', /^Request control/).click();
  await expect(screen.getByText('Asked for control of #12')).toBeVisible();
  await menu();
  await expect(screen.getByRole('menuitem', /^Request control/)).toHaveCount(0);
  await screen.getByRole('menuitem', /^Withdraw request/).click();
  // The toast follows the answer, so the menu opened after it is built from the task without a request.
  await expect(screen.getByText('Withdrew your request for control of #12')).toBeVisible();
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
  // The giver may take it back without asking (SPEC-490 OWN-009) instead of requesting it.
  await expect(screen.getByText('@anna-q has control of this task now.')).toBeVisible();
  await expect(screen.getByRole('button', 'Take back')).toBeVisible();
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
  await expect(screen.getByRole('button', 'Take control')).toBeVisible();
});

// SPEC-490 OWN-008 (AC-8): a request for control reaches the owner wherever they are: a toast
// that opens the task, an entry under the bell, and a mark on the task's row in the sidebar until
// it is answered.
test('the owner hears of a request for control on another page', async ({ app, browser, screen }) => {
  // A page without a terminal, which would take the keyboard and close the bell's panel.
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.task-row:has-text("Checkout API")'));
  await touchTask(browser, 'sb_studio', 12, { controlRequest: { by: { login: 'anna-q' }, at: new Date().toISOString() } });
  const asked = '@anna-q asks for control of #12 Checkout API';
  await expect(browser.locator('.toast').filter({ hasText: asked })).toBeVisible();
  const mark = browser.locator('.task-row:has-text("Checkout API") [title="@anna-q asks for control"]');
  await expect(mark).toBeVisible();
  await screen.getByRole('button', /^Notifications \(1 unread\)/).click();
  await expect(screen.getByRole('dialog', 'Notifications').getByRole('button', new RegExp(`^Unread: ${asked}`))).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await browser.keyboard.press('Escape');
  await browser.locator('.toast').filter({ hasText: asked }).getByRole('button', 'Open').click();
  await expect(screen.getByRole('button', 'Hand over')).toBeVisible();
  // Answered elsewhere: the mark goes.
  await touchTask(browser, 'sb_studio', 12, { controlRequest: null });
  await expect(mark).toBeHidden();
});

// SPEC-490 OWN-009, OWN-010 (AC-11): the owner hands the task over and can take it back without
// asking: the bar says so, Take back returns the terminal; whoever it is taken from is told.
test('the giver takes the task back, and whoever had it hears so', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_studio', 12, { controlRequest: { by: { login: 'anna-q' }, at: new Date().toISOString() } });
  await screen.getByRole('button', 'Hand over').click();
  const bar = browser.locator('.attention-bar .attention-text').filter({ hasText: '@anna-q has control of this task now.' });
  await expect(bar).toHaveText('@anna-q has control of this task now. It acts on GitHub as them; you can take it back at any time.');
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Take back').click();
  await expect(screen.getByText('#12 is yours again')).toBeVisible();
  await expect(bar).toBeHidden();
  await expect(browser.locator('.taskview .watch')).toBeHidden();
  await terminalShown(browser);

  // mkarp has had #11 since before the page loaded (anna-q handed it to him); she takes it back:
  // the server tells him, wherever he is (review round 2: no earlier event to compare with).
  await app.open(mock('/s/studio', '&ambient=0'));
  await eventsConnected(browser);
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { takeBack: (s: string, n: number, by: string) => void } }).__yuliusMock.takeBack('sb_studio', 11, 'anna-q'));
  const taken = '@anna-q took back control of #11 3D model viewer';
  await expect(browser.locator('.toast').filter({ hasText: taken })).toBeVisible();
  await screen.getByRole('button', /^Notifications \(\d+ unread\)/).click();
  await expect(screen.getByRole('dialog', 'Notifications').getByRole('button', new RegExp(`^Unread: ${taken}`))).toBeVisible();
});

// SPEC-490 OWN-008 (review round 2): a request that waited before the owner opened Yulius is
// under the bell too, once.
test('a request that waited before the page loaded is heard', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0&controlask=1'));
  await firstShown(browser.locator('.task-row:has-text("Checkout API")'));
  const asked = '@anna-q asks for control of #12 Checkout API';
  await expect(screen.getByRole('button', /^Notifications \(1 unread\)/)).toBeVisible();
  await screen.getByRole('button', /^Notifications/).click();
  await expect(screen.getByRole('dialog', 'Notifications').getByRole('button', new RegExp(`^Unread: ${asked}`))).toHaveCount(1);
});

// Review round 3: a sandbox whose first task list fails is scanned once its list loads.
test('a waiting request is heard once a failed task list loads', async ({ app, browser, screen }) => {
  type Failing = { __yuliusMock: { failPath: string | null } };
  await app.open(mock('/s/studio/t/13', '&ambient=0&controlask=1&fail=/api/sandboxes/sb_studio/tasks'));
  await firstShown(browser.locator('.load-error:has-text("Couldn’t load tasks")'));
  // The list loads again (its Try again); the page may redraw meanwhile, so the button is pressed
  // where it is at that moment.
  await browser.evaluate(() => {
    (window as unknown as Failing).__yuliusMock.failPath = null;
    (document.querySelector('.load-error button') as HTMLButtonElement | null)?.click();
  });
  await firstShown(browser.locator('.task-row:has-text("Checkout API")'));
  await expect(screen.getByRole('button', /^Notifications \(1 unread\)/)).toBeVisible();
});

// SPEC-510 TKC-002 (AC5): a teammate takes the viewer's task without asking: they hear of it
// wherever they are, and the task's bar offers Take back, which returns it.
test('whoever a task is taken from hears so and takes it back', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await eventsConnected(browser);
  await firstShown(browser.locator('.task-row:has-text("Checkout API")'));
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { takeControl: (s: string, n: number, by: string) => void } }).__yuliusMock.takeControl('sb_studio', 12, 'anna-q'));
  const taken = '@anna-q took control of #12 Checkout API';
  await expect(browser.locator('.toast').filter({ hasText: taken })).toBeVisible();
  await screen.getByRole('button', /^Notifications \(\d+ unread\)/).click();
  await expect(screen.getByRole('dialog', 'Notifications').getByRole('button', new RegExp(`^Unread: ${taken}`))).toBeVisible();
  await browser.keyboard.press('Escape');
  await browser.locator('.toast').filter({ hasText: taken }).getByRole('button', 'Open').click();
  await expect(screen.getByText('@anna-q has control of this task now.')).toBeVisible();
  await screen.getByRole('button', 'Take back').click();
  await expect(screen.getByText('#12 is yours again')).toBeVisible();
});

// SPEC-510 TKC-003 (AC5): the person chooses in Settings whether teammates ask first.
test('Settings saves whether teammates ask before taking control', async ({ app, browser, screen }) => {
  await app.open(mock('/settings'));
  const ask = screen.getByRole('switch', 'Ask me before taking control of my tasks');
  await firstShown(ask);
  await expect(ask).not.toBeChecked();
  expect(await axeViolations(browser)).toEqual([]);
  await ask.click();
  await expect(screen.getByText('Teammates now ask you for control of your tasks.')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __yuliusMock: { me: { user: { control_ask_first: boolean } } } }).__yuliusMock.me.user.control_ask_first)).toBe(true);
  await ask.click();
  await expect(screen.getByText('Teammates now take control of your tasks at once.')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __yuliusMock: { me: { user: { control_ask_first: boolean } } } }).__yuliusMock.me.user.control_ask_first)).toBe(false);
});
