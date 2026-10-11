import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Skills (SPEC-001 SKL-005, SKL-006; #136): "Skills should be reduced". Two groups instead of
// filters: the curated set included with Yulius (read-only, with its source and a thank-you to
// Open Mercato), then this repository's and the instance's own skills.

test('Skills shows the included set read-only with its source, then custom skills', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/skills'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Skills');
  await expect(screen.getByRole('heading', /^Included/)).toBeVisible();
  await expect(screen.getByRole('heading', /^Custom/)).toBeVisible();
  await expect(screen.getByText(/Thank you to the Open Mercato team/)).toBeVisible();
  await expect(screen.getByRole('link', 'Open Mercato skills')).toHaveAttribute('href', 'https://github.com/open-mercato/skills');
  // No filter tabs or search: the list is short.
  expect(await browser.evaluate(() => document.querySelectorAll('#main [role=tablist], #main input[type=search]').length)).toBe(0);
  const order = await browser.evaluate(() => [...document.querySelectorAll('#main .item-name')].map((n) => n.textContent?.trim()));
  expect(order).toEqual(['om-code-review', 'om-testing', 'yulius-rust-slice', 'design', 'quartio-brand']);

  // An included skill opens read-only.
  await screen.getByRole('button', 'Open skill om-code-review').click();
  await expect(screen.getByText(/Read-only: included skills ship with Yulius and stay current with it\./)).toBeVisible();
  await expect(screen.getByRole('button', 'Save')).toBeHidden();
});
