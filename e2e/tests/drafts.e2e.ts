import { test, type Browser, type Screen } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The new-task composer keeps its draft (SPEC-026; F 2026-10-12): "If I type something here and
// I change tab, I want it to be saved, including attachements."

type Dropping = Window & { __e2eDrop?: (selector: string, files: [string, string, string][]) => void };

/** Drops files (name, text, type) on the composer box as a drag from the computer does. */
async function drop(browser: Browser, files: [string, string, string][]): Promise<void> {
  await browser.evaluate((list) => {
    const target = document.querySelector('.composer-box');
    if (!target) throw new Error('no composer');
    const data = new DataTransfer();
    for (const [name, text, type] of list) data.items.add(new File([text], name, { type }));
    for (const type of ['dragenter', 'dragover', 'drop']) target.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, composed: true, dataTransfer: data }));
  }, files);
}

/** A 2×2 PNG, so an image thumbnail can be drawn from it. */
const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4z8DwH4QZYAwAR8oH+WdZbrcAAAAASUVORK5CYII=';

/** Drops a real PNG image named `name` on the composer box. */
async function dropImage(browser: Browser, name: string): Promise<void> {
  await browser.evaluate(([fileName, base64]) => {
    const target = document.querySelector('.composer-box');
    if (!target) throw new Error('no composer');
    const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
    const data = new DataTransfer();
    data.items.add(new File([bytes], fileName, { type: 'image/png' }));
    for (const type of ['dragenter', 'dragover', 'drop']) target.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, composed: true, dataTransfer: data }));
  }, [name, PNG]);
}

/** Leaves the composer for the Tasks page and comes back with the New task button. */
async function leaveAndReturn(browser: Browser, screen: Screen): Promise<void> {
  await browser.locator('.composer-crumbs a').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks$/);
  await expect(screen.getByRole('textbox', 'Task prompt')).toHaveCount(0);
  await browser.locator('.btn-newtask').click();
  await expect(browser).toHaveURL(/#\/s\/studio$/);
}

const attachmentNames = (browser: Browser) => browser.locator('.composer-attachment-name');

test('the prompt and its attachments come back after visiting another page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('If I type something here');
  await dropImage(browser, 'screen.png');
  await drop(browser, [['notes.pdf', 'pdf', 'application/pdf']]);
  await expect(attachmentNames(browser)).toHaveText(['screen.png', 'notes.pdf']);

  await leaveAndReturn(browser, screen);

  await expect(screen.getByRole('textbox', 'Task prompt')).toHaveValue('If I type something here');
  await expect(attachmentNames(browser)).toHaveText(['screen.png', 'notes.pdf']);
  await expect(browser.locator('.composer-attachment img')).toHaveCount(1);
  // A removed attachment stays removed.
  await screen.getByRole('button', 'Remove notes.pdf').click();
  await leaveAndReturn(browser, screen);
  await expect(attachmentNames(browser)).toHaveText(['screen.png']);
});

test('each sandbox keeps its own draft', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('For studio');
  // The hash route changes in the page, as the sandbox tabs do: no reload.
  await browser.evaluate(() => { location.hash = '#/s/product'; });
  await expect(browser.locator('.composer-crumbs')).toHaveText(/product/);
  await expect(prompt).toHaveValue('');
  await prompt.fill('For product');
  await browser.evaluate(() => { location.hash = '#/s/studio'; });
  await expect(browser.locator('.composer-crumbs')).toHaveText(/studio/);
  await expect(prompt).toHaveValue('For studio');
});

test('the mode, workflow and "Run after" come back with the draft', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').fill('Check something first');
  await screen.getByRole('button', /^Mode:/).click();
  await screen.getByRole('menuitemradio', /^Goal/).click();
  await screen.getByRole('button', /^Workflow:/).click();
  await screen.getByRole('menuitemradio', /^Fix \(default\)/).click();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^After #12 Checkout API/).click();

  await leaveAndReturn(browser, screen);

  await expect(screen.getByRole('button', 'Mode: Goal')).toBeVisible();
  await expect(screen.getByRole('button', 'Workflow: Fix')).toBeVisible();
  await expect(screen.getByRole('button', 'Run after: After #12')).toBeVisible();
});

/** The draft keys this browser stores. */
const storedDrafts = (browser: Browser) => browser.evaluate(() => Object.keys(localStorage).filter((key) => key.startsWith('yulius.draft.')));

/** Thumbnails stored with studio's draft (after its debounce). */
const storedThumbs = (browser: Browser) => browser.evaluate(() => {
  const key = Object.keys(localStorage).find((k) => k.startsWith('yulius.draft.') && k.endsWith('.sb_studio'));
  const draft = key ? JSON.parse(localStorage.getItem(key) || '{}') : {};
  return (draft.attached || []).filter((item: { thumb?: string }) => item.thumb).length;
});

type Holding = Window & { __yuliusMock: { holdUpload?: Promise<void>; holdCreate?: Promise<void>; tasks: { prompt: string }[] }; __release?: () => void };

test('the draft survives a reload and is cleared once the task starts', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').fill('Kept across a reload');
  await dropImage(browser, 'screen.png');
  await drop(browser, [['notes.pdf', 'pdf', 'application/pdf']]);
  await expect(attachmentNames(browser)).toHaveText(['screen.png', 'notes.pdf']);
  await screen.getByRole('button', /^Mode:/).click();
  await screen.getByRole('menuitemradio', /^Goal/).click();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^After #12 Checkout API/).click();
  // The image's small thumbnail is made a moment after the drop; the draft is stored once the
  // page goes away, whatever the debounce.
  await expect.poll(() => storedThumbs(browser)).toBe(1);
  await browser.reload();

  const prompt = screen.getByRole('textbox', 'Task prompt');
  await expect(prompt).toHaveValue('Kept across a reload');
  await expect(attachmentNames(browser)).toHaveText(['screen.png', 'notes.pdf']);
  // The image keeps a (stored, scaled) thumbnail.
  await expect(browser.locator('.composer-attachment img')).toHaveCount(1);
  // SPEC-025: it still opens, as its thumbnail; the PDF's bytes stayed with the old page, so its
  // chip is not a button any more.
  await screen.getByRole('button', 'Open screen.png').click();
  await expect(screen.getByRole('dialog', 'screen.png').getByRole('img', 'screen.png')).toBeVisible();
  await browser.locator('dialog.modal').press('Escape');
  await expect(browser.locator('dialog.modal')).toHaveCount(0);
  await expect(browser.locator('button.composer-attachment-open')).toHaveCount(1);
  await expect(screen.getByRole('button', 'Mode: Goal')).toBeVisible();
  await expect(screen.getByRole('button', 'Run after: After #12')).toBeVisible();

  await screen.getByRole('button', 'Run after: After #12').click();
  await screen.getByRole('menuitemradio', /^Start now/).click();
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const sent = await browser.evaluate(() => {
    const tasks = (window as unknown as { __yuliusMock: { tasks: { prompt: string }[] } }).__yuliusMock.tasks;
    return tasks[tasks.length - 1].prompt;
  });
  expect(sent).toMatch(/^Kept across a reload\n\n@\S+\/screen\.png\n@\S+\/notes\.pdf$/);

  await browser.locator('.btn-newtask').click();
  await expect(prompt).toHaveValue('');
  await expect(attachmentNames(browser)).toHaveCount(0);
  await expect(screen.getByRole('button', 'Mode: Auto mode')).toBeVisible();
  expect(await storedDrafts(browser)).toEqual([]);
  await browser.reload();
  await expect(prompt).toHaveValue('');
});

test('signing out removes the stored drafts', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').fill('Private words');
  await leaveAndReturn(browser, screen);
  await expect.poll(() => storedDrafts(browser)).toHaveLength(1);
  await screen.getByRole('button', /account menu$/).click();
  await screen.getByRole('menuitem', 'Sign out').click();
  await expect(browser).toHaveURL(/#\/signin/);
  expect(await storedDrafts(browser)).toEqual([]);
});

test('an upload that ends after leaving and coming back shows in the composer', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await browser.evaluate(() => {
    const w = window as unknown as Holding;
    w.__yuliusMock.holdUpload = new Promise<void>((resolve) => { w.__release = resolve; });
  });
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeVisible();
  await drop(browser, [['late.pdf', 'pdf', 'application/pdf']]);
  await leaveAndReturn(browser, screen);
  await expect(attachmentNames(browser)).toHaveCount(0);
  await browser.evaluate(() => (window as unknown as Holding).__release!());
  // What will be sent is what shows, and can be removed.
  await expect(attachmentNames(browser)).toHaveText(['late.pdf']);
  await screen.getByRole('button', 'Remove late.pdf').click();
  await expect(attachmentNames(browser)).toHaveCount(0);
});

test('a task still starting is not started again from a returned composer', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const before = await browser.evaluate(() => (window as unknown as Holding).__yuliusMock.tasks.length);
  await browser.evaluate(() => {
    const w = window as unknown as Holding;
    w.__yuliusMock.holdCreate = new Promise<void>((resolve) => { w.__release = resolve; });
  });
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('Only once');
  await prompt.press('ControlOrMeta+Enter');
  await leaveAndReturn(browser, screen);
  await screen.getByRole('textbox', 'Task prompt').press('ControlOrMeta+Enter');
  await expect(screen.getByText('This task is already starting.')).toBeVisible();
  await browser.evaluate(() => (window as unknown as Holding).__release!());
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#\d+ Only once$/);
  expect(await browser.evaluate(() => (window as unknown as Holding).__yuliusMock.tasks.length)).toBe(before + 1);
});

const MIXED = [
  'Finish the release before Friday, all of this has to land together please.',
  '1. Make the attention bar opaque over the terminal and keep its colours readable.',
  '2. Stop the tools rail pills from overflowing the rail on narrow windows.',
  '3. Route new tasks past exhausted accounts in the backend subscription pool.',
  '4. Make the server mark tasks Done reliably once the pull request is tested.',
].join('\n');

test('a split into several tasks clears the draft too', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill(MIXED);
  await drop(browser, [['spec.pdf', 'pdf', 'application/pdf']]);
  await expect(attachmentNames(browser)).toHaveText(['spec.pdf']);
  await prompt.press('ControlOrMeta+Enter');
  const card = screen.getByRole('region', 'Split proposal');
  await card.getByRole('button', 'Create 2 tasks').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await browser.locator('.btn-newtask').click();
  await expect(prompt).toHaveValue('');
  await expect(attachmentNames(browser)).toHaveCount(0);
  expect(await storedDrafts(browser)).toEqual([]);
});

test('a stored draft drops attachments the server no longer keeps and unsafe thumbnails', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await browser.evaluate(() => {
    const day = 24 * 3600 * 1000;
    const file = (name: string, at: number, thumb?: string) => ({ path: `/var/lib/yulius/sandboxes/sb_studio/attachments/drafts/usr_1/${name}`, name, type: 'image/png', at, thumb });
    localStorage.setItem('yulius.draft.usr_1.sb_studio', JSON.stringify({
      text: 'From last week', mode: 'nonsense', attached: [file('old.png', Date.now() - 8 * day), file('fresh.png', Date.now() - day, 'javascript:alert(1)')],
    }));
  });
  await browser.reload();
  await expect(screen.getByRole('textbox', 'Task prompt')).toHaveValue('From last week');
  await expect(attachmentNames(browser)).toHaveText(['fresh.png']);
  await expect(browser.locator('.composer-attachment img')).toHaveCount(0);
  await expect(screen.getByRole('button', 'Mode: Auto mode')).toBeVisible();
});

test('signing out in another browser tab forgets this tab\'s draft too', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').fill('Typed in this tab');
  // What another tab's sign-out leaves in the shared storage (a `storage` event here).
  await browser.evaluate(() => {
    for (const key of Object.keys(localStorage)) if (key.startsWith('yulius.draft.')) localStorage.removeItem(key);
    window.dispatchEvent(new StorageEvent('storage', { key: 'yulius.draftsForgotten', newValue: String(Date.now()), storageArea: localStorage }));
  });
  // Leaving would store the draft again; it does not.
  await leaveAndReturn(browser, screen);
  await expect(screen.getByRole('textbox', 'Task prompt')).toHaveValue('');
  expect(await storedDrafts(browser)).toEqual([]);
});

test('a picked start rule comes back after a reload; a malformed stored one is dropped', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('textbox', 'Task prompt').fill('Wait for the others');
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^When a rule is true/).click();
  const editor = screen.getByRole('dialog', 'Start when a rule is true');
  await editor.getByRole('combobox', 'Task of condition 1').selectOption({ label: '#12 Checkout API' });
  await editor.getByRole('button', 'Add condition').click();
  await editor.getByRole('combobox', 'Task of condition 2').selectOption({ label: '#4 Payment webhook' });
  await editor.getByRole('combobox', 'State of condition 2').selectOption({ label: 'has ended' });
  await editor.getByText('Any is true').click();
  await editor.getByRole('button', 'Use this rule').click();
  await expect(screen.getByRole('button', /Run after: When #12 done or #4 ended/)).toBeVisible();

  await browser.reload();
  await expect(screen.getByRole('textbox', 'Task prompt')).toHaveValue('Wait for the others');
  await expect(screen.getByRole('button', /Run after: When #12 done or #4 ended/)).toBeVisible();

  await browser.evaluate(() => {
    const key = Object.keys(localStorage).find((k) => k.startsWith('yulius.draft.') && k.endsWith('.sb_studio'))!;
    const draft = JSON.parse(localStorage.getItem(key)!);
    draft.startRule = { join: 'and', conditions: [{ number: 12, state: 'done); drop' }] };
    localStorage.setItem(key, JSON.stringify(draft));
  });
  await browser.reload();
  await expect(screen.getByRole('textbox', 'Task prompt')).toHaveValue('Wait for the others');
  await expect(screen.getByRole('button', /Run after: Start now/)).toBeVisible();
});
