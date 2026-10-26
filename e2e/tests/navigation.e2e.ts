import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, eventsConnected, failOnPageErrors, mock, touchTask } from './support.ts';

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
// Task #113 (SPEC-134): "in CD section add deployment in brackets: 'CD (deployment)'".
// Task #116 (SPEC-137): the Pipeline's stages sit beside CI and CD.
// Task #145 (SPEC-151): "make CI/CD one tab with a tab like in secrets team/agent".
// Task #152 (SPEC-161): "i dont like it here, move it somewhere else, maybe as part of dashboard":
// the Pipeline (stages) is a tab of the Dashboard.
const PRIMARY: [string, string, string?][] = [
  ['Tasks', 'tasks'],
  ['Git', 'git'],
  ['CI/CD', 'ci', 'CI/CD'],
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

// Task #157 (SPEC-167): "i want the same for sandboxes - i mean this icon "you can move it"", the
// Notes tabs' grip (SPEC-136) on the sandbox tabs, which already drag.
test('the sandbox tabs show a grip and say they can be clicked or dragged', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const row = screen.getByRole('navigation', 'Sandboxes');
  await expect(row.getByRole('link', /^product\b/)).toBeVisible();

  // TAB-001/002: a decorative grip before each sandbox's name, a title keeping the repository and
  // access and adding the Notes tabs' hint on its own line; "All sandboxes" does not move.
  const tabs = await browser.evaluate(() => [...document.querySelectorAll<HTMLElement>('.tabrow-tabs .tab')].map((tab) => {
    const grip = tab.querySelector<HTMLElement>('.tab-grip');
    const box = grip?.getBoundingClientRect();
    const name = tab.querySelector('.tab-name')?.getBoundingClientRect();
    return {
      name: tab.querySelector('.tab-name')?.textContent ?? null,
      title: tab.classList.contains('tab-home') ? tab.title.replace(/ \(.*\)$/, '') : tab.title,
      grip: grip ? { hidden: grip.getAttribute('aria-hidden'), first: tab.firstElementChild === grip, svg: !!grip.querySelector('svg'), text: grip.textContent, cursor: getComputedStyle(grip).cursor, shown: !!box && box.width > 0 && box.height > 0, gap: box && name ? Math.round(name.left - box.right) : null } : null,
    };
  }));
  // 4 px between the grip and the name, as on the Notes tabs.
  const grip = { hidden: 'true', first: true, svg: true, text: '', cursor: 'grab', shown: true, gap: 4 };
  const hint = 'Click to open, drag to reorder';
  expect(tabs).toEqual([
    { name: null, title: 'All sandboxes', grip: null },
    { name: 'studio', title: `Quartio-Studio/studio · admin access\n${hint}`, grip },
    { name: 'product', title: `Quartio-Studio/product · admin access\n${hint}`, grip },
    { name: 'sandbox-3', title: `Quartio-Studio/sandbox-3 · read access\n${hint}`, grip },
  ]);
  // The accessible names still begin with the sandboxes' names (the grip adds nothing).
  await expect(row.getByRole('link', /^studio\b/)).toHaveAttribute('aria-current', 'page');
  await expect(row.getByRole('link', /^product\b/)).toBeVisible();

  // The grip is faint on a tab at rest and takes the tab's colour when the tab is current or hovered.
  const gripColour = (name: string) => browser.evaluate((n) => {
    const tab = [...document.querySelectorAll<HTMLElement>('.tabrow-tabs .tab')].find((t) => t.querySelector('.tab-name')?.textContent === n)!;
    return { grip: getComputedStyle(tab.querySelector('.tab-grip')!).color, tab: getComputedStyle(tab).color };
  }, name);
  const current = await gripColour('studio');
  expect(current.grip).toEqual(current.tab);
  const resting = await gripColour('product');
  expect(resting.grip).not.toEqual(resting.tab);
  expect(await axeViolations(browser)).toEqual([]);
  const product = await row.getByRole('link', /^product\b/).boundingBox();
  if (!product) throw new Error('tabs not laid out');
  await browser.mouse.move(product.x + product.width / 2, product.y + product.height / 2);
  await expect.poll(async () => { const now = await gripColour('product'); return now.grip === now.tab; }).toBe(true);

  // Dragging is the Notes tabs' pointer drag (SPEC-336): the next test.

  // TAB-003: without a fine pointer (mouse, pen) the grip is not shown, as on the Notes tabs.
  const coarse = await browser.evaluate(() => {
    const found: string[] = [];
    const visit = (list: CSSRuleList, inside: boolean) => {
      for (const rule of [...list]) {
        if (rule instanceof CSSMediaRule) visit(rule.cssRules, inside || rule.conditionText.replace(/\s+/g, ' ').includes('not (any-pointer: fine)'));
        else if (inside && rule instanceof CSSStyleRule) found.push(rule.cssText);
      }
    };
    for (const sheet of [...document.styleSheets]) { try { visit(sheet.cssRules, false); } catch { /* another origin */ } }
    return found.join('\n');
  });
  expect(coarse).toMatch(/\.tab-grip \{ display: none; \}/);
});

// Task #320 (SPEC-336, #929): "this bar should be as smooth as this bar": the sandbox tabs drag as
// the Notes tabs do (tabdrag.js), not with the browser's own drag and drop.
test('a sandbox tab follows the pointer, the others slide aside, and it settles where it is let go', async ({ app, browser, screen }) => {
  // `ambient=0`: no ambient regroup moves the tabs under a still pointer.
  await app.open(mock('/s/studio', '&ambient=0'));
  const row = screen.getByRole('navigation', 'Sandboxes');
  await expect(row.getByRole('link', /^sandbox-3\b/)).toBeVisible();
  await eventsConnected(browser);
  const names = () => browser.evaluate(() => [...document.querySelectorAll('.tabrow-tabs .tab-name')].map((n) => n.textContent));
  // How far each tab is moved, by name ('home' for All sandboxes): 0 when it is not.
  const shifts = () => browser.evaluate(() => Object.fromEntries([...document.querySelectorAll<HTMLElement>('.tabrow-tabs .tab')].map((tab) => [
    tab.querySelector('.tab-name')?.textContent ?? 'home', Number(/translateX\((-?[\d.]+)px\)/.exec(tab.style.transform)?.[1] ?? 0),
  ])));
  const hash = () => browser.evaluate(() => location.hash);
  const start = await hash();
  expect(await names()).toEqual(['studio', 'product', 'sandbox-3']);

  // SMO-001: the browser's own link drag never starts (no ghost image).
  expect(await browser.evaluate(() => [...document.querySelectorAll<HTMLElement>('.tabrow-tabs .tab[data-id]')].map((tab) => tab.draggable))).toEqual([false, false, false]);

  // A drag that ends where it started reorders nothing and pins no order (sandboxes added later
  // keep the server's order).
  const third = await row.getByRole('link', /^sandbox-3\b/).boundingBox();
  const first = await row.getByRole('link', /^studio\b/).boundingBox();
  if (!third || !first) throw new Error('tabs not laid out');
  const y = third.y + third.height / 2;
  await browser.mouse.move(first.x + first.width / 2, y);
  await browser.mouse.down();
  for (const x of [first.x + first.width / 2 + 12, first.x + first.width / 2]) await browser.mouse.move(x, y);
  await browser.mouse.up();
  expect(await names()).toEqual(['studio', 'product', 'sandbox-3']);
  expect(await browser.evaluate(() => localStorage.getItem('yulius.sandboxOrder'))).toBeNull();
  expect(await hash()).toBe(start);

  // A sandbox renamed on the server while a tab is pressed (SMO-004). The mock delivers a pushed
  // event on a zero timeout, so a later zero timeout runs once the page handled it; the page's
  // title (not held) shows the store has it.
  const rename = (id: string, name: string) => browser.evaluate(async ([sandboxId, newName]) => {
    type Mock = { __yuliusMock: { sandboxes: { id: string, name: string }[], eventSockets: Set<{ push: (data: string) => void }> } };
    const db = (window as unknown as Mock).__yuliusMock;
    const pressed = document.querySelector('.tabrow-tabs .tab.dragging, .tabrow-tabs .tab:active');
    const sandbox = db.sandboxes.find((s) => s.id === sandboxId)!;
    sandbox.name = newName;
    for (const socket of db.eventSockets) socket.push(JSON.stringify({ type: 'sandbox.updated', sandbox }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    return { same: Boolean(pressed?.isConnected), drawn: [...document.querySelectorAll('.tabrow-tabs .tab-name')].some((n) => n.textContent === newName) };
  }, [id, name] as const);

  // Pressed on sandbox-3 and moved over studio: it follows the pointer, studio and product slide right.
  await browser.mouse.move(third.x + third.width / 2, y);
  await browser.mouse.down();
  for (const x of [third.x + third.width / 2 - 10, first.x + first.width, first.x + 4]) await browser.mouse.move(x, y);
  const moving = await shifts();
  expect(moving.home).toBe(0);
  expect(moving['sandbox-3']).toBeCloseTo(first.x + 4 - (third.x + third.width / 2), 0);
  expect(moving.studio).toBeGreaterThan(0);
  expect(moving.product).toBeGreaterThan(0);
  expect(await browser.evaluate(() => document.querySelector('.tabrow-tabs')!.classList.contains('reordering'))).toBe(true);

  // SMO-004: the update does not redraw the row under the pointer...
  expect(await rename('sb_studio', 'studio-renamed')).toEqual({ same: true, drawn: false });
  expect(await browser.evaluate(() => document.title)).toContain('studio-renamed');

  // SMO-002: ...let go, it takes its place, the update is drawn, nothing opens, and the row
  // settles back to no transforms.
  await browser.mouse.up();
  expect(await names()).toEqual(['sandbox-3', 'studio-renamed', 'product']);
  await expect.poll(async () => Object.values(await shifts()).every((x) => x === 0)).toBe(true);
  expect(await hash()).toBe(start);

  // SMO-003: Escape puts a dragged tab back and its release opens nothing.
  const product = await row.getByRole('link', /^product\b/).boundingBox();
  if (!product) throw new Error('tabs not laid out');
  await browser.mouse.move(product.x + product.width / 2, y);
  await browser.mouse.down();
  for (const x of [product.x + product.width / 2 - 10, first.x + 4]) await browser.mouse.move(x, y);
  expect((await shifts()).product).toBeLessThan(0);
  await browser.keyboard.press('Escape');
  await browser.mouse.move(product.x + product.width / 2, y);
  await browser.mouse.up();
  expect(await names()).toEqual(['sandbox-3', 'studio-renamed', 'product']);
  expect(Object.values(await shifts()).every((x) => x === 0)).toBe(true);
  expect(await hash()).toBe(start);

  // SMO-004 for a press that ends without a drag (the browser cancels it): the update held
  // meanwhile is drawn then.
  await browser.evaluate(() => window.addEventListener('pointerdown', (event) => { (window as unknown as { e2ePointer: number }).e2ePointer = event.pointerId; }, { once: true, capture: true }));
  await browser.mouse.move(product.x + product.width / 2, y);
  await browser.mouse.down();
  expect((await rename('sb_product', 'product-renamed')).drawn).toBe(false);
  await browser.evaluate(() => window.dispatchEvent(new PointerEvent('pointercancel', { pointerId: (window as unknown as { e2ePointer: number }).e2ePointer })));
  await expect.poll(names).toEqual(['sandbox-3', 'studio-renamed', 'product-renamed']);
  await browser.mouse.up();

  // The context menu moves a tab without dragging (WCAG 2.5.7).
  await browser.evaluate(() => {
    const tab = [...document.querySelectorAll<HTMLElement>('.tabrow-tabs .tab')].find((t) => t.querySelector('.tab-name')?.textContent === 'sandbox-3')!;
    const box = tab.getBoundingClientRect();
    tab.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: box.left + 8, clientY: box.bottom }));
  });
  await expect(screen.getByRole('menuitem', 'Move left')).toHaveAttribute('aria-disabled', 'true');
  await screen.getByRole('menuitem', 'Move right').click();
  expect(await names()).toEqual(['studio-renamed', 'sandbox-3', 'product-renamed']);

  // A plain click still opens the sandbox; the order is kept after a reload.
  await row.getByRole('link', /^product-renamed\b/).click();
  await expect(browser).toHaveURL(/#\/s\/product/);
  await browser.reload();
  await expect.poll(async () => (await names()).map((name) => name?.replace('-renamed', ''))).toEqual(['studio', 'sandbox-3', 'product']);
});

// Task #108 (SPEC-122): "We need tasks id somewhere here and not only PRs id because we use them
// quickly. But it should be communicated what id is for what". The screenshot's sidebar showed
// only pull request numbers.
test('sidebar rows show the task number apart from the pull request number', async ({ app, browser, screen }) => {
  // The chips come from the Git overview, read after the tasks: held, the rows show no chip yet.
  // `ambient=0`: no task changes by itself, so no row moves under the pointer of a hover (#515).
  await app.open(mock('/s/studio/tasks', '&ambient=0&hold=/api/sandboxes/sb_studio/git'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  const inRow = (title: string, selector: string) => browser.locator(`.task-list .task-row:has-text("${title}") ${selector}`);

  // TID-1, TID-2: the task's own number, said as a task's, before the chip and the title. It
  // shows before the pull request is known.
  await expect(inRow('Improve the code', '.task-num')).toHaveText('Task #13');
  await expect(inRow('Improve the code', '.tl-pr-chip')).toHaveCount(0);
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());
  await expect(inRow('Improve the code', '.tl-pr-chip')).toHaveText(/^PR #520/);
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
  // The row is read once its hover state shows (the control is visible), and until it is laid out.
  const overlap = (title: string, control: string) => browser.evaluate(([t, c]) => {
    const row = [...document.querySelectorAll('.task-list .task-row')].find((el) => el.textContent!.includes(t))!;
    const link = row.querySelector('.task-link')!;
    const right = Math.max(...[...link.children].map((el) => el.getBoundingClientRect().right));
    return right - row.querySelector(c)!.getBoundingClientRect().left;
  }, [title, control]);
  await browser.locator('.task-row:has-text("Improve the code")').hover();
  await expect(inRow('Improve the code', '.task-quick-archive')).toBeVisible();
  await expect.poll(() => overlap('Improve the code', '.task-quick-archive')).toBeLessThanOrEqual(0);
  // Review round 2: a row that cannot be archived shows only the menu, and its title ends before it.
  await at(264);
  await browser.locator('.task-row:has-text("Checkout API")').hover();
  await expect(inRow('Checkout API', '.task-kebab')).toBeVisible();
  await expect.poll(() => overlap('Checkout API', '.task-kebab')).toBeLessThanOrEqual(0);

  // A finished task whose shells still run reads "Working" without its pull request: the tooltip
  // names it (and the narrowest sidebar shows no chip).
  await touchTask(browser, 'sb_studio', 13, { background_shells: 1, background_shells_since: new Date().toISOString() });
  await expect.poll(() => browser.evaluate(() => (document.querySelector('.task-list .task-row:has(.task-num[title="Task #13"]) .task-link') as HTMLElement).title)).toMatch(/^Task #13 Improve the code — .* · PR #520 · /);
  expect(await axeViolations(browser)).toEqual([]);
});

// SPEC-122 TID-5: the Tasks page lists tasks by number too, as the sidebar does.
test('the Tasks page rows show the task number before the title', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  await expect(browser.locator('#main .tl-row:has-text("Improve the code") .task-num')).toHaveText('Task #13');
  await expect(browser.locator('#main .tl-row:has-text("Improve the code") .tl-link')).toHaveAttribute('title', /^Task #13 Improve the code/);
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
  // The chip comes with the Git overview, after the rows: measure once it is there.
  await expect(browser.locator('.task-list .task-row:has-text("Improve the code") .tl-pr-chip')).toHaveCount(1);
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
