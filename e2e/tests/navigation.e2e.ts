import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

test('a task opens from the sidebar', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  await screen.getByRole('link', /3D model viewer/).click();

  await expect(browser).toHaveURL(/#\/s\/studio\/t\/11$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#11 3D model viewer');
});

// Task #27 (SPEC-029): "Too much elements here. Only Cockpit, Tasks, Git, CI (and CD in the
// future), Secrets and Dashboard should be here. Other should be to be expanded".
// Then: "in CI in brackets put 'tests'": the link says what CI is; the page keeps its title.
// Task #33 (SPEC-035) moved the Cockpit to the right, as the Agent: it is not in this list.
const PRIMARY: [string, string, string?][] = [
  ['Tasks', 'tasks'],
  ['Git', 'git'],
  ['CI (tests)', 'ci', 'CI'],
  ['Secrets', 'secrets'],
  ['Dashboard', 'dashboard'],
];
const MORE = [
  ['Sessions', 'sessions'],
  ['Workflows', 'workflows'],
  ['Skills', 'skills'],
  ['Automations', 'automations'],
  ['MCP', 'mcp'],
];

test('the sandbox navigation shows its daily pages and expands the rest under More', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  const nav = screen.getByRole('navigation', 'Sandbox');
  const more = nav.getByRole('button', 'More');

  await expect(nav.getByRole('link')).toHaveCount(PRIMARY.length);
  await expect(browser.locator('nav.nav > .nav-item')).toHaveText([...PRIMARY.map(([page]) => page), 'More']);
  await expect(more).toHaveAttribute('aria-expanded', 'false');
  for (const [page] of MORE) await expect(nav.getByRole('link', page)).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  await more.click();
  await expect(more).toHaveAttribute('aria-expanded', 'true');
  await expect(nav.getByRole('link')).toHaveCount(PRIMARY.length + MORE.length);
  await expect(browser.locator('nav.nav .nav-more .nav-item')).toHaveText(MORE.map(([page]) => page));
  expect(await axeViolations(browser)).toEqual([]);

  // Kept after a reload, then closed again.
  await browser.reload();
  await expect(more).toHaveAttribute('aria-expanded', 'true');
  await more.click();
  await expect(more).toHaveAttribute('aria-expanded', 'false');
  for (const [page] of MORE) await expect(nav.getByRole('link', page)).toBeHidden();
  await browser.reload();
  await expect(more).toHaveAttribute('aria-expanded', 'false');
});

test('the sandbox pages open from the sandbox navigation', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  const nav = screen.getByRole('navigation', 'Sandbox');

  for (const [page, path, heading = page] of PRIMARY) {
    await nav.getByRole('link', page).click();
    await expect(browser).toHaveURL(new RegExp(`#/s/studio/${path}$`));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText(heading);
  }
  await nav.getByRole('button', 'More').click();
  for (const [page, path] of MORE) {
    await nav.getByRole('link', page).click();
    await expect(browser).toHaveURL(new RegExp(`#/s/studio/${path}$`));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText(page);
    await expect(nav.getByRole('link', page)).toHaveAttribute('aria-current', 'page');
  }
});

test('a page under More opens with the group expanded and the page current', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/mcp'));
  const nav = screen.getByRole('navigation', 'Sandbox');
  const more = nav.getByRole('button', 'More');
  const go = (path: string) => browser.evaluate((hash) => { location.hash = hash; }, `#${path}`);

  await expect(more).toHaveAttribute('aria-expanded', 'true');
  await expect(nav.getByRole('link', 'MCP')).toBeVisible();
  await expect(nav.getByRole('link', 'MCP')).toHaveAttribute('aria-current', 'page');

  // Shown for that page only: the remembered choice (collapsed) is unchanged.
  await nav.getByRole('link', 'Tasks').click();
  await expect(more).toHaveAttribute('aria-expanded', 'false');
  await browser.reload();
  await expect(more).toHaveAttribute('aria-expanded', 'false');

  // Moving to a More page inside the app opens it too.
  await go('/s/studio/skills');
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Skills');
  await expect(more).toHaveAttribute('aria-expanded', 'true');
  await expect(nav.getByRole('link', 'Skills')).toHaveAttribute('aria-current', 'page');

  // Closed there from the keyboard, it stays closed within that page, and More marks where the
  // current page is.
  await more.press('Enter');
  await expect(more).toHaveAttribute('aria-expanded', 'false');
  await expect(browser.locator('nav.nav .nav-toggle.active')).toHaveCount(1);
  await expect(more).toHaveAttribute('aria-current', 'true');
  await go('/s/studio/skills/nope');
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Skill not found');
  await expect(more).toHaveAttribute('aria-expanded', 'false');
  // Another More page opens it again.
  await go('/s/studio/mcp');
  await expect(more).toHaveAttribute('aria-expanded', 'true');
  await more.press('Space');
  await expect(more).toHaveAttribute('aria-expanded', 'false');

  // Focus inside a group that closes on leaving its page moves to More, not to the page's top.
  await go('/s/studio/workflows');
  await nav.getByRole('link', 'Workflows').focus();
  await go('/s/studio/tasks');
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  await expect(more).toHaveAttribute('aria-expanded', 'false');
  await expect(more).toBeFocused();
  await expect(browser.locator('nav.nav .nav-toggle.active')).toHaveCount(0);
  await expect(more).not.toHaveAttribute('aria-current');
});

test('another sandbox opens from the sandbox tabs', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));

  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^product\b/).click();

  await expect(browser).toHaveURL(/#\/s\/product$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
});

// Task #108 (SPEC-122): "We need tasks id somewhere here and not only PRs id because we use them
// quickly. But it should be communicated what id is for what". The screenshot's sidebar showed
// only pull request numbers.
test('sidebar rows show the task number apart from the pull request number', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  const inRow = (title: string, selector: string) => browser.locator(`.task-list .task-row:has-text("${title}") ${selector}`);

  // TID-1, TID-2: the task's own number, said as a task's, before the chip and the title.
  await expect(inRow('Improve the code', '.task-num')).toHaveText('Task #13');
  await expect(inRow('Improve the code', '.task-num')).toHaveAttribute('title', 'Task #13');
  await expect(inRow('Improve the code', '.task-num .sr-only')).toHaveText('Task ');
  await expect(inRow('Checkout API', '.task-num')).toHaveText('Task #12');
  const order = await browser.evaluate(() => {
    const row = [...document.querySelectorAll('.task-list .task-row')].find((el) => el.textContent!.includes('Improve the code'))!;
    return ['.task-num', '.tl-pr-chip', '.task-title'].map((s) => Math.round(row.querySelector(s)!.getBoundingClientRect().left));
  });
  expect(order).toEqual([...order].sort((a, b) => a - b));
  expect(new Set(order).size).toBe(3);
  // Every row has its number, whatever its group.
  const rows = await browser.locator('.task-list .task-row').count();
  await expect(browser.locator('.task-list .task-row .task-num')).toHaveCount(rows);

  // TID-4: whatever the sidebar's width, rows stay one line and nothing spills out of a row; the
  // number and a shown chip or time are whole and every title keeps a few letters. What gives way
  // is the time, then (narrowest) the chip, which the row's tooltip still names.
  const rowsNow = () => browser.evaluate(() => [...document.querySelectorAll('.task-list .task-link')].map((link) => {
    const shown = (s: string) => { const el = link.querySelector(s) as HTMLElement | null; return Boolean(el && el.offsetWidth > 0); };
    const cut = (s: string) => { const el = link.querySelector(s); return el ? el.scrollWidth > el.clientWidth + 1 : false; };
    const box = link.getBoundingClientRect();
    const right = Math.max(...[...link.children].map((el) => el.getBoundingClientRect().right));
    return {
      text: link.querySelector('.task-title')!.textContent,
      height: Math.round(box.height),
      spills: right > box.right - parseFloat(getComputedStyle(link).paddingRight) + 1,
      cut: cut('.task-num') || cut('.tl-pr-chip') || cut('.task-age'),
      title: Math.round(link.querySelector('.task-title')!.getBoundingClientRect().width),
      chip: shown('.tl-pr-chip'),
      age: shown('.task-age'),
      tooltip: (link as HTMLElement).title,
    };
  }));
  type Row = Awaited<ReturnType<typeof rowsNow>>[number];
  const broken = (rows: Row[]) => rows.filter((r) => r.height !== rows[0].height || r.spills || r.cut || r.title < 36);
  const splitter = screen.getByRole('separator', 'Resize sidebar');
  const at = async (width: number) => {
    await splitter.press('Home');
    for (let w = 200; w < width; w += 64) await splitter.press('Shift+ArrowRight');
    await expect(splitter).toHaveAttribute('aria-valuenow', String(Math.min(width, 480)));
    return rowsNow();
  };

  const wide = await at(456);
  expect(broken(wide)).toEqual([]);
  expect(wide.filter((r) => !r.age)).toEqual([]);
  expect(wide.filter((r) => r.chip).map((r) => r.text)).toContain('Improve the code');

  const usual = await at(328);
  expect(broken(usual)).toEqual([]);
  expect(usual.filter((r) => r.chip).map((r) => r.text)).toContain('Improve the code');

  const narrowest = await at(200);
  expect(broken(narrowest)).toEqual([]);
  expect(narrowest.filter((r) => r.chip)).toEqual([]);
  expect(narrowest.find((r) => r.text === 'Improve the code')!.tooltip).toContain('PR #520');
  // The tooltip names the pull request once, even when the state already does ("Done · PR #520").
  expect(narrowest.find((r) => r.text === 'Improve the code')!.tooltip.match(/PR #520/g)).toHaveLength(1);

  // Review round 1: hovering a row shows Archive and the menu, and the title ends before them.
  await at(264);
  await splitter.press('ArrowRight');
  await splitter.press('ArrowRight');
  await expect(splitter).toHaveAttribute('aria-valuenow', '296');
  await browser.locator('.task-row:has-text("Improve the code")').hover();
  const overlap = await browser.evaluate(() => {
    const row = [...document.querySelectorAll('.task-list .task-row')].find((el) => el.textContent!.includes('Improve the code'))!;
    const link = row.querySelector('.task-link')!;
    const right = Math.max(...[...link.children].map((el) => el.getBoundingClientRect().right));
    return right - row.querySelector('.task-quick-archive')!.getBoundingClientRect().left;
  });
  expect(overlap).toBeLessThanOrEqual(0);
  expect(await axeViolations(browser)).toEqual([]);
});

// SPEC-122 TID-5: the Tasks page lists tasks by number too, as the sidebar does.
test('the Tasks page rows show the task number before the title', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  await expect(browser.locator('#main .tl-row:has-text("Improve the code") .task-num')).toHaveText('Task #13');
  const rows = await browser.locator('#main .tl-row').count();
  await expect(browser.locator('#main .tl-row .tl-link .task-num')).toHaveCount(rows);
});

// Review round 1 of SPEC-122: on a phone the drawer's rows keep 48 px for the menu button.
test('the navigation drawer rows fit a 320 px phone with task and pull request numbers', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 320, height: 700 });
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  const menu = screen.getByRole('button', 'Open navigation');
  await menu.click();
  await expect(menu).toHaveAttribute('aria-expanded', 'true');
  await expect(browser.locator('.task-list .task-row:has-text("Improve the code") .task-num')).toBeVisible();
  const rows = await browser.evaluate(() => [...document.querySelectorAll('.task-list .task-link')].map((link) => {
    const box = link.getBoundingClientRect();
    const right = Math.max(...[...link.children].map((el) => el.getBoundingClientRect().right));
    return {
      text: link.querySelector('.task-title')!.textContent,
      spills: right > box.right - parseFloat(getComputedStyle(link).paddingRight) + 1,
      title: Math.round(link.querySelector('.task-title')!.getBoundingClientRect().width),
    };
  }));
  expect(rows.filter((r) => r.spills || r.title < 36)).toEqual([]);
});
