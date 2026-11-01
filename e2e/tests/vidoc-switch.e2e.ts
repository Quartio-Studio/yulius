import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// A project's Vidoc switch (SPEC-338): the superadmin turns a project's scans off and on in the
// Instance page's Vidoc card (VOF-005), and the Add sandbox dialog offers it when the instance has
// a Vidoc key (VOF-004).

type Mocked = { __yuliusMock: { sandboxes: { slug: string; profile: { vidoc?: boolean } }[]; resyncNow: () => void } };

/** The mock's stored switch of the sandbox `slug` (`null`: not read through the API yet). */
const switchOf = (browser: Browser, slug: string) => browser.evaluate((s: string) => {
  const sandbox = (window as unknown as Mocked).__yuliusMock.sandboxes.find((sb) => sb.slug === s);
  return sandbox ? (sandbox.profile.vidoc ?? null) : 'missing';
}, slug);

test('the superadmin switches a project’s Vidoc scans off and on in the Vidoc card', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&vidoc=config'));
  const card = screen.getByRole('region', 'Vidoc key');
  await expect(card.getByText(/A project switched off scans nothing/)).toBeVisible();
  const studio = screen.getByRole('switch', 'Vidoc scans for studio');
  await expect(studio).toBeChecked();
  await expect(screen.getByRole('switch', 'Vidoc scans for product')).toBeChecked();
  expect(await axeViolations(browser)).toEqual([]);

  await studio.click();
  await expect(screen.getByText(/Vidoc scans are off for studio/)).toBeVisible();
  await expect(studio).not.toBeChecked();
  expect(await switchOf(browser, 'studio')).toBe(false);
  expect(await switchOf(browser, 'product')).toBe(true);

  await studio.click();
  await expect(screen.getByText(/Vidoc scans are on for studio/)).toBeVisible();
  await expect(studio).toBeChecked();
  expect(await switchOf(browser, 'studio')).toBe(true);
});

test('a project imported with the Vidoc box unchecked starts switched off', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&vidoc=config'));
  await screen.getByRole('button', 'Import repository').click();
  const dialog = screen.getByRole('dialog', 'Add sandbox');
  const scan = dialog.getByRole('checkbox', 'Scan pull requests with Vidoc');
  await expect(scan).toBeChecked();
  expect(await axeViolations(browser)).toEqual([]);
  await scan.click();
  await expect(scan).not.toBeChecked();
  await screen.getByRole('radio', /Quartio-Studio\/website/).check();
  await dialog.getByRole('button', 'Import').click();
  await expect(dialog).toBeHidden();
  expect(await switchOf(browser, 'website')).toBe(false);
});

test('a project imported as offered keeps scanning, and without a key nothing is offered', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&vidoc=config'));
  await screen.getByRole('button', 'Import repository').click();
  let dialog = screen.getByRole('dialog', 'Add sandbox');
  await expect(dialog.getByRole('checkbox', 'Scan pull requests with Vidoc')).toBeChecked();
  await screen.getByRole('radio', /Quartio-Studio\/website/).check();
  await dialog.getByRole('button', 'Import').click();
  await expect(dialog).toBeHidden();
  expect(await switchOf(browser, 'website')).toBe(true);

  await app.open(mock('/'));
  await screen.getByRole('button', 'Import repository').click();
  dialog = screen.getByRole('dialog', 'Add sandbox');
  await expect(screen.getByRole('radio', /Quartio-Studio\/website/)).toBeVisible();
  await expect(dialog.getByRole('checkbox', 'Scan pull requests with Vidoc')).toHaveCount(0);
});

test('the Vidoc card follows switches changed elsewhere, and a saved key offers the box', async ({ app, browser, screen }) => {
  // Review round 2: the card keeps up with the sandbox list without a reload.
  await app.open(mock('/admin/instance', '&vidoc=config'));
  const product = screen.getByRole('switch', 'Vidoc scans for product');
  await expect(product).toBeChecked();
  await browser.evaluate(() => {
    const db = (window as unknown as Mocked).__yuliusMock;
    db.sandboxes.find((sb) => sb.slug === 'product')!.profile.vidoc = false;
    db.resyncNow();
  });
  await expect(product).not.toBeChecked();

  // No key yet: nothing offered; once one is saved here, the Add sandbox dialog offers the box.
  await app.open(mock('/admin/instance'));
  await expect(screen.getByText(/No key: security scans are off/)).toBeVisible();
  await browser.locator('#settings-vidoc').fill('vidoc-good-key-0003');
  await screen.getByRole('region', 'Vidoc key').getByRole('button', 'Save').click();
  await expect(screen.getByText(/vidoc-1 is available/)).toBeVisible();
  // Home in the same page (no reload), where the dialog opens.
  await browser.evaluate(() => { location.hash = '#/'; return null; });
  await screen.getByRole('button', 'Import repository').click();
  await expect(screen.getByRole('dialog', 'Add sandbox').getByRole('checkbox', 'Scan pull requests with Vidoc')).toBeChecked();
});

test('the Vidoc card stops listening once its page is gone, even with no event while shown', async ({ app, browser }) => {
  // Review round 3 (unit, in the page): the listener of a card left without any sandbox event must
  // not outlive it.
  await app.open(mock('/admin/instance', '&vidoc=config'));
  const result = await browser.evaluate(async () => {
    type Projects = (ctx: unknown) => HTMLElement;
    const { vidocProjects } = await import('/assets/instancekeys.js' as string) as { vidocProjects: Projects };
    let listeners = 0;
    let handler: (() => void) | null = null;
    const ctx = {
      store: {
        state: { sandboxes: [{ id: 'a', name: 'alpha', profile: { vidoc: true } }] },
        on: (_topic: string, fn: () => void) => { listeners += 1; handler = fn; return () => { listeners -= 1; }; },
      },
      api: {},
    };
    const box = vidocProjects(ctx);
    document.body.append(box);
    await new Promise((resolve) => setTimeout(resolve, 0));
    box.remove();
    (handler as unknown as () => void)();
    return { listeners, drawn: box.querySelectorAll('input').length };
  });
  expect(result).toEqual({ listeners: 0, drawn: 1 });
});

test('a Vidoc card left before its key loaded never starts listening', async ({ app, browser }) => {
  // Review round 3, follow-up (unit, in the page): the page is left while the key's state loads.
  await app.open(mock('/admin/instance', '&vidoc=config'));
  const listeners = await browser.evaluate(async () => {
    type Card = (ctx: unknown, service: string) => HTMLElement;
    const { keySettingsCard } = await import('/assets/instancekeys.js' as string) as { keySettingsCard: Card };
    let count = 0;
    let loaded: (value: unknown) => void = () => {};
    const ctx = {
      store: {
        state: { me: null, sandboxes: [{ id: 'a', name: 'alpha', profile: { vidoc: true } }] },
        on: () => { count += 1; return () => { count -= 1; }; },
      },
      api: { get: () => new Promise((resolve) => { loaded = resolve; }) },
    };
    const card = keySettingsCard(ctx, 'vidoc');
    document.body.append(card);
    card.remove();
    loaded({ configured: true, source: 'config', set_at: null, set_by: null, config_present: true, config_error: null });
    await new Promise((resolve) => setTimeout(resolve, 0));
    return count;
  });
  expect(listeners).toBe(0);
});

test('the superadmin switches Vidoc off for the whole instance and on again, keeping the key', async ({ app, browser, screen }) => {
  // SPEC-356 VIO-001, VIO-005: one switch for every project; the projects keep their own choice.
  await app.open(mock('/admin/instance', '&vidoc=config'));
  const card = screen.getByRole('region', 'Vidoc key');
  const instance = screen.getByRole('switch', 'Vidoc scans for every project');
  await expect(instance).toBeChecked();
  const studio = screen.getByRole('switch', 'Vidoc scans for studio');
  await expect(studio).toBeEnabled();

  await instance.click();
  await expect(screen.getByText(/Vidoc is off for every project/)).toBeVisible();
  await expect(instance).not.toBeChecked();
  await expect(card.getByText(/Off for every project since/)).toBeVisible();
  await expect(screen.getByRole('switch', 'Vidoc scans for studio')).toBeDisabled();
  await expect(screen.getByRole('switch', 'Vidoc scans for studio')).toBeChecked();
  await expect(card.getByText(/Kept, not used while Vidoc is off: the key from the server’s configuration/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  // Off for the instance: the Add sandbox dialog offers no Vidoc box.
  await browser.evaluate(() => { location.hash = '#/'; return null; });
  await screen.getByRole('button', 'Import repository').click();
  await expect(screen.getByRole('radio', /Quartio-Studio\/website/)).toBeVisible();
  await expect(screen.getByRole('dialog', 'Add sandbox').getByRole('checkbox', 'Scan pull requests with Vidoc')).toHaveCount(0);

  await app.open(mock('/admin/instance', '&vidoc=config&vidoc_off=1'));
  const again = screen.getByRole('switch', 'Vidoc scans for every project');
  await expect(again).not.toBeChecked();
  await again.click();
  await expect(screen.getByText(/Vidoc scans are on again/)).toBeVisible();
  await expect(again).toBeChecked();
  await expect(screen.getByRole('switch', 'Vidoc scans for studio')).toBeEnabled();
  expect(await switchOf(browser, 'studio')).toBe(true);
});

test('a failed instance switch goes back and says why', async ({ app, screen }) => {
  // SPEC-356 (review round 1): the server refuses the change; nothing moves.
  await app.open(mock('/admin/instance', '&vidoc=config&vidoc_fail=1'));
  const instance = screen.getByRole('switch', 'Vidoc scans for every project');
  await expect(instance).toBeChecked();
  await instance.click();
  await expect(screen.getByText(/Couldn’t switch Vidoc/)).toBeVisible();
  await expect(instance).toBeChecked();
  await expect(instance).toBeEnabled();
  await expect(screen.getByRole('switch', 'Vidoc scans for studio')).toBeEnabled();
});

// SPEC-359: the sandbox's own Security scan card on its CI/CD page, the same switch.

test('a sandbox admin switches its Vidoc scans off and on from the CI/CD page, and the Instance card agrees', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci', '&vidoc=config'));
  const card = screen.getByRole('region', 'Security scan (Vidoc)');
  const scan = card.getByRole('switch', 'Scan this project with Vidoc');
  await expect(scan).toBeChecked();
  await expect(scan).toBeEnabled();
  await expect(card.getByText(/agents scan their changes/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await scan.click();
  await expect(screen.getByText(/Vidoc scans are off for studio/)).toBeVisible();
  await expect(scan).not.toBeChecked();
  await expect(card.getByText(/nobody is asked about the key or its quota/)).toBeVisible();
  await expect(card.getByText(/Last changed by mkarp/)).toBeVisible();
  expect(await switchOf(browser, 'studio')).toBe(false);

  // The Instance page's card shows the same switch, and turns it on again.
  await browser.evaluate(() => { location.hash = '#/admin/instance'; return null; });
  const studio = screen.getByRole('switch', 'Vidoc scans for studio');
  await expect(studio).not.toBeChecked();
  await studio.click();
  await expect(studio).toBeChecked();
  await browser.evaluate(() => { location.hash = '#/s/studio/ci'; return null; });
  await expect(screen.getByRole('region', 'Security scan (Vidoc)').getByRole('switch', 'Scan this project with Vidoc')).toBeChecked();
});

test('a member without admin access sees the sandbox’s Vidoc switch but cannot change it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci', '&vidoc=config&role=member'));
  const card = screen.getByRole('region', 'Security scan (Vidoc)');
  const scan = card.getByRole('switch', 'Scan this project with Vidoc');
  await expect(scan).toBeChecked();
  await expect(scan).toBeDisabled();
  await expect(card.getByText(/Only this sandbox’s admins/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
});

test('a repository that turned Vidoc off in .config/vidoc.toml is said, and the switch offers no way on', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci', '&vidoc=config&vidoc_repo=off'));
  const card = screen.getByRole('region', 'Security scan (Vidoc)');
  const scan = card.getByRole('switch', 'Scan this project with Vidoc');
  await expect(card.getByText(/The repository turned Vidoc off/)).toBeVisible();
  await expect(scan).not.toBeChecked();
  await expect(scan).toBeDisabled();
  expect(await axeViolations(browser)).toEqual([]);
});

test('the sandbox’s card says when the instance switched Vidoc off, keeping the project’s choice', async ({ app, screen }) => {
  await app.open(mock('/s/studio/ci', '&vidoc=config&vidoc_off=1'));
  const card = screen.getByRole('region', 'Security scan (Vidoc)');
  await expect(card.getByText(/A superadmin switched Vidoc off for the whole instance/)).toBeVisible();
  const scan = card.getByRole('switch', 'Scan this project with Vidoc');
  await expect(scan).toBeChecked();
  await expect(scan).toBeDisabled();
});

test('the sandbox’s switch keeps the keyboard, and a refused change goes back and says why', async ({ app, browser, screen }) => {
  // SPEC-359 review round 1: VSB-004's failure and focus.
  await app.open(mock('/s/studio/ci', '&vidoc=config'));
  const scan = screen.getByRole('region', 'Security scan (Vidoc)').getByRole('switch', 'Scan this project with Vidoc');
  await scan.click();
  await expect(scan).not.toBeChecked();
  await expect(scan).toBeEnabled();
  const focused = await browser.evaluate(() => (document.activeElement as HTMLElement | null)?.getAttribute('aria-label') ?? null);
  expect(focused).toBe('Scan this project with Vidoc');

  await app.open(mock('/s/studio/ci', '&vidoc=config&vidoc_patch_fail=1'));
  const refused = screen.getByRole('region', 'Security scan (Vidoc)').getByRole('switch', 'Scan this project with Vidoc');
  await refused.click();
  await expect(screen.getByText(/Couldn’t change the Vidoc scans/)).toBeVisible();
  await expect(refused).toBeChecked();
  await expect(refused).toBeEnabled();
});

test('the sandbox’s card says an unreadable repository file and a missing key, and follows changes made elsewhere', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci', '&vidoc=config&vidoc_repo=bad'));
  let card = screen.getByRole('region', 'Security scan (Vidoc)');
  await expect(card.getByText(/must set enabled = true or enabled = false .* scans end “not run” until it is fixed/)).toBeVisible();
  await expect(card.getByRole('switch', 'Scan this project with Vidoc')).toBeEnabled();
  expect(await axeViolations(browser)).toEqual([]);

  await app.open(mock('/s/studio/ci'));
  card = screen.getByRole('region', 'Security scan (Vidoc)');
  await expect(card.getByText(/no Vidoc key reaches this project/)).toBeVisible();

  // Switched off elsewhere (the Instance card in another tab, a live update) while the card is shown.
  const scan = card.getByRole('switch', 'Scan this project with Vidoc');
  await expect(scan).toBeChecked();
  await browser.evaluate(() => {
    const db = (window as unknown as Mocked).__yuliusMock;
    db.sandboxes.find((sb) => sb.slug === 'studio')!.profile.vidoc = false;
    db.resyncNow();
  });
  await expect(scan).not.toBeChecked();
  await expect(card.getByText(/^Off: agents and CI scan nothing/)).toBeVisible();
});

type Counted = { __yuliusMock: { ciReads?: number; vidocReads?: number; heldWaiting?: number; release: () => void } };
const reads = (browser: Browser) => browser.evaluate(() => {
  const db = (window as unknown as Counted).__yuliusMock;
  return { ci: db.ciReads || 0, vidoc: db.vidocReads || 0 };
});

test('refreshing the CI runs does not read the Vidoc switch again', async ({ app, browser, screen }) => {
  // SPEC-359 review round 4: the view costs Git reads on the server; it changes only with a switch.
  await app.open(mock('/s/studio/ci', '&vidoc=config'));
  await expect(screen.getByRole('region', 'Security scan (Vidoc)')).toBeVisible();
  // The Now summary reads the runs too: counted from here.
  const start = await reads(browser);
  expect(start.vidoc).toBe(1);
  const check = screen.getByRole('button', 'Check for commits');
  await check.click();
  await expect.poll(async () => (await reads(browser)).ci).toBeGreaterThan(start.ci);
  await expect(check).toBeEnabled();
  const once = (await reads(browser)).ci;
  await check.click();
  await expect.poll(async () => (await reads(browser)).ci).toBeGreaterThan(once);
  expect((await reads(browser)).vidoc).toBe(1);
  // Its own change reads it again.
  await screen.getByRole('switch', 'Scan this project with Vidoc').click();
  await expect.poll(async () => (await reads(browser)).vidoc).toBe(2);
});

test('a refresh while the switch is sent shows the change on its way, disabled', async ({ app, browser, screen }) => {
  // SPEC-359 review round 4: the `pending` state.
  await app.open(mock('/s/studio/ci', '&vidoc=config&holdpost=/api/sandboxes/sb_studio'));
  const scan = () => screen.getByRole('region', 'Security scan (Vidoc)').getByRole('switch', 'Scan this project with Vidoc');
  await scan().click();
  await expect.poll(() => browser.evaluate(() => (window as unknown as Counted).__yuliusMock.heldWaiting || 0)).toBe(1);
  const before = (await reads(browser)).ci;
  await screen.getByRole('button', 'Check for commits').click();
  await expect.poll(async () => (await reads(browser)).ci).toBeGreaterThan(before);
  await expect(scan()).not.toBeChecked();
  await expect(scan()).toBeDisabled();
  await browser.evaluate(() => { (window as unknown as Counted).__yuliusMock.release(); return null; });
  await expect(screen.getByText(/Vidoc scans are off for studio/)).toBeVisible();
  await expect(scan()).not.toBeChecked();
  await expect(scan()).toBeEnabled();
  expect(await switchOf(browser, 'studio')).toBe(false);
});

type Holding = {
  __yuliusMock: { holdVidocReads?: boolean; vidocHolds: unknown[]; releaseVidoc: () => void; resyncNow: () => void; asked?: Record<string, number> };
};
const held = (browser: Browser) => browser.evaluate(() => (window as unknown as Holding).__yuliusMock.vidocHolds.length);
/** The Vidoc reads the page has asked for, counted as it asks (before the mock's latency). */
const askedVidoc = (browser: Browser) => browser.evaluate(() => (window as unknown as Holding).__yuliusMock.asked?.['/api/sandboxes/sb_studio/vidoc'] ?? 0);

// SPEC-359 review round 5: the read after a change was dropped for a newer refresh, which drew the
// view from before the change. #1002: while that read is held, the page reads the view again by
// itself (the sandbox list reloaded, its own poll), so the refresh may come after other rereads.
for (const rereads of [0, 2]) {
  const title = rereads
    ? 'a refresh after the page’s own rereads still shows the change'
    : 'a refresh that overtakes the read after a change still shows the change';
  test(title, async ({ app, browser, screen }) => {
    await app.open(mock('/s/studio/ci', '&vidoc=config'));
    const scan = () => screen.getByRole('region', 'Security scan (Vidoc)').getByRole('switch', 'Scan this project with Vidoc');
    await expect(scan()).toBeChecked();
    await browser.evaluate(() => { (window as unknown as Holding).__yuliusMock.holdVidocReads = true; return null; });
    await scan().click();
    await expect.poll(() => held(browser)).toBeGreaterThanOrEqual(1);
    // The sandbox list reloaded (a resync): the card, which still shows the view from before the
    // change, reads it again, held too.
    for (let n = 1; n <= rereads; n += 1) {
      const before = await held(browser);
      await browser.evaluate(() => { (window as unknown as Holding).__yuliusMock.resyncNow(); return null; });
      await expect.poll(() => held(browser)).toBeGreaterThan(before);
    }
    // The card drawn now (the change, sent); a newer refresh either draws it again or reads anew.
    await browser.evaluate(() => { (document.querySelector('#vidoc') as HTMLElement).dataset.drawn = 'before'; return null; });
    const asked = await askedVidoc(browser);
    const check = screen.getByRole('button', 'Check for commits');
    await check.click();
    // The refresh has started once the button is back (its look answered, the reload called).
    // Other reads may come before or after its own, so they are counted from the click, not to an
    // exact number (#1002).
    await expect(check).toBeEnabled();
    await expect.poll(() => browser.evaluate((n: number) => {
      const redrawn = (document.querySelector('#vidoc') as HTMLElement | null)?.dataset.drawn !== 'before';
      return redrawn || ((window as unknown as Holding).__yuliusMock.asked?.['/api/sandboxes/sb_studio/vidoc'] ?? 0) > n;
    }, asked)).toBe(true);
    await expect(scan()).not.toBeChecked();
    await browser.evaluate(() => {
      const db = (window as unknown as Holding).__yuliusMock;
      db.holdVidocReads = false;
      db.releaseVidoc();
      return null;
    });
    await expect(scan()).not.toBeChecked();
    await expect(scan()).toBeEnabled();
    await expect(screen.getByRole('region', 'Security scan (Vidoc)').getByText(/^Off: agents and CI scan nothing/)).toBeVisible();
  });
}

// SPEC-364 (#997): the card's switch sits at its right edge, like the Settings rows, not halfway
// across a wide card.
for (const viewport of [{ width: 1600, height: 900 }, { width: 390, height: 844 }]) {
  test(`the Security scan card’s label is at its left and its switch at its right at ${viewport.width} px`, async ({ app, browser, screen }) => {
    await browser.setViewport(viewport);
    await app.open(mock('/s/studio/ci', '&vidoc=config'));
    await expect(screen.getByRole('region', 'Security scan (Vidoc)').getByRole('switch', 'Scan this project with Vidoc')).toBeVisible();
    const edges = await browser.evaluate(() => {
      const card = document.querySelector('#vidoc')!;
      const style = getComputedStyle(card);
      const box = card.getBoundingClientRect();
      const left = box.left + parseFloat(style.borderLeftWidth) + parseFloat(style.paddingLeft);
      const right = box.right - parseFloat(style.borderRightWidth) - parseFloat(style.paddingRight);
      const name = card.querySelector('.vidoc-project-name')!;
      const label = name.getBoundingClientRect();
      const control = card.querySelector('.switch')!.getBoundingClientRect();
      return {
        labelGap: Math.round(label.left - left),
        switchGap: Math.round(right - control.right),
        labelLines: Math.round(label.height / parseFloat(getComputedStyle(name).lineHeight)),
      };
    });
    expect(edges).toEqual({ labelGap: 0, switchGap: 0, labelLines: 1 });
  });
}
