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
/** The channel's controls in page order, each with whether it can be used: "select on, button off". */
const channelControls = (browser: { evaluate: (fn: () => string) => Promise<string> }) => browser.evaluate(() => [...document.querySelectorAll('.claude-channel select, .claude-channel button')]
  .map((control) => `${control.tagName.toLowerCase()} ${(control as HTMLButtonElement).disabled ? 'off' : 'on'}`).join(', '));
/** The channel the select shows, and whether it can be used: "stable on". */
const channelSelect = (browser: { evaluate: (fn: () => string) => Promise<string> }) => browser.evaluate(() => {
  const el = document.getElementById('claude-channel') as HTMLSelectElement;
  return `${el.value} ${el.disabled ? 'off' : 'on'}`;
});
/** The mock's Claude Code state a test reaches into. */
type MockClaude = { __yuliusMock: { claude: { holdRead: Promise<void> | null; releaseRead?: () => void; failRead?: () => void; readsHeld?: number; lateAnswers?: string[] }; heldWaiting?: number } };
/** What each read the mock answered late said: its channel, and ", checking" when a check ran as it was asked. */
const lateAnswers = (browser: { evaluate: (fn: () => string[]) => Promise<string[]> }) => browser.evaluate(() => (window as unknown as MockClaude).__yuliusMock.claude.lateAnswers ?? []);

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
  // With moves stopped, the tasks on an older version are promised their next start only.
  expect(text).toMatch(/Still on an older Claude Code \(2\)/);
  expect(text).toMatch(/Each gets 2\.1\.400 at its next start\./);
  expect(text).not.toMatch(/when its turn ends/);
  expect(await browser.locator('.claude-problem').count()).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);

  // A failed check read nothing: the releases are an earlier check's, and said so.
  await app.open(mock('/admin/instance', '&claude=failed'));
  await firstShown(screen.getByText(/The check failed: registry\.npmjs\.org is unreachable\./));
  text = await cardText(browser);
  expect(text).toMatch(/Checked .* ago: npm had latest 2\.1\.322 · stable 2\.1\.311 when it was last read\./);
  expect(text).not.toMatch(/npm has/);
});

test('a Claude Code whose binary could not be read for its models says so', async ({ app, browser, screen }) => {
  // What `yulius doctor` warns about: neither "knows every offered model" nor the danger banner.
  await app.open(mock('/admin/instance', '&claude=unread'));
  await firstShown(screen.getByText('models not read'));
  const text = await cardText(browser);
  expect(text).toMatch(/The binary of Claude Code 2\.1\.322 could not be read for the models it knows, so whether it knows claude-fable-5-2, Yulius's default model, is not known\./);
  expect(text).not.toMatch(/knows every offered model/);
  expect(await browser.locator('.claude-problem').count()).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);
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
  const select = () => channelSelect(browser);
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

  // The way back, refused too: both controls are usable again, and then it goes through.
  const fails = (now: boolean) => browser.evaluate((value: boolean) => {
    (window as unknown as { __yuliusMock: { claude: { channelFails: boolean } } }).__yuliusMock.claude.channelFails = value;
  }, now);
  const controls = () => channelControls(browser);
  await fails(true);
  await screen.getByRole('button', 'Use the server’s (Latest)').click();
  await expect.poll(controls).toBe('select on, button on');
  await expect.poll(select).toBe('stable on');
  await fails(false);
  await screen.getByRole('button', 'Use the server’s (Latest)').click();
  await expect(screen.getByText(/Claude Code follows Latest now\. Checking for a release…/)).toBeVisible();
  await expect.poll(controls).toBe('select on');
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
  const controls = () => channelControls(browser);
  expect(await controls()).toBe('select on, button on');
  await screen.getByRole('button', 'Use the server’s (Latest)').click();
  // The request waits at the hold: what is read now is read while the change is being saved.
  await expect.poll(() => browser.evaluate(() => (window as unknown as MockClaude).__yuliusMock.heldWaiting ?? 0)).toBe(1);
  expect(await controls()).toBe('select off, button off');

  await browser.evaluate(() => (window as unknown as { __yuliusMock: { release(): void } }).__yuliusMock.release());
  await expect(screen.getByText(/Claude Code follows Latest now\. Checking for a release…/)).toBeVisible();
  await expect(screen.getByText(/From the server’s configuration \(YULIUS_CLAUDE_UPDATES\)/)).toBeVisible();
  await expect.poll(controls).toBe('select on');
});

test('a read asked before a check or a channel change does not undo what that drew, answered or failed after it', async ({ app, browser, screen }) => {
  const readsHeld = () => browser.evaluate(() => (window as unknown as MockClaude).__yuliusMock.claude.readsHeld ?? 0);
  const releaseRead = () => browser.evaluate(() => {
    const claude = (window as unknown as MockClaude).__yuliusMock.claude;
    claude.holdRead = null;
    claude.releaseRead?.();
  });

  // The page's first read is answered late: "Check now" is pressed first, and its answer drawn.
  await app.open(mock('/admin/instance', '&claude_hold_read=1'));
  await expect.poll(readsHeld).toBe(1);
  // Only that read is kept back: the card's later reads are answered as they come.
  await browser.evaluate(() => { (window as unknown as MockClaude).__yuliusMock.claude.holdRead = null; });
  await screen.getByRole('button', CHECK).click();
  await expect(screen.getByText(/Checking npm for a newer release…/)).toBeVisible();
  // The first read's answer comes now and says nothing of the check: the card goes on asking, and
  // shows what the check did.
  await releaseRead();
  await expect.poll(() => lateAnswers(browser)).toEqual(['latest']);
  await expect(screen.getByText(/Up to date: runs get 2\.1\.322\./)).toBeVisible({ timeout: 10000 });

  // The same for a channel change. A check runs, so the card asks again; that read is answered late.
  await browser.evaluate(() => {
    const claude = (window as unknown as MockClaude).__yuliusMock.claude;
    claude.holdRead = new Promise((resolve) => { claude.releaseRead = resolve; });
  });
  await screen.getByRole('button', CHECK).click();
  await expect.poll(readsHeld, { timeout: 10000 }).toBe(1);
  // Only that read is kept back. It was asked while the channel was Latest; then the channel changes.
  await browser.evaluate(() => { (window as unknown as MockClaude).__yuliusMock.claude.holdRead = null; });
  await screen.getByRole('combobox', 'Release channel').selectOption({ label: 'Stable' });
  await expect(screen.getByText(/Chosen here by mkarp/)).toBeVisible();
  await expect.poll(() => channelSelect(browser)).toBe('stable on');
  // Its answer arrives after the change was drawn, and names the old channel. The card keeps the
  // change, and the check the change started is followed to its end.
  await releaseRead();
  await expect.poll(() => lateAnswers(browser)).toEqual(['latest', 'latest']);
  await expect(screen.getByText(/Up to date: runs get 2\.1\.322\./)).toBeVisible({ timeout: 10000 });
  expect(await channelSelect(browser)).toBe('stable on');
  await expect(screen.getByText(/Chosen here by mkarp/)).toBeVisible();

  // And when such a read fails instead: the card keeps what the change drew and says nothing.
  await browser.evaluate(() => {
    const claude = (window as unknown as MockClaude).__yuliusMock.claude;
    claude.holdRead = new Promise((resolve, reject) => { claude.failRead = () => reject(new Error('lost')); });
  });
  await screen.getByRole('button', CHECK).click();
  await expect.poll(readsHeld, { timeout: 10000 }).toBe(1);
  await browser.evaluate(() => { (window as unknown as MockClaude).__yuliusMock.claude.holdRead = null; });
  await screen.getByRole('button', 'Use the server’s (Latest)').click();
  await expect(screen.getByText(/From the server’s configuration \(YULIUS_CLAUDE_UPDATES\)/)).toBeVisible();
  await browser.evaluate(() => { (window as unknown as MockClaude).__yuliusMock.claude.failRead?.(); });
  await expect.poll(readsHeld).toBe(0);
  const unread = async () => [await cardText(browser), ...await browser.evaluate(() => [...document.querySelectorAll('#toasts .toast-msg')].map((el) => (el as HTMLElement).innerText))]
    .filter((text) => /Couldn’t read/.test(text));
  expect(await unread()).toEqual([]);
  await expect(screen.getByText(/Up to date: runs get 2\.1\.322\./)).toBeVisible({ timeout: 10000 });
  expect(await unread()).toEqual([]);
  expect(await channelSelect(browser)).toBe('latest on');
});

test('a read asked while a channel change is being saved, and answered after it, does not undo it either', async ({ app, browser, screen }) => {
  const readsHeld = () => browser.evaluate(() => (window as unknown as MockClaude).__yuliusMock.claude.readsHeld ?? 0);
  await app.open(mock('/admin/instance', '&holdpost=/api/admin/claude-code/channel'));
  await firstShown(screen.getByText('Claude Code 2.1.322'));
  // A check runs, so the card will ask again; that read's answer is kept back.
  await screen.getByRole('button', CHECK).click();
  await expect(screen.getByText(/Checking npm for a newer release…/)).toBeVisible();
  await browser.evaluate(() => {
    const claude = (window as unknown as MockClaude).__yuliusMock.claude;
    claude.holdRead = new Promise((resolve) => { claude.releaseRead = resolve; });
  });
  // The change waits at the hold, and the card's read is asked meanwhile: the channel is Latest still.
  await screen.getByRole('combobox', 'Release channel').selectOption({ label: 'Stable' });
  await expect.poll(() => browser.evaluate(() => (window as unknown as MockClaude).__yuliusMock.heldWaiting ?? 0)).toBe(1);
  await expect.poll(readsHeld, { timeout: 10000 }).toBe(1);
  await browser.evaluate(() => { (window as unknown as MockClaude).__yuliusMock.claude.holdRead = null; });
  // The change is saved and drawn; then the read's answer arrives.
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { release(): void } }).__yuliusMock.release());
  await expect(screen.getByText(/Chosen here by mkarp/)).toBeVisible();
  await browser.evaluate(() => { (window as unknown as MockClaude).__yuliusMock.claude.releaseRead?.(); });
  await expect.poll(() => lateAnswers(browser)).toEqual(['latest']);
  await expect(screen.getByText(/Up to date: runs get 2\.1\.322\./)).toBeVisible({ timeout: 10000 });
  expect(await channelSelect(browser)).toBe('stable on');
});

test('“Check now” stays off while its request is on its way, also when the card is drawn meanwhile', async ({ app, browser, screen }) => {
  // The page's first read is answered late and the check's request waits at the hold.
  await app.open(mock('/admin/instance', '&claude_hold_read=1&holdpost=/api/admin/claude-code/check'));
  await expect.poll(() => browser.evaluate(() => (window as unknown as MockClaude).__yuliusMock.claude.readsHeld ?? 0)).toBe(1);
  await screen.getByRole('button', CHECK).click();
  await expect.poll(() => browser.evaluate(() => (window as unknown as MockClaude).__yuliusMock.heldWaiting ?? 0)).toBe(1);
  // Off from the press, before anything draws the card.
  await expect(screen.getByRole('button', CHECK)).toBeDisabled();
  // The first read draws the card while the check is still being asked for.
  await browser.evaluate(() => { (window as unknown as MockClaude).__yuliusMock.claude.releaseRead?.(); });
  await firstShown(screen.getByText('Claude Code 2.1.322'));
  await expect(screen.getByRole('button', CHECK)).toBeDisabled();
  // The check is taken: the card says it runs, then what it did, and the button is back.
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { release(): void } }).__yuliusMock.release());
  await expect(screen.getByText(/Checking npm for a newer release…/)).toBeVisible();
  await expect(screen.getByText(/Up to date: runs get 2\.1\.322\./)).toBeVisible({ timeout: 10000 });
  await expect(screen.getByRole('button', CHECK)).toBeEnabled();
});

test('a server that does not isolate sandboxes says why nothing is kept current', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&claude=unmanaged'));
  await firstShown(screen.getByText(/Sandboxes are not isolated on this server, so they run the host’s Claude Code as it is/));
  await expect(screen.getByRole('button', CHECK)).toBeDisabled();
  expect(await browser.locator('#claude-channel').count()).toBe(0);
  // Nothing is kept current, so there was no binary to read for its models.
  expect(await cardText(browser)).not.toMatch(/models not read|could not be read/);
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

  // While running tasks are no longer moved to that version, only its next start is promised.
  await app.open(mock('/s/studio/tasks/12', '&claude=gap'));
  await firstShown(screen.getByRole('heading', { level: 2, name: 'Details' }));
  await expect.poll(async () => (await details())['Claude Code']).toBe('2.1.322 · gets 2.1.400 at its next start');
});
