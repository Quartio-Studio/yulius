import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock } from './support.ts';

failOnPageErrors();

type Mocked = { __yuliusMock: { commands: Array<{ name: string; approve: string; refuse: string[] }>; commandRuns: Array<{ id: string; state: string }>; askRun(line: string, task: number): void } };

const runsOf = (browser: any) => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.commandRuns.map((r) => `${r.id} ${r.state}`));
const commandsOf = (browser: any) => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.commands.map((c) => `${c.name} ${c.approve} ${c.refuse.join(',')}`));

// SPEC-626 AC-8: administrators list, add, edit and remove a project's brokered commands next to
// its secrets.
test('administrators add, edit and remove brokered commands on the Secrets page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=commands'));
  const card = browser.locator('.item-card:has-text("Deploys the studio")');
  await firstShown(card.getByText('/usr/local/bin/deploy --key-file {keyfile}'));
  await expect(card.getByText('Asks when a run changes something')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Add command').first().click();
  const dialog = screen.getByRole('dialog', 'Add command');
  await screen.getByLabel('Name').fill('sign');
  await screen.getByLabel('Binary').fill('/usr/bin/cosign');
  await screen.getByLabel('Secret').fill('STRIPE_KEY');
  await screen.getByLabel('Fixed arguments').fill('sign\n--key\n{keyfile}');
  await screen.getByLabel('Flags Yulius sets').fill('--key');
  await dialog.getByRole('button', 'Add').click();
  await expect(dialog).toBeHidden();
  await expect(browser.locator('.item-card:has-text("/usr/bin/cosign sign --key {keyfile}")')).toBeVisible();
  expect(await commandsOf(browser)).toContain('sign always --key');

  // A binary in /tmp is refused under its field, and nothing is saved.
  await screen.getByRole('button', 'Edit sign').click();
  const editing = screen.getByRole('dialog', 'Edit sign');
  await screen.getByLabel('Binary').fill('/tmp/cosign');
  await editing.getByRole('button', 'Save').click();
  await expect(editing.getByText(/host's own program/)).toBeVisible();
  await screen.getByLabel('Binary').fill('/usr/local/bin/cosign');
  await editing.getByRole('button', 'Save').click();
  await expect(editing).toBeHidden();
  await expect(browser.locator('.item-card:has-text("/usr/local/bin/cosign sign")')).toBeVisible();

  await screen.getByRole('button', 'Delete sign').click();
  await screen.getByRole('dialog', 'Delete sign?').getByRole('button', 'Delete').click();
  await expect(browser.locator('.item-card:has-text("cosign")')).toBeHidden();
  expect((await commandsOf(browser)).some((c: string) => c.startsWith('sign '))).toBe(false);
});

// SPEC-626 AC-8, BRK-015: a run that waits shows its full command line with Approve and Reject; a
// run an agent asks while the page is open shows at once (`commands.updated`).
test('a waiting run is approved or rejected on the Secrets page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=commands'));
  await firstShown(screen.getByText('Waiting for approval (1)'));
  await expect(browser.locator('[data-run-line="run_waiting"]')).toHaveText('/usr/local/bin/deploy --key-file {keyfile} push --send');
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', "Approve task #111's run of deploy").click();
  await expect(screen.getByText('Waiting for approval (1)')).toBeHidden();
  expect(await runsOf(browser)).toContain('run_waiting running');

  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.askRun('/usr/local/bin/deploy --key-file {keyfile} push', 120));
  await expect(screen.getByText('Waiting for approval (1)')).toBeVisible();
  await screen.getByRole('button', "Reject task #120's run of deploy").click();
  await expect(screen.getByText('Waiting for approval (1)')).toBeHidden();
  expect((await runsOf(browser)).filter((r: string) => r.endsWith('rejected')).length).toBe(1);
});

// A member who cannot administer the project reads the commands but cannot add or change one.
test('a member reads the commands but changes none', async ({ app, screen }) => {
  await app.open(mock('/s/studio/secrets?section=commands', '&role=member'));
  await firstShown(screen.getByText('Deploys the studio with its service account'));
  await expect(screen.getByRole('button', 'Add command').first()).toBeDisabled();
  await expect(screen.getByRole('button', 'Edit deploy')).toBeDisabled();
});

type Asking = { __yuliusMock: { askRun(line: string, task: number): void; expireRun(id: string): void; commandRuns: Array<{ id: string; state: string; task_number: number }>; sandboxes: Array<{ id: string; access: string }>; commands: Array<{ name: string; version: number }> } };
const ask = (browser: any, line: string, task: number) => browser.evaluate(({ l, n }: { l: string; n: number }) => (window as unknown as Asking).__yuliusMock.askRun(l, n), { l: line, n: task });
const approved = (browser: any) => browser.evaluate(() => (window as unknown as Asking).__yuliusMock.commandRuns.filter((r) => r.state === 'running').map((r) => r.task_number));

// Review of slice 4: a run that arrives under a resting pointer takes the place of the one the
// person read; a click meant for that one approves nothing.
test('a click on Approve meant for another run approves nothing', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=commands'));
  const first = screen.getByRole('button', "Approve task #111's run of deploy");
  await firstShown(first);
  await first.hover();
  await ask(browser, '/usr/local/bin/deploy --key-file {keyfile} push --send --target=prod', 120);
  await expect(screen.getByText('Waiting for approval (2)')).toBeVisible();
  // The element under the unmoved pointer is #120's Approve now: it is off for a moment.
  await expect(browser.locator('[data-focus-key="run:run_2:approve"]')).toHaveAttribute('aria-disabled', 'true');
  // The click lands at once, as a press under a resting pointer does (a locator's click would wait).
  await browser.evaluate(() => (document.querySelector('[data-focus-key="run:run_2:approve"]') as HTMLElement).click());
  await expect(screen.getByText('Waiting for approval (2)')).toBeVisible();
  expect(await approved(browser)).toEqual([]);
  // Once settled, it answers its own run.
  await expect(browser.locator('[data-focus-key="run:run_2:approve"]')).not.toHaveAttribute('aria-disabled', 'true');
  await browser.locator('[data-focus-key="run:run_2:approve"]').click();
  await expect(screen.getByText('Waiting for approval (1)')).toBeVisible();
  expect(await approved(browser)).toEqual([120]);
});

// Review of slice 4: the keyboard on one run's Approve never moves to another run's.
test('Enter on Approve never answers a run the person did not focus', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=commands'));
  const first = screen.getByRole('button', "Approve task #111's run of deploy");
  await firstShown(first);
  await first.focus();
  // #111's run expires while #140 asks: focus does not land on #140's Approve.
  await browser.evaluate(() => { const m = (window as unknown as Asking).__yuliusMock; m.expireRun('run_waiting'); m.askRun('/usr/local/bin/deploy --key-file {keyfile} push --send --wipe', 140); });
  await expect(browser.locator('[data-focus-key="run:run_2:approve"]')).toBeVisible();
  expect(await browser.evaluate(() => document.activeElement?.getAttribute('data-focus-key') || document.activeElement?.tagName)).not.toBe('run:run_2:approve');
  await browser.keyboard.press('Enter');
  expect(await approved(browser)).toEqual([]);
  // The same task asks again with another line: the focus key names the run, not the task.
  await browser.locator('[data-focus-key="run:run_2:approve"]').focus();
  await browser.evaluate(() => { const m = (window as unknown as Asking).__yuliusMock; m.expireRun('run_2'); m.askRun('/usr/local/bin/deploy --key-file {keyfile} push --send --other', 140); });
  await expect(browser.locator('[data-focus-key="run:run_3:approve"]')).toBeVisible();
  expect(await browser.evaluate(() => document.activeElement?.getAttribute('data-focus-key'))).not.toBe('run:run_3:approve');
  await browser.keyboard.press('Enter');
  expect(await approved(browser)).toEqual([]);
});

// Review of slice 4 (AC-8): a person who only reads the project sees a waiting run's line but no
// Approve or Reject; an edit made from a stale read is refused in the dialog.
test('a reader sees no answer, and a stale edit is refused', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=commands'));
  await firstShown(screen.getByRole('button', 'Edit deploy'));
  await screen.getByRole('button', 'Edit deploy').click();
  const dialog = screen.getByRole('dialog', 'Edit deploy');
  await browser.evaluate(() => { (window as unknown as Asking).__yuliusMock.commands.find((c) => c.name === 'deploy')!.version += 1; });
  await dialog.getByRole('button', 'Save').click();
  await expect(dialog.getByText(/changed since you opened it/)).toBeVisible();

  await app.open(mock('/s/studio/secrets?section=commands'));
  await browser.evaluate(() => { (window as unknown as Asking).__yuliusMock.sandboxes.find((s) => s.id === 'sb_studio')!.access = 'read'; });
  await app.open(mock('/s/studio/secrets?section=commands', '&role=member'));
  await browser.evaluate(() => { (window as unknown as Asking).__yuliusMock.sandboxes.find((s) => s.id === 'sb_studio')!.access = 'read'; });
  await browser.evaluate(() => (window as unknown as Asking).__yuliusMock.askRun('/usr/local/bin/deploy --key-file {keyfile} push', 150));
  await expect(browser.locator('[data-run-line="run_2"]')).toBeVisible();
  await expect(screen.getByRole('button', "Approve task #150's run of deploy")).toBeHidden();
  await expect(screen.getByRole('button', "Reject task #150's run of deploy")).toBeHidden();
});
