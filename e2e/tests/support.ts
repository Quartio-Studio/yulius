// Shared helpers for the suites. Tests run the UI in mock mode: a static file server and
// assets/mock.js stand in for `yulius serve`, so no server, account or model is needed.
import { createRequire } from 'node:module';
import { afterEach, beforeEach, type Browser, type Screen } from '@e2e-dev/web';
import { expect, type App } from 'e2e';

/**
 * A UI path in mock mode (`?mock=1`, hash routing). `options` are further mock switches, such as
 * `&signedout=1` (see crates/yulius-server/web/README.md, "Run the mock locally").
 */
export const mock = (path: string, options = '') => `/?mock=1${options}#${path}`;

const axeScript = createRequire(import.meta.url).resolve('axe-core/axe.min.js');

type Instrumented = Window & { __e2eErrors?: string[]; axe?: { run: Function } };

/**
 * Loads axe-core into every page and records uncaught errors, unhandled rejections and
 * `console.error` calls. Call it before `app.open`.
 */
export async function instrument(browser: Browser): Promise<void> {
  await browser.addInitScript({ path: axeScript });
  await browser.addInitScript(() => {
    const errors: string[] = [];
    (window as Instrumented).__e2eErrors = errors;
    addEventListener('error', (event) => errors.push(`uncaught: ${event.message}`));
    addEventListener('unhandledrejection', (event) => errors.push(`unhandled rejection: ${String(event.reason)}`));
    const original = console.error.bind(console);
    console.error = (...args: unknown[]) => {
      errors.push(`console.error: ${args.map(String).join(' ')}`);
      original(...args);
    };
  });
}

/**
 * Makes `app.open` and `browser.reload` of a mock page return only once the mock is installed, so
 * a test may touch `window.__yuliusMock` right after either (#446, #455): the page's load event
 * does not wait for the app's dynamic import of mock.js, which a busy machine delays.
 */
function waitForTheMockOnEveryLoad(app: App, browser: Browser): void {
  // The app's own test for mock mode (app.js): `?mock=1`.
  const ready = () => expect.poll(() => browser.evaluate(() => new URLSearchParams(location.search).get('mock') !== '1'
    || Boolean((window as { __yuliusMock?: unknown }).__yuliusMock)), {
    message: 'window.__yuliusMock was never installed (mock.js did not load or the app did not start)',
  }).toBe(true);
  const open = app.open.bind(app);
  const reload = browser.reload.bind(browser);
  Object.assign(app, { open: async (path?: string) => { await open(path); await ready(); } });
  Object.assign(browser, { reload: async (...args: Parameters<Browser['reload']>) => { await reload(...args); await ready(); } });
}

/**
 * Registers hooks for the calling test file: every test loads axe-core and fails when the page
 * reported an error (uncaught exception, unhandled rejection or `console.error`). Opening or
 * reloading a mock page also waits for the mock (`waitForTheMockOnEveryLoad`).
 */
export function failOnPageErrors(): void {
  beforeEach(async ({ app, browser }) => {
    await instrument(browser);
    waitForTheMockOnEveryLoad(app, browser);
  });
  afterEach(async ({ browser }) => {
    expect(await pageErrors(browser)).toEqual([]);
  });
}

/** Errors the page reported since it loaded. */
export function pageErrors(browser: Browser): Promise<string[]> {
  return browser.evaluate(() => (window as Instrumented).__e2eErrors ?? ['instrument() was not called before app.open']);
}

/**
 * WCAG 2.x A and AA violations axe-core finds on the page, one line per rule.
 *
 * `label-content-name-mismatch` is off: it counts text hidden from assistive technology (the
 * account button's `aria-hidden` avatar initials) and unread-count badges as the visible label,
 * although the visible name is part of the accessible name in both cases.
 */
export function axeViolations(browser: Browser): Promise<string[]> {
  return browser.evaluate(async () => {
    const result = await (window as Instrumented).axe!.run(document, {
      runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] },
      rules: { 'label-content-name-mismatch': { enabled: false } },
    });
    return result.violations.map(
      (v: { impact: string; id: string; nodes: { target: string[] }[] }) =>
        `${v.impact} ${v.id}: ${v.nodes.map((node) => node.target.join(' ')).join(', ')}`,
    );
  });
}

/**
 * Budget for the Editor's first opening, which loads Monaco (about 120 files, 3.8 MB of script to
 * parse): on a starved runner that takes longer than an assertion's 5 s (#320, #356). It bounds
 * that one condition, not the steps after it.
 */
export const MONACO_LOAD_MS = 60_000;

/**
 * Waits until the Editor shows a Monaco editor, within Monaco's cold load (`MONACO_LOAD_MS`).
 * Touch `window.monaco.editor` only after it: Monaco's main chunk sets `window.monaco` before its
 * last chunk has registered its services, and the first call into the API fixes the services every
 * editor gets. A call in that gap (a poll of `getEditors()`) makes the editor's idle-time setup
 * throw "depends on UNKNOWN service ICodeLensCache" (#394).
 */
export async function editorShown(browser: Browser): Promise<void> {
  await expect.poll(() => browser.evaluate(() => document.querySelector('.ed-host .monaco-editor') !== null), {
    timeout: MONACO_LOAD_MS,
    message: 'the Editor never showed a Monaco editor (Monaco did not load)',
  }).toBe(true);
}

/**
 * Waits until the task view's terminal shows its first screen. The task view asks for the
 * keyboard when it opens, and that request lands when the terminal shows, over a button or link
 * focused meanwhile (SPEC-056 WUI-102). A test that puts the keyboard elsewhere on a task's page
 * waits for this first: before, it raced the terminal, which took the keyboard 30–100 ms after the
 * sidebar showed (SPEC-082 TST-003).
 */
export async function terminalShown(browser: Browser): Promise<void> {
  await expect.poll(() => browser.evaluate(() => document.querySelector('.taskview .term') !== null
    && document.querySelector('.taskview .term-pending, .taskview .term-placeholder') === null)).toBe(true);
}

/**
 * Opens the Superagent where it lives (SPEC-132): first in the tools rail, under the tools toggle,
 * which shows the rail first when it is hidden. The rail's Superagent button closes it again.
 */
export async function openSuperagent(browser: Browser, screen: Screen): Promise<void> {
  if (await browser.locator('.rail:not([hidden])').count() === 0) await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Superagent').click();
}

/**
 * Waits until the page's live updates (`/api/events`) are connected. Like the server, the mock
 * announces a change only to the pages connected then, and a page's first connection reads nothing
 * again, so a change made before reaches the page only with its next read. A busy machine opens the
 * connection late (#506).
 */
export async function eventsConnected(browser: Browser): Promise<void> {
  type Sockets = { __yuliusMock: { eventSockets: Set<unknown> } };
  await expect.poll(() => browser.evaluate(() => (window as unknown as Sockets).__yuliusMock.eventSockets.size > 0), {
    message: 'the page never connected to its live updates (/api/events)',
  }).toBe(true);
}

/**
 * Changes a mock task as the server would announce it (`__yuliusMock.touchTask`), once the page
 * hears announcements (`eventsConnected`).
 */
export async function touchTask(browser: Browser, sandbox: string, number: number, changes: Record<string, unknown>): Promise<void> {
  type Touching = { __yuliusMock: { touchTask: (sandbox: string, number: number, changes: unknown) => void } };
  await eventsConnected(browser);
  await browser.evaluate(([s, n, c]) => (window as unknown as Touching).__yuliusMock.touchTask(s, n, c), [sandbox, number, changes] as const);
}

/**
 * The new-task prompt's text as it shows (SPEC-124), each file in it as `[name]`: the prompt is a
 * text box with files inside its text, not a textarea, so it has no value to read.
 */
export function promptText(browser: Browser): Promise<string> {
  return browser.evaluate(() => {
    const input = document.querySelector('.composer-prompt');
    if (!input) return '';
    const copy = input.cloneNode(true) as HTMLElement;
    for (const atom of copy.querySelectorAll('.pi-atom')) atom.replaceWith(`[${atom.getAttribute('title') ?? ''}]`);
    return copy.textContent || '';
  });
}

/** Selects characters `start` to `end` of the new-task prompt's first line of text. */
export async function selectPromptText(browser: Browser, start: number, end: number): Promise<void> {
  await browser.evaluate(([from, to]) => {
    const text = [...document.querySelector('.composer-prompt')!.childNodes].find((n) => n.nodeType === 3)!;
    const range = document.createRange();
    range.setStart(text, from);
    range.setEnd(text, to);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
  }, [start, end]);
}
