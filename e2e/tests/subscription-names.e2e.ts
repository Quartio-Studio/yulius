import { type Browser, test } from '@e2e-dev/web';
import { type App, expect } from 'e2e';
import { axeViolations, eventsConnected, failOnPageErrors, firstShown, mock } from './support.ts';

failOnPageErrors();

// A subscription's own name, edited in place, and the email of its Claude account (SPEC-244).

/** The name and the email line of each subscription card on the Subscriptions page. */
const cards = (browser: Browser) => browser.evaluate(() =>
  [...document.querySelectorAll('main .account-card')].map((card) => ({
    name: card.querySelector('.account-name strong')?.textContent ?? null,
    email: card.querySelector('.account-sub')?.textContent ?? null,
  })));

type Mock = {
  __yuliusMock: {
    accounts: { id: string; label: string; email: string | null; email_typed?: boolean }[];
    eventSockets: Set<{ push: (data: string) => void }>;
    sent?: Record<string, number>;
    asked?: Record<string, number>;
    holdPath: string | null;
    held: Promise<void> | null;
    release: () => void;
    released: boolean;
  };
};

/** Opens the Subscriptions page and waits, within a first load's budget, for its cards. */
async function openSubscriptions(app: App, browser: Browser) {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await firstShown(browser.locator('main .account-card').first());
}

/** The changes sent for subscription `id`, counted by the mock as the page asks (not as it answers). */
const sentFor = (browser: Browser, id: string) => browser.evaluate((account) =>
  (window as unknown as Mock).__yuliusMock.sent?.[`PATCH /api/admin/accounts/${account}`] ?? 0, id);

/** Every request the page sent that is not a GET, as "METHOD path" (counted as the page asks). */
const sentPaths = (browser: Browser) => browser.evaluate(() => Object.keys((window as unknown as Mock).__yuliusMock.sent ?? {}).sort());

/** Another administrator renames the subscription named `from`: the server announces it. */
const renamedElsewhere = (browser: Browser, from: string, to: string) => browser.evaluate(([was, now]) => {
  const db = (window as unknown as Mock).__yuliusMock;
  const account = db.accounts.find((a) => a.label === was);
  if (!account) throw new Error(`no subscription named ${was}`);
  account.label = now;
  for (const socket of db.eventSockets) socket.push(JSON.stringify({ type: 'account.updated', account }));
}, [from, to]);

/** From now on the list's reads wait, as on a slow connection, until `__yuliusMock.release()`. */
const holdListReads = (browser: Browser) => browser.evaluate(() => {
  const db = (window as unknown as Mock).__yuliusMock;
  db.holdPath = '/api/admin/accounts';
  db.released = false;
  db.held = new Promise<void>((resolve) => { db.release = () => { db.released = true; resolve(); }; });
});

/** How often the page has read the list (counted as it asks). */
const listReads = (browser: Browser) => browser.evaluate(() => (window as unknown as Mock).__yuliusMock.asked?.['/api/admin/accounts'] ?? 0);

/** What the mock's server holds for the subscription named `label`. */
const stored = (browser: Browser, label: string) => browser.evaluate((name) => {
  const account = (window as unknown as Mock).__yuliusMock.accounts.find((a) => a.label === name);
  return account ? { email: account.email, typed: Boolean(account.email_typed) } : null;
}, label);

test('each subscription shows the email of its Claude account, or that it is unknown', async ({ app, browser }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await expect(browser.locator('main .account-card').first()).toBeVisible();
  const shown = await cards(browser);
  const emailOf = (name: string) => shown.find((c) => c.name === name)?.email ?? null;
  expect(emailOf('Claude #2')).toBe('team@example.com');
  // SUB-004: no email read yet reads as unknown, never as the provider or an empty line.
  expect(emailOf('Claude spare')).toBe('email unknown');

  expect(await axeViolations(browser)).toEqual([]);
});

test('a subscription is renamed in place and the new name shows with its email everywhere', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await screen.getByRole('button', 'Rename Claude #2').click();
  const field = screen.getByRole('textbox', 'New name for Claude #2');
  await expect(field).toBeFocused();

  // SUB-001: an empty name is refused and the field stays open.
  await field.fill('   ');
  await field.press('Enter');
  await expect(screen.getByText('Give the subscription a name.')).toBeVisible();
  await expect(field).toBeFocused();
  // Leaving the refused field does not pull focus back into it.
  await browser.locator('main h1').click();
  expect(await browser.evaluate(() => document.activeElement?.classList.contains('account-rename-input') ?? null)).toBe(false);
  await expect(field).toBeVisible();
  // Escape keeps the old name, also after a real name was typed (its blur must not save it),
  // and gives the keyboard back to the pencil.
  await field.fill('typed, then cancelled');
  await field.press('Escape');
  await expect(browser.locator('main .account-card:has-text("Claude #2") .account-name strong')).toHaveText('Claude #2');
  await expect(screen.getByRole('button', 'Rename Claude #2')).toBeFocused();
  expect(await browser.evaluate(() => document.body.innerText.includes('typed, then cancelled'))).toBe(false);

  // A click on the name edits it too; Enter saves.
  await browser.locator('main .account-card:has-text("Claude #2") .account-name-text').click();
  await screen.getByRole('textbox', 'New name for Claude #2').fill('  work Max ');
  await screen.getByRole('textbox', 'New name for Claude #2').press('Enter');
  await expect(screen.getByText('Subscription renamed to work Max.')).toBeVisible();
  await expect(screen.getByRole('button', 'Rename work Max')).toBeFocused();
  await expect(browser.locator('main .account-card:has-text("work Max") .account-sub')).toHaveText('team@example.com');
  expect((await cards(browser)).map((c) => c.name)).not.toContain('Claude #2');

  // The cancelled name was never sent: the subscription is not named that anywhere.
  expect((await cards(browser)).map((c) => c.name)).not.toContain('typed, then cancelled');
  // SUB-005: the Status panel and Task details name it the new way, with its email.
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold:has-text("work Max") .account-sub')).toHaveText('team@example.com');
  await browser.keyboard.press('Escape');
  await browser.evaluate(() => { window.location.hash = '#/s/studio/tasks/14'; });
  await expect(screen.getByText('work Max · team@example.com').first()).toBeVisible();
  await expect(browser.locator('main')).not.toContainText('Claude #2');

  expect(await axeViolations(browser)).toEqual([]);
});

test("leaving one subscription's name for another's Rename keeps the second one open", async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await screen.getByRole('button', 'Rename Claude #1').click();
  await expect(screen.getByRole('textbox', 'New name for Claude #1')).toBeFocused();
  // Unchanged, then saved by leaving: neither takes the keyboard from the next field.
  await screen.getByRole('button', 'Rename Claude #4').click();
  await expect(screen.getByRole('textbox', 'New name for Claude #4')).toBeFocused();
  await screen.getByRole('textbox', 'New name for Claude #4').fill('personal');
  await screen.getByRole('button', 'Rename Claude #5').click();
  const third = screen.getByRole('textbox', 'New name for Claude #5');
  await expect(third).toBeFocused();
  await expect(screen.getByText('Subscription renamed to personal.')).toBeVisible();
  await expect(third).toBeFocused();
  await expect(browser.locator('main .account-card:has-text("Claude #1") .account-name strong')).toHaveText('Claude #1');
  await third.press('Escape');
  await expect(screen.getByRole('button', 'Rename Claude #5')).toBeFocused();
});

// The email line is typed in place like the name (SPEC-463): production's subscriptions read
// "email unknown", because Claude gives no email to a `setup-token` sign-in.

test("a subscription's email is typed in place under its name: \"Agent TEAM\" over its address", async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  const emailLine = (name: string) => browser.locator(`main .account-card:has-text("${name}") .account-sub`);
  await screen.getByRole('button', 'Change the email of Claude spare').click();
  const field = screen.getByRole('textbox', 'Email of Claude spare');
  await expect(field).toBeFocused();
  await expect(field).toHaveValue('');

  // EML-001: what is not an address is refused before anything is sent, and the field stays open.
  await field.fill('not an address');
  await field.press('Enter');
  await expect(screen.getByText('Type an address like name@example.com.')).toBeVisible();
  await expect(field).toBeFocused();
  expect(await axeViolations(browser)).toEqual([]);
  // Leaving the refused field does not pull focus back into it.
  await browser.locator('main h1').click();
  expect(await browser.evaluate(() => document.activeElement?.classList.contains('account-email-input') ?? null)).toBe(false);
  await expect(field).toBeVisible();
  // Escape keeps it unknown, also after an address was typed (its blur must not save it), and
  // gives the keyboard back to the pencil.
  await field.fill('typed@example.com');
  await field.press('Escape');
  await expect(emailLine('Claude spare')).toHaveText('email unknown');
  await expect(screen.getByRole('button', 'Change the email of Claude spare')).toBeFocused();
  // Neither the refused value nor the cancelled one was sent.
  expect(await sentFor(browser, 'acc_x3')).toBe(0);

  // A click on "email unknown" edits it too; Enter saves the trimmed address.
  await emailLine('Claude spare').click();
  await screen.getByRole('textbox', 'Email of Claude spare').fill('  agent@example.com ');
  await screen.getByRole('textbox', 'Email of Claude spare').press('Enter');
  await expect(screen.getByText('Email of Claude spare is now agent@example.com.')).toBeVisible();
  await expect(screen.getByRole('button', 'Change the email of Claude spare')).toBeFocused();
  await expect(emailLine('Claude spare')).toHaveText('agent@example.com');
  expect(await stored(browser, 'Claude spare')).toEqual({ email: 'agent@example.com', typed: true });
  expect(await sentFor(browser, 'acc_x3')).toBe(1);

  // The name above it is edited the same way: the card reads "Agent TEAM" over its address.
  await screen.getByRole('button', 'Rename Claude spare').click();
  await screen.getByRole('textbox', 'New name for Claude spare').fill('Agent TEAM');
  await screen.getByRole('textbox', 'New name for Claude spare').press('Enter');
  await expect(screen.getByText('Subscription renamed to Agent TEAM.')).toBeVisible();
  expect((await cards(browser)).filter((c) => c.name === 'Agent TEAM')).toEqual([{ name: 'Agent TEAM', email: 'agent@example.com' }]);

  // EML-005: every view that names the subscription shows what was typed; only this page edits it.
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold:has-text("Agent TEAM") .account-sub')).toHaveText('agent@example.com');
  expect(await browser.evaluate(() => document.querySelectorAll('.status-pop .account-rename').length)).toBe(0);
  await browser.keyboard.press('Escape');

  // EML-002: the field holds the email. Emptied and left, it removes nothing: a slip must not
  // lose the address.
  await screen.getByRole('button', 'Change the email of Agent TEAM').click();
  await expect(screen.getByRole('textbox', 'Email of Agent TEAM')).toHaveValue('agent@example.com');
  await screen.getByRole('textbox', 'Email of Agent TEAM').fill('');
  await browser.locator('main h1').click();
  await expect(screen.getByRole('textbox', 'Email of Agent TEAM')).toBeHidden();
  await expect(emailLine('Agent TEAM')).toHaveText('agent@example.com');
  expect(await stored(browser, 'Agent TEAM')).toEqual({ email: 'agent@example.com', typed: true });
  // Enter in the empty field removes it: unknown again.
  await screen.getByRole('button', 'Change the email of Agent TEAM').click();
  const again = screen.getByRole('textbox', 'Email of Agent TEAM');
  await again.fill('');
  await again.press('Enter');
  await expect(screen.getByText('Email of Agent TEAM removed.')).toBeVisible();
  await expect(emailLine('Agent TEAM')).toHaveText('email unknown');
  expect(await stored(browser, 'Agent TEAM')).toEqual({ email: null, typed: false });

  expect(await axeViolations(browser)).toEqual([]);
});

test("leaving a subscription's email for another field saves it and keeps that field open", async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  // An email Claude gave is changed the same way; unchanged, it is not sent.
  await screen.getByRole('button', 'Change the email of Claude #1').click();
  await expect(screen.getByRole('textbox', 'Email of Claude #1')).toHaveValue('mkarp@example.com');
  // From the email to the name of the same card: the email closes as it was, and nothing is
  // sent, so an email Claude gave is not marked as typed.
  await screen.getByRole('button', 'Rename Claude #1').click();
  await expect(screen.getByRole('textbox', 'New name for Claude #1')).toBeFocused();
  await expect(browser.locator('main .account-card:has-text("mkarp@example.com") .account-sub')).toHaveText('mkarp@example.com');
  expect(await sentFor(browser, 'acc_c1')).toBe(0);
  expect(await stored(browser, 'Claude #1')).toEqual({ email: 'mkarp@example.com', typed: false });
  // From a typed email to another card's email: saved by leaving, and the update it causes
  // does not redraw the second field away.
  await screen.getByRole('button', 'Change the email of Claude #4').click();
  await screen.getByRole('textbox', 'Email of Claude #4').fill('personal@example.com');
  await screen.getByRole('button', 'Change the email of Claude #5').click();
  const next = screen.getByRole('textbox', 'Email of Claude #5');
  await expect(next).toBeFocused();
  await expect(next).toHaveValue('studio@example.com');
  await next.fill('half-typed@');
  await expect(screen.getByText('Email of Claude #4 is now personal@example.com.')).toBeVisible();
  // EML-001: the update of Claude #4 that arrived meanwhile neither closed nor saved this field.
  await expect(browser.locator('main .account-card:has-text("Claude #4") .account-sub')).toHaveText('personal@example.com');
  await expect(next).toBeFocused();
  await expect(next).toHaveValue('half-typed@');
  await next.press('Escape');
  await expect(screen.getByRole('button', 'Change the email of Claude #5')).toBeFocused();
  // Nothing was sent for the emails left unchanged or cancelled; one change for the one typed.
  expect([await sentFor(browser, 'acc_c1'), await sentFor(browser, 'acc_x1'), await sentFor(browser, 'acc_x2')]).toEqual([0, 1, 0]);
  expect((await cards(browser)).map((c) => c.email)).toEqual(['mkarp@example.com', 'team@example.com', 'ops@example.com', 'personal@example.com', 'studio@example.com', 'email unknown']);
});

/** Whether the list says a refresh waits for an open field (`data-refresh="waiting"`). */
const refreshWaits = (browser: Browser) => browser.locator('main [data-refresh="waiting"]');

test('a field opened while the list is read again is neither closed nor saved by that read', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await holdListReads(browser);

  // The email is saved; its update makes the page read the list again. (Claude #4's usage was
  // read minutes ago, so the page asks for none that could answer while its field is open.)
  await screen.getByRole('button', 'Change the email of Claude #4').click();
  await screen.getByRole('textbox', 'Email of Claude #4').fill('agent@example.com');
  await screen.getByRole('textbox', 'Email of Claude #4').press('Enter');
  await expect(screen.getByText('Email of Claude #4 is now agent@example.com.')).toBeVisible();
  // The person goes on to the name while that read is on its way. Meanwhile another
  // administrator renames another subscription: the read carries it.
  await screen.getByRole('button', 'Rename Claude #4').click();
  const name = screen.getByRole('textbox', 'New name for Claude #4');
  await name.fill('Agent TE');
  await expect(refreshWaits(browser)).toHaveCount(0);
  await browser.evaluate(() => {
    const db = (window as unknown as Mock).__yuliusMock;
    const other = db.accounts.find((a) => a.label === 'Claude #2');
    if (other) other.label = 'renamed elsewhere';
    db.release();
  });
  // The read has landed, and the list says it waits: the field is as it was, nothing was sent.
  await expect(refreshWaits(browser)).toHaveCount(1);
  await expect(name).toBeFocused();
  await expect(name).toHaveValue('Agent TE');
  expect(await sentFor(browser, 'acc_x1')).toBe(1);
  expect((await cards(browser)).map((c) => c.name)).not.toContain('renamed elsewhere');

  // The name is saved by leaving the field. No focus leaves the list after that, and still the
  // list is read again: both lines from the server, and what the held read carried.
  await name.fill('Agent TEAM');
  await browser.locator('main h1').click();
  await expect(screen.getByText('Subscription renamed to Agent TEAM.')).toBeVisible();
  await expect(refreshWaits(browser)).toHaveCount(0);
  await expect.poll(async () => (await cards(browser)).map((c) => c.name)).toContain('renamed elsewhere');
  expect((await cards(browser)).filter((c) => c.name === 'Agent TEAM')).toEqual([{ name: 'Agent TEAM', email: 'agent@example.com' }]);
});

test('a name saved with Tab leaves the keyboard on that subscription, wherever its card moves', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  // Claude #4 is the fourth card; as "Agent TEAM" it sorts first.
  await screen.getByRole('button', 'Rename Claude #4').click();
  const name = screen.getByRole('textbox', 'New name for Claude #4');
  await expect(name).toBeFocused();
  await name.fill('Agent TEAM');
  await name.press('Tab');
  await expect(screen.getByText('Subscription renamed to Agent TEAM.')).toBeVisible();
  // The list is read again once the save has settled, and sorted by name.
  await expect.poll(async () => (await cards(browser))[0]?.name ?? null).toBe('Agent TEAM');
  // The keyboard is where Tab put it, on this subscription's email, not on the control that
  // now sits at that place in the list (Claude #3's "Sign in again").
  await expect(screen.getByRole('button', 'Change the email of Agent TEAM')).toBeFocused();
  await browser.keyboard.press('Enter');
  const email = screen.getByRole('textbox', 'Email of Agent TEAM');
  await expect(email).toBeFocused();
  await expect(email).toHaveValue('pawel@example.com');
  await email.fill('agent@example.com');
  await email.press('Enter');
  await expect(screen.getByText('Email of Agent TEAM is now agent@example.com.')).toBeVisible();
  expect(await stored(browser, 'Agent TEAM')).toEqual({ email: 'agent@example.com', typed: true });
  // Both changes went to that subscription. Beside the page's own usage reads nothing else was
  // sent: no other subscription was changed and no sign-in was started.
  expect(await sentFor(browser, 'acc_x1')).toBe(2);
  expect((await sentPaths(browser)).filter((path) => !path.endsWith('/usage'))).toEqual(['PATCH /api/admin/accounts/acc_x1']);

  // Shift+Tab the other way: from the last card's name back to the Remove of the card before
  // it. That card moves down when this one becomes the first; the keyboard stays on its Remove.
  const focused = () => browser.evaluate(() => {
    const control = document.activeElement;
    return {
      control: control?.getAttribute('aria-label') ?? control?.textContent ?? null,
      of: control?.closest('.card')?.querySelector('.account-name strong')?.textContent ?? null,
    };
  });
  await screen.getByRole('button', 'Rename Claude spare').click();
  const last = screen.getByRole('textbox', 'New name for Claude spare');
  await expect(last).toBeFocused();
  await last.fill('A spare');
  await last.press('Shift+Tab');
  await expect(screen.getByText('Subscription renamed to A spare.')).toBeVisible();
  await expect.poll(async () => (await cards(browser))[0]?.name ?? null).toBe('A spare');
  await expect.poll(focused).toEqual({ control: 'Remove', of: 'Claude #5' });
});

test('with the mouse: the name, then the pencil of the email under it; both are saved', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await screen.getByRole('button', 'Rename Claude #4').click();
  await screen.getByRole('textbox', 'New name for Claude #4').fill('Agent TEAM');
  // The press on the email's pencil leaves the name, which is saved meanwhile.
  await screen.getByRole('button', 'Change the email of Claude #4').click();
  const email = screen.getByRole('textbox', /^Email of /);
  await expect(email).toBeFocused();
  await expect(screen.getByText('Subscription renamed to Agent TEAM.')).toBeVisible();
  await expect(email).toBeFocused();
  await email.fill('agent@example.com');
  await email.press('Enter');
  await expect(screen.getByText('Email of Agent TEAM is now agent@example.com.')).toBeVisible();
  await expect(screen.getByRole('button', 'Change the email of Agent TEAM')).toBeFocused();
  expect(await stored(browser, 'Agent TEAM')).toEqual({ email: 'agent@example.com', typed: true });
  expect(await sentFor(browser, 'acc_x1')).toBe(2);
  expect((await cards(browser)).filter((c) => c.name === 'Agent TEAM')).toEqual([{ name: 'Agent TEAM', email: 'agent@example.com' }]);
});

test("an email saved with Tab leaves the keyboard on that card's next control", async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  // Claude #3 needs a new sign-in: the control after its email is "Sign in again".
  await screen.getByRole('button', 'Change the email of Claude #3').click();
  const email = screen.getByRole('textbox', 'Email of Claude #3');
  await expect(email).toBeFocused();
  await email.fill('ops-team@example.com');
  await email.press('Tab');
  await expect(screen.getByText('Email of Claude #3 is now ops-team@example.com.')).toBeVisible();
  await expect(refreshWaits(browser)).toHaveCount(0);
  await expect.poll(() => browser.evaluate(() => {
    const control = document.activeElement;
    return { control: control?.textContent ?? null, of: control?.closest('.card')?.querySelector('.account-name strong')?.textContent ?? null };
  })).toEqual({ control: 'Sign in again', of: 'Claude #3' });
  expect(await sentFor(browser, 'acc_c3')).toBe(1);
});

test('at phone width a long address is cut short of its pencil, which stays on its line', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 360, height: 780 });
  await openSubscriptions(app, browser);
  const long = 'firstname.lastname+agents-team@subdomain.example.com';
  await screen.getByRole('button', 'Change the email of Claude #4').click();
  const email = screen.getByRole('textbox', 'Email of Claude #4');
  await expect(email).toBeFocused();
  await email.fill(long);
  await email.press('Enter');
  await expect(screen.getByText(`Email of Claude #4 is now ${long}.`)).toBeVisible();
  const line = await browser.evaluate(() => {
    const card = [...document.querySelectorAll('main .account-card')].find((c) => c.textContent?.includes('Claude #4'));
    const text = card?.querySelector('.account-email-text');
    const pencil = card?.querySelector('.account-email-edit .account-rename');
    if (!card || !text || !pencil) return null;
    const [c, t, p] = [card, text, pencil].map((el) => el.getBoundingClientRect());
    return {
      sameLine: p.top < t.bottom && p.bottom > t.top,
      pencilAfterText: p.left >= t.right,
      pencilInCard: p.right <= c.right + 0.5,
      cut: text.scrollWidth > text.clientWidth,
      pageScrolls: document.documentElement.scrollWidth > document.documentElement.clientWidth,
    };
  });
  expect(line).toEqual({ sameLine: true, pencilAfterText: true, pencilInCard: true, cut: true, pageScrolls: false });
  // The whole address is in the field, and a refusal fits the card too.
  await screen.getByRole('button', 'Change the email of Claude #4').click();
  const again = screen.getByRole('textbox', 'Email of Claude #4');
  await expect(again).toHaveValue(long);
  await again.fill('not an address');
  await again.press('Enter');
  await expect(screen.getByText('Type an address like name@example.com.')).toBeVisible();
  expect(await browser.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth)).toBe(false);
  expect(await axeViolations(browser)).toEqual([]);
});

test("a subscription's usage answer that lands after a save leaves the keyboard on its pencil", async ({ app, browser, screen }) => {
  // Claude spare's usage is days old, so the page asks for it whenever it draws the list (an
  // idle subscription's usage stays old, so on a real instance that is every time). The
  // answers wait here until the test lets them go.
  await app.open(mock('/admin/accounts', '&ambient=0&holdpost=/api/admin/accounts/acc_x3/usage'));
  await firstShown(browser.locator('main .account-card').first());
  await screen.getByRole('button', 'Rename Claude spare').click();
  const name = screen.getByRole('textbox', 'New name for Claude spare');
  await expect(name).toBeFocused();
  await name.fill('A spare');
  await name.press('Enter');
  await expect(screen.getByText('Subscription renamed to A spare.')).toBeVisible();
  // The list is drawn again (the card is first now) and asks for the usage once more.
  await expect.poll(async () => (await cards(browser))[0]?.name ?? null).toBe('A spare');
  await expect(screen.getByRole('button', 'Rename A spare')).toBeFocused();
  const foot = browser.locator('main .account-card:has-text("A spare") .account-foot');
  await expect(foot).toContainText('Updated 3 d ago');
  await expect.poll(() => browser.evaluate(() => (window as unknown as Mock).__yuliusMock.sent?.['POST /api/admin/accounts/acc_x3/usage'] ?? 0)).toBe(2);
  // The answers land and the card's head is drawn again: the keyboard is still on the pencil,
  // so Tab goes on to the email under the name.
  await browser.evaluate(() => (window as unknown as Mock).__yuliusMock.release());
  await expect(foot).toContainText('Updated just now');
  await expect(screen.getByRole('button', 'Rename A spare')).toBeFocused();
  await browser.keyboard.press('Tab');
  await expect(screen.getByRole('button', 'Change the email of A spare')).toBeFocused();
  await browser.keyboard.press('Enter');
  await expect(screen.getByRole('textbox', 'Email of A spare')).toBeFocused();
});

test('a refresh that waited for an open field is read when the field closes without a save', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await eventsConnected(browser);
  await screen.getByRole('button', 'Change the email of Claude #4').click();
  const email = screen.getByRole('textbox', 'Email of Claude #4');
  await expect(email).toBeFocused();
  await email.fill('half-typed@');
  await renamedElsewhere(browser, 'Claude #2', 'renamed elsewhere');
  await expect(refreshWaits(browser)).toHaveCount(1);
  await expect(email).toBeFocused();
  await expect(email).toHaveValue('half-typed@');
  expect((await cards(browser)).map((c) => c.name)).not.toContain('renamed elsewhere');
  // Cancelled with Escape: nothing is sent, the list is read again, and the keyboard is on
  // this subscription's pencil although its card moved up.
  await email.press('Escape');
  await expect.poll(async () => (await cards(browser)).map((c) => c.name)).toContain('renamed elsewhere');
  await expect(refreshWaits(browser)).toHaveCount(0);
  await expect(screen.getByRole('button', 'Change the email of Claude #4')).toBeFocused();
  expect(await sentFor(browser, 'acc_x1')).toBe(0);

  // The same when the field closes because nothing was changed (Enter on the value as it was).
  await screen.getByRole('button', 'Rename Claude #4').click();
  const name = screen.getByRole('textbox', 'New name for Claude #4');
  await expect(name).toBeFocused();
  await renamedElsewhere(browser, 'Claude #1', 'also renamed');
  await expect(refreshWaits(browser)).toHaveCount(1);
  await name.press('Enter');
  await expect.poll(async () => (await cards(browser)).map((c) => c.name)).toContain('also renamed');
  await expect(refreshWaits(browser)).toHaveCount(0);
  await expect(screen.getByRole('button', 'Rename Claude #4')).toBeFocused();
  expect(await sentFor(browser, 'acc_x1')).toBe(0);
});

test('a refresh that waited is read when its field is left as it was: by a click, by Tab, emptied', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await eventsConnected(browser);
  const names = async () => (await cards(browser)).map((c) => c.name);

  // Left by a click outside the list, unchanged.
  await screen.getByRole('button', 'Change the email of Claude #4').click();
  await expect(screen.getByRole('textbox', 'Email of Claude #4')).toBeFocused();
  await renamedElsewhere(browser, 'Claude #2', 'renamed elsewhere');
  await expect(refreshWaits(browser)).toHaveCount(1);
  await browser.locator('main h1').click();
  await expect.poll(names).toContain('renamed elsewhere');
  await expect(refreshWaits(browser)).toHaveCount(0);

  // Left with Tab, unchanged: the keyboard goes on to the email's pencil and stays there.
  await screen.getByRole('button', 'Rename Claude #4').click();
  const name = screen.getByRole('textbox', 'New name for Claude #4');
  await expect(name).toBeFocused();
  await renamedElsewhere(browser, 'Claude #1', 'also renamed');
  await expect(refreshWaits(browser)).toHaveCount(1);
  await name.press('Tab');
  await expect.poll(names).toContain('also renamed');
  await expect(refreshWaits(browser)).toHaveCount(0);
  await expect(screen.getByRole('button', 'Change the email of Claude #4')).toBeFocused();

  // Emptied and left: nothing is removed (EML-002), and the list is read again all the same.
  await browser.keyboard.press('Enter');
  const email = screen.getByRole('textbox', 'Email of Claude #4');
  await expect(email).toBeFocused();
  await email.fill('');
  await renamedElsewhere(browser, 'Claude #5', 'third rename');
  await expect(refreshWaits(browser)).toHaveCount(1);
  await browser.locator('main h1').click();
  await expect.poll(names).toContain('third rename');
  await expect(refreshWaits(browser)).toHaveCount(0);
  expect(await stored(browser, 'Claude #4')).toEqual({ email: 'pawel@example.com', typed: false });
  expect(await sentFor(browser, 'acc_x1')).toBe(0);
});

test("the list read after a subscription was removed does not redraw a field opened meanwhile", async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await holdListReads(browser);
  // The person removes Claude #5: the page reads the list again, and that read is slow.
  await browser.locator('main .card:has-text("studio@example.com") button:has-text("Remove")').click();
  await screen.getByRole('dialog', 'Remove Claude #5?').getByRole('button', 'Remove').click();
  await expect(screen.getByText('Subscription Claude #5 removed.')).toBeVisible();
  await expect.poll(() => listReads(browser)).toBe(2);
  // Meanwhile they start on another subscription's email.
  await screen.getByRole('button', 'Change the email of Claude spare').click();
  const email = screen.getByRole('textbox', 'Email of Claude spare');
  await expect(email).toBeFocused();
  await email.fill('agent@exam');
  // The read lands: the half-typed address is neither drawn away nor saved.
  await browser.evaluate(() => (window as unknown as Mock).__yuliusMock.release());
  await expect(refreshWaits(browser)).toHaveCount(1);
  await expect(email).toBeFocused();
  await expect(email).toHaveValue('agent@exam');
  expect(await sentFor(browser, 'acc_x3')).toBe(0);
  expect(await stored(browser, 'Claude spare')).toEqual({ email: null, typed: false });
  // Finished: saved once, and the list shows the removal.
  await email.fill('agent@example.com');
  await email.press('Enter');
  await expect(screen.getByText('Email of Claude spare is now agent@example.com.')).toBeVisible();
  await expect.poll(async () => (await cards(browser)).map((c) => c.name)).not.toContain('Claude #5');
  await expect(refreshWaits(browser)).toHaveCount(0);
  expect(await stored(browser, 'Claude spare')).toEqual({ email: 'agent@example.com', typed: true });
  expect(await sentFor(browser, 'acc_x3')).toBe(1);
});

test('a press on a control is delivered before the field it leaves does its work', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await screen.getByRole('button', 'Rename Claude #4').click();
  const name = screen.getByRole('textbox', 'New name for Claude #4');
  await expect(name).toBeFocused();
  await name.fill('Agent TEAM');
  const pencil = await browser.evaluate(() => {
    const control = document.querySelector('main [data-focus-key="account:acc_x1:email"]');
    // The click on the name's pencil scrolled only that pencil into view; this one is a line lower.
    control?.scrollIntoView({ block: 'center' });
    const box = control?.getBoundingClientRect();
    return box ? { x: box.left + box.width / 2, y: box.top + box.height / 2 } : null;
  });
  if (!pencil) throw new Error("Claude #4's email pencil is not on the page");
  // What is under the pointer: the control (by its key) or, failing that, the element.
  const under = () => browser.evaluate(([x, y]) => {
    const el = document.elementFromPoint(x, y);
    const keyed = el?.closest('[data-focus-key]');
    return keyed ? keyed.getAttribute('data-focus-key') : `${el?.tagName ?? 'nothing'}.${el?.className ?? ''} "${(el?.textContent ?? '').trim().slice(0, 40)}"`;
  }, [pencil.x, pencil.y]);
  await browser.mouse.move(pencil.x, pencil.y);
  expect(await under()).toBe('account:acc_x1:email');
  // The button goes down on the email's pencil and stays down, as it does for a tenth of a
  // second in a person's click. The name's field has lost the keyboard, but it neither saves
  // nor closes yet: what it does then (a refusal's line, the list read again after a save)
  // could move or replace the control under the pointer, and the click would be lost.
  await browser.mouse.down();
  await expect.poll(() => browser.evaluate(() => document.activeElement?.getAttribute('data-focus-key') ?? null)).toBe('account:acc_x1:email');
  expect(await sentFor(browser, 'acc_x1')).toBe(0);
  await expect(name).toBeVisible();
  expect(await under()).toBe('account:acc_x1:email');
  await browser.mouse.up();
  // The click arrived: the email's field is open. Then the name is saved.
  const email = screen.getByRole('textbox', /^Email of /);
  await expect(email).toBeFocused();
  await expect(screen.getByText('Subscription renamed to Agent TEAM.')).toBeVisible();
  await expect(email).toBeFocused();
  await expect(refreshWaits(browser)).toHaveCount(1);
  await email.fill('agent@example.com');
  await email.press('Enter');
  await expect(screen.getByText('Email of Agent TEAM is now agent@example.com.')).toBeVisible();
  await expect.poll(async () => (await cards(browser))[0] ?? null).toEqual({ name: 'Agent TEAM', email: 'agent@example.com' });
  await expect(refreshWaits(browser)).toHaveCount(0);
  expect(await sentFor(browser, 'acc_x1')).toBe(2);
});

test("a click on a card's button arrives although leaving a field shows or removes its refusal (#1174)", async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  const remove = browser.locator('main .card:has-text("pawel@example.com") button:has-text("Remove")');
  const dialog = screen.getByRole('dialog', 'Remove Claude #4?');

  // The name emptied, then Remove: the refusal's line appears under the field as it is left,
  // and the button is where it was when the click ends.
  await screen.getByRole('button', 'Rename Claude #4').click();
  const name = screen.getByRole('textbox', 'New name for Claude #4');
  await expect(name).toBeFocused();
  await name.fill('   ');
  await remove.click();
  await expect(dialog).toBeVisible();
  await expect(screen.getByText('Give the subscription a name.')).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(name).toBeVisible();

  // The other way round: the field shows a refusal and is left with its value as it was, so it
  // closes and the refusal's line goes.
  await name.fill('Claude #4');
  await remove.click();
  await expect(dialog).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(name).toBeHidden();
  await expect(screen.getByText('Give the subscription a name.')).toBeHidden();
  await expect(browser.locator('main .account-card:has-text("pawel@example.com") .account-name strong')).toHaveText('Claude #4');

  // The same under the email: what is not an address is refused as the field is left.
  await screen.getByRole('button', 'Change the email of Claude #4').click();
  const email = screen.getByRole('textbox', 'Email of Claude #4');
  await expect(email).toBeFocused();
  await email.fill('not an address');
  await browser.locator('main .card:has-text("Claude #4") button:has-text("Remove")').click();
  await expect(dialog).toBeVisible();
  await expect(screen.getByText('Type an address like name@example.com.')).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(email).toBeVisible();
  // Nothing was sent or removed by any of it.
  expect(await sentFor(browser, 'acc_x1')).toBe(0);
  expect((await cards(browser)).length).toBe(6);
});

test("a field's late claim on the keyboard does not take it back from where Tab went", async ({ app, browser, screen }) => {
  // The name's save waits here until the test lets it go.
  await app.open(mock('/admin/accounts', '&ambient=0&holdpost=/api/admin/accounts/acc_x1'));
  await firstShown(browser.locator('main .account-card').first());
  // A field takes the keyboard a moment after it opens (a 0 ms timer). On a busy page that
  // moment can come after the person's first keys: hold such timers back until asked.
  type Timers = { __heldTimers: (() => void)[]; __holdTimers: boolean };
  await browser.evaluate(() => {
    const page = window as unknown as Timers;
    const real = window.setTimeout.bind(window);
    page.__heldTimers = [];
    page.__holdTimers = true;
    window.setTimeout = ((run: (...args: unknown[]) => void, ms?: number, ...args: unknown[]) => {
      if (page.__holdTimers && !ms) { page.__heldTimers.push(() => run(...args)); return 0; }
      return real(run, ms, ...args);
    }) as typeof window.setTimeout;
  });
  await screen.getByRole('button', 'Rename Claude #4').click();
  const name = screen.getByRole('textbox', 'New name for Claude #4');
  await expect(name).toBeVisible();
  await browser.evaluate(() => { (window as unknown as Timers).__holdTimers = false; });
  // The person is faster: the name and Tab, which starts the save (held) and moves on.
  await name.fill('Agent TEAM');
  await name.press('Tab');
  await expect(screen.getByRole('button', 'Change the email of Claude #4')).toBeFocused();
  // Now the field's moment comes, while its save is on its way: the keyboard stays.
  await browser.evaluate(() => { (window as unknown as Timers).__heldTimers.splice(0).forEach((run) => run()); });
  await expect(screen.getByRole('button', 'Change the email of Claude #4')).toBeFocused();
  await browser.evaluate(() => (window as unknown as Mock).__yuliusMock.release());
  await expect(screen.getByText('Subscription renamed to Agent TEAM.')).toBeVisible();
  await expect.poll(async () => (await cards(browser))[0]?.name ?? null).toBe('Agent TEAM');
  await expect(screen.getByRole('button', 'Change the email of Agent TEAM')).toBeFocused();
});

test('an email saved by leaving its field is drawn from the server: the initials follow it', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  const initials = browser.locator('main .account-card:has-text("Claude spare") .account-avatar');
  await expect(initials).toHaveText('CS');
  // Typed and left: the save's update arrives while the field is still shown, and the field
  // that closes afterwards no longer has the focus.
  await screen.getByRole('button', 'Change the email of Claude spare').click();
  await screen.getByRole('textbox', 'Email of Claude spare').fill('agent@example.com');
  await browser.locator('main h1').click();
  await expect(screen.getByText('Email of Claude spare is now agent@example.com.')).toBeVisible();
  await expect(initials).toHaveText('AG');
  await expect(refreshWaits(browser)).toHaveCount(0);
});

test('a change the server refuses keeps the field open with the server\'s words, for the name and the email', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await screen.getByRole('button', 'Change the email of Claude #5').click();
  const email = screen.getByRole('textbox', 'Email of Claude #5');
  await expect(email).toBeFocused();
  // Another administrator removes the subscription meanwhile: the server no longer has it.
  await browser.evaluate(() => {
    const db = (window as unknown as Mock).__yuliusMock;
    db.accounts = db.accounts.filter((a) => a.label !== 'Claude #5');
  });
  const inCard = (selector: string) => browser.locator(`main .account-card:has-text("Claude #5") ${selector}`);
  await email.fill('studio-team@example.com');
  await email.press('Enter');
  await expect(inCard('[role="alert"]')).toHaveText('not found');
  await expect(email).toBeFocused();
  await expect(email).toHaveValue('studio-team@example.com');
  await email.press('Escape');
  await expect(inCard('.account-sub')).toHaveText('studio@example.com');

  await screen.getByRole('button', 'Rename Claude #5').click();
  const name = screen.getByRole('textbox', 'New name for Claude #5');
  await name.fill('studio');
  await name.press('Enter');
  // While its name is a field, the card is told by its email.
  await expect(browser.locator('main .account-card:has-text("studio@example.com") [role="alert"]')).toHaveText('not found');
  await expect(name).toBeFocused();
  await name.press('Escape');
  await expect(inCard('.account-name strong')).toHaveText('Claude #5');
});

test('an address is kept as it was typed, also with letters outside ASCII in its domain', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await screen.getByRole('button', 'Change the email of Claude spare').click();
  await screen.getByRole('textbox', 'Email of Claude spare').fill('jan@b\u00fccher.example');
  await screen.getByRole('textbox', 'Email of Claude spare').press('Enter');
  await expect(browser.locator('main .account-card:has-text("Claude spare") .account-sub')).toHaveText('jan@b\u00fccher.example');
  expect(await stored(browser, 'Claude spare')).toEqual({ email: 'jan@b\u00fccher.example', typed: true });
});

test('someone who does not manage subscriptions sees their names without emails (UI-022)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks/14', '&ambient=0&role=member'));
  await expect(screen.getByText('Claude #2').first()).toBeVisible();
  await expect(browser.locator('main')).not.toContainText('@example.com');
  await expect(browser.locator('main')).not.toContainText('email unknown');
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold:has-text("Claude #2") .account-sub')).toHaveText('Claude');
  expect(await browser.evaluate(() => document.querySelector('.status-pop')?.textContent?.includes('@') ?? null)).toBe(false);
});

test('a subscription without an email reads "email unknown" in Task details (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const lines = await browser.evaluate(async () => {
    const url = '/assets/format.js';
    const { accountRef } = await import(url);
    return [accountRef('personal', null), accountRef('work Max', 'pawel@example.com'), accountRef(null, null), accountRef('personal', null, { unknown: false })];
  });
  // An administrator's view says the email is unknown; anyone else's leaves it out (UI-022).
  expect(lines).toEqual(['personal · email unknown', 'work Max · pawel@example.com', null, 'personal']);
});
