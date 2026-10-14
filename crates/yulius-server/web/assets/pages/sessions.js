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
const ROLE_LABEL = { user: 'Prompt', assistant: 'Claude', tool: 'Tool', summary: 'Summary', system: 'System' };
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
    // Which time it is, said (SPEC-087 TIM-003): "ended 5 h ago", not a bare "5 h ago".
    const whenWord = session.ended_at ? 'ended' : session.imported_at ? 'imported' : 'started';
    return h('li', { class: 'ss-row' },
      h('a', { class: 'ss-row-link', href: href(path), dataset: { route: path } },
        h('span', { class: ['ss-kind', `ss-kind-${session.kind}`], 'aria-hidden': 'true' }, icon(session.kind === 'task' ? 'terminal' : 'upload', { size: 16 })),
        h('span', { class: 'ss-row-main' },
          h('span', { class: 'ss-row-title' }, session.title || 'Untitled session'),
          h('span', { class: 'ss-row-meta' },
            h('span', null, originLine(session)),
            session.kind === 'task' && session.author ? h('span', null, `@${session.author.login}`) : null,
            session.git_branch ? h('span', { class: 'ss-branch' }, icon('branch', { size: 12 }), session.git_branch) : null,
            when ? h('span', { title: dateTime(when) }, `${whenWord} ${relLong(when)}`) : null,
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
    return h('pre', { class: 'ss-code', ...scroller(code, 'Code') }, h('code', null, code.replace(/\n$/, '')));
  }));
}

/**
 * Long code, results and diffs scroll inside their box (480 px, about 20 lines): those are named
 * regions a keyboard reaches, as the Git page's diffs. Short ones are no tab stops.
 */
function scroller(text, label) {
  const lines = String(text || '').split('\n');
  const rows = lines.reduce((sum, line) => sum + Math.max(1, Math.ceil(line.length / 80)), 0);
  return rows > 16 ? { tabindex: '0', role: 'region', 'aria-label': label } : null;
}

const linesOf = (text) => String(text || '').replace(/\n+$/, '').split('\n');

/** The TUI's gutter mark (`>`, `⏺`, `⎿`, `✻`), decoration only. */
const mark = (glyph, kind) => h('span', { class: ['ss-mark', kind && `ss-mark-${kind}`], 'aria-hidden': 'true' }, glyph);

/** A line of the TUI: its mark in the gutter, then the content; spans so it may sit in a summary, divs (`block`) around block content. */
const tuiLine = (glyph, content, { kind, className, block = false } = {}) => h(block ? 'div' : 'span', { class: ['ss-line', ...[className].flat(Infinity)] }, mark(glyph, kind), h(block ? 'div' : 'span', { class: 'ss-line-body' }, content));

/** A hunk as context, removed and added lines: the lines both sides share at the start and the end are context. */
function hunkLines(hunk) {
  const before = hunk.old ? linesOf(hunk.old) : [];
  const after = hunk.new ? linesOf(hunk.new) : [];
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
      return h('pre', { class: 'ss-diff-hunk', ...scroller([...head, ...removed, ...added, ...tail].join('\n'), `Changed lines of ${diff.path}`) },
        head.map((line) => h('span', { class: 'ss-ctx' }, `  ${line}\n`)),
        removed.map((line) => h('span', { class: 'ss-del' }, `- ${line}\n`)),
        added.map((line) => h('span', { class: 'ss-add' }, `+ ${line}\n`)),
        tail.map((line) => h('span', { class: 'ss-ctx' }, `  ${line}\n`)));
    }));
}

/**
 * What the TUI prints under a folded call's `⎿`: a summary of an applied edit, write or read, else
 * the first lines of the result. Without a result (the session ended, or it is on the next page)
 * nothing is claimed.
 */
export function callPreview(call, result) {
  if (!result) return [];
  if (call.diff && !result.is_error) {
    let added = 0;
    let removed = 0;
    for (const hunk of call.diff.hunks) {
      const lines = hunkLines(hunk);
      added += lines.added.length;
      removed += lines.removed.length;
    }
    if (call.name === 'Write') return [`Wrote ${plural(added, 'line')} to ${call.diff.path}`];
    return [`Updated ${call.diff.path} with ${plural(added, 'addition')} and ${plural(removed, 'removal')}`];
  }
  if (!result.text) return ['(No content)'];
  const lines = linesOf(result.text);
  if (call.name === 'Read' && !result.is_error) return [`Read ${plural(lines.length, 'line')}`];
  return lines.length > FOLD_LINES ? [...lines.slice(0, FOLD_LINES), `… +${plural(lines.length - FOLD_LINES, 'line')}`] : lines;
}

// Claude Code's wrappers (crates/yulius-app/src/transcripts.rs META_PREFIXES), recorded at the start
// of a meta text and only separated by white space. A wrapper the server cut short runs to the end.
const WRAPPER_TAGS = ['command-name', 'command-message', 'command-args', 'local-command-stdout', 'local-command-stderr', 'bash-input', 'bash-stdout', 'bash-stderr', 'system-reminder', 'user-prompt-submit-hook'];
const WRAPPER_OPEN = new RegExp(`^<(${WRAPPER_TAGS.join('|')})>`);
const HIDDEN_TAGS = new Set(['system-reminder', 'user-prompt-submit-hook', 'command-message']);
const ANSI = /\u001b\[[0-9;?]*[A-Za-z]/g; // eslint-disable-line no-control-regex
const OUT_TAGS = { 'local-command-stdout': 'out', 'bash-stdout': 'out', 'local-command-stderr': 'err', 'bash-stderr': 'err' };

/** `value` without the start of `close` that a cut left at its end (`…</local-command-std`). */
function withoutCutClose(value, close) {
  for (let length = close.length - 1; length > 1; length -= 1) {
    if (value.endsWith(close.slice(0, length))) return value.slice(0, -length);
  }
  return value;
}

/**
 * A recorded meta text as the TUI prints it: `{ kind, text }` lines where kind is `command`
 * (`/name args`), `shell` (a `!` command), `out` or `err` (output under ⎿) or `note` (other text,
 * shown faint as written). Reminders, hook context and the caveat line are not printed: `[]`.
 * Only wrappers at the start are read; a tag mentioned in prose stays prose.
 */
export function metaLines(text) {
  const raw = String(text || '').trim();
  const found = [];
  let command = null;
  let rest = raw;
  for (let open = rest.match(WRAPPER_OPEN); open; open = rest.match(WRAPPER_OPEN)) {
    const tag = open[1];
    const close = `</${tag}>`;
    const end = rest.indexOf(close, open[0].length);
    // Command output keeps the CLI's colour codes; the page prints it plain.
    const value = (end < 0 ? withoutCutClose(rest.slice(open[0].length), close) : rest.slice(open[0].length, end)).replace(ANSI, '').trim();
    rest = end < 0 ? '' : rest.slice(end + close.length).trimStart();
    if (tag === 'command-name' || (tag === 'command-args' && (!command || command.args))) found.push(command = { kind: 'command', name: '', args: '' });
    if (tag === 'command-name') command.name = value;
    else if (tag === 'command-args') command.args = value;
    else if (tag === 'bash-input') found.push({ kind: 'shell', text: value });
    else if (OUT_TAGS[tag]) found.push({ kind: OUT_TAGS[tag], text: value });
    else if (!HIDDEN_TAGS.has(tag)) found.push({ kind: 'note', text: value });
  }
  if (rest === raw && /^\[Request interrupted/.test(rest)) found.push({ kind: 'err', text: 'Interrupted by user' });
  else if (rest && !(rest === raw && rest.startsWith('Caveat: The messages below were generated'))) found.push({ kind: 'note', text: rest });
  return found.map((line) => (line.kind === 'command'
    ? { kind: 'command', text: [line.name, line.args].filter(Boolean).join(' ') }
    : line)).filter((line) => line.text);
}

// A note longer than this is context the TUI does not print (a skill's body, hook context): folded
// (SPEC-090). Notes people act on, such as the Stop hook's goal and feedback, are at most 4 lines.
const NOTE_FOLD_LINES = 8;
const NOTE_FOLD_CHARS = 2000;

/** Whether a meta note folds to one `⎿ Context added (size)` line: over 8 lines or 2,000 characters (UTF-16 units). */
export function foldsNote(text) {
  const value = String(text || '');
  return value.length > NOTE_FOLD_CHARS || linesOf(value).length > NOTE_FOLD_LINES;
}

const utf8 = new TextEncoder();

/** A long meta note as one faint line that opens to the text as recorded. */
const foldedNote = (text) => h('details', { class: 'ss-fold ss-note' },
  h('summary', null, tuiLine('⎿', `Context added (${bytes(utf8.encode(text).length)})`, { kind: 'out', className: 'ss-out' })),
  h('div', { class: 'ss-meta ss-dim ss-note-text', ...scroller(text, 'Added context') }, text));

const parsedMeta = new WeakMap(); // meta block → its lines, parsed once

/** `metaLines` of a meta block. */
function metaOf(item) {
  if (!parsedMeta.has(item)) parsedMeta.set(item, metaLines(item.text));
  return parsedMeta.get(item);
}

const outLines = (lines, className, attrs = null) => h('span', { class: ['ss-line', 'ss-out', ...[className].flat(Infinity)], ...attrs }, mark('⎿', 'out'), h('span', { class: 'ss-line-body' }, h('span', { class: 'ss-out-text' }, lines.join('\n'))));

function toolCall(item, result) {
  const failed = Boolean(result && result.is_error);
  const preview = callPreview(item, result);
  // An applied edit shows its lines unfolded; a failed one stays folded on why it failed, and one
  // without a result folded too: nothing says it was applied.
  const diff = item.diff && result && !failed ? item.diff : null;
  return h('details', { class: ['ss-fold', 'ss-tool', failed && 'ss-tool-error'], open: Boolean(diff) },
    h('summary', null,
      tuiLine('⏺', h('span', { class: 'ss-call' },
        h('span', { class: 'ss-tool-name' }, TOOL_LABEL[item.name] || item.name),
        item.summary ? ['(', h('span', { class: 'ss-tool-summary' }, item.summary), ')'] : null,
        failed ? h('span', { class: 'ss-tool-flag' }, ' error') : null), { kind: failed ? 'err' : result ? 'ok' : 'call' }),
      preview.length ? outLines(preview, ['ss-preview', !diff && 'ss-preview-folded', failed && 'ss-out-err']) : null),
    item.diff ? diffView(item.diff) : h('pre', { class: 'ss-code ss-input', ...scroller(item.input, `Input of ${item.name}`) }, h('code', null, item.input)),
    result ? outLines(linesOf(result.text || '(No content)'), ['ss-result', failed && 'ss-out-err'], scroller(result.text, `Result of ${item.name}`)) : null,
    result && result.truncated ? h('p', { class: 'ss-dim xs' }, 'Cut here: the result is longer.') : null);
}

function block(item, results, role) {
  switch (item.type) {
    case 'text':
      return h('div', { class: 'ss-block' },
        role === 'user' ? tuiLine('>', richText(item.text), { kind: 'prompt', className: 'ss-prompt', block: true })
          : role === 'summary' ? tuiLine('※', richText(item.text), { kind: 'dim', block: true })
            : tuiLine('⏺', richText(item.text), { kind: 'say', block: true }),
        item.truncated ? h('p', { class: 'ss-dim xs' }, 'Cut here: the message is longer.') : null);
    case 'thinking':
      return h('details', { class: 'ss-fold ss-thinking' }, h('summary', null, tuiLine('✻', 'Thinking…', { kind: 'think' })), richText(item.text, 'ss-text ss-dim'));
    case 'tool_call':
      return toolCall(item, results.get(item.id));
    case 'tool_result':
      return results.has(item.tool_use_id) && results.get(item.tool_use_id).shown ? null : h('details', { class: ['ss-fold', 'ss-tool', item.is_error && 'ss-tool-error'] },
        h('summary', null, outLines(callPreview({ name: '' }, item), ['ss-preview', 'ss-preview-folded', item.is_error && 'ss-out-err'])),
        outLines(linesOf(item.text || '(No content)'), ['ss-result', item.is_error && 'ss-out-err'], scroller(item.text, 'Result')));
    case 'image':
      return tuiLine('⎿', h('span', { class: 'ss-dim' }, '[Image] (not kept)'), { kind: 'out' });
    case 'meta': {
      const lines = metaOf(item).map((line) => {
        if (line.kind === 'command') return tuiLine('>', line.text, { kind: 'prompt', className: 'ss-prompt' });
        if (line.kind === 'shell') return tuiLine('!', line.text, { kind: 'shell', className: 'ss-prompt' });
        if (line.kind === 'out' || line.kind === 'err') return outLines(linesOf(line.text), line.kind === 'err' && 'ss-out-err');
        return foldsNote(line.text) ? foldedNote(line.text) : h('div', { class: 'ss-meta ss-dim' }, line.text);
      });
      return lines.length ? h('div', { class: 'ss-block' }, lines) : null;
    }
    default:
      return null;
  }
}

const SVG_NS = 'http://www.w3.org/2000/svg';
const svgEl = (name, attributes, ...children) => {
  const node = document.createElementNS(SVG_NS, name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
  node.append(...children);
  return node;
};
/** Pixel art as SVG rects: each row's runs of set cells (`"x,y"`) as one rect of 1-pixel height. */
const pixelRuns = (cells, width, height, className) => {
  const rects = [];
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      if (!cells.has(`${x},${y}`)) continue;
      let run = 1;
      while (cells.has(`${x + run},${y}`)) run += 1;
      rects.push(svgEl('rect', { class: className, x, y, width: run, height: 1 }));
      x += run - 1;
    }
  }
  return rects;
};
/** A sprite's cells from rows of `#` (set) and `.` (clear). */
const spriteCells = (rows, x0 = 0, y0 = 0) => {
  const cells = new Set();
  rows.forEach((row, dy) => [...row].forEach((c, dx) => { if (c === '#') cells.add(`${x0 + dx},${y0 + dy}`); }));
  return cells;
};
/** How many pixels the spider climbs its thread (`ss-spider-climb` in styles.css climbs as many). */
const SPIDER_CLIMB = 6;

/**
 * A spider web in pixel art for the well's top-right corner (SPEC-075): threads from the corner,
 * rings between them and a spider hanging from one, as rows of square pixels. The spider and its
 * thread are groups of their own, so the stylesheet can make it climb (SPEC-101): its thread is
 * one rect that shortens by `--ss-silk-top`, and its legs are two frames.
 */
function spiderWeb() {
  const width = 28;
  const height = 34;
  const web = new Set();
  const plot = (into, x, y) => { if (x >= 0 && x < width && y >= 0 && y < height) into.add(`${x},${y}`); };
  const line = (into, x0, y0, x1, y1) => { // Bresenham
    const dx = Math.abs(x1 - x0);
    const dy = -Math.abs(y1 - y0);
    const sx = x0 < x1 ? 1 : -1;
    const sy = y0 < y1 ? 1 : -1;
    let err = dx + dy;
    for (;;) {
      plot(into, x0, y0);
      if (x0 === x1 && y0 === y1) return;
      const e2 = 2 * err;
      if (e2 >= dy) { err += dy; x0 += sx; }
      if (e2 <= dx) { err += dx; y0 += sy; }
    }
  };
  const corner = [width - 1, 0];
  // From the top wall (0°) round to the right wall (90°); the walls carry no thread of their own.
  const angles = [0, 22.5, 45, 67.5, 90].map((deg) => (deg * Math.PI) / 180);
  const at = (angle, r) => [Math.round(corner[0] - r * Math.cos(angle)), Math.round(corner[1] + r * Math.sin(angle))];
  for (const angle of angles.slice(1, -1)) line(web, ...corner, ...at(angle, 27));
  for (const r of [8, 15, 22]) {
    for (let i = 0; i + 1 < angles.length; i += 1) line(web, ...at(angles[i], r), ...at(angles[i + 1], r));
  }
  const [hx, hy] = at(angles[2], 15);
  const body = 29; // the spider's top row at rest
  const sprite = (rows) => spriteCells(rows, hx - 3, body);
  const spider = sprite(['.......', '..###..', '#######', '..###..', '.......']);
  const legs = [sprite(['.#...#.', '', '', '', '.#...#.']), sprite(['#.....#', '', '', '', '#.....#'])];
  for (const cell of [...spider, ...legs[0]]) web.delete(cell);
  // The thread it hangs by, from below the hub to just above it; it climbs SPIDER_CLIMB pixels.
  const silk = body - 1 - hy;
  const pixels = (cells, className) => pixelRuns(cells, width, height, className);
  const svg = svgEl('svg', { class: 'ss-web', viewBox: `0 0 ${width} ${height}`, 'aria-hidden': 'true', focusable: 'false', 'shape-rendering': 'crispEdges' },
    ...pixels(web, 'ss-web-thread'),
    svgEl('rect', { class: 'ss-web-silk', x: hx, y: hy + 1, width: 1, height: silk }),
    svgEl('g', { class: 'ss-web-spider' }, ...pixels(spider, 'ss-web-body'), svgEl('g', { class: 'ss-web-legs ss-web-legs-a' }, ...pixels(legs[0], 'ss-web-leg')), svgEl('g', { class: 'ss-web-legs ss-web-legs-b' }, ...pixels(legs[1], 'ss-web-leg'))));
  svg.style.setProperty('--ss-silk-top', String((silk - SPIDER_CLIMB) / silk));
  return svg;
}

/** A cockroach in pixel art, facing right: a body with its feelers and two frames of legs (SPEC-101). */
const ROACH = {
  width: 16,
  height: 7,
  body: ['...............#', '..............#.', '..#########.#...', '.#############..', '..#########.#...', '..............#.', '...............#'],
  legs: [
    ['..#..#..#.......', '...#..#..#......', '', '', '', '...#..#..#......', '..#..#..#.......'],
    ['....#..#..#.....', '...#..#..#......', '', '', '', '...#..#..#......', '....#..#..#.....'],
  ],
};

const reducedMotion = () => Boolean(window.matchMedia?.('(prefers-reduced-motion: reduce)').matches);

/** How long until the next cockroach (SPEC-101): 20–60 s after a session opens, then 45–150 s apart. */
export function cockroachDelay(first, random = Math.random) {
  const [min, max] = first ? [20000, 60000] : [45000, 150000];
  return min + Math.floor(random() * (max - min));
}

/**
 * Send one cockroach across the part of the well one can see, at a random height and in a random
 * direction, removed once it is out. None runs under reduced motion, in a hidden page or a well that
 * is not shown (null then).
 */
export function runCockroach(well, random = Math.random) {
  if (reducedMotion() || document.hidden || !well.isConnected) return null;
  const pixels = (rows, className) => pixelRuns(spriteCells(rows), ROACH.width, ROACH.height, className);
  const roach = svgEl('svg', { class: 'ss-roach', viewBox: `0 0 ${ROACH.width} ${ROACH.height}`, 'aria-hidden': 'true', focusable: 'false', 'shape-rendering': 'crispEdges' },
    ...pixels(ROACH.body, 'ss-roach-body'),
    ...ROACH.legs.map((rows, i) => svgEl('g', { class: `ss-roach-legs ss-roach-legs-${'ab'[i]}` }, ...pixels(rows, 'ss-roach-leg'))));
  const box = well.getBoundingClientRect();
  const scroller = well.closest('.page');
  const view = scroller ? scroller.getBoundingClientRect() : { top: 0, bottom: window.innerHeight };
  const top = Math.max(box.top, view.top);
  const bottom = Math.min(box.bottom, view.bottom);
  well.append(roach);
  const size = roach.getBoundingClientRect();
  if (bottom - top < size.height) {
    roach.remove();
    return null;
  }
  // From the well's padding edge, inside its border.
  roach.style.top = `${Math.floor(top - box.top - well.clientTop + random() * (bottom - top - size.height))}px`;
  const rightward = random() >= 0.5;
  const [from, to] = rightward ? [-size.width, box.width] : [box.width, -size.width];
  const face = rightward ? '' : ' scaleX(-1)';
  const run = roach.animate([{ transform: `translateX(${from}px)${face}` }, { transform: `translateX(${to}px)${face}` }], { duration: 2000 + Math.round(random() * 2000), easing: 'linear' });
  run.finished.then(() => roach.remove(), () => roach.remove());
  return roach;
}

/**
 * Send cockroaches across the well now and then until the returned stop() is called, which also
 * removes one still running. `timer` stands in for setTimeout and clearTimeout in tests.
 */
export function scheduleCockroaches(well, { random = Math.random, timer = { set: (fn, ms) => setTimeout(fn, ms), clear: (id) => clearTimeout(id) } } = {}) {
  let stopped = false;
  let handle = null;
  const next = (first) => {
    handle = timer.set(() => {
      if (stopped) return;
      runCockroach(well, random);
      next(false);
    }, cockroachDelay(first, random));
  };
  next(true);
  return () => {
    stopped = true;
    timer.clear(handle);
    for (const roach of well.querySelectorAll('.ss-roach')) roach.remove();
  };
}

/** Where the TUI's prompt box was: locked, nothing can be typed or sent (not a control). */
const lockedPrompt = () => h('div', { class: 'ss-locked' }, mark('>', 'prompt'), h('span', { class: 'ss-line-body' }, 'Read-only session · nothing can be sent'), icon('lock', { size: 14 }));

/** Mount one session's read-only transcript. */
export function mountSessionView(ctx, sandbox, id, route) {
  const back = sessionsPath(sandbox);
  const target = route && route.query && /^\d+$/.test(route.query.get('line') || '') ? Number(route.query.get('line')) : null;
  let from = target === null ? 0 : Math.max(0, target - 5);
  let session = null;
  let next = null;
  let alive = true;
  let loading = false;
  let stopCockroaches = () => {};
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
    const whenWord = session.ended_at ? 'ended' : 'started';
    setChildren(meta,
      h('span', { class: 'gt-state' }, icon(session.kind === 'task' ? 'terminal' : 'upload', { size: 14 }), originLine(session)),
      session.kind === 'task' && session.author ? h('span', null, `@${session.author.login}`) : null,
      session.git_branch ? h('span', { class: 'gt-branch' }, icon('branch', { size: 12 }), session.git_branch) : null,
      when ? h('span', { title: dateTime(when) }, `${whenWord} ${relLong(when)}`) : null,
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
    const typed = item.blocks.some((b) => b.type === 'meta' && metaOf(b).some((line) => line.kind === 'command' || line.kind === 'shell'));
    const label = role === 'user' || typed ? 'Prompt' : role === 'assistant' ? 'Claude' : ROLE_LABEL[role] || role;
    // A prompt (typed or a command) carries its time, as turns start there.
    const when = label === 'Prompt' && item.at;
    return h('li', { class: ['ss-entry', `ss-${role}`, item.sidechain && 'ss-sidechain', item.line === target && 'ss-target', when && 'ss-has-when'], id: `line-${item.line}`, dataset: { line: String(item.line) } },
      h('span', { class: 'sr-only' }, item.sidechain ? `${label} (subagent)` : label),
      shown,
      when ? h('span', { class: 'ss-when ss-dim xs', title: dateTime(item.at) }, relLong(item.at)) : null);
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
      if (first) {
        const tui = h('div', { class: 'ss-tui' }, spiderWeb(), entriesEl, more, lockedPrompt());
        setChildren(body, earlier, tui);
        stopCockroaches = scheduleCockroaches(tui);
      }
      append(page.entries || []);
      if (first && !entriesEl.children.length && next === null) {
        stopCockroaches();
        setChildren(body, emptyState({ iconName: 'chat', title: 'Nothing to show', text: 'This transcript has no messages yet, or it is no longer on the server.' }));
      }
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
  return { el, destroy() { alive = false; stopCockroaches(); } };
}
