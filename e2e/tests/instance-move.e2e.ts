import { test, type Browser, type Screen } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock } from './support.ts';

failOnPageErrors();

// Settings → Instance, "Move to another host" (SPEC-470 MVB-030..MVB-037): a superadmin moves the
// instance from the page. The mock plays root's helper and its command: `move=…` picks what
// happens (see mock.js), `movepoll=<ms>` makes the card read a move in flight that often, and
// `__yuliusMock.migration.stepMs` sets the helper's pace. A move freezes the mock at its cutover
// until the page asked twice in vain; then the new instance answers, this address redirects
// (`move=redirect`) or the old instance answers again (`move=rollback`).

type Asked = { action: string; target: string | null; ssh_port: number | null; public_url: string | null; data_disks: string[]; confirm: string | null };
type MoveMock = {
  __yuliusMock: {
    migration: { stepMs: number; ackMs: number; asked?: Asked[]; holdReads: boolean; holds: unknown[]; releaseReads(): void; frozen: boolean; missed: number };
    sent?: Record<string, number>;
  };
};

const API = '/api/admin/instance/migration';
const HOST = 'new.yulius.example';
const TARGET = `root@${HOST}`;
const URL = `https://${HOST}`;
/** A quick card and a quick helper: what a test waits for arrives within its usual budget. */
const QUICK = '&movepoll=200';

const pace = (browser: Browser, stepMs: number) => browser.evaluate((ms: number) => {
  (window as unknown as MoveMock).__yuliusMock.migration.stepMs = ms;
}, stepMs);
const asked = (browser: Browser) => browser.evaluate(() => (window as unknown as MoveMock).__yuliusMock.migration.asked ?? []);
const sent = (browser: Browser) => browser.evaluate((path: string) => ((window as unknown as MoveMock).__yuliusMock.sent || {})[`POST ${path}`] || 0, API);
/** The text of the card, as a person reads it. */
const cardText = (browser: Browser) => browser.evaluate(() => {
  const el = document.querySelector<HTMLElement>('section.move');
  return el ? el.innerText : '';
});
/** The names of the checklist's lines, in order. */
const lineNames = (browser: Browser, list = '.move-checklist .move-lines') => browser.evaluate((selector: string) => (
  [...document.querySelectorAll(`section.move ${selector} .move-line-text strong`)].map((el) => el.textContent || '')
), list);

const card = (screen: Screen) => screen.getByRole('region', 'Move to another host');

/** Types the new host with a public URL and two disks, and asks for the plan. */
async function showThePlan(screen: Screen): Promise<void> {
  const c = card(screen);
  await c.getByLabel('New host').fill(TARGET);
  await c.getByLabel('New public URL').fill(URL);
  await c.getByLabel('Data disks').fill('/dev/vdc, /dev/vdd');
  await c.getByRole('button', 'Show the plan').click();
}

/** Opens the typed confirmation of a move and confirms it with the host's name. */
async function runMigration(screen: Screen, button = 'Run migration…'): Promise<void> {
  await card(screen).getByRole('button', button).click();
  const dialog = screen.getByRole('dialog', new RegExp(`to ${HOST.replaceAll('.', '\\.')}\\?$`));
  await dialog.getByRole('textbox', `Type ${HOST} to confirm`).fill(HOST);
  await dialog.getByRole('button', button === 'Run migration…' ? 'Run migration' : 'Continue the move').click();
  await expect(dialog).toBeHidden();
}

// MVB-030..MVB-032: the form, the plan that ends with the key to add, the plan with what is the
// person's first, and the move behind the host typed by hand.
test('the plan asks for the key first, then says what is yours, and a move needs the host typed', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', `&move=key${QUICK}`));
  const c = card(screen);
  await firstShown(c.getByRole('button', 'Show the plan'));
  await pace(browser, 60);
  await expect(c.getByText(/It asks for everything a person has to do in the plan, before anything freezes\./)).toBeVisible();
  await expect(c.getByLabel('SSH port')).toHaveValue('22');
  await expect(c.getByText(/^Leave empty to keep http/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  // Nobody can sign in for the move's key from here: the plan ends with the line to add.
  await showThePlan(screen);
  await expect(c.getByText('Add the move’s key on the new host, then show the plan again.')).toBeVisible();
  await expect(c.getByText(new RegExp(`reaches ${TARGET.replaceAll('.', '\\.')} with a one-time key and removes it at the end`))).toBeVisible();
  const key = await browser.evaluate(() => {
    const el = document.querySelector<HTMLElement>('section.move [data-move-key]');
    return el ? el.innerText.trim() : '';
  });
  expect(key).toMatch(/^ssh-ed25519 \S+ yulius-migrate-7c1e09ab$/);
  await expect(c.getByRole('button', 'Copy the key line')).toBeVisible();
  await expect(c.getByRole('button', 'Run migration…')).toBeHidden();
  // The line is in the checklist too, as the command wrote it; the key box is what says it is
  // the person's, once.
  expect(await lineNames(browser)).toEqual(['old host', 'one-time key']);
  expect(await browser.evaluate(() => document.querySelector('section.move .move-attention') === null)).toBe(true);
  expect(await axeViolations(browser)).toEqual([]);

  // The key is added on the new host: the same plan goes to its end.
  await c.getByRole('button', 'Show the plan').click();
  await expect(c.getByText(new RegExp(`^The plan for ${TARGET.replaceAll('.', '\\.')}, public URL`))).toBeVisible();
  await expect(c.getByText('Nothing is frozen, and nothing was changed but the move’s own files. Its lines: changed 1, left alone 12, manual 2, failed 0.')).toBeVisible();
  await expect(c.getByText('Add the move’s key on the new host, then show the plan again.')).toBeHidden();
  // What a person has to act on is repeated before the checklist.
  const yours = await browser.evaluate(() => {
    const box = document.querySelector<HTMLElement>('section.move .move-attention');
    return box ? box.innerText : '';
  });
  expect(yours).toMatch(/^Yours to do \(2\)/);
  expect(yours).toMatch(/public URL: .*Change its DNS record when the checklist says the new instance runs/);
  expect(yours).toMatch(/GitHub App: .*add the callback URL https:\/\/new\.yulius\.example\/api\/auth\/github\/callback/);
  const names = await lineNames(browser);
  expect(names).toHaveLength(15);
  expect(names.slice(0, 3)).toEqual(['old host', 'one-time key', 'yulius build']);
  expect(await axeViolations(browser)).toEqual([]);

  // The move names the host that gets the data, and needs its name typed.
  await c.getByRole('button', 'Run migration…').click();
  const dialog = screen.getByRole('dialog', /^Move Yulius to new\.yulius\.example\?$/);
  await expect(dialog.getByText(/Root on this host will give this instance’s data to root@new\.yulius\.example: every repository, every secret and key/)).toBeVisible();
  await expect(dialog.getByText(/this host keeps a frozen copy/)).toBeVisible();
  await expect(dialog.getByText(/The plan said: about 44 min frozen/)).toBeVisible();
  await expect(dialog.getByText(/2 steps of the plan are yours/)).toBeVisible();
  const go = dialog.getByRole('button', 'Run migration');
  await expect(go).toBeDisabled();
  const typed = dialog.getByRole('textbox', `Type ${HOST} to confirm`);
  await typed.fill(TARGET);
  await expect(go).toBeDisabled();
  await typed.fill(HOST);
  await expect(go).toBeEnabled();
  expect(await axeViolations(browser)).toEqual([]);
  await dialog.getByRole('button', 'Cancel').click();
  await expect(dialog).toBeHidden();
  // Two plans were asked for, with what was typed, and no move.
  expect(await sent(browser)).toBe(2);
  expect(await asked(browser)).toEqual([
    { action: 'plan', target: TARGET, ssh_port: 22, public_url: URL, data_disks: ['/dev/vdc', '/dev/vdd'], confirm: null },
    { action: 'plan', target: TARGET, ssh_port: 22, public_url: URL, data_disks: ['/dev/vdc', '/dev/vdd'], confirm: null },
  ]);
});

// MVB-033, MVB-035, MVB-037: the move's phases and lines as they come, the freeze said in place
// while nothing answers, and the finished checklist once the new instance answers here.
test('a move shows its checklist, says when the instance is frozen, and ends on the new instance', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', QUICK));
  const c = card(screen);
  await firstShown(c.getByRole('button', 'Show the plan'));
  await pace(browser, 60);
  await showThePlan(screen);
  await expect(c.getByRole('button', 'Run migration…')).toBeVisible();
  await runMigration(screen);
  // Asked with the plan's values and the host that was typed.
  await expect.poll(async () => (await asked(browser))[1] ?? null).toEqual({ action: 'move', target: TARGET, ssh_port: 22, public_url: URL, data_disks: ['/dev/vdc', '/dev/vdd'], confirm: HOST });

  // The phases, with the one in progress, and the lines as the command writes them. The answer
  // is at the top of the card, which the page shows: the button pressed was a long way below.
  await expect(c.getByText(`Moving to ${HOST}`)).toBeVisible();
  const top = await browser.evaluate(() => {
    const el = document.querySelector<HTMLElement>('section.move');
    return el ? Math.round(el.getBoundingClientRect().top) : -1;
  });
  expect(top).toBeGreaterThanOrEqual(0);
  expect(top).toBeLessThan(200);
  await expect.poll(() => browser.evaluate(() => {
    const step = document.querySelector<HTMLElement>('section.move .move-phases .update-step-current .update-step-title');
    return step ? step.innerText : '';
  })).toMatch(/^Prepare/);
  await expect.poll(() => lineNames(browser)).toContain('software');
  // No form and no second move while it runs; it can be aborted.
  await expect(c.getByLabel('New host')).toBeHidden();
  await expect(c.getByRole('button', 'Abort…')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  // Nothing answers: the card says why, keeps the checklist and keeps trying.
  await expect(c.getByText(`Yulius is frozen while its data is copied to ${HOST}.`)).toBeVisible();
  await expect(c.getByText(/Nothing answers here meanwhile\. The plan said: about 44 min frozen.*This page keeps trying/)).toBeVisible();
  expect(await lineNames(browser)).toContain('software');
  await expect(c.getByRole('button', 'Abort…')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  // The name leads to the new host: the new instance answers with the finished checklist.
  await expect(c.getByText('The move finished: Yulius now runs on the new host.')).toBeVisible({ timeout: 15000 });
  await expect(c.getByText(`Yulius is frozen while its data is copied to ${HOST}.`)).toBeHidden();
  await expect(c.getByText(/^Yulius was moved to this host .*changed 11, left alone 15, manual 4, failed 0\.$/)).toBeVisible();
  const still = await browser.evaluate(() => {
    const el = document.querySelector<HTMLElement>('section.move .move-arrived .move-attention-list');
    return el ? el.innerText : '';
  });
  expect(still).toMatch(/old host: it keeps a frozen copy with master\.key and every secret/);
  expect(still).toMatch(/public URL: /);
  const finished = await lineNames(browser, '.move-arrived .move-lines');
  expect(finished).toHaveLength(30);
  expect(finished.slice(-2)).toEqual(['remote clients', 'old host']);
  // The new instance moves like any other: its form is ready, nothing of the old host's run.
  await expect(c.getByRole('button', 'Show the plan')).toBeEnabled();
  await expect(c.getByText(`Moving to ${HOST}`)).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);
});

// MVB-036: the old address redirects to the new one: the card says so and links to this page there.
test('when the old address redirects, the card links to the instance at its new address', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', `&move=redirect${QUICK}`));
  const c = card(screen);
  await firstShown(c.getByRole('button', 'Show the plan'));
  await pace(browser, 60);
  await showThePlan(screen);
  await runMigration(screen);
  await expect(c.getByText(`Yulius is frozen while its data is copied to ${HOST}.`)).toBeVisible({ timeout: 15000 });
  await expect(c.getByText(`Yulius now runs at ${URL}.`)).toBeVisible({ timeout: 15000 });
  await expect(c.getByText(/This address redirects there\. Sign in again at the new address/)).toBeVisible();
  await expect(c.getByText(`Yulius is frozen while its data is copied to ${HOST}.`)).toBeHidden();
  const link = await browser.evaluate(() => {
    const a = document.querySelector<HTMLAnchorElement>('section.move .move-moved a.btn');
    return a ? { text: a.innerText.trim(), href: a.href } : null;
  });
  expect(link).toEqual({ text: 'Open Yulius at its new address', href: `${URL}/admin/instance` });
  expect(await axeViolations(browser)).toEqual([]);
});

// MVB-033, MVB-034, MVB-035: a move that does not complete says where it stopped, on the old
// instance that answers again; it is continued with the same values or aborted, and the abort
// says what it does.
test('a move that does not complete is continued or aborted, and the abort says what it does', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', `&move=rollback${QUICK}`));
  const c = card(screen);
  await firstShown(c.getByRole('button', 'Show the plan'));
  await pace(browser, 60);
  await showThePlan(screen);
  await runMigration(screen);
  await expect(c.getByText(`Yulius is frozen while its data is copied to ${HOST}.`)).toBeVisible({ timeout: 15000 });

  // The old instance answers again: what failed first, then the whole checklist.
  await expect(c.getByText('The move did not complete')).toBeVisible({ timeout: 15000 });
  await expect(c.getByText(/The checklist says where it stopped\. Yulius answers here; continue the move with the same values, or abort it\./)).toBeVisible();
  const failed = await browser.evaluate(() => {
    const box = document.querySelector<HTMLElement>('section.move .move-attention[role="alert"]');
    return box ? box.innerText : '';
  });
  expect(failed).toMatch(/^Failed \(1\)/);
  expect(failed).toMatch(/new instance: yulius\.service did not report the old host’s build within 120 s/);
  expect(await lineNames(browser)).toContain('copy proven');
  expect(await axeViolations(browser)).toEqual([]);

  // Continued with the plan's values, behind the same typed confirmation.
  await c.getByRole('button', 'Continue the move…').click();
  const again = screen.getByRole('dialog', /^Continue the move to new\.yulius\.example\?$/);
  await expect(again.getByText(/Root on this host will give this instance’s data to root@new\.yulius\.example/)).toBeVisible();
  await expect(again.getByRole('button', 'Continue the move')).toBeDisabled();
  await again.getByRole('button', 'Cancel').click();
  await expect(again).toBeHidden();

  // The abort says what it does before it does it.
  await c.getByRole('button', 'Abort…').click();
  const dialog = screen.getByRole('dialog', /^Abort the move\?$/);
  await expect(dialog.getByText(/Yulius keeps running on this host; if the move had frozen it, it is thawed\./)).toBeVisible();
  await expect(dialog.getByText(/The move’s one-time key is removed from the new host and deleted here/)).toBeVisible();
  await expect(dialog.getByText(/What was already installed on the new host .* stays there\./)).toBeVisible();
  await expect(dialog.getByText(/it never guesses that the new instance is stopped/)).toBeVisible();
  await expect(dialog.getByText(/The run in flight is stopped first\./)).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);
  await dialog.getByRole('button', 'Abort the move').click();
  await expect(dialog).toBeHidden();
  await expect(c.getByText('The move was aborted')).toBeVisible();
  await expect(c.getByText('Yulius runs on this host as before.')).toBeVisible();
  expect(await lineNames(browser)).toEqual(['old instance', 'one-time key']);
  // Nothing is recorded any more: no abort, no move, and a new plan can be asked.
  await expect(c.getByRole('button', 'Abort…')).toBeHidden();
  await expect(c.getByRole('button', 'Continue the move…')).toBeHidden();
  await expect(c.getByRole('button', 'Show the plan')).toBeEnabled();
  const last = (await asked(browser)).pop();
  expect(last).toEqual({ action: 'abort', target: null, ssh_port: null, public_url: null, data_disks: [], confirm: null });
});

// MVB-033, MVB-034: an abort stops a run in flight, and a read that was on its way before the
// abort and answers after it does not bring the run back.
test('an abort stops a plan in flight, and a read answered late does not undo it', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', QUICK));
  const c = card(screen);
  await firstShown(c.getByRole('button', 'Show the plan'));
  // A plan that takes its time.
  await pace(browser, 4000);
  await showThePlan(screen);
  await expect(c.getByText('Asking the move helper for the plan…')).toBeVisible();
  await expect(c.getByText(new RegExp(`^Planning the move to ${TARGET.replaceAll('.', '\\.')}`))).toBeVisible({ timeout: 15000 });
  await expect(c.getByRole('button', 'Show the plan')).toBeHidden();
  await expect(c.getByRole('button', 'Run migration…')).toBeHidden();

  // The card's next read stays on its way while the person aborts.
  await browser.evaluate(() => { (window as unknown as MoveMock).__yuliusMock.migration.holdReads = true; });
  await expect.poll(() => browser.evaluate(() => (window as unknown as MoveMock).__yuliusMock.migration.holds.length)).toBeGreaterThanOrEqual(1);
  await c.getByRole('button', 'Abort…').click();
  const dialog = screen.getByRole('dialog', /^Abort the move\?$/);
  await expect(dialog.getByText('The run in flight is stopped first.')).toBeVisible();
  await pace(browser, 400);
  await dialog.getByRole('button', 'Abort the move').click();
  await expect(dialog).toBeHidden();
  await expect(c.getByText('Stopping it…')).toBeVisible();
  await expect(c.getByRole('button', 'Abort…')).toBeHidden();

  // The read from before the abort answers now, with the plan still running and no abort.
  await browser.evaluate(() => {
    const m = (window as unknown as MoveMock).__yuliusMock.migration;
    m.holdReads = false;
    m.releaseReads();
  });
  expect(await cardText(browser)).not.toMatch(/Planning the move to/);
  await expect(c.getByRole('button', 'Abort…')).toBeHidden();
  await expect(c.getByText('The move was aborted')).toBeVisible({ timeout: 15000 });
  expect(await cardText(browser)).not.toMatch(/Planning the move to/);
});

// MVB-031, MVB-033: a plan that found a problem offers no move; a move that fails while preparing
// never froze anything.
test('a plan with a failed line offers no move, and a failed preparation says so', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', `&move=planfail${QUICK}`));
  const c = card(screen);
  await firstShown(c.getByRole('button', 'Show the plan'));
  await pace(browser, 60);
  await showThePlan(screen);
  await expect(c.getByText(new RegExp(`^The plan for ${TARGET.replaceAll('.', '\\.')}.* found problems$`))).toBeVisible();
  await expect(c.getByText('Nothing is frozen, and nothing was changed but the move’s own files. Fix what failed, then show the plan again.')).toBeVisible();
  const failed = await browser.evaluate(() => {
    const box = document.querySelector<HTMLElement>('section.move .move-attention[role="alert"]');
    return box ? box.innerText : '';
  });
  expect(failed).toMatch(/^Failed \(1\)\s+disk space: 12\.0 GiB and 800000 inodes free/);
  await expect(c.getByRole('button', 'Run migration…')).toBeHidden();
  await expect(c.getByRole('button', 'Show the plan')).toBeEnabled();
  expect(await axeViolations(browser)).toEqual([]);

  await app.open(mock('/admin/instance', `&move=fail${QUICK}`));
  await firstShown(c.getByRole('button', 'Show the plan'));
  await pace(browser, 60);
  await showThePlan(screen);
  await runMigration(screen);
  await expect(c.getByText('The move did not complete')).toBeVisible({ timeout: 15000 });
  expect(await cardText(browser)).toMatch(/Failed \(1\)\s+software: the new host: tar: \/usr\/local: Cannot write: No space left on device/);
  expect(await cardText(browser)).not.toMatch(/is frozen while its data is copied/);
  await expect(c.getByRole('button', 'Continue the move…')).toBeVisible();
  await expect(c.getByRole('button', 'Abort…')).toBeVisible();
});

// MVB-030, MVB-033: what is typed is checked where it was typed; a helper that does not answer,
// refuses, or is not there is said with what to do.
test('a field outside the rules is said in place, and a helper that is silent, refuses or is missing is said', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', `&move=unanswered${QUICK}`));
  const c = card(screen);
  await firstShown(c.getByRole('button', 'Show the plan'));
  // Nothing typed, then something that is no host.
  await c.getByRole('button', 'Show the plan').click();
  await expect(c.getByText('Name the new host: root@new-host.')).toBeVisible();
  expect(await sent(browser)).toBe(0);
  await c.getByLabel('New host').fill('admin@new.yulius.example');
  await c.getByRole('button', 'Show the plan').click();
  await expect(c.getByText('the new host is reached as root: `root@host`')).toBeVisible();
  await expect(c.getByLabel('New host')).toBeFocused();
  await c.getByLabel('New host').fill(TARGET);
  await c.getByLabel('New public URL').fill('http://new.yulius.example/x');
  await c.getByRole('button', 'Show the plan').click();
  await expect(c.getByText('the public URL must be `https://host`, with nothing after it')).toBeVisible();
  await c.getByLabel('New public URL').fill('');
  await c.getByLabel('Data disks').fill('/etc/passwd');
  await c.getByRole('button', 'Show the plan').click();
  await expect(c.getByText('a data disk must be a device of the new host: `/dev/…`')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await c.getByLabel('Data disks').fill('');

  // Nobody takes the request: the card says which unit to enable, and it can be asked again.
  await browser.evaluate(() => { (window as unknown as MoveMock).__yuliusMock.migration.ackMs = 300; });
  await c.getByRole('button', 'Show the plan').click();
  await expect(c.getByText('Asking the move helper for the plan…')).toBeVisible();
  await expect(c.getByText('The move helper did not take the request.')).toBeVisible();
  await expect(c.getByText('sudo systemctl enable --now yulius-migrate.path')).toBeVisible();
  await expect(c.getByRole('button', 'Show the plan')).toBeEnabled();
  expect(await asked(browser)).toEqual([{ action: 'plan', target: TARGET, ssh_port: 22, public_url: null, data_disks: [], confirm: null }]);
  expect(await axeViolations(browser)).toEqual([]);

  // The helper refuses (an update runs): its reason, and nothing changed.
  await app.open(mock('/admin/instance', `&move=refused${QUICK}`));
  await firstShown(c.getByRole('button', 'Show the plan'));
  await pace(browser, 60);
  await c.getByLabel('New host').fill(TARGET);
  await c.getByRole('button', 'Show the plan').click();
  await expect(c.getByText('The move helper refused the request.')).toBeVisible();
  await expect(c.getByText(/an update is running \(yulius-update\.service\): ask again once it has finished\. Nothing was changed\./)).toBeVisible();
  await expect(c.getByRole('button', 'Run migration…')).toBeHidden();

  // No helper on this server: what it is, and the command that does the same.
  await app.open(mock('/admin/instance', '&move=nohelper'));
  await firstShown(c.getByText('Moving from this page needs the move helper on this server.'));
  await expect(c.getByText('sudo yulius instance migrate root@new-host')).toBeVisible();
  await expect(c.getByRole('button', 'Show the plan')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);
});

// MVB-037, MVB-020: on the host a move arrived at, its checklist; a move run on the host is shown
// as recorded; a member sees nothing of it.
test('the card shows the move that brought the instance here, and a move run on the host', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&move=arrived'));
  const c = card(screen);
  await firstShown(c.getByText(/^Yulius was moved to this host .* from https:\/\/old\.yulius\.example\. changed 3, left alone 14, manual 4, failed 0\.$/));
  await expect(c.getByText('The move finished: Yulius now runs on the new host.')).toBeHidden();
  await expect(c.getByText('Still for a person, as the move left it:')).toBeVisible();
  // The whole checklist is one click away.
  const shown = () => browser.evaluate(() => {
    const details = document.querySelector<HTMLDetailsElement>('section.move .move-arrived details');
    return Boolean(details && details.open);
  });
  expect(await shown()).toBe(false);
  await browser.locator('section.move .move-arrived summary').click();
  expect(await shown()).toBe(true);
  expect(await lineNames(browser, '.move-arrived .move-lines')).toHaveLength(21);
  // A new move can be planned from here.
  await expect(c.getByRole('button', 'Show the plan')).toBeEnabled();
  expect(await axeViolations(browser)).toEqual([]);

  await app.open(mock('/admin/instance', '&move=cli'));
  await firstShown(c.getByText(/A move to root@new\.yulius\.example is recorded on this host by the command line \(yulius instance migrate\), phase Prepare/));
  await expect(c.getByText(/Whether it still runs is not known here\./)).toBeVisible();
  expect(await lineNames(browser)).toEqual(['old host', 'packages']);
  await expect(c.getByRole('button', 'Abort…')).toBeVisible();
  await expect(c.getByRole('button', 'Run migration…')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  await app.open(mock('/admin/instance', '&role=member'));
  await firstShown(screen.getByText('Only the superadmin manages updates and the instance keys.'));
  expect(await browser.evaluate(() => document.querySelector('section.move') === null)).toBe(true);
  expect(await browser.evaluate((path: string) => (((window as unknown as { __yuliusMock: { asked?: Record<string, number> } }).__yuliusMock.asked || {})[path] || 0), API)).toBe(0);
});
