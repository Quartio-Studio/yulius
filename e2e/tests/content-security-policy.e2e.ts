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

// Review round 1, F1: creating the GitHub App posts the manifest from a blank tab the page opens
// (`admin.js` `postManifest`); that tab inherits the page's policy, and GitHub's address is
// configurable, so no `form-action` may stand in the way.
test('a form the page posts to another site from a tab it opened is sent', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeVisible();
  await browser.evaluate(() => {
    const w = window as unknown as { __tab: Window | null; __e2eErrors: string[] };
    const button = document.createElement('button');
    button.id = 'csp-post-probe';
    button.textContent = 'Post elsewhere';
    button.addEventListener('click', () => {
      const tab = window.open('', '_blank');
      w.__tab = tab;
      if (!tab) return;
      tab.document.addEventListener('securitypolicyviolation', (event) => w.__e2eErrors.push(`refused in the tab: ${event.effectiveDirective} ${event.blockedURI}`));
      const form = tab.document.createElement('form');
      form.method = 'post';
      // Another origin: the same server by another name.
      form.action = `http://localhost:${location.port}/settings/apps/new`;
      const input = tab.document.createElement('input');
      input.type = 'hidden';
      input.name = 'manifest';
      input.value = '{}';
      form.append(input);
      tab.document.body.append(form);
      form.submit();
    });
    document.body.append(button);
  });
  await browser.locator('#csp-post-probe').click();
  // Once the tab has left `about:blank` for the other origin, its address can no longer be read.
  await expect.poll(() => browser.evaluate(() => {
    const tab = (window as unknown as { __tab: Window | null }).__tab;
    if (!tab) return 'no tab';
    try {
      return tab.location.href === 'about:blank' ? 'still blank' : 'same origin';
    } catch {
      return 'left for the other site';
    }
  })).toBe('left for the other site');
  await browser.evaluate(() => (window as unknown as { __tab: Window | null }).__tab?.close());
});

// Review round 1, F2: a recording attached to a draft or a queued message opens from a blob: URL.
test('a recording plays from a blob: URL', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeVisible();
  const state = await browser.evaluate(() => new Promise<string>((resolve) => setTimeout(() => {
    // A tenth of a second of silence: a 44-byte WAV header and 800 samples of 8-bit PCM.
    const samples = 800;
    const bytes = new Uint8Array(44 + samples).fill(128, 44);
    const view = new DataView(bytes.buffer);
    const ascii = (at: number, text: string) => [...text].forEach((c, i) => view.setUint8(at + i, c.charCodeAt(0)));
    ascii(0, 'RIFF'); view.setUint32(4, 36 + samples, true); ascii(8, 'WAVE'); ascii(12, 'fmt ');
    view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true);
    view.setUint32(24, 8000, true); view.setUint32(28, 8000, true); view.setUint16(32, 1, true);
    view.setUint16(34, 8, true); ascii(36, 'data'); view.setUint32(40, samples, true);
    const audio = document.createElement('audio');
    audio.addEventListener('loadedmetadata', () => resolve(`loaded ${audio.duration.toFixed(1)} s`));
    audio.addEventListener('error', () => resolve(`error ${audio.error?.code ?? '?'}`));
    audio.src = URL.createObjectURL(new Blob([bytes], { type: 'audio/wav' }));
  }, 0)));
  expect(state).toBe('loaded 0.1 s');
});
