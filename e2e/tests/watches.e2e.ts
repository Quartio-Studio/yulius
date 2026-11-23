import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock } from './support.ts';

failOnPageErrors();

// Watches page (SPEC-630 WCH-013): a project's scripts on a schedule, with their state, last run
// and output, and Run now, Pause/Resume, Edit and Remove for those who may.

/** The badges, schedule and button labels of one watch card. */
const cardOf = (browser, name: string) => browser.evaluate((title) => {
  const card = [...document.querySelectorAll('.item-card')].find((c) => c.querySelector('.item-name')?.textContent === title);
  return card ? {
    badges: [...card.querySelectorAll('.item-title .badge')].map((b) => b.textContent.trim()),
    text: card.textContent,
    buttons: [...card.querySelectorAll('.item-controls button')].map((b) => ({ label: b.getAttribute('aria-label') || b.textContent.trim(), disabled: b.disabled })),
  } : null;
}, name);

test('the Watches page lists watches with their state and output, and manages them (WCH-013)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/watches'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Watches');
  await firstShown(screen.getByRole('button', 'Edit release-check'));
  const release = await cardOf(browser, 'release-check');
  expect(release!.badges).toEqual(['Active']);
  expect(release!.text).toContain('every 1h');
  expect(release!.text).toContain('watch:release-check');
  const ended = await cardOf(browser, 'merge-queue-targets');
  expect(ended!.badges).toEqual(['Ended']);
  expect(ended!.buttons.map((b) => b.label)).toContain('Resume');
  // Another person's failing watch: an administrator may pause or remove it, not run or edit it.
  const failing = await cardOf(browser, 'staging-health');
  expect(failing!.badges).toEqual(['Active', 'Failing']);
  expect(failing!.text).toContain('cron */30 * * * * (UTC+1)');
  expect(failing!.text).toContain('Last run failed: exit 7 (3 in a row)');
  const disabled = Object.fromEntries(failing!.buttons.map((b) => [b.label, b.disabled]));
  expect(disabled).toEqual({ 'Run now': true, Pause: false, 'Edit staging-health': true, 'Remove staging-health': false });

  // Run now: the run's result shows when the page hears of it.
  await screen.getByRole('button', 'Run now').first().click();
  await expect(screen.getByText('ran now (72)')).toBeAttached();

  // An open fold stays open while the page hears of runs.
  await browser.evaluate(() => {
    const card = [...document.querySelectorAll('.item-card')].find((c) => c.querySelector('.item-name')?.textContent === 'release-check');
    (card!.querySelector('details.watch-output') as HTMLDetailsElement).open = true;
  });
  await screen.getByRole('button', 'Run now').first().click();
  await expect(screen.getByText('ran now (73)')).toBeVisible();

  // Pause and resume.
  await screen.getByRole('button', 'Resume').click();
  await expect.poll(async () => (await cardOf(browser, 'merge-queue-targets'))!.badges).toEqual(['Active']);
  const pause = (await cardOf(browser, 'release-check'))!.buttons.find((b) => b.label === 'Pause');
  expect(pause!.disabled).toBe(false);
  await browser.evaluate(() => {
    const card = [...document.querySelectorAll('.item-card')].find((c) => c.querySelector('.item-name')?.textContent === 'release-check');
    ([...card!.querySelectorAll('.item-controls button')].find((b) => b.textContent === 'Pause') as HTMLButtonElement).click();
  });
  await expect.poll(async () => (await cardOf(browser, 'release-check'))!.badges).toEqual(['Paused']);
  expect((await cardOf(browser, 'release-check'))!.buttons.map((b) => b.label)).toContain('Resume');

  // New watch.
  await screen.getByRole('button', 'New watch').click();
  const dialog = screen.getByRole('dialog', 'New watch');
  await dialog.getByRole('textbox', /^Name/).fill('disk-guard');
  await dialog.getByRole('textbox', /^Script/).fill('df -h / | tail -1');
  await dialog.getByRole('textbox', 'Interval or cron').fill('15m');
  await dialog.getByRole('button', 'Create').click();
  await expect.poll(async () => (await cardOf(browser, 'disk-guard'))?.text ?? '').toContain('every 15m');

  // Edit keeps the name; a bad interval is said in the dialog.
  await screen.getByRole('button', 'Edit disk-guard').click();
  const edit = screen.getByRole('dialog', 'Edit disk-guard');
  await expect(edit.getByRole('textbox', /^Name/)).toBeDisabled();
  await edit.getByRole('textbox', 'Interval or cron').fill('soon');
  await edit.getByRole('button', 'Save').click();
  await expect(edit.getByText(/interval is a number and s, m, h or d/)).toBeVisible();
  await edit.getByRole('textbox', 'Interval or cron').fill('2h');
  await edit.getByRole('button', 'Save').click();
  await expect.poll(async () => (await cardOf(browser, 'disk-guard'))?.text ?? '').toContain('every 2h');

  // Remove, after a confirmation.
  await screen.getByRole('button', 'Remove disk-guard').click();
  await screen.getByRole('dialog', 'Remove watch “disk-guard”?').getByRole('button', 'Remove').click();
  await expect.poll(async () => cardOf(browser, 'disk-guard')).toBeNull();
  expect(await axeViolations(browser)).toEqual([]);
});
