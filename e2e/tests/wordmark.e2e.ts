// The wordmark is set like the composer's quote (SPEC-123, task #110): the same face, weight, size,
// line height and letter spacing, and the name as written ("Yulius"), not in spaced capitals.
import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

const TYPE = ['fontFamily', 'fontWeight', 'fontSize', 'lineHeight', 'letterSpacing', 'textTransform', 'color'] as const;

/** The computed type of the first element matching `selector`, and its text as rendered. */
const typeOf = (browser: Browser, selector: string) => browser.evaluate(({ selector, keys }) => {
  const el = document.querySelector<HTMLElement>(selector)!;
  const style = getComputedStyle(el);
  return { text: el.innerText.trim(), ...Object.fromEntries(keys.map((key) => [key, style[key]])) };
}, { selector, keys: [...TYPE] });

/** Whether the self-hosted Tenor Sans (an @font-face of tokens.css) is loaded at its one weight, 400. */
const tenorLoaded = (browser: Browser) => browser.evaluate(async () => {
  await document.fonts.ready;
  return [...document.fonts].some((face) => face.family.replace(/"/g, '') === 'Tenor Sans' && face.weight === '400' && face.status === 'loaded');
});

test('the sidebar wordmark is set like the composer quote', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  await expect(browser.locator('.composer-quote blockquote')).toBeVisible();
  await expect(browser.locator('.sidebar .brand-name')).toBeVisible();

  const quote = await typeOf(browser, '.composer-quote blockquote');
  const wordmark = await typeOf(browser, '.sidebar .brand-name');
  // The quote's own text differs; everything that sets its type is the wordmark's.
  expect({ ...wordmark, text: undefined }).toEqual({ ...quote, text: undefined });
  expect(wordmark.text).toBe('Yulius');
  expect(wordmark.fontWeight).toBe('400');
  expect(wordmark.textTransform).toBe('none');
  expect(wordmark.fontFamily).toMatch(/^"Tenor Sans"/);
  expect(await tenorLoaded(browser)).toBe(true);
});

test('the sign-in wordmark is set like the composer quote', async ({ app, browser, screen }) => {
  // The composer's quote first, to compare against: the sign-in screen has none.
  await app.open(mock('/s/studio'));
  await expect(browser.locator('.composer-quote blockquote')).toBeVisible();
  const quote = await typeOf(browser, '.composer-quote blockquote');

  await app.open(mock('/', '&signedout=1'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sign in');
  const wordmark = await typeOf(browser, '.auth-logo');
  expect({ ...wordmark, text: undefined }).toEqual({ ...quote, text: undefined });
  expect(wordmark.text).toBe('Yulius');
  expect(await tenorLoaded(browser)).toBe(true);
});

/** How far the sidebar brand's name and tagline run past the sidebar's right edge (0 when inside). */
const brandOverflow = (browser: Browser) => browser.evaluate(() => {
  const side = document.querySelector('.sidebar')!.getBoundingClientRect();
  const parts = [...document.querySelectorAll('.sidebar .brand-name, .sidebar .brand-by')];
  return Math.max(0, ...parts.map((el) => el.getBoundingClientRect().right - side.right));
});

test('the sidebar brand fits a narrow sidebar and the phone drawer', async ({ app, browser, screen }) => {
  // Below 1024 px the sidebar is 200 px wide.
  await browser.setViewport({ width: 1000, height: 800 });
  await app.open(mock('/s/studio'));
  await expect(browser.locator('.sidebar .brand-name')).toBeVisible();
  expect(await brandOverflow(browser)).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);

  await browser.setViewport({ width: 390, height: 844 });
  const menu = screen.getByRole('button', 'Open navigation');
  await menu.click();
  await expect(menu).toHaveAttribute('aria-expanded', 'true');
  await expect(browser.locator('.sidebar .brand-name')).toBeVisible();
  expect(await brandOverflow(browser)).toBe(0);
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
});

// Review round 1: the brand lab's Roma proposals (the default look at four contrast levels, UI-041)
// tracked the wordmark for capitals, so `?lab=roma` spaced the mixed-case name apart from the quote.
test('the brand lab keeps the wordmark set like the quote in every Roma proposal', async ({ app, browser }) => {
  for (const id of ['roma', 'roma-1', 'roma-2', 'roma-3']) {
    await app.open(mock('/s/studio', `&lab=${id}`));
    await expect(browser.locator('html')).toHaveAttribute('data-brand', /^roma/);
    await expect(browser.locator('.composer-quote blockquote')).toBeVisible();
    const quote = await typeOf(browser, '.composer-quote blockquote');
    const wordmark = await typeOf(browser, '.sidebar .brand-name');
    expect({ id, letterSpacing: wordmark.letterSpacing, textTransform: wordmark.textTransform })
      .toEqual({ id, letterSpacing: quote.letterSpacing, textTransform: quote.textTransform });
  }
});
