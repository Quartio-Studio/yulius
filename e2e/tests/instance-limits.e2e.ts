import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Settings → Instance, "Resource limits" (SPEC-459 HSL-008): the host's size and each limit with
// its value, where it comes from and the host-sized formula; a superadmin saves a number, goes
// back to host-sized or removes the setting. On a host of another size than the one a number was
// pinned on (`limits=moved`: the unit's 8 sessions and 300 GB from a 12-CPU host, on a 64-CPU
// one) a notice lists the limits that no longer fit and takes the host-sized values or keeps them.

/** What the row of the limit `key` says. */
const row = (browser: Browser, key: string) => browser.evaluate((k: string) => {
  const el = document.querySelector<HTMLElement>(`[data-limit="${k}"]`);
  return el ? el.innerText.replace(/\s+/g, ' ') : 'missing';
}, key);

test('the card shows the host and each limit with its source and formula', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  await expect(screen.getByRole('heading', { level: 2, name: 'Resource limits' })).toBeVisible();
  const card = screen.getByRole('region', 'Resource limits');
  await expect(card.getByText(/64 CPUs · 755 GiB memory · 3\.5 TB data disk/)).toBeVisible();
  const sessions = await row(browser, 'max_sessions');
  expect(sessions).toMatch(/Live sessions 34 host-sized/);
  expect(sessions).toMatch(/half the CPUs plus 2, at most one per 2 GiB of memory, at least 2/);
  expect(await row(browser, 'sandbox_memory_gb')).toMatch(/Sandbox memory 660 GiB host-sized/);
  expect(await row(browser, 'sandbox_disk_gb')).toMatch(/Sandbox disk quota 875 GB host-sized/);
  expect(await browser.locator('[data-limit]').count()).toBe(6);
  await expect(card.getByText(/sized for a host of another size/)).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);
});

test('a saved number wins, a refused one says its range, and the setting can be removed', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  const input = screen.getByRole('spinbutton', 'Live sessions');
  await expect(input).toBeVisible();
  await input.fill('40');
  await screen.getByRole('button', 'Save Live sessions').click();
  await expect.poll(() => row(browser, 'max_sessions')).toMatch(/Live sessions 40 settings/);
  expect(await row(browser, 'max_sessions')).toMatch(/Saved here by .+\. Host-sized would be 34/);

  await input.fill('0');
  await screen.getByRole('button', 'Save Live sessions').click();
  await expect(screen.getByText(/Live sessions must be between 1 and 1024/)).toBeVisible();
  expect(await row(browser, 'max_sessions')).toMatch(/Live sessions 40 settings/);

  await screen.getByRole('button', 'Remove setting of Live sessions').click();
  await expect.poll(() => row(browser, 'max_sessions')).toMatch(/Live sessions 34 host-sized/);
  expect(await axeViolations(browser)).toEqual([]);
});

test('a number that is not a whole one is refused at its field without a request', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  const input = screen.getByRole('spinbutton', 'Live sessions');
  await expect(input).toBeVisible();
  for (const typed of ['1.5', '-3']) {
    await input.fill(typed);
    await screen.getByRole('button', 'Save Live sessions').click();
    await expect(screen.getByText(/Live sessions must be a whole number between 1 and 1024/)).toBeVisible();
    // The message belongs to the field: a screen reader reads it with the input.
    const field = await browser.evaluate(() => {
      const el = document.querySelector<HTMLInputElement>('[data-limit="max_sessions"] input');
      const said = el ? document.getElementById(el.getAttribute('aria-describedby') || '') : null;
      return { invalid: el ? el.getAttribute('aria-invalid') : null, said: said ? said.textContent : null, typed: el ? el.value : null };
    });
    expect(field.invalid).toBe('true');
    expect(field.said).toMatch(/whole number/);
    expect(field.typed).toBe(typed);
    expect(await row(browser, 'max_sessions')).toMatch(/Live sessions 34 host-sized/);
  }
  expect(await axeViolations(browser)).toEqual([]);
});

test('limits that could not be loaded say so with a way to try again', async ({ app, screen }) => {
  await app.open(mock('/admin/instance', '&fail=/api/admin/limits'));
  const card = screen.getByRole('region', 'Resource limits');
  await expect(card.getByText(/Couldn’t load the resource limits/)).toBeVisible();
  await expect(card.getByRole('button', 'Try again')).toBeVisible();
});

test('a changed sandbox memory says it applies at the next restart', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  await screen.getByRole('spinbutton', 'Sandbox memory').fill('100');
  await screen.getByRole('button', 'Save Sandbox memory').click();
  await expect.poll(() => row(browser, 'sandbox_memory_gb')).toMatch(/Sandbox memory 100 GiB settings/);
  expect(await row(browser, 'sandbox_memory_gb')).toMatch(/Applies at the next restart of the server/);
});

test('on a moved host the notice lists the limits that no longer fit and takes the host-sized values', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&limits=moved'));
  const card = screen.getByRole('region', 'Resource limits');
  const notice = card.getByRole('status', 'Limits sized for another host');
  await expect(notice).toBeVisible();
  await expect(notice.getByText(/Live sessions: 8, configured on 12 CPUs · 47 GiB memory · 387 GB data disk; this host would get 34/)).toBeVisible();
  await expect(notice.getByText(/Sandbox disk quota: 300 GB, .*this host would get 875 GB/)).toBeVisible();
  expect(await row(browser, 'max_sessions')).toMatch(/Live sessions 8 unit/);
  expect(await axeViolations(browser)).toEqual([]);

  await notice.getByRole('button', 'Use host-sized values').click();
  await expect(notice).toBeHidden();
  const sessions = await row(browser, 'max_sessions');
  expect(sessions).toMatch(/Live sessions 34 host-sized/);
  expect(sessions).toMatch(/The unit’s YULIUS_MAX_SESSIONS=8 is overruled/);
  expect(await row(browser, 'sandbox_disk_gb')).toMatch(/Sandbox disk quota 875 GB host-sized/);
});

test('keeping the numbers on a moved host clears the notice and leaves them in force', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&limits=moved'));
  const notice = screen.getByRole('status', 'Limits sized for another host');
  await notice.getByRole('button', 'Keep these numbers').click();
  await expect(notice).toBeHidden();
  expect(await row(browser, 'max_sessions')).toMatch(/Live sessions 8 unit/);
  expect(await row(browser, 'sandbox_disk_gb')).toMatch(/Sandbox disk quota 300 GB unit/);

  // One limit goes back to host-sized by itself, whatever the unit says.
  await screen.getByRole('button', 'Host-sized Live sessions').click();
  await expect.poll(() => row(browser, 'max_sessions')).toMatch(/Live sessions 34 host-sized/);
});

test('members never see the limits', async ({ app, screen }) => {
  await app.open(mock('/admin/instance', '&role=member'));
  await expect(screen.getByText(/Only the superadmin/)).toBeVisible();
  await expect(screen.getByRole('heading', { level: 2, name: 'Resource limits' })).toBeHidden();
});
