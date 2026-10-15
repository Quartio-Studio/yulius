import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The consistency pass (#147, final-stage item 1(d)): one rule per state, one way to say a list is
// empty, failed to load or is missing, one header pattern, and nothing cut off.

/** A sandbox card's "needs you" count on Home. */
const cardCount = (browser: Browser, name: string) =>
  browser.evaluate((n: string) => {
    const card = [...document.querySelectorAll('.home-card')].find((c) => c.querySelector('.home-card-name')?.textContent === n);
    return card ? Number(card.querySelector('.home-count strong')?.textContent) : null;
  }, name);

/** A sandbox tab's "needs you" count (its accessible text says it), 0 without one. */
const tabCount = (browser: Browser, name: string) =>
  browser.evaluate((n: string) => {
    const tab = [...document.querySelectorAll('.tabrow-tabs .tab')].find((t) => t.querySelector('.tab-name')?.textContent === n);
    if (!tab) return null;
    const match = (tab.querySelector('.sr-only')?.textContent || '').match(/(\d+) needs? you/);
    return match ? Number(match[1]) : 0;
  }, name);

/** The sidebar's "Needs you" count (0 without the group), once its tasks are listed. */
const sidebarNeeds = (browser: Browser) =>
  browser.evaluate(() => {
    if (!document.querySelector('.task-list .task-group, .task-list .placeholder')) return null;
    return Number(document.querySelector('.task-group.group-needs .task-group-count')?.textContent || 0);
  });

/** Both sides at the same moment (the mock's tasks change while the test runs). */
const agree = async (a: () => Promise<number | null>, b: () => Promise<number | null>) => {
  const [x, y] = [await a(), await b()];
  return x !== null && x === y;
};

test('Home cards, the sandbox tabs and the sidebar count “needs you” the same way', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  // product's one is a prompt held for a decision, which the old count (activity only) missed;
  // sandbox-3 has nothing waiting.
  await expect.poll(() => cardCount(browser, 'product')).toBe(1);
  await expect.poll(() => cardCount(browser, 'sandbox-3')).toBe(0);
  for (const name of ['studio', 'product', 'sandbox-3']) {
    await expect.poll(() => agree(() => cardCount(browser, name), () => tabCount(browser, name))).toBe(true);
  }
  for (const name of ['studio', 'product', 'sandbox-3']) {
    await app.open(mock(`/s/${name}/tasks`));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
    await expect.poll(() => agree(() => sidebarNeeds(browser), () => tabCount(browser, name))).toBe(true);
  }
});

/** A sandbox card's count labelled `label` ("working", "queued") on Home. */
const cardCountOf = (browser: Browser, name: string, label: string) =>
  browser.evaluate(([n, l]: [string, string]) => {
    const card = [...document.querySelectorAll('.home-card')].find((c) => c.querySelector('.home-card-name')?.textContent === n);
    const item = card && [...card.querySelectorAll('.home-count')].find((c) => c.querySelector('span:last-child')?.textContent === l);
    return item ? Number(item.querySelector('strong')?.textContent) : null;
  }, [name, label] as [string, string]);

/** A sandbox tab's "working" number (its accessible text says it), 0 without one. */
const tabWorking = (browser: Browser, name: string) =>
  browser.evaluate((n: string) => {
    const tab = [...document.querySelectorAll('.tabrow-tabs .tab')].find((t) => t.querySelector('.tab-name')?.textContent === n);
    if (!tab) return null;
    const match = (tab.querySelector('.sr-only')?.textContent || '').match(/(\d+) working/);
    return match ? Number(match[1]) : 0;
  }, name);

/** The sidebar's "Working" count (0 without the group), once its tasks are listed. */
const sidebarWorking = (browser: Browser) =>
  browser.evaluate(() => {
    if (!document.querySelector('.task-list .task-group, .task-list .placeholder')) return null;
    return Number(document.querySelector('.task-group.group-working .task-group-count')?.textContent || 0);
  });

test('a card’s working and queued counts split the sidebar’s Working group', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  // product's chained task is queued, and its working one waits for you (a held prompt), so it is
  // not working; sandbox-3's one task works. The live-process count said product 1 working, 0 queued.
  await expect.poll(() => cardCountOf(browser, 'product', 'working')).toBe(0);
  await expect.poll(() => cardCountOf(browser, 'product', 'queued')).toBe(1);
  await expect.poll(() => cardCountOf(browser, 'sandbox-3', 'working')).toBe(1);
  await expect.poll(() => cardCountOf(browser, 'sandbox-3', 'queued')).toBe(0);
  // The card's working + queued, then the sidebar's Working count, read in the same page (hash
  // navigation keeps the mock's tasks, which move on by themselves meanwhile).
  const split = async (name: string) => {
    await browser.evaluate(() => { window.location.hash = '#/'; });
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
    const [working, queued] = [await cardCountOf(browser, name, 'working'), await cardCountOf(browser, name, 'queued')];
    await browser.evaluate((n: string) => { window.location.hash = `#/s/${n}/tasks`; }, name);
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
    await expect.poll(() => sidebarWorking(browser)).not.toBeNull();
    return working === null || queued === null ? null : working + queued === await sidebarWorking(browser);
  };
  // studio has queued, starting, switching, paused, still-working (background shells), Done and
  // Your-turn tasks: the live-process count said 8 there for a sidebar Working of 6.
  for (const name of ['studio', 'product', 'sandbox-3']) {
    await expect.poll(() => split(name), { timeout: 30000 }).toBe(true);
  }
  // The tabs say the card's working number.
  await browser.evaluate(() => { window.location.hash = '#/'; });
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  for (const name of ['studio', 'product', 'sandbox-3']) {
    await expect.poll(() => agree(() => cardCountOf(browser, name, 'working'), () => tabWorking(browser, name))).toBe(true);
  }
});

test('queued, starting, paused and chained tasks each have their own icon', async ({ app, browser, screen }) => {
  await app.open(mock('/design'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Design system');
  const marks = await browser.evaluate(() => Object.fromEntries(['Queued', 'Starting', 'Paused', 'Queued after #4'].map((label) => {
    const mark = document.querySelector(`.ds-states [role="img"][aria-label="${label}"] svg`);
    return [label, mark ? mark.innerHTML : null];
  })));
  const shapes = Object.values(marks);
  expect(shapes.every(Boolean)).toBe(true);
  expect(new Set(shapes).size).toBe(4);
});

test('empty lists use the one empty state, with what to do next', async ({ app, browser, screen }) => {
  // Tasks: no match for a search.
  await app.open(mock('/s/studio/tasks?q=zzzz'));
  await expect(browser.locator('.tl .empty-state')).toHaveText(/No tasks match “zzzz”/);
  await browser.locator('.tl .empty-state').getByRole('button', 'Clear filters').click();
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();

  // Archived: the last archived task restored.
  await app.open(mock('/archived'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Archived tasks');
  await screen.getByRole('button', 'Restore').click();
  await expect(browser.locator('.empty-state')).toHaveText(/No archived tasks/);
  expect(await axeViolations(browser)).toEqual([]);
});

test('a list that fails to load says what failed, in place, with Try again', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/users', '&fail=/api/admin/users'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Users');
  await expect(screen.getByRole('alert').first()).toHaveText(/Couldn’t load the users: The mock server failed on purpose/);
  await expect(screen.getByRole('button', 'Try again')).toBeVisible();
  // Not a toast.
  expect(await browser.evaluate(() => document.querySelectorAll('#toasts .toast').length)).toBe(0);

  // The sandbox list: said on Home, and in the sidebar on pages that do not show sandboxes.
  await app.open(mock('/', '&fail=/api/sandboxes'));
  await expect(screen.getByRole('alert').first()).toHaveText(/Couldn’t load your sandboxes/);
  await app.open(mock('/settings', '&fail=/api/sandboxes'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Settings');
  await expect(browser.locator('.sidebar [role="alert"]')).toHaveText(/Couldn’t load your sandboxes/);
  expect(await browser.evaluate(() => document.querySelectorAll('#toasts .toast').length)).toBe(0);
});

test('a missing page, sandbox, task or workflow has a heading and a way back', async ({ app, browser, screen }) => {
  await app.open(mock('/nope/x'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Page not found');
  await screen.getByRole('link', 'All sandboxes').last().click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');

  // SPEC-161 review round 1: a name an object has of itself is no page alias.
  await app.open(mock('/s/studio/constructor'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Page not found');

  await app.open(mock('/s/nosuch/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandbox not found');
  await expect(screen.getByText(/There is no sandbox “nosuch”/)).toBeVisible();

  await app.open(mock('/s/studio/t/999'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Task not found');
  await browser.locator('.not-found').getByRole('link', 'Tasks').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks$/);

  // A wrong workflow id is not an editable "Untitled workflow".
  await app.open(mock('/s/studio/workflows/nope'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Workflow not found');
  await expect(screen.getByRole('button', 'Save')).toBeHidden();
  await expect(screen.getByRole('textbox', 'Workflow name')).toBeHidden();
  await browser.locator('.not-found').getByRole('link', 'Workflows').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Workflows');

  // Nothing to edit means nothing to guard: the way back leaves without "Leave without saving?".
  await app.open(mock('/s/studio/automations/nope'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Automation not found');
  await browser.locator('.not-found').getByRole('link', 'Automations').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Automations');
  await expect(screen.getByRole('dialog')).toBeHidden();

  await app.open(mock('/s/studio/skills/nope'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Skill not found');
  await expect(screen.getByRole('button', 'Try again')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);
});

test('pages outside a sandbox show no sandbox navigation, as Home does', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('navigation', 'Sandbox')).toBeVisible();
  for (const [path, heading] of [['/settings', 'Settings'], ['/archived', 'Archived tasks'], ['/device', 'CLI device approval'], ['/admin/users', 'Users'], ['/design', 'Design system']]) {
    await app.open(mock(path));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText(heading);
    await expect(screen.getByRole('navigation', 'Sandbox')).toBeHidden();
    await expect(screen.getByText('Open a sandbox to see its tasks.')).toBeVisible();
    // No sandbox tab is current, and the tools belong to a sandbox.
    expect(await browser.evaluate(() => document.querySelectorAll('.tabrow-tabs .tab.active:not(.tab-home)').length)).toBe(0);
    await expect(browser.locator('.tools-toggle')).toBeHidden();
  }
  // New task still starts in the sandbox used last.
  await screen.getByRole('link', /^New task/).click();
  await expect(browser).toHaveURL(/#\/s\/studio$/);
});

test('nothing is cut off: the notification badge at the window edge and the Origin pill', async ({ app, browser, screen }) => {
  const badgeInside = () => browser.evaluate(() => {
    const badge = document.querySelector('.notif-badge') as HTMLElement | null;
    if (!badge || badge.hidden) return null;
    const r = badge.getBoundingClientRect();
    return r.top >= 0 && r.right <= window.innerWidth && r.left >= 0;
  });
  for (const width of [1440, 390]) {
    await browser.setViewport({ width, height: 844 });
    await app.open(mock('/'));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
    // The mock's events bring notifications after a few seconds.
    await expect.poll(badgeInside, { timeout: 15000 }).toBe(true);
  }
  await browser.setViewport({ width: 1440, height: 900 });
  await app.open(mock('/s/studio/tasks'));
  const origin = browser.locator('.tl-origin').first();
  await expect(origin).toHaveText('Automation');
  expect(await browser.evaluate(() => { const o = document.querySelector('.tl-origin')!; return o.scrollWidth <= o.clientWidth; })).toBe(true);
});

test('the task overview fits a 390 px phone: no part of it scrolls sideways', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/tasks/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await expect(screen.getByRole('heading', 'Sessions')).toBeVisible();
  const problems = () => browser.evaluate(() => {
    const out: string[] = [];
    for (const el of document.querySelectorAll('#main *')) {
      const style = getComputedStyle(el);
      const r = el.getBoundingClientRect();
      if (r.width === 0 || style.visibility === 'hidden') continue;
      if (['auto', 'scroll'].includes(style.overflowX) && el.scrollWidth > el.clientWidth + 1) out.push(`scrolls: ${el.className}`);
      if (r.right > window.innerWidth + 0.5 && !el.closest('[class*="sr-only"]')) out.push(`wider: ${el.tagName}.${el.className}`);
    }
    return out.slice(0, 5);
  });
  await expect.poll(problems).toEqual([]);
  expect(await axeViolations(browser)).toEqual([]);
});

test('an ended task offers what its menu offers: no Continue when archived; Check subscriptions opens them', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/1'));
  await expect(screen.getByText('This task is closed. Its branch and changes are kept.')).toBeVisible();
  await expect(screen.getByRole('button', 'Continue')).toBeHidden();

  await app.open(mock('/s/studio/t/2'));
  await expect(screen.getByText(/usually a subscription must sign in again/)).toBeVisible();
  await screen.getByRole('button', 'Check subscriptions').click();
  await expect(browser).toHaveURL(/#\/admin\/accounts$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Subscriptions');
});

test('a name for a new subscription is asked for at its field, not in a toast', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts'));
  await screen.getByRole('button', 'Add subscription').click();
  const dialog = screen.getByRole('dialog', 'Add a Claude subscription');
  await dialog.getByRole('button', 'Continue to sign-in').click();
  await expect(dialog.getByRole('alert')).toHaveText('Give the subscription a name.');
  // The name is not asked for in a toast. Other toasts (a background notification about a
  // subscription that must sign in again) may show meanwhile, so only this message counts.
  expect(
    await browser.evaluate(
      () => [...document.querySelectorAll('#toasts .toast')].filter((t) => /Give the subscription a name/.test(t.textContent || '')).length,
    ),
  ).toBe(0);
});
