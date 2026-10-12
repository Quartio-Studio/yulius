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
const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAFElEQVR4nGP8z8DwnwEJMDGgAgYAPgQBAeHC6s0AAAAASUVORK5CYII=';

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
