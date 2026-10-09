// Git → a task's changes (SPEC-001 UI-031): what the agent changed, before you open its session.
// Changes (the diff against the integration branch: file tree, unified or split, wrapped or not),
// Commits and Branch, with "Go to task" at the top. Changes and Commits read GET
// /api/tasks/{tid}/changes and /commits (docs/api.md, "Task changes", proposed); a server without
// them answers 404, and the review then shows the Branch tab alone — the position and size from the
// Git overview (GET /api/sandboxes/{sid}/git) and the comparison on GitHub.

import { enc } from '../api.js';
import { h, icon, setChildren, placeholder, statusMark, prChip, keepFocus, loadError } from '../dom.js';
import { href, replaceQuietly } from '../router.js';
import { relShort, dateTime, plural } from '../format.js';
import { taskInfo } from '../status.js';
import { readJson, writeJson } from '../store.js';

const PREFS_KEY = 'yulius.diffView'; // { layout: 'unified' | 'split', wrap: boolean }, this browser
const TABS = [['changes', 'Changes'], ['commits', 'Commits'], ['branch', 'Branch']];
const LINE_LIMIT = 400; // lines drawn per file before "Show all"
const STATUS_TEXT = { added: 'added', deleted: 'deleted', renamed: 'renamed', modified: null };

/** Numbers with their sign, coloured: "+684 −44". */
function diffStat(insertions, deletions) {
  return h('span', { class: 'gt-stat' }, h('span', { class: 'gt-add' }, `+${insertions}`), ' ', h('span', { class: 'gt-del' }, `−${deletions}`));
}

/** Files grouped into folders; a folder with one folder inside reads as one ("src/api"). */
function fileTree(files) {
  const root = { dirs: new Map(), files: [], insertions: 0, deletions: 0 };
  for (const file of files) {
    const parts = file.path.split('/');
    let node = root;
    node.insertions += file.insertions;
    node.deletions += file.deletions;
    for (const part of parts.slice(0, -1)) {
      if (!node.dirs.has(part)) node.dirs.set(part, { dirs: new Map(), files: [], insertions: 0, deletions: 0 });
      node = node.dirs.get(part);
      node.insertions += file.insertions;
      node.deletions += file.deletions;
    }
    node.files.push({ name: parts[parts.length - 1], file });
  }
  return root;
}

function compact(name, node) {
  while (node.files.length === 0 && node.dirs.size === 1) {
    const [child, next] = [...node.dirs][0];
    name = `${name}/${child}`;
    node = next;
  }
  return [name, node];
}

/** Split view rows: context on both sides, a run of removals beside the additions that replace it. */
function splitRows(lines) {
  const rows = [];
  for (let i = 0; i < lines.length;) {
    if (lines[i].kind === 'ctx') { rows.push([lines[i], lines[i]]); i += 1; continue; }
    const dels = [];
    const adds = [];
    while (i < lines.length && lines[i].kind === 'del') dels.push(lines[i++]);
    while (i < lines.length && lines[i].kind === 'add') adds.push(lines[i++]);
    for (let k = 0; k < Math.max(dels.length, adds.length); k += 1) rows.push([dels[k] || null, adds[k] || null]);
  }
  return rows;
}

const SIGN = { add: '+', del: '−', ctx: ' ' };

/** Mount the review of one task's work. */
export function mountGitTaskView(ctx, sandbox, task, route) {
  const base = `/s/${encodeURIComponent(sandbox.slug)}`;
  const here = `${base}/git/t/${task.number}`;
  const prefs = { layout: 'unified', wrap: false, ...readJson(PREFS_KEY, {}) };
  let tab = TABS.some(([id]) => id === route.query.get('tab')) ? route.query.get('tab') : 'changes';
  let changes = null;
  let changesError = null;
  let commits = null;
  let commitsError = null;
  let unsupported = false; // the server has no task changes API: Branch only
  const reads = new AbortController(); // leaving the page cancels its reads
  const expandedFiles = new Set(); // files drawn in full past LINE_LIMIT
  const collapsedFiles = new Set();

  const pr = () => ctx.store.pullsOf(sandbox.id).get(task.id) || null;
  const gitRow = () => ctx.store.gitRowOf(sandbox.id, task.id);
  const profile = sandbox.profile || {};
  const integration = () => (changes && changes.base) || profile.integration_branch || 'integration';

  // ---- header
  const info = taskInfo(task, ctx.store.state.actionRequests, ctx.store.pullsOf(task.sandbox_id).get(task.id));
  const taskPath = `${base}/t/${task.number}`;
  const meta = h('div', { class: 'gt-meta' });
  const head = h('header', { class: 'gt-head' },
    h('a', { class: 'gt-back', href: href(`${base}/git`), dataset: { route: `${base}/git` } }, icon('back', { size: 14 }), 'Git'),
    h('div', { class: 'gt-title-row' },
      h('h1', { class: 'page-title gt-title' }, h('span', { class: 'muted' }, `#${task.number} `), task.title || `Task #${task.number}`),
      h('a', { class: 'btn btn-primary gt-go', href: href(taskPath), dataset: { route: taskPath } }, 'Go to task', icon('forward', { size: 14 }))),
    meta);

  function renderMeta() {
    const p = pr();
    setChildren(meta,
      h('span', { class: 'gt-state' }, statusMark(info, 14), info.label),
      task.branch ? h('span', { class: 'gt-branch mono', title: `${task.branch} into ${integration()}` }, icon('branch', { size: 12 }), task.branch, h('span', { class: 'muted' }, ` → ${integration()}`)) : null,
      p ? prChip(p) : null,
      changes ? [h('span', { class: 'muted' }, plural(changes.files.length, 'file')), diffStat(sum('insertions'), sum('deletions'))] : null);
  }
  const sum = (key) => (changes ? changes.files.reduce((total, f) => total + (f[key] || 0), 0) : 0);

  // ---- tabs (APG tabs: arrows move, the panel follows)
  const tabButtons = new Map();
  const tablist = h('div', { class: 'gt-tabs', role: 'tablist', 'aria-label': 'Task review' }, TABS.map(([id, label]) => {
    const button = h('button', { type: 'button', role: 'tab', id: `gt-tab-${id}`, 'aria-controls': 'gt-panel', class: 'gt-tab', onClick: () => select(id),
      onKeydown: (event) => {
        const step = { ArrowRight: 1, ArrowLeft: -1 }[event.key];
        if (!step) return;
        event.preventDefault();
        const index = TABS.findIndex(([t]) => t === id);
        const next = TABS[(index + step + TABS.length) % TABS.length][0];
        select(next);
        tabButtons.get(next).focus();
      } }, label, h('span', { class: 'gt-tab-count' }));
    tabButtons.set(id, button);
    return button;
  }));
  const panel = h('div', { class: 'gt-panel', id: 'gt-panel', role: 'tabpanel' });

  function select(id) {
    tab = id;
    const query = id === 'changes' ? '' : `?tab=${id}`;
    replaceQuietly(`${here}${query}`);
    render();
  }

  // ---- Changes
  function savePrefs() { writeJson(PREFS_KEY, prefs); }

  function toolbar() {
    const layout = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Diff layout' }, [['unified', 'Unified'], ['split', 'Split']].map(([id, label]) => h('label', null,
      h('input', { type: 'radio', name: 'gt-layout', checked: prefs.layout === id, onChange: () => { prefs.layout = id; savePrefs(); keepFocus(panel, renderPanel); } }), label)));
    const wrap = h('button', { type: 'button', class: ['btn-icon', 'gt-wrap'], 'aria-pressed': String(prefs.wrap), 'aria-label': 'Wrap long lines', title: 'Wrap long lines',
      onClick: () => { prefs.wrap = !prefs.wrap; savePrefs(); keepFocus(panel, renderPanel); } }, icon('wrap', { size: 16 }));
    const refresh = h('button', { type: 'button', class: 'btn-icon', 'aria-label': 'Reload changes', title: 'Reload changes', onClick: () => { changes = null; changesError = null; render(); loadChanges(); } }, icon('refresh', { size: 16 }));
    return h('div', { class: 'gt-toolbar' },
      h('span', { class: 'gt-summary' },
        `Changes against ${integration()}`,
        changes && changes.uncommitted ? h('span', { class: 'gt-uncommitted', title: 'Files the agent changed but has not committed yet; they are included here' }, ` · ${plural(changes.uncommitted, 'uncommitted file')}`) : null),
      h('span', { class: 'spacer' }), refresh, layout, wrap);
  }

  function lineCells(line, side) {
    if (!line) return [h('td', { class: 'gt-ln' }), h('td', { class: 'gt-sign' }), h('td', { class: 'gt-code gt-empty' })];
    const number = side === 'old' ? line.old : side === 'new' ? line.new : null;
    return [h('td', { class: 'gt-ln' }, number ?? ''), h('td', { class: 'gt-sign', 'aria-hidden': 'true' }, SIGN[line.kind]), h('td', { class: ['gt-code', `gt-k-${line.kind}`] }, line.text)];
  }

  function fileBody(file, index) {
    if (file.binary) return h('p', { class: 'gt-note' }, 'Binary file; no text diff.');
    if (!file.hunks || !file.hunks.length) return h('p', { class: 'gt-note' }, file.status === 'renamed' ? 'Renamed without changes.' : 'No text changes.');
    const all = expandedFiles.has(index);
    let budget = all ? Infinity : LINE_LIMIT;
    let total = 0;
    const rows = [];
    for (const hunk of file.hunks) {
      total += hunk.lines.length;
      if (budget <= 0) continue;
      rows.push(h('tr', { class: 'gt-hunk' }, h('td', { colspan: prefs.layout === 'split' ? 6 : 4 }, hunk.header)));
      const lines = hunk.lines.slice(0, budget);
      budget -= lines.length;
      if (prefs.layout === 'split') {
        for (const [left, right] of splitRows(lines)) rows.push(h('tr', null, lineCells(left, 'old'), lineCells(right, 'new')));
      } else {
        for (const line of lines) {
          rows.push(h('tr', { class: `gt-row-${line.kind}` },
            h('td', { class: 'gt-ln' }, line.old ?? ''), h('td', { class: 'gt-ln' }, line.new ?? ''),
            h('td', { class: 'gt-sign', 'aria-hidden': 'true' }, SIGN[line.kind]), h('td', { class: ['gt-code', `gt-k-${line.kind}`] }, line.text)));
        }
      }
    }
    // Column widths come first: a fixed split table takes them from here, not from the hunk rows.
    const cols = prefs.layout === 'split'
      ? h('colgroup', null, ['ln', 'sign', 'code', 'ln', 'sign', 'code'].map((kind) => h('col', { class: `gt-col-${kind}` })))
      : null;
    const table = h('table', { class: ['gt-diff', prefs.layout === 'split' && 'gt-split'], 'aria-label': `Changes in ${file.path}` }, cols, h('tbody', null, rows));
    const shown = Math.min(total, all ? total : LINE_LIMIT);
    return [
      // Scrolls sideways when lines are long, so it takes focus for keyboard scrolling.
      h('div', { class: 'gt-diff-scroll', tabindex: '0', role: 'region', 'aria-label': `Diff of ${file.path}` }, table),
      shown < total ? h('button', { type: 'button', class: 'link-btn small gt-more', onClick: () => { expandedFiles.add(index); keepFocus(panel, renderPanel); } }, `Show all ${total} lines`) : null,
    ];
  }

  function fileBlock(file, index) {
    const id = `gt-file-${index}`;
    const open = !collapsedFiles.has(index);
    const status = STATUS_TEXT[file.status];
    const headButton = h('button', { type: 'button', class: 'gt-file-head', 'aria-expanded': String(open), 'aria-controls': `${id}-body`,
      onClick: () => { if (open) collapsedFiles.add(index); else collapsedFiles.delete(index); keepFocus(panel, renderPanel); } },
      icon(open ? 'down' : 'right', { size: 14 }),
      h('span', { class: 'gt-file-path mono' }, file.old_path && file.old_path !== file.path ? `${file.old_path} → ${file.path}` : file.path),
      status ? h('span', { class: ['badge', file.status === 'deleted' ? 'badge-danger' : file.status === 'added' ? 'badge-ok' : ''] }, status) : null,
      h('span', { class: 'spacer' }), diffStat(file.insertions, file.deletions));
    return h('section', { class: 'gt-file', id }, headButton, open ? h('div', { id: `${id}-body` }, fileBody(file, index)) : null);
  }

  function treeView(files) {
    const indexOf = new Map(files.map((f, i) => [f, i]));
    const jump = (file) => {
      const index = indexOf.get(file);
      collapsedFiles.delete(index);
      keepFocus(panel, renderPanel);
      const target = panel.querySelector(`#gt-file-${index}`);
      if (target) { target.scrollIntoView({ block: 'start' }); target.querySelector('.gt-file-head')?.focus({ preventScroll: true }); }
    };
    const node = (name, dir, depth) => {
      const [label, content] = compact(name, dir);
      return h('li', null,
        h('details', { open: true },
          h('summary', { class: 'gt-tree-row', style: { '--gt-depth': String(depth) } }, icon('folder', { size: 14 }), h('span', { class: 'gt-tree-name' }, label), h('span', { class: 'gt-add' }, `+${content.insertions}`)),
          h('ul', { class: 'gt-tree-list' }, children(content, depth + 1))));
    };
    const children = (dir, depth) => [
      ...[...dir.dirs].sort(([a], [b]) => a.localeCompare(b)).map(([name, child]) => node(name, child, depth)),
      ...dir.files.sort((a, b) => a.name.localeCompare(b.name)).map(({ name, file }) => h('li', null,
        h('button', { type: 'button', class: 'gt-tree-row gt-tree-file', style: { '--gt-depth': String(depth) }, title: file.path, onClick: () => jump(file) },
          icon('file', { size: 14 }), h('span', { class: ['gt-tree-name', file.status === 'deleted' && 'gt-deleted'] }, name), h('span', { class: 'gt-add' }, `+${file.insertions}`)))),
    ];
    return h('nav', { class: 'gt-tree', 'aria-label': 'Changed files' }, h('ul', { class: 'gt-tree-list' }, children(fileTree(files), 0)));
  }

  function changesPanel() {
    if (!task.branch) return h('p', { class: 'gt-empty-note' }, 'No changes yet: the agent has not created its branch.');
    if (changesError) {
      const missing = changesError.status === 404 && !changesError.message.includes('branch');
      return loadError(missing
        ? `This server cannot show a task's changes yet (it needs GET /api/tasks/{id}/changes, docs/api.md). ${changesError.message}`
        : `Couldn’t load the changes: ${changesError.message}`, () => { changesError = null; render(); loadChanges(); });
    }
    if (!changes) return placeholder('Loading changes…', 'loading');
    if (!changes.files.length) return [toolbar(), h('p', { class: 'gt-empty-note' }, `No differences from ${integration()}.`)];
    return [
      toolbar(),
      h('div', { class: 'gt-changes' },
        treeView(changes.files),
        h('div', { class: ['gt-files', prefs.wrap && 'gt-wrapped'] },
          h('p', { class: 'gt-files-summary' }, `${plural(changes.files.length, 'file')} changed `, diffStat(sum('insertions'), sum('deletions'))),
          changes.files.map((file, index) => fileBlock(file, index)),
          changes.truncated ? h('p', { class: 'gt-note' }, 'The diff was cut short by the server; open the task or the pull request for the rest.') : null)),
    ];
  }

  // ---- Commits
  function commitsPanel() {
    if (commitsError) return loadError(`Couldn’t load the commits: ${commitsError.message}`, () => { commitsError = null; render(); loadCommits(); });
    if (!commits) return placeholder('Loading commits…', 'loading');
    if (!commits.length) return h('p', { class: 'gt-empty-note' }, `No commits on ${task.branch || 'the task branch'} yet${changes && changes.uncommitted ? '; its changes are not committed' : ''}.`);
    const repoUrl = sandbox.repo && sandbox.repo.url;
    return h('ol', { class: 'gt-commits' }, commits.map((c) => h('li', { class: 'gt-commit' },
      repoUrl ? h('a', { class: 'mono gt-sha', href: `${repoUrl}/commit/${c.sha}`, target: '_blank', rel: 'noopener noreferrer', title: c.sha }, c.sha.slice(0, 7)) : h('code', { class: 'gt-sha', title: c.sha }, c.sha.slice(0, 7)),
      h('span', { class: 'gt-commit-subject' }, c.subject),
      c.insertions !== undefined ? diffStat(c.insertions, c.deletions || 0) : null,
      h('span', { class: 'muted small' }, c.author || ''),
      h('span', { class: 'muted small', title: dateTime(c.at) }, relShort(c.at)))));
  }

  // ---- Branch
  function branchPanel() {
    const p = pr();
    const repoUrl = sandbox.repo && sandbox.repo.url;
    const int = integration();
    const rows = [
      ['Task branch', task.branch ? h('code', null, task.branch) : 'not created yet'],
      ['Into', h('code', null, int)],
      ['Then live on', h('code', null, profile.production_branch || sandbox.default_branch || 'main')],
      changes ? ['Position', `${plural(changes.ahead ?? 0, 'commit')} ahead, ${changes.behind ?? 0} behind ${int}`]
        : gitRow() ? ['Position', `${plural(gitRow().ahead ?? 0, 'commit')} ahead, ${gitRow().behind ?? 0} behind ${int}`] : null,
      !changes && gitRow() && gitRow().files_changed ? ['Changes', [plural(gitRow().files_changed, 'file'), ' ', diffStat(gitRow().insertions || 0, gitRow().deletions || 0)]] : null,
      changes && changes.head_sha ? ['Head', h('code', null, changes.head_sha.slice(0, 12))] : null,
      ['Pull request', p ? prChip(p) : 'none yet'],
    ].filter(Boolean);
    return h('div', { class: 'gt-branch-card card card-pad' },
      h('dl', { class: 'kv' }, rows.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)])),
      repoUrl && task.branch ? h('a', { class: ['btn', unsupported ? 'btn-primary' : 'btn-sm'], href: `${repoUrl}/compare/${encodeURIComponent(int)}...${encodeURIComponent(task.branch)}`, target: '_blank', rel: 'noopener noreferrer' }, icon('external', { size: 14 }), unsupported ? 'View the changes on GitHub' : 'Compare on GitHub') : null);
  }

  // ---- render and load
  function renderPanel() {
    panel.setAttribute('aria-labelledby', `gt-tab-${tab}`);
    setChildren(panel, tab === 'commits' ? commitsPanel() : tab === 'branch' ? branchPanel() : changesPanel());
  }

  function render() {
    renderMeta();
    for (const [id, button] of tabButtons) {
      const on = id === tab;
      button.setAttribute('aria-selected', String(on));
      button.tabIndex = on ? 0 : -1;
      button.classList.toggle('active', on);
      button.hidden = unsupported && id !== 'branch';
      const count = id === 'changes' && changes ? changes.files.length : id === 'commits' && commits ? commits.length : null;
      setChildren(button.querySelector('.gt-tab-count'), count === null ? null : String(count));
    }
    renderPanel();
  }

  let alive = true;
  async function loadChanges() {
    if (!task.branch) return;
    try {
      const data = await ctx.api.get(`/api/tasks/${enc(task.id)}/changes`, { signal: reads.signal });
      if (!alive) return;
      changes = { ...data, files: Array.isArray(data.files) ? data.files : [] };
    } catch (error) {
      if (!alive || error.code === 'aborted') return;
      if (missingApi(error)) noApi(); else changesError = error;
    }
    render();
  }
  async function loadCommits() {
    try {
      const data = await ctx.api.get(`/api/tasks/${enc(task.id)}/commits`, { signal: reads.signal });
      if (!alive) return;
      commits = Array.isArray(data) ? data : [];
    } catch (error) {
      if (!alive || error.code === 'aborted') return;
      if (missingApi(error)) noApi(); else commitsError = error;
    }
    render();
  }
  // No such route on this server (404/405, not a task without a branch): Branch only.
  function missingApi(error) { return (error.status === 404 || error.status === 405) && !/branch/i.test(error.message || ''); }
  function noApi() { unsupported = true; tab = 'branch'; }

  const unwatchPulls = ctx.store.watchPulls(sandbox.id);
  // The Git overview (pull requests, position, size) refreshes the header and the Branch tab.
  const offs = [ctx.store.on('pulls', (sid) => { if (sid === sandbox.id) { renderMeta(); if (tab === 'branch') renderPanel(); } })];
  const el = h('div', { class: 'page gt-page' }, h('div', { class: 'page-inner gt-inner' }, head, tablist, panel));
  render();
  loadChanges();
  loadCommits();

  return {
    el,
    /** A link to another tab of this review (?tab=…) switches in place. */
    update(next) {
      const wanted = unsupported ? 'branch' : TABS.some(([id]) => id === next.query.get('tab')) ? next.query.get('tab') : 'changes';
      if (wanted !== tab) { tab = wanted; render(); }
    },
    destroy() { alive = false; reads.abort(); unwatchPulls(); offs.forEach((off) => off()); },
  };
}
