import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Synthetic mock values (assets/mock.js, "secrets vault"); not real credentials.
const TEAM_VALUE = 'mock-not-a-real-password';
const AGENT_VALUE = 'mock-agent-value-1';
const MOCK_CODE = '123456';

/** Sets up two-factor in the dialog a first reveal opens (SPEC-040): a wrong code is refused. */
async function setUpTwoFactor(screen: any, browser: any) {
  const setup = screen.getByRole('dialog', 'Set up two-factor');
  await expect(setup).toBeVisible();
  await expect(setup.getByText(/Add this key to your authenticator app/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await expect(setup.getByRole('link', 'Open in an authenticator app')).toHaveAttribute('href', /^otpauth:\/\/totp\//);
  await screen.getByLabel('Code from your app').fill('000000');
  await setup.getByRole('button', 'Unlock').click();
  await expect(setup.getByRole('alert')).toHaveText(/wrong code/i);
  await screen.getByLabel('Code from your app').fill(MOCK_CODE);
  await setup.getByRole('button', 'Unlock').click();
  await expect(setup).toBeHidden();
}

// F 2026-10-12 (task #39): "It should be only value title notes. 2FA should be per wallet and
// should be mandatory." A team secret is a title, a value and notes; reading one needs your own
// two-factor code.
test('team secrets are a title, a value and notes, shown after a two-factor code', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Secrets');
  await expect(screen.getByText('Shared Gmail')).toBeVisible();
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), TEAM_VALUE)).toBe(false);
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Show Shared Gmail').click();
  await setUpTwoFactor(screen, browser);
  const shown = screen.getByRole('dialog', 'Shared Gmail');
  await expect(shown).toBeVisible();
  // The value stays masked until the eye is pressed (SPEC-114).
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), TEAM_VALUE)).toBe(false);
  await shown.getByRole('button', 'Show value').click();
  await expect(shown.getByText(TEAM_VALUE)).toBeVisible();
  await expect(shown.getByText(/Recovery codes/)).toBeVisible();
  await shown.getByRole('button', 'Done').click();
  await expect(shown).toBeHidden();
  // The setup and the reveal are in the activity trail.
  await expect(screen.getByText(/turned on two-factor/).first()).toBeVisible();
  await expect(screen.getByText(/revealed/).first()).toBeVisible();

  await screen.getByRole('button', 'Add team secret').click();
  const add = screen.getByRole('dialog', 'Add team secret');
  const labels = await browser.evaluate(() => [...document.querySelectorAll('dialog.modal .field-label')].map((l) => l.textContent!.trim()));
  expect(labels).toEqual(['Title', 'Value', 'Notes']);
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByLabel('Title').fill('AWS root');
  await screen.getByLabel('Value').fill('mock-aws-root-value');
  await add.getByRole('button', 'Add').click();
  await expect(add).toBeHidden();
  // Listed, and recorded in the activity trail ("added AWS root").
  await expect(screen.getByText('AWS root').first()).toBeVisible();
  await expect(screen.getByText(/added/).first()).toBeVisible();
  expect(await browser.evaluate(() => document.body.innerHTML.includes('mock-aws-root-value'))).toBe(false);
});

// F 2026-10-14 (task #107): "Too many options. It should be: key name (like VIDOC_API_KEY), value
// and description. Agent will find by itself the type, the adres etc" (SPEC-121).
test('an agent secret is a name, a value and a description, readable by its managers after a code', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=agents'));
  await expect(screen.getByRole('radio', 'Agent secrets')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByText('STRIPE_KEY')).toBeVisible();
  await expect(screen.getByText('api.stripe.com')).toBeVisible();
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), AGENT_VALUE)).toBe(false);
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Add agent secret').first().click();
  const dialog = screen.getByRole('dialog', 'Add agent secret');
  const visibleLabels = () => browser.evaluate(() => [...document.querySelectorAll('dialog.modal .field-label')].filter((l) => (l as HTMLElement).checkVisibility()).map((l) => l.textContent!.trim()));
  expect(await visibleLabels()).toEqual(['Name', 'Value', 'Description']);
  // An instance administrator finds the scope under More options, closed.
  await dialog.getByText('More options').click();
  expect(await visibleLabels()).toEqual(['Name', 'Value', 'Description', 'Scope']);
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByLabel('Name').fill('RESEND_KEY');
  await screen.getByLabel('Value').fill('short');
  await dialog.getByRole('button', 'Add').click();
  await expect(browser.locator('.field-error')).toHaveText(/At least 8 characters/);
  await screen.getByLabel('Value').fill('re_mock_value_123');
  await screen.getByLabel('Description').fill('Resend API key for transactional mail');
  await dialog.getByRole('button', 'Add').click();
  await expect(dialog).toBeHidden();
  await expect(screen.getByText('RESEND_KEY')).toBeVisible();
  // Bound to no host yet: agents find it and a person allows it.
  const card = browser.locator('.item-card:has-text("RESEND_KEY")');
  await expect(card.getByText(/Not sent anywhere yet/)).toBeVisible();
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), 're_mock_value_123')).toBe(false);
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), AGENT_VALUE)).toBe(false);

  // Editing keeps the hosts; the request style shows only for a secret that has one.
  await screen.getByRole('button', 'Edit RESEND_KEY').click();
  const edit = screen.getByRole('dialog', 'Edit RESEND_KEY');
  expect(await visibleLabels()).toEqual(['Name', 'Value', 'Description', 'Hosts']);
  await edit.getByRole('button', 'Cancel').click();
  await screen.getByRole('button', 'Edit STRIPE_KEY').click();
  const stripe = screen.getByRole('dialog', 'Edit STRIPE_KEY');
  expect(await visibleLabels()).toEqual(['Name', 'Value', 'Description', 'Hosts', 'Added to requests as']);
  await stripe.getByRole('button', 'Cancel').click();

  // Its manager reads it back after a two-factor code.
  await screen.getByRole('button', 'Show STRIPE_KEY').click();
  await setUpTwoFactor(screen, browser);
  const shown = screen.getByRole('dialog', 'STRIPE_KEY');
  await expect(shown.getByRole('button', 'Show value')).toHaveAttribute('aria-pressed', 'false');
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), AGENT_VALUE)).toBe(false);
  await shown.getByRole('button', 'Show value').click();
  await expect(shown.getByText(AGENT_VALUE)).toBeVisible();
  await shown.getByRole('button', 'Done').click();

  // The section survives a reload; the Team section is one click away.
  await browser.reload();
  await expect(screen.getByRole('radio', 'Agent secrets')).toHaveAttribute('aria-checked', 'true');
  await screen.getByRole('radio', 'Team secrets').click();
  await expect(screen.getByText('Shared Gmail')).toBeVisible();
});

/** A synthetic key file: armour built at run time (no scanner takes the source for a key). */
function keyFile(label: string, body: string[], breakText: string): string {
  return [`-----BEGIN ${label}-----`, ...body, `-----END ${label}-----`].join(breakText);
}

/** Pastes `text` into the open dialog's Value field as the clipboard would; its value after. */
function pasteValue(browser: any, text: string) {
  return browser.evaluate((t: string) => {
    const input = document.querySelector('dialog.modal input[aria-label="Value"]') as HTMLInputElement;
    input.focus();
    const data = new DataTransfer();
    data.setData('text/plain', t);
    input.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
    return input.value;
  }, text);
}

// Task #179 (SPEC-193, #591): "Creating multiline keys fails. Make sure value can be multiline".
// The Value field is one line, so a pasted key keeps its line breaks as \n (the vault's form for a
// multi-line key); a key stored flattened before is marked for its managers to paste again.
test('a pasted multi-line key keeps its line breaks as \\n, and a flattened one asks to be pasted again', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=agents'));
  const pemBody = ['bW9jay1wZW0tbGluZS0wMQ==', 'bW9jay1wZW0tbGluZS0wMg=='];
  const sshBody = ['bW9jay1zc2gtbGluZS0wMQ==', 'bW9jay1zc2gtbGluZS0wMg==', 'bW9jay1zc2gtbGluZS0wMw=='];
  const pemStored = keyFile('MOCK PRIVATE KEY', pemBody, '\\n');
  const sshStored = keyFile('MOCK OPENSSH PRIVATE KEY', sshBody, '\\n');

  // Add: a PEM copied on Windows (CRLF, with a final line break).
  await screen.getByRole('button', 'Add agent secret').first().click();
  const add = screen.getByRole('dialog', 'Add agent secret');
  await screen.getByLabel('Name').fill('DEPLOY_PEM');
  expect(await pasteValue(browser, `${keyFile('MOCK PRIVATE KEY', pemBody, '\r\n')}\r\n`)).toBe(pemStored);
  await add.getByRole('button', 'Add').click();
  await expect(add).toBeHidden();
  await expect(screen.getByText('DEPLOY_PEM')).toBeVisible();

  // The older secret whose line breaks were lost is marked, not rewritten.
  const old = browser.locator('.item-card:has-text("OLD_DEPLOY_KEY")');
  await expect(old.getByText('Paste again')).toBeVisible();
  await expect(old.getByText(/Its line breaks were lost/)).toBeVisible();
  await expect(browser.locator('.item-card:has-text("DEPLOY_PEM")').getByText('Paste again')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  // Edit: an OpenSSH key (LF, final line break) pasted over a selection replaces it.
  await screen.getByRole('button', 'Edit OLD_DEPLOY_KEY').click();
  const edit = screen.getByRole('dialog', 'Edit OLD_DEPLOY_KEY');
  await screen.getByLabel('Value').fill('selected');
  await browser.evaluate(() => (document.querySelector('dialog.modal input[aria-label="Value"]') as HTMLInputElement).select());
  expect(await pasteValue(browser, `${keyFile('MOCK OPENSSH PRIVATE KEY', sshBody, '\n')}\n`)).toBe(sshStored);
  await edit.getByRole('button', 'Save').click();
  await expect(edit).toBeHidden();
  await expect(old.getByText('Paste again')).toBeHidden();

  // Both read back as the vault holds them.
  await screen.getByRole('button', 'Show DEPLOY_PEM').click();
  await setUpTwoFactor(screen, browser);
  for (const [name, stored] of [['DEPLOY_PEM', pemStored], ['OLD_DEPLOY_KEY', sshStored]]) {
    if (name !== 'DEPLOY_PEM') await screen.getByRole('button', `Show ${name}`).click();
    const shown = screen.getByRole('dialog', name);
    await shown.getByRole('button', 'Show value').click();
    expect(await browser.evaluate(() => document.querySelector('dialog.modal .secret-value')!.textContent)).toBe(stored);
    await shown.getByRole('button', 'Done').click();
    await expect(shown).toBeHidden();
  }

  // Only a key keeps its line breaks: other text (a value already escaped, pretty-printed JSON,
  // whose \n would break it) pastes as the browser pastes it (review round 1).
  await screen.getByRole('button', 'Add agent secret').first().click();
  const browserPastes = (text: string) => browser.evaluate((t: string) => {
    const input = document.querySelector('dialog.modal input[aria-label="Value"]') as HTMLInputElement;
    const data = new DataTransfer();
    data.setData('text/plain', t);
    const event = new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true });
    input.dispatchEvent(event);
    return !event.defaultPrevented;
  }, text);
  expect(await browserPastes(pemStored)).toBe(true);
  expect(await browserPastes('{\n  "type": "service_account",\n  "project_id": "mock-project"\n}\n')).toBe(true);
  expect(await browserPastes(`${keyFile('MOCK PRIVATE KEY', pemBody, '\n')} trailing words\n`)).toBe(true);
  // CR alone is a line break too; blank lines around the key and spaces around each line (a key
  // indented in YAML or Markdown) are dropped, and the paste can be undone.
  const indented = `\n    ${keyFile('MOCK PRIVATE KEY', pemBody, '\r    ')}\t\r\n \n`;
  expect(await pasteValue(browser, indented)).toBe(pemStored);
  await screen.getByLabel('Value').press('Control+z');
  expect(await browser.evaluate(() => (document.querySelector('dialog.modal input[aria-label="Value"]') as HTMLInputElement).value)).toBe('');
  // A key and its certificate in one file (`cat key.pem cert.pem`) is a key block too (review
  // round 2).
  const bundle = [keyFile('MOCK PRIVATE KEY', pemBody, '\n'), keyFile('CERTIFICATE', ['bW9jay1jZXJ0'], '\n')].join('\n');
  expect(await pasteValue(browser, `${bundle}\n`)).toBe(bundle.replaceAll('\n', '\\n'));
});

// SPEC-121 ASH-005: the hosts an agent asked for wait on the Secrets page for a person to allow.
test('a person allows the hosts an agent asked for', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=agents'));
  const card = browser.locator('.item-card:has-text("VIDOC_API_KEY")');
  await expect(card.getByText('Task #107 asks to send it to')).toBeVisible();
  await expect(card.getByText('api.vidoc.test')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Allow api.vidoc.test for VIDOC_API_KEY').click();
  await expect(card.getByText(/asks to send it to/)).toBeHidden();
  await expect(card.getByText(/Not sent anywhere yet/)).toBeHidden();
  await expect(card.getByText('api.vidoc.test')).toBeVisible();

  // A member (no instance administration) gets the three fields and no More options.
  await app.open(mock('/s/studio/secrets?section=agents', '&role=member'));
  await screen.getByRole('button', 'Add agent secret').first().click();
  const dialog = screen.getByRole('dialog', 'Add agent secret');
  const labels = await browser.evaluate(() => [...document.querySelectorAll('dialog.modal .field-label')].filter((l) => (l as HTMLElement).checkVisibility()).map((l) => l.textContent!.trim()));
  expect(labels).toEqual(['Name', 'Value', 'Description']);
  await expect(dialog.getByText('More options')).toBeHidden();
});

// SPEC-129 R4: an agent asks to copy a secret into the project's .env; a person allows it here,
// and can revoke it later.
test('a person allows an agent to copy a secret into .env, then revokes it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=agents'));
  const card = browser.locator('.item-card:has-text("STRIPE_KEY")');
  await expect(card.getByText(/Task #111 asks to copy its value into the project’s git-ignored \.env/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Allow copying STRIPE_KEY into .env files').click();
  await expect(card.getByText(/asks to copy/)).toBeHidden();
  await expect(card.getByText('Agents in this sandbox may copy its value into git-ignored .env files')).toBeVisible();
  await screen.getByRole('button', 'Stop agents copying STRIPE_KEY into .env files').click();
  await expect(card.getByText(/may copy its value/)).toBeHidden();
  // A secret nobody asked about says nothing about .env.
  await expect(browser.locator('.item-card:has-text("VIDOC_API_KEY")').getByText(/\.env/)).toBeHidden();
});

// Task #113 (SPEC-134, review H1): production deploys get a secret only when someone who manages
// it allows that, apart from .env copies.
test('a person allows a secret for production deploys, then revokes it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=agents'));
  const card = browser.locator('.item-card:has-text("VIDOC_API_KEY")');
  await expect(card.getByText(/Production deploys \(CD\) do not get it/)).toBeVisible();
  await screen.getByRole('button', 'Let production deploys (CD) get VIDOC_API_KEY').click();
  await expect(card.getByText('Production deploys (CD) get its value; agents never do.')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Stop production deploys getting VIDOC_API_KEY').click();
  await expect(card.getByText(/do not get it until/)).toBeVisible();
});

// Task #191 (SPEC-204, #621): "i couldnt set allow this for production deploys". A production
// secret is exclusive (ADR-0109), so the page offers neither side of it where the server refuses.
test('a secret agents use offers no production allowance, and a production secret no Allow for agents', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=agents'));
  // STRIPE_KEY is sent to api.stripe.com: no button, no warning, and what to do instead.
  const stripe = browser.locator('.item-card:has-text("STRIPE_KEY")');
  await expect(stripe.getByText(/Production deploys never get a secret agents use/)).toBeVisible();
  await expect(screen.getByRole('button', 'Let production deploys (CD) get STRIPE_KEY')).toBeHidden();
  await expect(browser.locator('.item-card:has-text("STRIPE_KEY") .badge-warn:has-text("Production")')).toBeHidden();
  // PROD_API_TOKEN is a production secret an agent asked to copy into .env: Dismiss only.
  const prod = browser.locator('.item-card:has-text("PROD_API_TOKEN")');
  await expect(prod.getByText(/Task #112 asks to copy its value/)).toBeVisible();
  await expect(prod.getByText(/production secret: agents never get it/)).toBeVisible();
  await expect(screen.getByRole('button', 'Allow copying PROD_API_TOKEN into .env files')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Dismiss copying PROD_API_TOKEN into .env files').click();
  await expect(prod.getByText(/asks to copy/)).toBeHidden();
  // VIDOC_API_KEY has no hosts yet, so it can become one; then its waiting host request is Dismiss only.
  const vidoc = browser.locator('.item-card:has-text("VIDOC_API_KEY")');
  await screen.getByRole('button', 'Let production deploys (CD) get VIDOC_API_KEY').click();
  await expect(vidoc.getByText('Production deploys (CD) get its value; agents never do.')).toBeVisible();
  await expect(vidoc.getByText(/production secret: agents never get it/)).toBeVisible();
  await expect(screen.getByRole('button', 'Allow api.vidoc.test for VIDOC_API_KEY')).toBeHidden();
  await screen.getByRole('button', 'Dismiss the hosts asked for VIDOC_API_KEY').click();
  await expect(vidoc.getByText(/asks to send it to/)).toBeHidden();
});

// F 2026-10-14 (task #99): "Value in passwords should be hidden by default even after show, and
// shown after explicitly clicking eye icon. On hover there should be option to copy + the current
// copy button." (SPEC-114)
test('a shown value stays masked until the eye is pressed, and copies on hover', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets'));
  // What the page puts on the clipboard, without a clipboard permission.
  await browser.evaluate(() => {
    (window as any).__copied = [];
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text: string) => { (window as any).__copied.push(text); } } });
  });
  const copied = () => browser.evaluate(() => (window as any).__copied.filter((t: string) => t !== ''));

  await screen.getByRole('button', 'Show Shared Gmail').click();
  await setUpTwoFactor(screen, browser);
  const shown = screen.getByRole('dialog', 'Shared Gmail');
  await expect(shown).toBeVisible();
  // Masked: dots of a fixed length, the value nowhere in the page, a name for screen readers.
  const eye = shown.getByRole('button', 'Show value');
  await expect(eye).toHaveAttribute('aria-pressed', 'false');
  const box = browser.locator('dialog.modal .secret-box');
  await expect(box).toContainText('••••••••••••');
  await expect(browser.locator('dialog.modal .secret-box .sr-only')).toHaveText('Value hidden');
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), TEAM_VALUE)).toBe(false);
  // The notes still show.
  await expect(shown.getByText(/Recovery codes/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  // The quick copy shows on hover only, and copies the value while it is masked.
  const quick = browser.locator('dialog.modal .secret-quick-copy');
  await expect(quick).toBeHidden();
  await box.hover();
  await expect(quick).toBeVisible();
  await expect(quick).toHaveAttribute('aria-label', 'Copy');
  await quick.click();
  await expect(screen.getByText(/Value copied/).first()).toBeVisible();
  expect(await copied()).toEqual([TEAM_VALUE]);
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), TEAM_VALUE)).toBe(false);

  // The eye shows it; pressed again it is masked and gone from the page.
  await eye.click();
  await expect(eye).toHaveAttribute('aria-pressed', 'true');
  await expect(shown.getByText(TEAM_VALUE)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await eye.click();
  await expect(eye).toHaveAttribute('aria-pressed', 'false');
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), TEAM_VALUE)).toBe(false);

  // The current Copy button stays and copies too.
  await shown.getByRole('button', 'Copy value').click();
  expect(await copied()).toEqual([TEAM_VALUE, TEAM_VALUE]);

  // Keyboard: focus on the eye shows the quick copy, and it is the next stop.
  await browser.mouse.move(0, 0);
  await expect(quick).toBeHidden();
  await eye.focus();
  await expect(quick).toBeVisible();
  await browser.keyboard.press('Tab');
  expect(await browser.evaluate(() => document.activeElement!.getAttribute('aria-label'))).toBe('Copy');

  // The light theme has no violations either, masked and shown.
  await browser.evaluate(() => document.documentElement.setAttribute('data-theme', 'light'));
  expect(await axeViolations(browser)).toEqual([]);
  await eye.click();
  expect(await axeViolations(browser)).toEqual([]);
  await eye.click();
  await browser.evaluate(() => document.documentElement.removeAttribute('data-theme'));

  // A new Show starts masked again, even after the eye was pressed.
  await eye.click();
  await expect(shown.getByText(TEAM_VALUE)).toBeVisible();
  await shown.getByRole('button', 'Done').click();
  await expect(shown).toBeHidden();
  await screen.getByRole('button', 'Show Shared Gmail').click();
  await expect(screen.getByRole('dialog', 'Shared Gmail').getByRole('button', 'Show value')).toHaveAttribute('aria-pressed', 'false');
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), TEAM_VALUE)).toBe(false);

  // At 390 px the box, its buttons and Copy stay inside the dialog, shown or masked.
  await browser.setViewport({ width: 390, height: 844 });
  for (const press of [false, true]) {
    if (press) await screen.getByRole('dialog', 'Shared Gmail').getByRole('button', 'Show value').click();
    const outside = await browser.evaluate(() => {
      const dialog = document.querySelector('dialog.modal')!.getBoundingClientRect();
      const parts = ['.secret-box', '.secret-eye', '.secret-quick-copy', '.secret-box + .btn'].map((s) => document.querySelector(`dialog.modal ${s}`)!.getBoundingClientRect());
      return parts.filter((r) => r.left < dialog.left || r.right > dialog.right).length;
    });
    expect(outside).toBe(0);
  }
});

// Review of SPEC-040: an older secret's address, user name and seed live in its notes, so its
// notes are edited from their current text, never replaced blindly.
test('editing a team secret loads its current notes after a two-factor code', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets'));
  await screen.getByRole('button', 'Edit Shared Gmail').click();
  const edit = screen.getByRole('dialog', 'Edit Shared Gmail');
  // The stored notes cannot be overwritten blindly: the box opens read-only until they are loaded.
  await expect(browser.locator('dialog.modal textarea')).toHaveAttribute('readonly', '');
  await screen.getByRole('button', 'Edit current notes of Shared Gmail').click();
  await setUpTwoFactor(screen, browser);
  await expect(browser.locator('dialog.modal textarea')).toHaveValue(/User name: team@example\.test/);
  await browser.locator('dialog.modal textarea').fill('User name: team@example.test\nRecovery codes: new');
  await edit.getByRole('button', 'Save').click();
  await expect(edit).toBeHidden();
  await screen.getByRole('button', 'Show Shared Gmail').click();
  await expect(screen.getByRole('dialog', 'Shared Gmail').getByText(/Recovery codes: new/)).toBeVisible();
});

test('members who only read a sandbox cannot add agent secrets', async ({ app, screen }) => {
  await app.open(mock('/s/sandbox-3/secrets?section=agents'));
  await expect(screen.getByRole('button', 'Add agent secret')).toBeDisabled();
});

// Founder feedback 2026-10-11 ("ux is a little broken"): the add button sat in a row glued to the
// banner and the list. Like every list page, the header holds the shown section's add button, and
// the section's parts keep the page's spacing.
test('each section adds from the page header and its parts keep their spacing', async ({ app, browser, screen }) => {
  const headerButton = () => browser.evaluate(() => [...document.querySelectorAll('.page-head .page-actions button')].map((b) => b.textContent!.trim()));
  const gaps = () => browser.evaluate(() => {
    const parts = [...document.querySelector('.page-inner > .stack > .stack')!.children].map((el) => el.getBoundingClientRect());
    return parts.slice(1).map((r, i) => Math.round(r.top - parts[i].bottom));
  });
  await app.open(mock('/s/studio/secrets?section=agents'));
  await expect(screen.getByText('STRIPE_KEY')).toBeVisible();
  expect(await headerButton()).toEqual(['Add agent secret']);
  for (const gap of await gaps()) expect(gap).toBeGreaterThanOrEqual(8);

  await screen.getByRole('radio', 'Team secrets').click();
  await expect(screen.getByText('Shared Gmail')).toBeVisible();
  expect(await headerButton()).toEqual(['Add team secret']);
  for (const gap of await gaps()) expect(gap).toBeGreaterThanOrEqual(8);
});

// Founder feedback 2026-10-11: "when i click outside any popup i want it to close".
test('a click outside a dialog closes it; a click or a selection inside does not', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets'));
  await screen.getByRole('button', 'Add team secret').first().click();
  const add = screen.getByRole('dialog', 'Add team secret');
  await expect(add).toBeVisible();
  await screen.getByLabel('Title').fill('Draft');
  await screen.getByLabel('Title').click();
  await expect(add).toBeVisible();

  // Selecting text in a field and releasing past the dialog's edge keeps it open.
  const field = await browser.locator('dialog.modal input').first().boundingBox();
  await browser.mouse.move(field!.x + 8, field!.y + field!.height / 2);
  await browser.mouse.down();
  await browser.mouse.move(4, 4);
  await browser.mouse.up();
  await expect(add).toBeVisible();

  // A click on the backdrop, outside the dialog.
  await browser.mouse.move(4, 4);
  await browser.mouse.down();
  await browser.mouse.up();
  await expect(add).toBeHidden();
});

// Issue #291 (SPEC-061): a person who lost their authenticator cannot read secrets; an instance
// administrator (or the superadmin) resets their two-factor from the Users page after a
// confirmation and their own two-factor code, and the reset is in the vault's activity.
test('an administrator resets a lost two-factor from the Users page after their own code', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/users', '&role=admin'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Users');
  // Instance administrators read the people; admitting and changing them is the superadmin's.
  await expect(screen.getByRole('button', 'Admit user')).toBeHidden();
  await expect(screen.getByRole('checkbox', 'Access for anna-q')).toBeHidden();
  expect(await browser.evaluate(() => /\bnull\b/.test(document.querySelector('main')!.innerText))).toBe(false);
  const anna = browser.locator('tr:has-text("@anna-q")');
  await expect(anna).toContainText('On');
  // Their own two-factor is not reset here.
  await expect(screen.getByRole('button', 'Reset two-factor of @mkarp')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Reset two-factor of @anna-q').click();
  const confirm = screen.getByRole('dialog', 'Reset two-factor of @anna-q?');
  await expect(confirm.getByText(/set two-factor up again/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await confirm.getByRole('button', 'Reset').click();
  // The administrator's own code first (here: setting their own two-factor up).
  await setUpTwoFactor(screen, browser);
  await expect(anna).toContainText('Off');
  await expect(screen.getByRole('button', 'Reset two-factor of @anna-q')).toBeHidden();

  // In the same page (the mock keeps its state): the Secrets activity names whom.
  await browser.evaluate(() => { window.location.hash = '#/s/studio/secrets'; });
  await expect(screen.getByText(/reset the two-factor of/).first()).toBeVisible();
});
