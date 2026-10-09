// Shared helpers for the suites. Tests run the UI in mock mode: a static file server and
// assets/mock.js stand in for `yulius serve`, so no server, account or model is needed.
import { createRequire } from 'node:module';
import { afterEach, beforeEach, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';

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
 * Registers hooks for the calling test file: every test loads axe-core and fails when the page
 * reported an error (uncaught exception, unhandled rejection or `console.error`).
 */
export function failOnPageErrors(): void {
  beforeEach(async ({ browser }) => {
    await instrument(browser);
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
