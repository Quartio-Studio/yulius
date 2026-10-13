// Sessions (SPEC-001 SES-002, SES-003, UI-049): every Claude Code session of a sandbox as a
// read-only document — the sessions of its tasks and those people uploaded with `yulius onboard`
// — with kind and person filters and full-text search; one session opens as a transcript of
// messages, tool calls, edits (as diffs) and results. Secrets are redacted by the server.
// Reads GET /api/sandboxes/{sid}/sessions, /api/sessions/{id}/transcript; acts with
// POST /api/sessions/{id}/continue and DELETE /api/sessions/{id} (docs/api.md "Sessions").

import { enc } from '../api.js';
import { h, icon, setChildren, placeholder, emptyState, filterTabs, badge, toast, errorToast, confirmDialog, openModal, loadError, coalesce } from '../dom.js';
import { href, navigate, replaceQuietly, currentLocation } from '../router.js';
import { relLong, dateTime, bytes, plural } from '../format.js';
import { pageFrame, notFound } from './common.js';
import { BRAND } from '../brand.js';

const KIND_OPTIONS = [
  { value: '', label: 'All' },
  { value: 'task', label: 'Tasks', icon: 'terminal' },
  { value: 'imported', label: 'Imported', icon: 'upload' },
];
const ROLE_LABEL = { user: 'Prompt', assistant: 'Claude', tool: 'Tool', summary: 'Summary' };
const PAGE = 200;

const sessionsPath = (sandbox) => `/s/${encodeURIComponent(sandbox.slug)}/sessions`;
const sessionPath = (sandbox, id, line) => `${sessionsPath(sandbox)}/${encodeURIComponent(id)}${line !== undefined ? `?line=${line}` : ''}`;

/** "Task #12 · Working" or "Imported by @anna-q". */
function originLine(session) {
  if (session.kind === 'task' && session.task) return `Task #${session.task.number}`;
  return `Imported by @${session.author ? session.author.login : 'someone'}`;
}

/** A snippet with its matches marked. */
function snippet(parts) {
  return h('span', { class: 'ss-snippet' }, (parts || []).map((part) => (part.hit ? h('mark', null, part.text) : part.text)));
}

/** How people add their own sessions (the CLI). */
function openHowTo(ctx) {
  const server = (ctx.store.state.me && ctx.store.state.me.instance && ctx.store.state.me.instance.public_url) || window.location.origin;
  const command = `yulius onboard --server ${server}`;
  const copy = h('button', { type: 'button', class: 'btn btn-sm', onClick: async () => {
    try { await navigator.clipboard.writeText(command); toast('Copied.', { kind: 'ok' }); } catch { toast('Copy it from the box.', { kind: 'info' }); }
  } }, 'Copy');
  openModal({
    title: 'Add your Claude Code sessions',
    body: h('div', { class: 'stack' },
      h('p', null, 'Run this in the repository on your computer. It finds your Claude Code sessions of this repository, lets you choose all, recent ones or pick them, redacts secrets on your computer (and lists what it redacted), and uploads only new or changed sessions — attributed to you.'),
      h('div', { class: 'ss-command' }, h('code', null, command), copy),
      h('p', { class: 'muted small' }, 'Re-run it any time: finished uploads are kept and an interrupted upload resumes. Add --dry-run to see what would be sent.')),
    actions: [{ label: 'Close' }],
  });
}

/** Mount the Sessions page of a sandbox. */
export function mountSessionsPage(ctx, sandbox) {
  const base = sessionsPath(sandbox);
  const query = currentLocation().query;
  const state = {
    q: query.get('q') || '',
    kind: ['task', 'imported'].includes(query.get('kind')) ? query.get('kind') : '',
    author: query.get('author') || '',
  };
  const frame = pageFrame({
    title: 'Sessions',
    sub: 'every Claude Code session of this sandbox, read-only — agents search them too',
    actions: [h('button', { type: 'button', class: 'btn', onClick: () => openHowTo(ctx) }, icon('upload', { size: 14 }), 'Add your sessions')],
  });
  const search = h('input', { class: 'input ss-search', type: 'search', placeholder: 'Search what was said and done', 'aria-label': 'Search sessions', value: state.q });
  const kinds = filterTabs({ label: 'Kind of session', options: KIND_OPTIONS, value: state.kind, onChange: (value) => { state.kind = value; sync(); reload(); } });
  const people = h('select', { class: 'select select-sm ss-people', 'aria-label': 'Person', onChange: () => { state.author = people.value; sync(); reload(); } });
  const summary = h('span', { class: 'ss-summary muted small', 'aria-live': 'polite' });
  const list = h('div', { class: 'ss-list' }, placeholder('Loading sessions…', 'loading'));
  frame.body.append(h('div', { class: 'ss-toolbar' }, h('label', { class: 'ss-search-wrap' }, icon('search', { size: 14 }), search), kinds.el, people, summary), list);

  let alive = true;
  let generation = 0;
  let timer = null;
  search.addEventListener('input', () => {
    clearTimeout(timer);
    timer = setTimeout(() => { state.q = search.value.trim(); sync(); reload(); }, 300);
  });

  function sync() {
    const params = new URLSearchParams();
    if (state.q) params.set('q', state.q);
    if (state.kind) params.set('kind', state.kind);
    if (state.author) params.set('author', state.author);
    const qs = params.toString();
    if (alive) replaceQuietly(qs ? `${base}?${qs}` : base);
  }

  function renderPeople(authors) {
    const current = state.author;
    setChildren(people, h('option', { value: '' }, 'Everyone'), (authors || []).map((login) => h('option', { value: login, selected: login === current }, `@${login}`)));
    people.value = current;
    people.hidden = !authors || authors.length < 2;
  }

  function row(session) {
    const path = sessionPath(sandbox, session.id, session.hits && session.hits[0] ? session.hits[0].line : undefined);
    const when = session.ended_at || session.imported_at || session.started_at;
    return h('li', { class: 'ss-row' },
      h('a', { class: 'ss-row-link', href: href(path), dataset: { route: path } },
        h('span', { class: ['ss-kind', `ss-kind-${session.kind}`], 'aria-hidden': 'true' }, icon(session.kind === 'task' ? 'terminal' : 'upload', { size: 16 })),
        h('span', { class: 'ss-row-main' },
          h('span', { class: 'ss-row-title' }, session.title || 'Untitled session'),
          h('span', { class: 'ss-row-meta' },
            h('span', null, originLine(session)),
            session.kind === 'task' && session.author ? h('span', null, `@${session.author.login}`) : null,
            session.git_branch ? h('span', { class: 'ss-branch' }, icon('branch', { size: 12 }), session.git_branch) : null,
            when ? h('span', { title: dateTime(when) }, relLong(when)) : null,
            h('span', null, bytes(session.bytes)),
            session.redactions ? badge(`${plural(session.redactions, 'secret')} redacted`, 'warn', 'Redacted on the uploader’s computer before upload') : null,
            session.continued_in ? badge(`continued in #${session.continued_in.number}`, 'accent') : null),
          (session.hits || []).map((hit) => h('span', { class: 'ss-hit' }, h('span', { class: 'ss-hit-role' }, ROLE_LABEL[hit.role] || hit.role), snippet(hit.snippet)))),
        icon('right', { size: 14 })));
  }

  async function reload() {
    generation += 1;
    const mine = generation;
    const params = new URLSearchParams();
    if (state.q) params.set('q', state.q);
    if (state.kind) params.set('kind', state.kind);
    if (state.author) params.set('author', state.author);
    list.classList.add('ss-busy');
    list.setAttribute('aria-busy', 'true');
    try {
      const data = await ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/sessions?${params}`);
      if (!alive || mine !== generation) return;
      renderPeople(data.authors);
      const sessions = Array.isArray(data.sessions) ? data.sessions : [];
      const counts = new Map([['task', sessions.filter((s) => s.kind === 'task').length], ['imported', sessions.filter((s) => s.kind === 'imported').length]]);
      if (!state.kind) kinds.setCounts(counts, sessions.length);
      summary.textContent = state.q ? `${plural(data.total, 'session')} match` : '';
      if (sessions.length === 0) {
        setChildren(list, state.q || state.kind || state.author
          ? emptyState({ iconName: 'search', title: 'Nothing found', text: state.q ? 'No session mentions all of these words. Try fewer or other words: names of files, functions, errors or features.' : 'No session matches these filters.' })
          : emptyState({ iconName: 'chat', title: 'No sessions yet', text: 'Sessions of tasks appear here as tasks run. Upload the Claude Code sessions you had on your computer with yulius onboard.', action: h('button', { type: 'button', class: 'btn btn-primary', onClick: () => openHowTo(ctx) }, 'Add your sessions') }));
        return;
      }
      setChildren(list, h('ul', { class: 'ss-rows', 'aria-label': 'Sessions' }, sessions.map(row)));
    } catch (error) {
      if (!alive || mine !== generation) return;
      setChildren(list, loadError(`Couldn’t load the sessions: ${error.message}`, () => reload()));
    } finally {
      if (mine === generation) {
        list.classList.remove('ss-busy');
        list.removeAttribute('aria-busy');
      }
    }
  }
  reload();
  // Task sessions grow while tasks run: refresh quietly, at most every few seconds.
  const refresh = coalesce(() => { if (alive && !state.q) reload(); }, { minInterval: 5000 });
  const offs = [ctx.store.on('task', (task) => { if (task.sandbox_id === sandbox.id) refresh(); })];
  return {
    el: frame.el,
    destroy() { alive = false; clearTimeout(timer); refresh.cancel(); offs.forEach((off) => off()); },
  };
}

// ------------------------------------------------------------------ the read-only transcript

// A session is printed as Claude Code's TUI printed it (SPEC-075): `>` prompts, `⏺` messages and
// tool calls, `⎿` results folded to their first lines, in the terminal's typeface.
const TOOL_LABEL = { Edit: 'Update', MultiEdit: 'Update' }; // as the TUI names them
const FOLD_LINES = 3;

/** Text with ``` fences as code blocks (never parsed as HTML). */
function richText(text, className = 'ss-text') {
  const parts = String(text || '').split(/```/);
  return h('div', { class: className }, parts.map((part, index) => {
    if (index % 2 === 0) return part ? h('span', null, part) : null;
    const newline = part.indexOf('\n');
    const code = newline >= 0 && /^[\w+-]*$/.test(part.slice(0, newline)) ? part.slice(newline + 1) : part;
    return h('pre', { class: 'ss-code' }, h('code', null, code.replace(/\n$/, '')));
  }));
}

const linesOf = (text) => String(text || '').replace(/\n+$/, '').split('\n');

/** The TUI's gutter mark (`>`, `⏺`, `⎿`, `✻`), decoration only. */
const mark = (glyph, kind) => h('span', { class: ['ss-mark', kind && `ss-mark-${kind}`], 'aria-hidden': 'true' }, glyph);

/** A line of the TUI: its mark in the gutter, then the content (spans, so it may sit in a summary). */
const tuiLine = (glyph, content, { kind, className } = {}) => h('span', { class: ['ss-line', className] }, mark(glyph, kind), h('span', { class: 'ss-line-body' }, content));

/** A hunk as context, removed and added lines: the lines both sides share at the start and the end are context. */
function hunkLines(hunk) {
  const before = hunk.old ? linesOf(hunk.old) : [];
  const after = linesOf(hunk.new);
  let head = 0;
  while (head < before.length && head < after.length && before[head] === after[head]) head += 1;
  let tail = 0;
  while (tail < before.length - head && tail < after.length - head && before[before.length - 1 - tail] === after[after.length - 1 - tail]) tail += 1;
  return {
    head: after.slice(0, head),
    removed: before.slice(head, before.length - tail),
    added: after.slice(head, after.length - tail),
    tail: after.slice(after.length - tail),
  };
}

function diffView(diff) {
  return h('div', { class: 'ss-diff', role: 'group', 'aria-label': `Changes to ${diff.path}` },
    diff.hunks.map((hunk) => {
      const { head, removed, added, tail } = hunkLines(hunk);
      return h('pre', { class: 'ss-diff-hunk' },
        head.map((line) => h('span', { class: 'ss-ctx' }, `  ${line}\n`)),
        removed.map((line) => h('span', { class: 'ss-del' }, `- ${line}\n`)),
        added.map((line) => h('span', { class: 'ss-add' }, `+ ${line}\n`)),
        tail.map((line) => h('span', { class: 'ss-ctx' }, `  ${line}\n`)));
    }));
}

/** What the TUI prints under a folded call's `⎿`: a summary of the edit or read, else the first lines of the result. */
export function callPreview(call, result) {
  if (call.diff) {
    let added = 0;
    let removed = 0;
    for (const hunk of call.diff.hunks) {
      const lines = hunkLines(hunk);
      added += lines.added.length;
      removed += lines.removed.length;
    }
    return [`Updated ${call.diff.path} with ${plural(added, 'addition')} and ${plural(removed, 'removal')}`];
  }
  if (!result) return [];
  if (!result.text) return ['(No content)'];
  const lines = linesOf(result.text);
  if (call.name === 'Read' && !result.is_error) return [`Read ${plural(lines.length, 'line')}`];
  return lines.length > FOLD_LINES ? [...lines.slice(0, FOLD_LINES), `… +${plural(lines.length - FOLD_LINES, 'line')}`] : lines;
}

const outLines = (lines, className) => tuiLine('⎿', h('span', { class: 'ss-out-text' }, lines.join('\n')), { kind: 'out', className: ['ss-out', className] });

function toolCall(item, result) {
  const failed = Boolean(result && result.is_error);
  const preview = callPreview(item, result);
  return h('details', { class: ['ss-fold', 'ss-tool', failed && 'ss-tool-error'], open: Boolean(item.diff) },
    h('summary', null,
      tuiLine('⏺', h('span', { class: 'ss-call' },
        h('span', { class: 'ss-tool-name' }, TOOL_LABEL[item.name] || item.name),
        item.summary ? ['(', h('span', { class: 'ss-tool-summary' }, item.summary), ')'] : null,
        failed ? h('span', { class: 'ss-tool-flag' }, ' error') : null), { kind: failed ? 'err' : result ? 'ok' : 'call' }),
      preview.length ? outLines(preview, ['ss-preview', !item.diff && 'ss-preview-folded', failed && 'ss-out-err']) : null),
    item.diff ? diffView(item.diff) : h('pre', { class: 'ss-code ss-input' }, h('code', null, item.input)),
    result && !item.diff ? outLines(linesOf(result.text || '(No content)'), ['ss-result', failed && 'ss-out-err']) : null,
    result && item.diff ? outLines(linesOf(result.text || '(No content)'), 'ss-result') : null,
    result && result.truncated ? h('p', { class: 'ss-dim xs' }, 'Cut here: the result is longer.') : null);
}

function block(item, results, role) {
  switch (item.type) {
    case 'text':
      return h('div', { class: 'ss-block' },
        role === 'user' ? tuiLine('>', richText(item.text), { kind: 'prompt', className: 'ss-prompt' })
          : role === 'summary' ? tuiLine('※', richText(item.text), { kind: 'dim' })
            : tuiLine('⏺', richText(item.text), { kind: 'say' }),
        item.truncated ? h('p', { class: 'ss-dim xs' }, 'Cut here: the message is longer.') : null);
    case 'thinking':
      return h('details', { class: 'ss-fold ss-thinking' }, h('summary', null, tuiLine('✻', 'Thinking…', { kind: 'think' })), richText(item.text, 'ss-text ss-dim'));
    case 'tool_call':
      return toolCall(item, results.get(item.id));
    case 'tool_result':
      return results.has(item.tool_use_id) && results.get(item.tool_use_id).shown ? null : h('details', { class: ['ss-fold', 'ss-tool', item.is_error && 'ss-tool-error'] },
        h('summary', null, outLines(callPreview({ name: '' }, item), ['ss-preview', 'ss-preview-folded', item.is_error && 'ss-out-err'])),
        outLines(linesOf(item.text || '(No content)'), ['ss-result', item.is_error && 'ss-out-err']));
    case 'image':
      return tuiLine('⎿', h('span', { class: 'ss-dim' }, '[Image] (not kept)'), { kind: 'out' });
    case 'meta':
      return h('div', { class: 'ss-meta ss-dim xs' }, item.text);
    default:
      return null;
  }
}

/** Mount one session's read-only transcript. */
export function mountSessionView(ctx, sandbox, id, route) {
  const back = sessionsPath(sandbox);
  const target = route && route.query && /^\d+$/.test(route.query.get('line') || '') ? Number(route.query.get('line')) : null;
  let from = target === null ? 0 : Math.max(0, target - 5);
  let session = null;
  let next = null;
  let alive = true;
  let loading = false;
  const results = new Map(); // tool_use_id → result, shown inside its call

  const titleEl = h('h1', { class: 'page-title gt-title' }, 'Session');
  const meta = h('div', { class: 'gt-meta' });
  const actions = h('div', { class: 'ss-actions' });
  const notice = h('p', { class: 'ss-notice muted small' }, icon('lock', { size: 12 }), ' Read-only. Secrets are redacted.');
  const head = h('header', { class: 'gt-head' },
    h('a', { class: 'gt-back', href: href(back), dataset: { route: back } }, icon('back', { size: 14 }), 'Sessions'),
    h('div', { class: 'gt-title-row' }, titleEl, actions),
    meta, notice);
  const entriesEl = h('ol', { class: 'ss-entries', 'aria-label': 'Transcript' });
  const earlier = h('div', { class: 'ss-more' });
  const more = h('div', { class: 'ss-more' });
  const body = h('div', { class: 'ss-body' }, placeholder('Loading the transcript…', 'loading'));
  const el = h('div', { class: 'page ss-page' }, h('div', { class: 'page-inner ss-inner' }, head, body));

  function renderHead() {
    if (!session) return;
    setChildren(titleEl, session.title || 'Untitled session');
    const when = session.ended_at || session.started_at;
    setChildren(meta,
      h('span', { class: 'gt-state' }, icon(session.kind === 'task' ? 'terminal' : 'upload', { size: 14 }), originLine(session)),
      session.kind === 'task' && session.author ? h('span', null, `@${session.author.login}`) : null,
      session.git_branch ? h('span', { class: 'gt-branch' }, icon('branch', { size: 12 }), session.git_branch) : null,
      when ? h('span', { title: dateTime(when) }, relLong(when)) : null,
      h('span', null, bytes(session.bytes)),
      session.redactions ? badge(`${plural(session.redactions, 'secret')} redacted before upload`, 'warn') : null);
    const items = [];
    if (session.task) {
      const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${session.task.number}`;
      items.push(h('a', { class: 'btn', href: href(path), dataset: { route: path } }, `Open task #${session.task.number}`, icon('forward', { size: 14 })));
    }
    if (session.continued_in) {
      const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${session.continued_in.number}`;
      items.push(h('a', { class: 'btn', href: href(path), dataset: { route: path } }, `Continued in #${session.continued_in.number}`, icon('forward', { size: 14 })));
    } else if (session.can_continue) {
      items.push(h('button', { type: 'button', class: 'btn btn-primary', onClick: continueSession }, 'Continue in a new task'));
    }
    if (session.can_delete) items.push(h('button', { type: 'button', class: 'btn btn-quiet-danger', onClick: remove }, icon('trash', { size: 14 }), 'Delete'));
    setChildren(actions, items);
  }

  async function continueSession() {
    try {
      const task = await ctx.api.post(`/api/sessions/${enc(id)}/continue`);
      await ctx.store.loadTasks(sandbox.id).catch(() => null);
      toast(`Continuing in task #${task.number}.`, { kind: 'ok' });
      navigate(`/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`);
    } catch (error) {
      errorToast(error, 'Couldn’t continue the session');
    }
  }

  async function remove() {
    const ok = await confirmDialog({ title: `Delete “${session.title}”?`, message: `The uploaded transcript is removed from ${BRAND.name} for everyone; agents can no longer search it. The copy on the uploader’s computer stays.`, confirmLabel: 'Delete', danger: true });
    if (!ok) return;
    try {
      await ctx.api.del(`/api/sessions/${enc(id)}`);
      toast('Session deleted.', { kind: 'ok' });
      navigate(back);
    } catch (error) {
      errorToast(error, 'Couldn’t delete the session');
    }
  }

  function entry(item) {
    const role = item.role === 'user' && item.blocks.every((b) => b.type === 'tool_result' || b.type === 'meta') ? 'tool' : item.role;
    const shown = item.blocks.map((b) => block(b, results, role)).filter(Boolean);
    if (!shown.length) return null;
    const label = role === 'user' ? 'Prompt' : role === 'assistant' ? 'Claude' : ROLE_LABEL[role] || role;
    return h('li', { class: ['ss-entry', `ss-${role}`, item.sidechain && 'ss-sidechain', item.line === target && 'ss-target'], id: `line-${item.line}`, dataset: { line: String(item.line) } },
      h('span', { class: 'sr-only' }, item.sidechain ? `${label} (subagent)` : label),
      shown,
      role === 'user' && item.at ? h('span', { class: 'ss-when ss-dim xs', title: dateTime(item.at) }, relLong(item.at)) : null);
  }

  function append(entries) {
    // Results are shown inside the call they answer (when both are on this page).
    for (const item of entries) for (const b of item.blocks) if (b.type === 'tool_result') results.set(b.tool_use_id, { ...b });
    for (const item of entries) for (const b of item.blocks) if (b.type === 'tool_call' && results.has(b.id)) results.get(b.id).shown = true;
    entriesEl.append(...entries.map(entry).filter(Boolean));
  }

  function renderMore() {
    setChildren(more, next !== null && next !== undefined
      ? h('button', { type: 'button', class: 'btn', disabled: loading, onClick: () => load(next) }, loading ? 'Loading…' : 'Load more')
      : (entriesEl.children.length ? h('p', { class: 'ss-dim small' }, 'End of the session.') : null));
    setChildren(earlier, from > 0 && target !== null
      ? h('a', { class: 'btn btn-sm', href: href(sessionPath(sandbox, id)), dataset: { route: sessionPath(sandbox, id) } }, 'Show from the start')
      : null);
  }

  async function load(start) {
    if (loading) return;
    loading = true;
    renderMore();
    try {
      const page = await ctx.api.get(`/api/sessions/${enc(id)}/transcript?from=${start}&limit=${PAGE}`);
      if (!alive) return;
      const first = session === null;
      session = page.session;
      next = page.next;
      renderHead();
      if (first) setChildren(body, earlier, h('div', { class: 'ss-tui' }, entriesEl, more));
      append(page.entries || []);
      if (first && !entriesEl.children.length && next === null) setChildren(body, emptyState({ iconName: 'chat', title: 'Nothing to show', text: 'This transcript has no messages yet, or it is no longer on the server.' }));
      if (first && target !== null) {
        const found = entriesEl.querySelector(`#line-${target}`);
        if (found) found.scrollIntoView({ block: 'center' });
      }
    } catch (error) {
      if (!alive) return;
      // A session that is not there (deleted, or a wrong link) is a missing page, not a retry (#147).
      if (session === null && error.status === 404) setChildren(el, notFound({ title: 'Session not found', text: `There is no such session in ${sandbox.name}. It may have been deleted.`, back: ['Sessions', sessionsPath(sandbox)] }));
      else if (session === null) setChildren(body, loadError(`Couldn’t load the session: ${error.message}`, () => load(start)));
      else errorToast(error, 'Couldn’t load more');
    } finally {
      loading = false;
      if (alive && session) renderMore();
    }
  }
  load(from);
  return { el, destroy() { alive = false; } };
}
