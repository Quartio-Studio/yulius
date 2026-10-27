// File references in terminal output as links to the built-in editor (SPEC-001 UI-018, F 2026-10-10:
// "clicking on this should open editor … other files in claude should be clickable and open editor,
// as links open websites"). Every path Claude Code (or a shell) prints that names an existing file
// of the terminal's folder becomes a link, like a web link: the same hover underline and a plain
// click, which opens the Editor panel at the file and line.
//
// Forms: bare relative paths (`src/app.js`, `README.md`), paths in backticks, quotes, brackets or
// parentheses (Claude Code's `Read(src/app.js)`), `path:line`, `path:line:col`, `path#Lline`,
// absolute paths inside the folder, and OSC 8 hyperlinks to `file://` URLs.
//
// Plain click, not ⌘/Ctrl-click: xterm activates a link only for a click — the button goes down and
// up on the same link without a drag — so selecting text by dragging (and copying it) keeps working,
// exactly as for web links (UI-018).
//
// Only existing files inside the folder are links: a path is resolved against the folder (or the
// shell's working directory inside it), never above it (`..`) and never into `.git`, then looked up
// in its directory's listing from the files API — one request per directory, cached, asked only
// when the pointer rests on a line (never per keystroke or per output chunk). The server checks the
// path again when the editor opens it (symbolic links that leave the folder are refused there).
//
// The parsing and resolving functions are pure; createFileChecker needs only a list function;
// trackOscFileLinks and createFileLinkProvider use xterm's public parser, buffer and link APIs.
//
// Web links are the web-links addon's, except a URL a program cut at the right edge and continued on
// the next rows itself, which createContinuedUrlProvider links whole (SPEC-339, at the end).

/** Longest wrapped line (rows) considered as one line of text. */
const MAX_WRAPPED_ROWS = 8;
/** Most path candidates looked up per line. */
const MAX_CANDIDATES = 24;
/** Characters of a path segment. */
const SEGMENT = String.raw`[\w.@+%~-]`;
// A path: optional leading "/", "./" or "../", segments separated by "/", then an optional
// position (":12", ":12:5" or "#L12"). It starts where no path character precedes it.
const PATH_RE = new RegExp(String.raw`(?<![\w.@+%~/-])((?:\.{1,2}/|/)?(?:${SEGMENT}+/)*${SEGMENT}+)(?::(\d{1,7})(?::(\d{1,5}))?|#L(\d{1,7})(?:C(\d{1,5}))?)?`, 'g');
/** URLs (web links are the web-links addon's): paths inside them are not file links. */
const URL_RE = /\b[a-z][a-z0-9+.-]*:\/\/[^\s"'`<>()[\]{}]+/gi;

/** Whether a token looks like a file name: it has a folder part, an extension or is a dotfile. */
function looksLikeFile(token) {
  if (token.includes('/')) return /[^/.]/.test(token.replace(/^\.{1,2}\//, ''));
  return /^[\w@+%-][\w.@+%~-]*\.[A-Za-z0-9_]{1,12}$/.test(token) || /^\.\w[\w.-]*$/.test(token);
}

/**
 * Path candidates in a line of text: [{start, end, text, raw, line, col}] where `start`/`end` index
 * `text` (end exclusive), `raw` is the path as printed and `line`/`col` the position, if any.
 */
export function findPathCandidates(text) {
  const urls = [];
  for (const m of String(text).matchAll(URL_RE)) urls.push([m.index, m.index + m[0].length]);
  const found = [];
  for (const m of String(text).matchAll(PATH_RE)) {
    let raw = m[1];
    const hasPosition = m[2] !== undefined || m[4] !== undefined;
    let end = m.index + m[0].length;
    if (!hasPosition) {
      // A sentence's full stop (or an ellipsis) is not part of the name.
      const trimmed = raw.replace(/\.+$/, '');
      end -= raw.length - trimmed.length;
      raw = trimmed;
    }
    if (!raw || !looksLikeFile(raw)) continue;
    const start = m.index;
    if (urls.some(([a, b]) => start < b && end > a)) continue;
    const line = m[2] !== undefined ? Number(m[2]) : m[4] !== undefined ? Number(m[4]) : null;
    const col = m[3] !== undefined ? Number(m[3]) : m[5] !== undefined ? Number(m[5]) : null;
    found.push({ start, end, text: text.slice(start, end), raw, line: line || null, col: col || null });
    if (found.length >= MAX_CANDIDATES) break;
  }
  return found;
}

const trimSlashes = (path) => String(path || '').replace(/\/+$/, '');

/**
 * The path of `raw` relative to the folder, or null when it is not inside it. `cwd` is the working
 * directory relative to the folder ('' = the folder itself; null = outside it, so relative paths
 * name nothing here); `roots` are absolute paths of the folder as programs print it (e.g. the
 * worktree, its canonical path). Never `..` above the folder, never `.git`, never a home path (`~`).
 */
export function resolvePath(raw, { cwd = '', roots = [] } = {}) {
  let path = String(raw || '');
  if (!path || path.startsWith('~')) return null;
  if (path.startsWith('/')) {
    const root = roots.map(trimSlashes).filter(Boolean).sort((a, b) => b.length - a.length)
      .find((r) => path === r || path.startsWith(`${r}/`));
    if (!root) return null;
    path = path.slice(root.length + 1);
  } else if (cwd === null) {
    return null;
  } else if (cwd) {
    path = `${cwd}/${path}`;
  }
  const parts = [];
  for (const part of path.split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') {
      if (!parts.length) return null;
      parts.pop();
      continue;
    }
    if (part === '.git') return null;
    parts.push(part);
  }
  return parts.length ? parts.join('/') : null;
}

/**
 * An absolute working directory relative to the folder: '' for the folder itself (or when it is
 * unknown), 'sub/dir' inside it, null outside it.
 */
export function relativeCwd(cwd, roots = []) {
  if (!cwd || !String(cwd).startsWith('/')) return '';
  if (roots.map(trimSlashes).includes(trimSlashes(cwd))) return '';
  return resolvePath(cwd, { roots });
}

/**
 * The file a `file://` URL names, with an optional position (`#L12`, `#L12C5`, `#12` or a
 * trailing `:12[:5]`): {path, line, col} with an absolute path, or null for other URLs.
 */
export function parseFileUrl(uri) {
  let url;
  try { url = new URL(uri); } catch { return null; }
  if (url.protocol !== 'file:') return null;
  let path;
  try { path = decodeURIComponent(url.pathname); } catch { return null; }
  let line = null;
  let col = null;
  const hash = /^#L?(\d+)(?:[C:,](\d+))?$/i.exec(url.hash || '');
  if (hash) {
    line = Number(hash[1]);
    col = hash[2] ? Number(hash[2]) : null;
  } else {
    const tail = /:(\d+)(?::(\d+))?$/.exec(path);
    if (tail) {
      path = path.slice(0, tail.index);
      line = Number(tail[1]);
      col = tail[2] ? Number(tail[2]) : null;
    }
  }
  return path.startsWith('/') ? { path, line: line || null, col: col || null } : null;
}

/**
 * Whether paths are existing files, from directory listings: `list(dir)` resolves to the
 * directory's entries ([{name, kind}]) or rejects (missing, refused). One request per directory
 * at a time; listings are kept `ttl` ms, and a name missing from a listing older than `recheck` ms
 * asks again (an agent may have just written the file). At most `maxDirs` directories are kept.
 */
export function createFileChecker(list, { now = () => Date.now(), ttl = 30000, recheck = 3000, missingTtl = 10000, maxDirs = 300 } = {}) {
  const dirs = new Map();

  function load(dir) {
    const entry = { at: now(), names: null, pending: null };
    entry.pending = Promise.resolve()
      .then(() => list(dir))
      .then((entries) => {
        entry.names = new Map((Array.isArray(entries) ? entries : []).map((e) => [e.name, e.kind]));
      }, () => { entry.names = null; })
      .then(() => { entry.at = now(); entry.pending = null; });
    dirs.delete(dir);
    dirs.set(dir, entry);
    while (dirs.size > maxDirs) dirs.delete(dirs.keys().next().value);
    return entry;
  }

  /** Whether `path` (relative to the folder) is a file (or a link to one) in its directory. */
  async function isFile(path) {
    const slash = path.lastIndexOf('/');
    const dir = slash < 0 ? '' : path.slice(0, slash);
    const name = path.slice(slash + 1);
    let entry = dirs.get(dir);
    if (entry && !entry.pending) {
      const age = now() - entry.at;
      const stale = entry.names ? age > ttl || (age > recheck && !entry.names.has(name)) : age > missingTtl;
      if (stale) entry = null;
    }
    if (!entry) entry = load(dir);
    if (entry.pending) await entry.pending;
    const kind = entry.names && entry.names.get(name);
    return kind === 'file' || kind === 'symlink';
  }

  return { isFile, clear: () => dirs.clear() };
}

/**
 * The text of the (soft-wrapped) line that buffer row `y` (1-based) belongs to, with each code
 * unit's cell: {text, cells: [{x, y, w}]} (1-based columns and rows).
 */
function logicalLine(buffer, y) {
  let first = y - 1;
  while (first > 0 && y - 1 - first < MAX_WRAPPED_ROWS) {
    const line = buffer.getLine(first);
    if (!line || !line.isWrapped) break;
    first -= 1;
  }
  let text = '';
  const cells = [];
  for (let row = first; row < first + MAX_WRAPPED_ROWS * 2; row += 1) {
    const line = buffer.getLine(row);
    if (!line || (row > first && !line.isWrapped)) break;
    for (let x = 0; x < line.length; x += 1) {
      const cell = line.getCell(x);
      if (!cell) break;
      const width = cell.getWidth();
      if (width === 0) continue; // the second half of a wide character
      const chars = cell.getChars() || ' ';
      for (let k = 0; k < chars.length; k += 1) cells.push({ x: x + 1, y: row + 1, w: width });
      text += chars;
    }
  }
  return { text, cells };
}

/** The text of buffer cells from `start` to `end` (1-based, inclusive), across rows. */
function textOf(buffer, start, end) {
  let text = '';
  for (let y = start.y; y <= end.y; y += 1) {
    const line = buffer.getLine(y - 1);
    if (!line) return null;
    text += line.translateToString(false, y === start.y ? start.x - 1 : 0, y === end.y ? end.x : line.length);
  }
  return text;
}

/**
 * OSC 8 hyperlinks to `file://` URLs. xterm draws every hyperlink with a dashed underline and lets
 * only its link handler judge it; a file link must be a link only when the file exists inside the
 * folder, and plain text otherwise. So the terminal takes these sequences itself (xterm never sees
 * them: the label prints as plain text) and remembers where each label was printed and what it
 * said; the file link provider offers the ones whose label is still there (a full-screen program
 * redraws over them) and whose file exists. http(s) hyperlinks stay xterm's.
 */
export function trackOscFileLinks(term, { max = 256 } = {}) {
  let links = [];
  let open = null;
  const cursor = () => {
    const buffer = term.buffer.active;
    return { x: buffer.cursorX + 1, y: buffer.baseY + buffer.cursorY + 1 };
  };
  function finish(start, after) {
    // The label ends in the cell before the cursor (the previous row's last cell at a row start).
    let end = { x: after.x - 1, y: after.y };
    if (end.x < 1) {
      const above = term.buffer.active.getLine(after.y - 2);
      if (!above || after.y - 1 < start.y) return;
      end = { x: above.length, y: after.y - 1 };
    }
    if (end.y < start.y || (end.y === start.y && end.x < start.x)) return; // an empty label
    const label = textOf(term.buffer.active, start, end);
    if (!label || !label.trim()) return;
    links.push({ uri: start.uri, start: { x: start.x, y: start.y }, end, label });
    if (links.length > max) links = links.slice(-max);
  }
  term.parser.registerOscHandler(8, (data) => {
    const separator = data.indexOf(';');
    const uri = separator < 0 ? '' : data.slice(separator + 1);
    let mine = false;
    if (open) {
      finish(open, cursor());
      open = null;
      mine = !uri; // the end of a file link
    }
    if (/^file:/i.test(uri)) {
      open = { uri, ...cursor() };
      return true;
    }
    return mine; // other hyperlinks (and their ends) are xterm's
  });
  return {
    /** File hyperlinks on buffer row `y` (1-based) whose label is still on the screen. */
    linksOn(y) {
      const buffer = term.buffer.active;
      return links.filter((l) => l.start.y <= y && l.end.y >= y && textOf(buffer, l.start, l.end) === l.label);
    },
    /** The screen was replaced (a snapshot): the old positions mean nothing. */
    reset() {
      links = [];
      open = null;
    },
  };
}

const overlaps = (a, b) => !(a.end.y < b.start.y || (a.end.y === b.start.y && a.end.x < b.start.x) || b.end.y < a.start.y || (b.end.y === a.start.y && b.end.x < a.start.x));

/**
 * An xterm link provider for file references. `context()` returns {cwd, roots} (or null: no file
 * links now); `checker` is createFileChecker's; `osc` is trackOscFileLinks's (optional);
 * `open({path, line, col}, event)` opens the editor.
 */
export function createFileLinkProvider(term, { context, checker, osc = null, open }) {
  return {
    provideLinks(y, callback) {
      const ctx = context();
      if (!ctx) { callback(undefined); return; }
      const found = [];
      for (const link of osc ? osc.linksOn(y) : []) {
        const file = parseFileUrl(link.uri);
        const path = file && resolvePath(file.path, ctx);
        if (path) found.push({ path, line: file.line, col: file.col, text: link.uri, range: { start: link.start, end: link.end } });
      }
      const { text, cells } = logicalLine(term.buffer.active, y);
      for (const c of findPathCandidates(text)) {
        const first = cells[c.start];
        const last = cells[c.end - 1];
        if (!first || !last) continue;
        const range = { start: { x: first.x, y: first.y }, end: { x: last.x + last.w - 1, y: last.y } };
        if (range.end.y < y || range.start.y > y) continue; // another row of the wrapped line asks for it
        if (found.some((f) => overlaps(f.range, range))) continue; // already a hyperlink
        const path = resolvePath(c.raw, ctx);
        if (path) found.push({ path, line: c.line, col: c.col, text: c.text, range });
      }
      if (!found.length) { callback(undefined); return; }
      Promise.all(found.map((f) => checker.isFile(f.path).catch(() => false))).then((exists) => {
        const links = found.filter((_, i) => exists[i]).map((f) => ({
          range: f.range,
          text: f.text,
          decorations: { underline: true, pointerCursor: true },
          activate: (event) => open({ path: f.path, line: f.line, col: f.col }, event),
        }));
        callback(links.length ? links : undefined);
      });
    },
  };
}

// ---- web links a program wrapped itself (SPEC-339, ADR-0230)
//
// Claude Code lays out its rows itself: a word longer than a row (a URL) is cut at the right edge
// and continued on the next row where the text starts, after a cursor move. xterm joins only its
// own soft wraps, so the web-links addon saw two lines and linked the first fragment. A row
// continues into the next only as Claude Code continues it (WRL-001); anything less joins nothing.

/** Most rows one continued URL spans; a longer one is no link rather than a cut one. */
const MAX_CONTINUED_ROWS = 32;
/** Markers that Claude Code repeats on each row of a quote. */
const QUOTE_BARS = new Set(['▎', '│', '┃']);
/** The web-links addon's URL pattern (vendor/xterm/addon-web-links.mjs). */
const WEB_URL_RE = /(https?|HTTPS?):[/]{2}[^\s"'!*(){}|\\^<>`]*[^\s"':,.!?{}|\\^~[\]`()<>]/g;

/** A row as one string per cell ('' for the second half of a wide character), or null. */
const cellsOf = (row) => (row == null ? null : typeof row === 'string' ? [...row] : row);

/** Index of the last non-blank cell, or -1. */
function lastFilled(cells) {
  let i = cells.length - 1;
  while (i >= 0 && cells[i] === ' ') i -= 1;
  return i;
}

/** Index of the first non-blank cell at or after `from`. */
function skipBlanks(cells, from) {
  let i = from;
  while (i < cells.length && cells[i] === ' ') i += 1;
  return i;
}

/**
 * Where a row's text starts, and its marker: after the indent, one marker (`❯`, `●`, `⏺`, `-`,
 * `⎿`, `▎`) and its spaces, then a list number (`1.`) and its spaces.
 */
function textStart(cells) {
  let start = skipBlanks(cells, 0);
  let marker = null;
  const first = cells[start];
  if (first && first !== ' ' && !/[\p{L}\p{N}_]/u.test(first)) {
    const after = cells[start + 1] === '' ? start + 2 : start + 1; // a wide marker
    if (cells[after] === ' ') {
      marker = first;
      start = skipBlanks(cells, after);
    }
  }
  let digits = start;
  while (/^[0-9]$/.test(cells[digits] || '')) digits += 1;
  if (digits > start && digits - start <= 3 && (cells[digits] === '.' || cells[digits] === ')') && cells[digits + 1] === ' ') {
    start = skipBlanks(cells, digits + 1);
  }
  return { start, marker };
}

/**
 * Where row `b`'s text starts when row `a` continues into it as Claude Code cut it, else -1.
 * `gap`: blank columns `a` may leave at the right edge (the prompt's row pads one).
 */
function continuation(a, b, cols, gap) {
  if (!a || !b) return -1;
  const last = lastFilled(a);
  if (last < cols - 1 - gap) return -1; // short of the right edge
  const { start, marker } = textStart(a);
  // The next row's text starts in the same column: under blanks, or under the same quote bar.
  const lead = b.slice(0, start);
  const under = lead.every((cell) => cell === ' ') || (QUOTE_BARS.has(marker) && lead.join('') === a.slice(0, start).join(''));
  if (!under || start >= b.length || b[start] === ' ' || b[start] === '') return -1;
  let head = last;
  while (head > start && a[head - 1] !== ' ') head -= 1;
  let tail = start;
  while (tail < b.length && b[tail] !== ' ') tail += 1;
  // Only a word longer than the row's text is cut; a shorter one moves whole to the next row.
  if (last + 1 - head + tail - start <= last + 1 - start) return -1;
  // The next row starting a URL is a new one, unless the cut fell inside the scheme (`htt|ps://`).
  const next = b.slice(start, tail).join('');
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(next) && !/^[a-z]+$/i.test(a.slice(head, last + 1).join(''))) return -1;
  return start;
}

/** URLs in rows `first`…`last`, joined where each continuation row's text starts (`starts`), that cross a cut. */
function urlsAcross(grid, first, last, starts) {
  let text = '';
  const where = []; // per code unit of `text`: {row, col}
  const cuts = []; // offsets in `text` where a continuation row starts
  for (let row = first; row <= last; row += 1) {
    const cells = grid[row];
    if (row > first) cuts.push(text.length);
    for (let col = row > first ? starts[row - first - 1] : 0; col <= lastFilled(cells); col += 1) {
      for (let k = 0; k < cells[col].length; k += 1) where.push({ row, col });
      text += cells[col];
    }
  }
  const found = [];
  for (const m of text.matchAll(WEB_URL_RE)) {
    const start = m.index;
    const end = start + m[0].length;
    if (!cuts.some((cut) => cut > start && cut < end)) continue; // one row: the addon's
    try { new URL(m[0]); } catch { continue; }
    const tail = where[end - 1];
    const wide = grid[tail.row][tail.col + 1] === '';
    found.push({ url: m[0], start: { ...where[start] }, end: { row: tail.row, col: tail.col + (wide ? 1 : 0) } });
  }
  return found;
}

/**
 * URLs a program continued on the next rows itself: [{url, start: {row, col}, end: {row, col}}]
 * (0-based indexes into `rows`, end inclusive). `rows` are strings (one character per cell) or
 * arrays of cells, null for a row xterm soft-wrapped; `cols` is the terminal's width.
 */
export function findContinuedUrls(rows, cols) {
  const grid = rows.map(cellsOf);
  const found = [];
  let first = 0;
  while (first < grid.length) {
    const gap = grid[first] && textStart(grid[first]).marker === '❯' ? 1 : 0;
    let last = first;
    const starts = [];
    for (;;) {
      const start = last + 1 < grid.length ? continuation(grid[last], grid[last + 1], cols, gap) : -1;
      if (start < 0) break;
      starts.push(start);
      last += 1;
    }
    if (last > first && last - first < MAX_CONTINUED_ROWS) found.push(...urlsAcross(grid, first, last, starts));
    first = last + 1;
  }
  return found;
}

/** A buffer line as cells, or null for a soft-wrapped continuation (or none). */
function lineCells(line) {
  if (!line || line.isWrapped) return null;
  const cells = [];
  let cell;
  for (let x = 0; x < line.length; x += 1) {
    cell = line.getCell(x, cell);
    if (!cell) break;
    cells.push(cell.getWidth() === 0 ? '' : cell.getChars() || ' ');
  }
  return cells;
}

/**
 * An xterm link provider for URLs a program continued on the next rows (findContinuedUrls);
 * `open(url, event)` opens one. Register it before the web-links addon: where both link a URL's
 * first row, xterm keeps the link of the provider registered first.
 */
export function createContinuedUrlProvider(term, { open }) {
  return {
    provideLinks(y, callback) {
      const buffer = term.buffer.active;
      const top = Math.max(0, y - 1 - MAX_CONTINUED_ROWS);
      const bottom = Math.min(buffer.length - 1, y - 1 + MAX_CONTINUED_ROWS);
      const rows = [];
      for (let row = top; row <= bottom; row += 1) rows.push(lineCells(buffer.getLine(row)));
      const links = findContinuedUrls(rows, term.cols)
        .filter((l) => top + l.start.row <= y - 1 && top + l.end.row >= y - 1)
        .map((l) => ({
          range: { start: { x: l.start.col + 1, y: top + l.start.row + 1 }, end: { x: l.end.col + 1, y: top + l.end.row + 1 } },
          text: l.url,
          decorations: { underline: true, pointerCursor: true },
          activate: (event) => open(l.url, event),
        }));
      callback(links.length ? links : undefined);
    },
  };
}
