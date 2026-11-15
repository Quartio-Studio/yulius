// The UI runs under the server's content security policy in every e2e test (SPEC-483 W1b,
// ADR-0338): e2e/serve.py sends the file `yulius serve` sends, and a load the policy refuses fails
// the test that makes it.
import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { failOnPageErrors, mock, pageErrors } from './support.ts';

failOnPageErrors();

const POLICY = readFileSync(
  fileURLToPath(new URL('../../crates/yulius-server/src/content-security-policy.txt', import.meta.url)),
  'utf8',
).trim();

test('the page arrives with the server\'s policy', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeVisible();
  const sent = await browser.evaluate(async () => (await fetch(location.href)).headers.get('content-security-policy'));
  expect(sent).toBe(POLICY);
});

test('a script, an image or an evaluation the policy refuses fails the test', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeVisible();
  // DevTools' own evaluation may evaluate strings: the page's code runs in a later task.
  const evaluated = await browser.evaluate(() => new Promise<string>((resolve) => setTimeout(() => {
    const script = document.createElement('script');
    script.textContent = 'window.__ranInline = true;';
    document.body.append(script);
    const image = document.createElement('img');
    image.src = 'https://example.invalid/pixel.png';
    document.body.append(image);
    try {
      resolve(String(new Function('return "ran"')()));
    } catch (error) {
      resolve((error as Error).name);
    }
  }, 0)));
  expect(evaluated).toBe('EvalError');
  expect(await browser.evaluate(() => (window as { __ranInline?: boolean }).__ranInline ?? null)).toBeNull();
  await expect.poll(() => pageErrors(browser)).toEqual(expect.arrayContaining([
    'refused by the policy: script-src-elem inline',
    'refused by the policy: img-src https://example.invalid/pixel.png',
    'refused by the policy: script-src eval',
  ]));
  // The refusals were this test's own: the page's error list is emptied for the check after it.
  await browser.evaluate(() => { (window as { __e2eErrors?: string[] }).__e2eErrors!.length = 0; });
});
