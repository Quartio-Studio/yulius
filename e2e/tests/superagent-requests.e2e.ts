import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import type { Browser } from '@e2e-dev/web';
import { axeViolations, eventsConnected, failOnPageErrors, firstShown, mock as mockPage, openSuperagent } from './support.ts';

failOnPageErrors();

/**
 * A mock page without the sandbox browser's sign-in request 5 s after the start (`browserask=0`):
 * its toast goes below the others and moves them up, so on a slow machine it moved a toast's
 * button between a click's press and release, and the click was lost (#978).
 */
const mock = (path: string) => mockPage(path, '&browserask=0');

// SPEC-335: the Superagent starts a task in another sandbox only once its person approves that
// one request, which its panel shows above the session in full.

type Asking = {
  __yuliusMock: {
    askSuperagent: (source: string, request: unknown) => string;
    withdrawSuperagentRequest: (source: string, id: string) => void;
    amendSuperagentRequest: (source: string, id: string, prompt: string) => void;
    askBrowserSignIn: () => boolean;
    tasks: { sandbox_id: string; number: number; title: string; prompt: string; status: string; harness: string; harness_choice: string; model: string; model_default: boolean; effort: string; effort_default: boolean }[];
  };
};

const PROMPT = 'Update the API client to the v2 endpoints.\nDone when the client tests pass.';

/** The person's Superagent in `source` asks to start a task in `sandbox`; returns its id. */
async function ask(browser: Browser, source: string, request: Record<string, unknown>): Promise<string> {
  await eventsConnected(browser);
  return browser.evaluate(([s, r]) => (window as unknown as Asking).__yuliusMock.askSuperagent(s, r), [source, request] as const);
}

const productTasks = (browser: Browser) => browser.evaluate(() => (window as unknown as Asking).__yuliusMock.tasks
  .filter((t) => t.sandbox_id === 'sb_product').map((t) => ({ title: t.title, prompt: t.prompt, status: t.status, harness: t.harness })));

test('a request shows in full above the Superagent and Create task starts it in that sandbox', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  const before = await productTasks(browser);
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: PROMPT, title: 'API client v2', options: { prepared: true, workflow: 'Feature', harness: 'antigravity' } });

  const card = screen.getByRole('group', 'Your Superagent asks to start a task in product');
  await expect(card).toBeVisible();
  await expect(card).toHaveText(/Quartio-Studio\/product/);
  await expect(card).toHaveText(/API client v2/);
  // The whole prompt, line breaks kept, and the start options it set.
  expect(await browser.evaluate(() => document.querySelector('.agent-request-prompt')?.textContent ?? null)).toBe(PROMPT);
  await expect(card).toHaveText(/Prepared: waits for Start now/);
  await expect(card).toHaveText(/Workflow: Feature/);
  // The agent it names shows too, before the person approves (SPEC-505 RND-001).
  await expect(card).toHaveText(/Agent: antigravity/);
  expect(await axeViolations(browser)).toEqual([]);
  // Nothing reached the other sandbox yet.
  expect(await productTasks(browser)).toEqual(before);

  await screen.getByRole('button', 'Create task').click();
  await expect(card).toBeHidden();
  const created = await productTasks(browser);
  expect(created.length).toBe(before.length + 1);
  expect(created[created.length - 1]).toEqual({ title: 'API client v2', prompt: PROMPT, status: 'prepared', harness: 'antigravity' });
  // The toast names the task and opens it ("in product" alone also matches the error toast).
  await expect(browser.locator('.toast:has-text("Created #"):has-text("in product")')).toBeVisible();
  await browser.locator('.toast:has-text("Created #"):has-text("in product") button:has-text("Open")').click();
  await expect(browser).toHaveURL(/#\/s\/product\/t\/\d+$/);
});

// SPEC-620 CDM-001: with neither agent nor model, an approved task runs Claude Code on Opus 5.5.
test('approved tasks inherit the Claude Code defaults and honor explicit settings', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  for (const [choice, expected] of [
    [{}, { harness: 'claude', harness_choice: 'chosen', model: 'claude-opus-5-5', model_default: true, effort: 'high', effort_default: true }],
    [{ effort: 'xhigh' }, { harness: 'claude', model: 'claude-opus-5-5', effort: 'xhigh', effort_default: false }],
    [{ harness: 'codex', model: 'codex' }, { harness: 'codex', model: 'codex', effort: 'high', effort_default: true }],
    [{ model: 'gemini-4-neon' }, { harness: 'antigravity', harness_choice: 'chosen', model: 'gemini-4-neon' }],
    [{ harness: 'auto' }, { harness_choice: 'random' }],
  ] as const) {
    await ask(browser, 'sb_studio', { sandbox: 'product', prompt: PROMPT, options: { prepared: true, ...choice } });
    await screen.getByRole('button', 'Create task').click();
    await expect(screen.getByRole('group', 'Your Superagent asks to start a task in product')).toBeHidden();
    const task = await browser.evaluate(() => (window as unknown as Asking).__yuliusMock.tasks.filter((t) => t.sandbox_id === 'sb_product').at(-1));
    expect(task).toMatchObject(expected);
  }
});

// #978: the mock's sign-in request from the sandbox browser (5 s after the start) added a toast
// below "Created #N", which moved its Open button up between the click's press and release on a
// slow machine: no click, and the page stayed. The request is fired at that very moment; this
// file's pages leave it out (`browserask=0`), so it must not come and the click must open the task.
test('a sign-in request from the sandbox browser cannot move a pressed Open (#978)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: PROMPT, title: 'API client v2' });
  await screen.getByRole('button', 'Create task').click();
  await expect(browser.locator('.toast:has-text("Created #")')).toBeVisible();
  const open = await browser.evaluate(() => {
    const toast = [...document.querySelectorAll('.toast')].find((t) => t.textContent!.startsWith('Created #'))!;
    const box = toast.querySelector('button.btn')!.getBoundingClientRect();
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  });
  await browser.mouse.move(open.x, open.y);
  await browser.mouse.down();
  // Sent only without `browserask=0`: its toast then moves Open, and the release below misses it.
  if (await browser.evaluate(() => (window as unknown as Asking).__yuliusMock.askBrowserSignIn())) {
    await expect(browser.locator('.toast:has-text("needs your action")')).toBeVisible();
  }
  await browser.mouse.up();
  await expect(browser).toHaveURL(/#\/s\/product\/t\/\d+$/);
});

test('Decline creates nothing, and a withdrawn request goes by itself', async ({ app, browser, screen }) => {
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

  // SPEC-487 XRQ-001: a request stays until answered; the Superagent withdraws it: the card goes.
  const id = await ask(browser, 'sb_studio', { sandbox: 'product', prompt: 'Another one' });
  await expect(card).toBeVisible();
  await browser.evaluate((i) => (window as unknown as Asking).__yuliusMock.withdrawSuperagentRequest('sb_studio', i), id);
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

// SPEC-428 review round 3: with All sandboxes in front, a request of this sandbox's Superagent marks
// the This sandbox tab, also when that tab has not been shown in this page (no session is started for
// it).
test('a request while All sandboxes is in front marks the This sandbox tab', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  const strip = screen.getByRole('region', 'Tool panel').getByRole('tablist', 'Superagents');
  await strip.getByRole('tab', 'All sandboxes').click();
  await browser.reload();
  await firstShown(screen.getByRole('heading', { level: 1 }));
  await openSuperagent(browser, screen);
  const all = strip.getByRole('tab', 'All sandboxes');
  await expect(all).toHaveAttribute('aria-selected', 'true');
  const marker = browser.locator('.superagent-tabs [data-key="sandbox"] .superagent-tab-dot');
  await expect(marker).toBeHidden();

  const id = await ask(browser, 'sb_studio', { sandbox: 'product', prompt: PROMPT, title: 'API client v2' });
  await expect(marker).toBeVisible();
  const here = strip.getByRole('tab', 'This sandbox');
  expect(await browser.evaluate(() => {
    const tab = document.querySelector('.superagent-tabs [data-key="sandbox"]');
    const note = tab && document.getElementById(tab.getAttribute('aria-describedby') || '');
    return note ? note.textContent : null;
  })).toBe('Your Superagent asks to start a task');
  expect(await browser.evaluate(() => (window as unknown as { __yuliusMock: { cockpitStarts?: number } }).__yuliusMock.cockpitStarts ?? 0)).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);

  await here.click();
  await expect(screen.getByRole('group', 'Your Superagent asks to start a task in product')).toBeVisible();
  await browser.evaluate((r) => (window as unknown as Asking).__yuliusMock.withdrawSuperagentRequest('sb_studio', r), id);
  await expect(screen.getByRole('group', 'Your Superagent asks to start a task in product')).toBeHidden();
  await expect(marker).toBeHidden();
});

/** The number of product's newest task. */
const newest = (browser: Browser) => browser.evaluate(() => Math.max(...(window as unknown as Asking).__yuliusMock.tasks
  .filter((t) => t.sandbox_id === 'sb_product').map((t) => t.number)));

// SPEC-442 AC-XST-7: the same cards ask to start a task of another sandbox now, or to send it a
// message from the person, which is read to its end as a prompt is.
test('a prepared task of another sandbox is started and messaged from the Superagent', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: PROMPT, title: 'API client v2', options: { prepared: true } });
  await screen.getByRole('button', 'Create task').click();
  await expect(browser.locator('.toast:has-text("Created #")')).toBeVisible();
  const created = await productTasks(browser);
  const number = await newest(browser);
  expect(created.at(-1)!.status).toBe('prepared');

  await ask(browser, 'sb_studio', { sandbox: 'product', action: 'update', number });
  const start = screen.getByRole('group', `Your Superagent asks to start #${number} now in product`);
  await expect(start).toBeVisible();
  await expect(start).toHaveText(/API client v2/);
  await expect(start).toHaveText(/prepared/);
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Start now').click();
  await expect(start).toBeHidden();
  expect((await productTasks(browser)).at(-1)!.status).toBe('queued');
  await expect(browser.locator(`.toast:has-text("Started #${number} in product")`)).toBeVisible();

  const message = 'Use the v2 endpoints only.\nKeep v1 working.';
  await ask(browser, 'sb_studio', { sandbox: 'product', action: 'message', number, prompt: message });
  const card = screen.getByRole('group', `Your Superagent asks to send #${number} a message from you in product`);
  await expect(card).toBeVisible();
  expect(await browser.evaluate(() => document.querySelector('.agent-request-prompt')?.getAttribute('aria-label') ?? null)).toBe('Message');
  expect(await browser.evaluate(() => document.querySelector('.agent-request-prompt')?.textContent ?? null)).toBe(message);
  await screen.getByRole('button', 'Send message').click();
  await expect(card).toBeHidden();
  expect((await productTasks(browser)).at(-1)!.prompt).toContain('Use the v2 endpoints only.');
});

test('Decline on a stop request leaves the task of another sandbox as it was', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: PROMPT, title: 'API client v2' });
  await screen.getByRole('button', 'Create task').click();
  await expect(browser.locator('.toast:has-text("Created #")')).toBeVisible();
  const number = await newest(browser);
  const before = await productTasks(browser);

  await ask(browser, 'sb_studio', { sandbox: 'product', action: 'stop', number });
  const card = screen.getByRole('group', `Your Superagent asks to stop #${number} in product`);
  await expect(card).toBeVisible();
  await screen.getByRole('button', 'Decline').click();
  await expect(card).toBeHidden();
  await expect(browser.locator('.toast:has-text("Declined: nothing was changed.")')).toBeVisible();
  expect(await productTasks(browser)).toEqual(before);
});

// SPEC-442 review round 1: a message is read to its end before it can be sent, as a prompt is;
// an update whose card says it starts the task now (`starts_now`) says so even with an option.
test('Send message takes a click only once the whole message was in view', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  await ask(browser, 'sb_studio', { sandbox: 'product', prompt: PROMPT, title: 'API client v2', options: { prepared: true } });
  await screen.getByRole('button', 'Create task').click();
  await expect(browser.locator('.toast:has-text("Created #")')).toBeVisible();
  const number = await newest(browser);

  const long = Array.from({ length: 80 }, (_, i) => `Line ${i + 1} of the message.`).join('\n');
  await ask(browser, 'sb_studio', { sandbox: 'product', action: 'message', number, prompt: long });
  const card = screen.getByRole('group', `Your Superagent asks to send #${number} a message from you in product`);
  await expect(card).toHaveText(/Scroll to the end of the message to send it/);
  await expect(screen.getByRole('button', 'Decline')).toBeEnabled();
  await expect(screen.getByRole('button', 'Send message')).toBeDisabled();
  await browser.evaluate(() => {
    const text = document.querySelector('.agent-request-prompt');
    if (text) text.scrollTop = text.scrollHeight;
  });
  await expect(screen.getByRole('button', 'Send message')).toBeEnabled();
  await screen.getByRole('button', 'Decline').click();
  await expect(card).toBeHidden();
  await expect(browser.locator('.toast:has-text("Declined: nothing was sent.")')).toBeVisible();

  await ask(browser, 'sb_studio', { sandbox: 'product', action: 'update', number, options: { supertask: true }, startsNow: true });
  const start = screen.getByRole('group', `Your Superagent asks to start #${number} now in product`);
  await expect(start).toHaveText(/Supertask: runs alone/);
  await expect(screen.getByRole('button', 'Start now')).toBeVisible();
});

// SPEC-487 XRQ-005 (wall 9): a decision only the person makes waits on the Superagent's own
// panel, its question read to its end as a prompt is; Yes answers yes and No answers no.
test('a decision the Superagent leaves to its person is answered Yes or No on its panel', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  const question = 'May I merge #12 tonight?\nIts checks passed; nobody reviewed the docs.';
  await ask(browser, 'sb_studio', { sandbox: 'studio', action: 'ask', prompt: question });
  const card = screen.getByRole('group', 'Your Superagent asks you to decide');
  await expect(card).toBeVisible();
  expect(await browser.evaluate(() => document.querySelector('.agent-request-prompt')?.textContent ?? null)).toBe(question);
  expect(await browser.evaluate(() => document.querySelector('.agent-request-prompt')?.getAttribute('aria-label') ?? null)).toBe('Question');
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Yes').click();
  await expect(card).toBeHidden();
  await expect(browser.locator('.toast:has-text("Answered yes")')).toBeVisible();

  await ask(browser, 'sb_studio', { sandbox: 'studio', action: 'ask', prompt: 'OK to spend on the paid plan?' });
  await expect(card).toBeVisible();
  await screen.getByRole('button', 'No').click();
  await expect(card).toBeHidden();
  await expect(browser.locator('.toast:has-text("Answered no")')).toBeVisible();
  expect(await browser.evaluate(() => (window as unknown as { __yuliusMock: { superagentAnswers: string[] } }).__yuliusMock.superagentAnswers)).toEqual(['yes', 'no']);
});

// Review of slice 8a, F1: a request the Superagent amends while its card is shown is drawn
// again with the new text, read to its end again, and approved as drawn (the mock refuses an
// answer naming an older revision, as the server does).
test('an amended request is drawn again and approved as drawn', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  const before = await productTasks(browser);
  const id = await ask(browser, 'sb_studio', { sandbox: 'product', prompt: 'Rewrite the README' });
  const card = screen.getByRole('group', 'Your Superagent asks to start a task in product');
  await expect(card).toBeVisible();
  await browser.evaluate((i) => (window as unknown as Asking).__yuliusMock.amendSuperagentRequest('sb_studio', i, 'Rewrite the README and the guide'), id);
  await expect.poll(() => browser.evaluate(() => document.querySelector('.agent-request-prompt')?.textContent ?? null)).toBe('Rewrite the README and the guide');
  await screen.getByRole('button', 'Create task').click();
  await expect(card).toBeHidden();
  const created = await productTasks(browser);
  expect(created.length).toBe(before.length + 1);
  expect(created[created.length - 1].prompt).toBe('Rewrite the README and the guide');
});

// SPEC-487 REL-001 (the person's decision of 2026-11-13): a production release the Superagent
// asks for waits on its panel, and its button takes a click only once the project's slug is
// typed, as the CD page's confirmation does; Decline needs none. Onboarding is a plain click.
test('a production deploy asked by the Superagent takes the project slug typed', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  await ask(browser, 'sb_studio', { sandbox: 'studio', action: 'deploy', options: { environment: 'prod' } });
  const card = screen.getByRole('group', 'Your Superagent asks to deploy prod in studio');
  await expect(card).toBeVisible();
  await expect(card).toHaveText(/Environment: prod/);
  const deploy = screen.getByRole('button', 'Deploy');
  const typed = browser.locator('.agent-request input[aria-label="Type studio to confirm"]');
  await expect(typed).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await typed.fill('studi');
  await expect(deploy).toBeDisabled();
  await typed.fill('studio');
  await expect(deploy).toBeEnabled();
  await deploy.click();
  await expect(card).toBeHidden();
  await expect(browser.locator('.toast:has-text("Deploy started in studio")')).toBeVisible();
  const released = () => browser.evaluate(() => (window as unknown as { __yuliusMock: { superagentReleases?: unknown[] } }).__yuliusMock.superagentReleases ?? []);
  expect(await released()).toEqual([{ action: 'deploy', confirm: 'studio' }]);

  // Declined without typing anything: nothing is released.
  await ask(browser, 'sb_studio', { sandbox: 'studio', action: 'promote' });
  const promote = screen.getByRole('group', 'Your Superagent asks to promote production in studio');
  await expect(promote).toBeVisible();
  await expect(screen.getByRole('button', 'Promote')).toBeDisabled();
  await screen.getByRole('button', 'Decline').click();
  await expect(promote).toBeHidden();
  expect(await released()).toEqual([{ action: 'deploy', confirm: 'studio' }]);

  // Onboarding takes a click, no slug.
  await ask(browser, 'sb_studio', { sandbox: 'studio', action: 'onboard' });
  const onboard = screen.getByRole('group', 'Your Superagent asks to start onboarding in studio');
  await expect(onboard).toBeVisible();
  await screen.getByRole('button', 'Start onboarding').click();
  await expect(onboard).toBeHidden();
  expect(await released()).toEqual([{ action: 'deploy', confirm: 'studio' }, { action: 'onboard', confirm: null }]);
});

// SPEC-487 SEC-001, SEC-002 (wall 10, the person's decision of 2026-11-13): the Superagent asks
// for a secret's value; the person types it masked on its panel, line breaks kept, and it goes
// to the vault: the page never shows it, and Store takes a click only once there is a value.
test('a secret the Superagent asks for is typed masked on its panel and stored', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await openSuperagent(browser, screen);
  await ask(browser, 'sb_studio', { sandbox: 'studio', action: 'secret', title: 'DEPLOY_KEY', prompt: "the dev VM's SSH key", options: { scope: 'personal' } });
  const card = screen.getByRole('group', 'Your Superagent asks you for the secret DEPLOY_KEY in studio');
  await expect(card).toBeVisible();
  await expect(card).toHaveText(/the dev VM's SSH key/);
  // Review of slice 10a, M2: whose it is, said on the card.
  await expect(card).toHaveText(/Yours only \(personal\)/);
  await expect(card).not.toHaveText(/Replaces/);
  const field = browser.locator('.agent-request textarea[aria-label="DEPLOY_KEY’s value"]');
  await expect(field).toBeVisible();
  expect(await browser.evaluate(() => getComputedStyle(document.querySelector('.agent-request textarea')!).getPropertyValue('-webkit-text-security'))).toBe('disc');
  expect(await field.getAttribute('data-1p-ignore')).not.toBeNull();
  const store = screen.getByRole('button', 'Store');
  await expect(store).toBeDisabled();
  expect(await axeViolations(browser)).toEqual([]);
  // M4: a value the server refuses stays typed, and the card waits.
  await field.fill('1234567\n');
  await expect(store).toBeEnabled();
  await store.click();
  await expect(browser.locator('.toast:has-text("Couldn’t store DEPLOY_KEY")')).toBeVisible();
  await expect(card).toBeVisible();
  await expect(field).toHaveValue('1234567\n');
  await field.fill('-----BEGIN KEY-----\nAAAA\n-----END KEY-----');
  await store.click();
  await expect(card).toBeHidden();
  await expect(browser.locator('.toast:has-text("Stored DEPLOY_KEY")')).toBeVisible();
  expect(await browser.evaluate(() => (window as unknown as { __yuliusMock: { superagentSecrets?: unknown[] } }).__yuliusMock.superagentSecrets ?? []))
    .toEqual([{ name: 'DEPLOY_KEY', value: '-----BEGIN KEY-----\nAAAA\n-----END KEY-----' }]);
  // Nothing of it stays on the page.
  expect(await browser.evaluate(() => document.body.innerText.includes('AAAA'))).toBe(false);

  // A project secret that replaces one says so.
  await ask(browser, 'sb_studio', { sandbox: 'studio', action: 'secret', title: 'API_TOKEN', prompt: null, options: { scope: 'sandbox' }, replaces: true });
  const second = screen.getByRole('group', 'Your Superagent asks you for the secret API_TOKEN in studio');
  await expect(second).toHaveText(/This project’s \(sandbox\)/);
  await expect(second).toHaveText(/Replaces the value of API_TOKEN/);
});
