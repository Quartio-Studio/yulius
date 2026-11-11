import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock } from './support.ts';

failOnPageErrors();

// Settings → Instance, "Claude Code" (SPEC-461 CCM-008..CCM-013): which Claude Code sandboxes run
// and what it knows, loudly with the fix when it does not know the default model, "Check now",
// the release channel changed in place, and the tasks still on an older version; and a task's
// info with the model it asks for, the models that answered and the Claude Code that runs it.
// The mock's check takes two seconds; the card asks every two seconds while one runs.

/** The card's own "Check now": the page has another, for Yulius's updates. */
const CHECK = 'Check now for a Claude Code release';
const bodyText = (browser: { evaluate: (fn: () => string) => Promise<string> }) => browser.evaluate(() => document.body.innerText);
const cardText = (browser: { evaluate: (fn: () => string) => Promise<string> }) => browser.evaluate(() => (document.querySelector('.claude-code') as HTMLElement).innerText);

test('the card shows the version, npm’s releases, the last check and the tasks still on an older one', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  await firstShown(screen.getByRole('heading', { level: 2, name: 'Claude Code' }));
  await firstShown(screen.getByText('Claude Code 2.1.322'));
  await expect(screen.getByText('knows every offered model')).toBeVisible();
  const text = await cardText(browser);
  expect(text).toMatch(/kept current by Yulius · the host has 2\.1\.311/);
  expect(text).toMatch(/Checked .* ago: npm has latest 2\.1\.322 · stable 2\.1\.311\. Installed 2\.1\.322: new runs use it\./);
  expect(text).not.toMatch(/does not know/);
  // The running tasks whose session started before the release, each linking to the task.
  expect(text).toMatch(/Still on an older Claude Code \(2\)/);
  expect(text).toMatch(/Each moves to 2\.1\.322 when its turn ends, in the same session, or at its next start\./);
  await expect(screen.getByRole('link', '#12 Checkout API')).toBeVisible();
  await expect(screen.getByRole('link', '#10 AWS login for deploy')).toBeVisible();
  expect(text).toMatch(/studio · Claude Code 2\.1\.311/);
  // The channel in effect and where it is set.
  expect(await browser.evaluate(() => (document.getElementById('claude-channel') as HTMLSelectElement).value)).toBe('latest');
  expect(text).toMatch(/Every release, within the hour of its publication\. From the server’s configuration \(YULIUS_CLAUDE_UPDATES\)/);
  expect(await axeViolations(browser)).toEqual([]);

  // Check now: it says it checks, the button waits, then what the check did.
  await screen.getByRole('button', CHECK).click();
  await expect(screen.getByText('Checking npm for a newer release…')).toBeVisible();
  await expect(screen.getByRole('button', CHECK)).toBeDisabled();
  await expect(screen.getByText(/Up to date: runs get 2\.1\.322\./)).toBeVisible({ timeout: 10000 });
  await expect(screen.getByRole('button', CHECK)).toBeEnabled();
  expect(await axeViolations(browser)).toEqual([]);
});

test('a Claude Code without the default model is said loudly with the fix, and a check installs one that knows it', async ({ app, browser, screen }) => {
  // The incident of 2026-11-11: sandboxes run the host's 2.1.311.
  await app.open(mock('/admin/instance', '&claude=incident'));
  await firstShown(screen.getByText('default model unknown'));
  const alert = await browser.evaluate(() => (document.querySelector('.claude-problem[role="alert"]') as HTMLElement).innerText);
  expect(alert).toMatch(/Claude Code 2\.1\.311 does not know claude-fable-5-2, Yulius's default model/);
  expect(alert).toMatch(/Yulius installs a release that knows it at its next check/);
  expect(alert).toMatch(/sudo npm install -g @anthropic-ai\/claude-code@latest/);
  await expect(screen.getByText('default model unknown')).toBeVisible();
  await expect(screen.getByText('the host’s install')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', CHECK).click();
  await expect(screen.getByText(/Installed 2\.1\.322: new runs use it\./)).toBeVisible({ timeout: 10000 });
  await expect(screen.getByText('knows every offered model')).toBeVisible();
  expect(await browser.locator('.claude-problem').count()).toBe(0);
  expect(await bodyText(browser)).not.toMatch(/does not know claude-fable-5-2/);
});

test('a refused release, another unknown model, a failed check and stopped moves are said plainly', async ({ app, browser, screen }) => {
  // The newest release was refused too: no release knows the default model yet.
  await app.open(mock('/admin/instance', '&claude=ahead'));
  await firstShown(screen.getByText('default model unknown'));
  let text = await cardText(browser);
  expect(text).toMatch(/No Claude Code release knows it yet \(2\.1\.322, the newest, does not\)/);
  expect(text).toMatch(/Refused, nothing was installed: 2\.1\.322 does not know claude-fable-5-2 \(Yulius's default model\)\./);
  expect(text).not.toMatch(/sudo npm install/);
  expect(await axeViolations(browser)).toEqual([]);

  // Another offered model is unknown: a warning, not the danger banner; and why moves stopped.
  await app.open(mock('/admin/instance', '&claude=gap'));
  await firstShown(screen.getByText('an offered model unknown'));
  text = await cardText(browser);
  expect(text).toMatch(/Claude Code 2\.1\.400 does not know Haiku 4\.5 \(claude-haiku-4-5\), which the composer offers/);
  expect(text).toMatch(/Running tasks are no longer moved to 2\.1\.400: the session of task #9 ended as it restarted on it\. Sessions that start or continue still get it; choose Off/);
  expect(await browser.locator('.claude-problem').count()).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);

  // A failed check read nothing: the releases are an earlier check's, and said so.
  await app.open(mock('/admin/instance', '&claude=failed'));
  await firstShown(screen.getByText(/The check failed: registry\.npmjs\.org is unreachable\./));
  text = await cardText(browser);
  expect(text).toMatch(/Checked .* ago: npm had latest 2\.1\.322 · stable 2\.1\.311 when it was last read\./);
  expect(text).not.toMatch(/npm has/);
});

test('the channel is changed in place: Stable, back to the server’s, and Off', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  await firstShown(screen.getByText('Claude Code 2.1.322'));
  const channel = screen.getByRole('combobox', 'Release channel');
  await channel.selectOption({ label: 'Stable' });
  await expect(screen.getByText(/Claude Code follows Stable now\. Checking for a release…/)).toBeVisible();
  await expect(screen.getByText(/Chosen here by mkarp/)).toBeVisible();
  expect(await browser.evaluate(() => (document.getElementById('claude-channel') as HTMLSelectElement).value)).toBe('stable');
  // A channel change is checked at once.
  await expect(screen.getByText(/Up to date: runs get 2\.1\.322\./)).toBeVisible({ timeout: 10000 });
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Use the server’s (Latest)').click();
  await expect(screen.getByText(/From the server’s configuration \(YULIUS_CLAUDE_UPDATES\)/)).toBeVisible();
  expect(await browser.evaluate(() => (document.getElementById('claude-channel') as HTMLSelectElement).value)).toBe('latest');
  expect(await browser.locator('.claude-channel button').count()).toBe(0);

  // Off: sandboxes run the host's own install and nothing is checked.
  await screen.getByRole('combobox', 'Release channel').selectOption({ label: 'Off' });
  await expect(screen.getByText(/Claude Code updates are off: sandboxes run the host’s install from their next start\./)).toBeVisible();
  await expect(screen.getByText('Claude Code 2.1.311')).toBeVisible();
  await expect(screen.getByText('the host’s install')).toBeVisible();
  await expect(screen.getByRole('button', CHECK)).toBeDisabled();
  // What an earlier check read is not said as today's (once the check that ran has ended).
  await expect(screen.getByText('Updates are off: nothing is checked.')).toBeVisible({ timeout: 10000 });
  expect(await cardText(browser)).not.toMatch(/npm has|Checked/);
});

test('a channel change or a check the server refuses leaves the card as the server has it, and usable', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&claude_channel_fail=1&failpost=/api/admin/claude-code/check'));
  await firstShown(screen.getByText('Claude Code 2.1.322'));
  const select = () => browser.evaluate(() => {
    const el = document.getElementById('claude-channel') as HTMLSelectElement;
    return `${el.value} ${el.disabled ? 'off' : 'on'}`;
  });
  await screen.getByRole('combobox', 'Release channel').selectOption({ label: 'Stable' });
  await expect(screen.getByText(/Couldn’t change the channel/)).toBeVisible();
  // The select is back on the channel in effect and can be used again.
  await expect.poll(select).toBe('latest on');

  await screen.getByRole('button', CHECK).click();
  await expect(screen.getByText(/Couldn’t check for a Claude Code release/)).toBeVisible();
  await expect.poll(() => browser.evaluate(() => (document.querySelector('.claude-code .settings-head button') as HTMLButtonElement).disabled)).toBe(false);

  // The server takes the change again: the same choice goes through.
  await browser.evaluate(() => {
    (window as unknown as { __yuliusMock: { claude: { channelFails: boolean } } }).__yuliusMock.claude.channelFails = false;
  });
  await screen.getByRole('combobox', 'Release channel').selectOption({ label: 'Stable' });
  await expect(screen.getByText(/Claude Code follows Stable now\. Checking for a release…/)).toBeVisible();
  await expect.poll(select).toBe('stable on');
});

test('while a channel change is being saved neither the select nor “Use the server’s” takes another', async ({ app, browser, screen }) => {
  // The channel's request waits until the test lets it go (#1192).
  await app.open(mock('/admin/instance', '&holdpost=/api/admin/claude-code/channel'));
  await firstShown(screen.getByText('Claude Code 2.1.322'));
  // A channel was chosen here before: the card, drawn again by a check, offers the way back.
  await browser.evaluate(() => {
    const claude = (window as unknown as { __yuliusMock: { claude: { channel: string; chosen: { by: string; at: string } } } }).__yuliusMock.claude;
    claude.channel = 'stable';
    claude.chosen = { by: 'mkarp', at: new Date().toISOString() };
  });
  await screen.getByRole('button', CHECK).click();
  // The check has ended, so nothing redraws the card while the change is saved.
  await expect(screen.getByText(/Up to date: runs get 2\.1\.322\./)).toBeVisible({ timeout: 10000 });
  const controls = () => browser.evaluate(() => [...document.querySelectorAll('.claude-channel select, .claude-channel button')]
    .map((control) => `${control.tagName.toLowerCase()} ${(control as HTMLButtonElement).disabled ? 'off' : 'on'}`).join(', '));
  expect(await controls()).toBe('select on, button on');
  await screen.getByRole('button', 'Use the server’s (Latest)').click();
  await expect.poll(controls).toBe('select off, button off');

  await browser.evaluate(() => (window as unknown as { __yuliusMock: { release(): void } }).__yuliusMock.release());
  await expect(screen.getByText(/Claude Code follows Latest now\. Checking for a release…/)).toBeVisible();
  await expect(screen.getByText(/From the server’s configuration \(YULIUS_CLAUDE_UPDATES\)/)).toBeVisible();
  await expect.poll(controls).toBe('select on');
});

test('a server that does not isolate sandboxes says why nothing is kept current', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&claude=unmanaged'));
  await firstShown(screen.getByText(/Sandboxes are not isolated on this server, so they run the host’s Claude Code as it is/));
  await expect(screen.getByRole('button', CHECK)).toBeDisabled();
  expect(await browser.locator('#claude-channel').count()).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);
});

test('a task’s info shows the model asked for, what answered and the Claude Code that runs it', async ({ app, browser, screen }) => {
  // A running task whose session started before the release.
  await app.open(mock('/s/studio/tasks/12'));
  await firstShown(screen.getByRole('heading', { level: 2, name: 'Details' }));
  const details = () => browser.evaluate(() => {
    const out: Record<string, string> = {};
    document.querySelectorAll('.ti-kv dt').forEach((dt) => { out[(dt as HTMLElement).innerText] = (dt.nextElementSibling as HTMLElement).innerText; });
    return out;
  });
  await expect.poll(async () => (await details())['Claude Code']).toBe('2.1.311 · moves to 2.1.322 when its turn ends, or at its next start');
  const rows = await details();
  expect(rows.Model).toBe('Opus 4 · High effort');
  expect(rows['Answered by']).toBe('Opus 4');
  expect(rows.Models).toBeUndefined();
  // Each session says what it ran on.
  const session = await browser.evaluate(() => (document.querySelector('.ti-sessions td[data-label="Ran on"]') as HTMLElement).innerText);
  expect(session).toBe('Claude Code 2.1.311 · Opus 4');
  expect(await axeViolations(browser)).toEqual([]);

  // A task on the version new runs get says nothing about moving.
  await app.open(mock('/s/studio/tasks/11'));
  await firstShown(screen.getByRole('heading', { level: 2, name: 'Details' }));
  await expect.poll(async () => (await details())['Claude Code']).toBe('2.1.322');
});
