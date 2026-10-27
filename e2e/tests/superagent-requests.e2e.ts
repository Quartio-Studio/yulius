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

// Review findings: a prompt cannot hide words from the person who approves it.
test('a prompt shows its invisible characters and its runs of empty lines', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  // Words in Unicode tag characters ("ASCII smuggling"), a blank-drawing filler, a variation selector.
  const tags = [...'hi'].map((c) => String.fromCodePoint(0xE0000 + c.charCodeAt(0))).join('');
  await ask(browser, 'sb_studio', {
    sandbox: 'product',
    title: 'Typo\u202Efix',
    prompt: `Fix the typo.${'\n'.repeat(11)}Also delete\u202E the tests.\u200B${tags}\u3164\uFE0F`,
    options: { start_rule: '#1\u200D done' },
  });
  const card = screen.getByRole('group', 'Your Superagent asks to start a task in product');
  await expect(card).toBeVisible();
  expect(await browser.evaluate(() => document.querySelector('.agent-request-prompt')?.textContent ?? null))
    .toBe('Fix the typo.\n[10 empty lines]\nAlso delete⟨U+202E⟩ the tests.⟨U+200B⟩⟨U+E0068⟩⟨U+E0069⟩⟨U+3164⟩⟨U+FE0F⟩');
  expect(await browser.evaluate(() => document.querySelector('.agent-request-title')?.textContent ?? null)).toBe('Typo⟨U+202E⟩fix');
  await expect(card).toHaveText(/When #1⟨U\+200D⟩ done/);
});

test('Create task takes a click only once the whole prompt was in view', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  const long = Array.from({ length: 80 }, (_, i) => `Line ${i + 1} of the request.`).join('\n');
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: long });
  const card = screen.getByRole('group', 'Your Superagent asks to start a task in product');
  await expect(card).toHaveText(/Scroll to the end of the prompt to create the task/);
  await expect(screen.getByRole('button', 'Decline')).toBeEnabled();
  await expect(screen.getByRole('button', 'Create task')).toBeDisabled();
  await browser.evaluate(() => {
    const prompt = document.querySelector('.agent-request-prompt');
    if (prompt) prompt.scrollTop = prompt.scrollHeight;
  });
  await expect(screen.getByRole('button', 'Create task')).toBeEnabled();
  await expect(card).not.toHaveText(/Scroll to the end/);
});

test('a new request takes no click at once, and none while its answer is on its way', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  // How the buttons are when the card appears, recorded as it is inserted.
  await browser.evaluate(() => {
    const seen: string[] = [];
    (window as unknown as { __seen: string[] }).__seen = seen;
    new MutationObserver(() => {
      for (const button of document.querySelectorAll('.agent-request-actions button:not([data-seen])')) {
        (button as HTMLElement).dataset.seen = '1';
        seen.push(`${button.textContent}:${(button as HTMLButtonElement).disabled}`);
      }
    }).observe(document.body, { childList: true, subtree: true });
  });
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: 'Short' });
  await expect(screen.getByRole('button', 'Create task')).toBeEnabled();
  expect(await browser.evaluate(() => (window as unknown as { __seen: string[] }).__seen)).toEqual(['Create task:true', 'Decline:true']);
  // Held answer: both buttons wait for it.
  await browser.evaluate(() => {
    const db = (window as unknown as { __yuliusMock: { holdAnswer?: Promise<void>; releaseAnswer?: () => void } }).__yuliusMock;
    db.holdAnswer = new Promise((resolve) => { db.releaseAnswer = resolve; });
  });
  await screen.getByRole('button', 'Create task').click();
  await expect(screen.getByRole('button', 'Create task')).toBeDisabled();
  await expect(screen.getByRole('button', 'Decline')).toBeDisabled();
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { releaseAnswer: () => void } }).__yuliusMock.releaseAnswer());
  await expect(screen.getByRole('group', 'Your Superagent asks to start a task in product')).toBeHidden();
});

// Review round 3: three waiting requests leave the session in view, and each card is reached by
// scrolling the list; Create task counts a prompt as read only once its end was on screen.
test('three waiting requests keep the session in view and each is reached by scrolling', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  const prompt = Array.from({ length: 6 }, (_, i) => `Step ${i + 1} of the change.`).join('\n');
  for (const n of [1, 2, 3]) {
    await ask(browser, 'sb_studio', { sandbox: 'product', title: `Request ${n}`, prompt, options: { workflow: 'Feature', mode: 'goal' } });
  }
  await expect.poll(() => browser.locator('.agent-request').count()).toBe(3);
  // The session keeps room to be read and typed into.
  expect(await browser.evaluate(() => (document.querySelector('.panel-view:not([hidden]) .agent-term') as HTMLElement | null)?.clientHeight ?? 0))
    .toBeGreaterThan(150);
  const first = browser.locator('.agent-request:has-text("Request 1") button:has-text("Create task")');
  const third = browser.locator('.agent-request:has-text("Request 3") button:has-text("Create task")');
  await expect(first).toBeEnabled();
  // The third card is armed (its Decline takes clicks) but below the list's fold: its prompt was
  // not on screen yet, so Create task waits and says why.
  await expect(browser.locator('.agent-request:has-text("Request 3") button:has-text("Decline")')).toBeEnabled();
  await expect(browser.locator('.agent-request:has-text("Request 3") .agent-request-hint')).toBeVisible();
  await expect(third).toBeDisabled();
  await browser.evaluate(() => document.querySelector('.agent-request:last-child .agent-request-actions')?.scrollIntoView({ block: 'end' }));
  await expect(third).toBeEnabled();
  await third.click();
  await expect.poll(() => browser.locator('.agent-request').count()).toBe(2);
});

test('lines that only look blank are counted as empty lines', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  const blank = '\n\u{00A0}\n\u{3000}\n\u{2800}\n \u{2007}\n';
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: `Fix the typo.${blank}Hidden\u{2028}\u{2028}\u{2028}below` });
  await expect(screen.getByRole('group', 'Your Superagent asks to start a task in product')).toBeVisible();
  expect(await browser.evaluate(() => document.querySelector('.agent-request-prompt')?.textContent ?? null))
    .toBe('Fix the typo.\n[4 empty lines]\nHidden\n[2 empty lines]\nbelow');
});

// Review round 4: a double-click on Create task approves one request, never the one that moves
// into its place, and an answered card takes no second click while it goes.
test('a card that moves up when another goes takes no click at once', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  await ask(browser, 'sb_studio', { sandbox: 'product', title: 'One', prompt: 'Same shape' });
  await ask(browser, 'sb_studio', { sandbox: 'product', title: 'Two', prompt: 'Same shape' });
  const second = browser.locator('.agent-request:has-text("Two") button:has-text("Create task")');
  await expect(second).toBeEnabled();
  // How the second card's Create task is the moment the first card goes.
  await browser.evaluate(() => {
    const w = window as unknown as { __whenGone: string[] };
    w.__whenGone = [];
    const list = document.querySelector('.agent-requests');
    if (!list) return;
    new MutationObserver((changes) => {
      if (!changes.some((c) => c.removedNodes.length)) return;
      const left = list.querySelector('.agent-request:last-child .agent-request-actions .btn-primary') as HTMLButtonElement | null;
      w.__whenGone.push(`${left?.closest('.agent-request')?.textContent?.includes('Two') ?? false}:${left?.disabled ?? null}`);
    }).observe(list, { childList: true });
  });
  await browser.locator('.agent-request:has-text("One") button:has-text("Create task")').click();
  await expect.poll(() => browser.locator('.agent-request').count()).toBe(1);
  expect(await browser.evaluate(() => (window as unknown as { __whenGone: string[] }).__whenGone[0] ?? null)).toBe('true:true');
  await expect(second).toBeEnabled();
});

test('an answered card takes no second click while it goes', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: 'Short' });
  await expect(screen.getByRole('button', 'Create task')).toBeEnabled();
  // The list's next read is held, so the answered card stays on screen.
  await browser.evaluate(() => {
    const db = (window as unknown as { __yuliusMock: { holdList?: Promise<void>; releaseList?: () => void } }).__yuliusMock;
    db.holdList = new Promise((resolve) => { db.releaseList = resolve; });
  });
  await screen.getByRole('button', 'Create task').click();
  await expect(browser.locator('.toast:has-text("in product")')).toBeVisible();
  await expect(screen.getByRole('button', 'Create task')).toBeDisabled();
  await expect(screen.getByRole('button', 'Decline')).toBeDisabled();
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { releaseList: () => void } }).__yuliusMock.releaseList());
  await expect(browser.locator('.agent-request')).toBeHidden();
});
