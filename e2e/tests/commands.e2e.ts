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
