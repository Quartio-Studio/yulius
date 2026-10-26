import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import type { Browser } from '@e2e-dev/web';
import { axeViolations, eventsConnected, failOnPageErrors, mock, openSuperagent } from './support.ts';

failOnPageErrors();

// SPEC-335: the Superagent starts a task in another sandbox only once its person approves that
// one request, which its panel shows above the session in full.

type Asking = {
  __yuliusMock: {
    askSuperagent: (source: string, request: unknown) => string;
    endSuperagentRequest: (source: string, id: string) => void;
    tasks: { sandbox_id: string; title: string; prompt: string; status: string }[];
  };
};

const PROMPT = 'Update the API client to the v2 endpoints.\nDone when the client tests pass.';

/** The person's Superagent in `source` asks to start a task in `sandbox`; returns its id. */
async function ask(browser: Browser, source: string, request: Record<string, unknown>): Promise<string> {
  await eventsConnected(browser);
  return browser.evaluate(([s, r]) => (window as unknown as Asking).__yuliusMock.askSuperagent(s, r), [source, request] as const);
}

const productTasks = (browser: Browser) => browser.evaluate(() => (window as unknown as Asking).__yuliusMock.tasks
  .filter((t) => t.sandbox_id === 'sb_product').map((t) => ({ title: t.title, prompt: t.prompt, status: t.status })));

test('a request shows in full above the Superagent and Create task starts it in that sandbox', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  const before = await productTasks(browser);
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: PROMPT, title: 'API client v2', options: { prepared: true, workflow: 'Feature' } });

  const card = screen.getByRole('group', 'Your Superagent asks to start a task in product');
  await expect(card).toBeVisible();
  await expect(card).toHaveText(/Quartio-Studio\/product/);
  await expect(card).toHaveText(/API client v2/);
  // The whole prompt, line breaks kept, and the start options it set.
  expect(await browser.evaluate(() => document.querySelector('.agent-request-prompt')?.textContent ?? null)).toBe(PROMPT);
  await expect(card).toHaveText(/Prepared: waits for Start now/);
  await expect(card).toHaveText(/Workflow: Feature/);
  expect(await axeViolations(browser)).toEqual([]);
  // Nothing reached the other sandbox yet.
  expect(await productTasks(browser)).toEqual(before);

  await screen.getByRole('button', 'Create task').click();
  await expect(card).toBeHidden();
  const created = await productTasks(browser);
  expect(created.length).toBe(before.length + 1);
  expect(created[created.length - 1]).toEqual({ title: 'API client v2', prompt: PROMPT, status: 'prepared' });
  // The toast names the task and opens it.
  await expect(browser.locator('.toast:has-text("in product")')).toBeVisible();
  await browser.locator('.toast:has-text("in product") button:has-text("Open")').click();
  await expect(browser).toHaveURL(/#\/s\/product\/t\/\d+$/);
});

test('Decline creates nothing, and a request whose call ended goes by itself', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  const before = await productTasks(browser);
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: 'Rewrite the README' });
  const card = screen.getByRole('group', 'Your Superagent asks to start a task in product');
  await expect(card).toBeVisible();
  await screen.getByRole('button', 'Decline').click();
  await expect(card).toBeHidden();
  await expect(browser.locator('.toast:has-text("Declined")')).toBeVisible();
  expect(await productTasks(browser)).toEqual(before);

  // The Superagent's call ended (interrupted, or nobody answered): the card goes.
  const id = await ask(browser, 'sb_studio', { sandbox: 'product', prompt: 'Another one' });
  await expect(card).toBeVisible();
  await browser.evaluate((i) => (window as unknown as Asking).__yuliusMock.endSuperagentRequest('sb_studio', i), id);
  await expect(card).toBeHidden();
  expect(await productTasks(browser)).toEqual(before);
});

test('a request of the Superagent in another sandbox does not show in this one', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  await ask(browser, 'sb_product', { sandbox: 'studio', prompt: 'From the product Superagent' });
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: 'From this one' });
  await expect(screen.getByRole('group', 'Your Superagent asks to start a task in product')).toBeVisible();
  expect(await browser.locator('.agent-request').count()).toBe(1);
});
