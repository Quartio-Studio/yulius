import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// SPEC-247 RMT-001, RMT-040: Settings → Connect your local agent makes a personal API token, shows
// it once with the commands that use it, lists tokens with scope and last use, and revokes them.
// The mock's tokens are not real credentials.

const ORIGIN_COMMAND = /^claude mcp add --transport http yulius https?:\/\/[^ ]+\/mcp --header "Authorization: Bearer <token>"$/;

test('a token is made, shown once with its commands, listed and revoked', async ({ app, browser, screen }) => {
  await app.open(mock('/settings'));
  await expect(screen.getByRole('heading', 'Connect your local agent')).toBeVisible();
  const example = await browser.evaluate(() => document.querySelector('[aria-label="example Claude Code command"]')?.textContent ?? null);
  expect(example).toMatch(ORIGIN_COMMAND);
  // The CLI's own token is listed as such, never with its value.
  await expect(browser.locator('.agent-token:has-text("yulius CLI")')).toContainText('yulius login');

  // A name is needed first.
  await screen.getByRole('button', 'Create token').click();
  await expect(browser.locator('.agent-created-box')).toHaveCount(0);

  await screen.getByLabel('Token name').fill('laptop Claude Code');
  await screen.getByRole('button', 'Create token').click();
  await expect(browser.locator('.agent-created-box')).toContainText('the token is shown only this once');
  const shown = await browser.evaluate(() => ({
    token: document.querySelector('[aria-label="token"]')?.textContent ?? null,
    claude: document.querySelector('[aria-label="Claude Code command"]')?.textContent ?? null,
    cli: document.querySelector('[aria-label="CLI command"]')?.textContent ?? null,
  }));
  expect(shown.token).toMatch(/^yul_mock\d+$/);
  expect(shown.claude).toContain(`--header "Authorization: Bearer ${shown.token}"`);
  expect(shown.cli).toMatch(/^yulius remote login https?:\/\//);
  await expect(browser.locator('button[aria-label="Copy Claude Code command"]')).toBeVisible();
  const row = browser.locator('.agent-token:has-text("laptop Claude Code")');
  await expect(row).toContainText('Operate');
  await expect(row).toContainText('never used');
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Revoke laptop Claude Code').click();
  const dialog = screen.getByRole('dialog', 'Revoke “laptop Claude Code”?');
  await expect(dialog).toBeVisible();
  await browser.locator('dialog.modal button:has-text("Revoke")').click();
  await expect(browser.locator('.agent-token:has-text("laptop Claude Code")')).toHaveCount(0);
  await expect(browser.locator('.agent-created-box')).toHaveCount(0);
});

test('a read-only token says what it may do', async ({ app, browser, screen }) => {
  await app.open(mock('/settings'));
  await screen.getByLabel('Token name').fill('dashboards');
  await browser.locator('label.field:has(> .field-label:text-is("Scope")) select').selectOption('Read only');
  await expect(browser.locator('label.field:has(> .field-label:text-is("Scope")) .field-hint')).toHaveText('Lists, tasks, transcripts, CI and files; changes nothing');
  await screen.getByRole('button', 'Create token').click();
  await expect(browser.locator('.agent-token:has-text("dashboards")')).toContainText('Read only');
});

// SPEC-270 (task #256): the form read as two misaligned rows. The Scope hint pushed the name field
// and a smaller Create token button down to its last line, and the select kept the native look.
function tokenFormBoxes() {
  const box = (el: Element | null) => {
    const r = el!.getBoundingClientRect();
    return { top: Math.round(r.top), bottom: Math.round(r.bottom), left: Math.round(r.left), right: Math.round(r.right), height: Math.round(r.height) };
  };
  const form = document.querySelector('.agent-token-form')!;
  const fields = [...form.querySelectorAll(':scope > .field')];
  return {
    form: { ...box(form), scrollWidth: form.scrollWidth, clientWidth: form.clientWidth },
    nameLabel: box(fields[0].querySelector('.field-label')),
    name: box(fields[0].querySelector('input')),
    scopeLabel: box(fields[1].querySelector('.field-label')),
    scope: box(fields[1].querySelector('select')),
    scopeIsSelect: fields[1].querySelector('select')!.classList.contains('select'),
    hint: box(fields[1].querySelector('.field-hint')),
    button: box(form.querySelector('button[type="submit"]')),
  };
}

test('the token form reads as one row', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1440, height: 900 });
  await app.open(mock('/settings'));
  await expect(screen.getByLabel('Token name')).toBeVisible();
  const b = await browser.evaluate(tokenFormBoxes);
  // Labels on one line, controls and the button on the next, all the same height.
  expect(b.scopeLabel.top).toBe(b.nameLabel.top);
  expect([b.scope.top, b.button.top]).toEqual([b.name.top, b.name.top]);
  expect([b.scope.height, b.button.height]).toEqual([b.name.height, b.name.height]);
  // Name, scope and button side by side; the hint under the select, within its column.
  expect(b.name.right).toBeLessThan(b.scope.left);
  expect(b.scope.right).toBeLessThan(b.button.left);
  expect(b.hint.top).toBeGreaterThanOrEqual(b.scope.bottom);
  expect(b.hint.left).toBeGreaterThanOrEqual(b.scope.left);
  expect(b.hint.right).toBeLessThanOrEqual(b.scope.right);
  // The app's select, like every other list to choose from.
  expect(b.scopeIsSelect).toBe(true);
});

// Review: between 641 and 900 px the sidebar left the one-row form a name input too narrow for its
// placeholder, so the form stacks by its own width, not the window's.
for (const width of [390, 800]) test(`the token form stacks at ${width} px`, async ({ app, browser, screen }) => {
  await browser.setViewport({ width, height: 844 });
  await app.open(mock('/settings'));
  await expect(screen.getByLabel('Token name')).toBeVisible();
  const b = await browser.evaluate(tokenFormBoxes);
  // Name, then scope with its hint, then the button, one under the other, inside the form.
  expect(b.scopeLabel.top).toBeGreaterThan(b.name.bottom);
  expect(b.hint.top).toBeGreaterThanOrEqual(b.scope.bottom);
  expect(b.button.top).toBeGreaterThan(b.hint.bottom);
  expect(b.button.height).toBe(b.name.height);
  expect([b.name.left, b.scope.left, b.button.left]).toEqual([b.form.left, b.form.left, b.form.left]);
  expect([b.name.right, b.scope.right]).toEqual([b.form.right, b.form.right]);
  expect(b.form.scrollWidth).toBeLessThanOrEqual(b.form.clientWidth);
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
});

// Review: a field message (`invalid()` in dom.js appends `.field-error` to the field) takes its own
// row under the hint instead of drawing over it.
test('a message on the scope field sits under its hint', async ({ app, browser, screen }) => {
  await app.open(mock('/settings'));
  await expect(screen.getByLabel('Token name')).toBeVisible();
  const b = await browser.evaluate(() => {
    const field = document.querySelectorAll('.agent-token-form > .field')[1];
    const note = document.createElement('span');
    note.className = 'field-error';
    note.textContent = 'Choose a scope';
    field.append(note);
    const box = (el: Element) => { const r = el.getBoundingClientRect(); return { top: Math.round(r.top), bottom: Math.round(r.bottom) }; };
    return { hint: box(field.querySelector('.field-hint')!), error: box(note), name: box(document.querySelector('.agent-token-form input')!), select: box(field.querySelector('select')!) };
  });
  expect(b.error.top).toBeGreaterThanOrEqual(b.hint.bottom);
  expect(b.select.top).toBe(b.name.top);
});
