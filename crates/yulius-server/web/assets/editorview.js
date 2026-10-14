// Built-in code editor (SPEC-001 EDT-001..004): a small VS Code-styled editor for quick changes
// and reviews — explorer, tabs and Monaco (VS Code's editor component) with syntax colors and
// suggestions (language services for JavaScript/TypeScript, JSON, CSS and HTML; keywords and
// words of the open files for other popular languages). Each user's view (open files, unsaved
// text) lives only in this browser tab: nothing is stored on the server or shared. Saving
// writes the file into the sandbox; a file changed on disk since it was opened (agents edit
// the same files) is not overwritten silently. Word wrap (EDT-005) is one switch for every open
// file — the toolbar button or ⌥Z / Alt+Z as in VS Code — off by default and kept per user in
// this browser.
//
// Like the notepads (SPEC-127): the selection is drawn rounded and gliding (editorlayer.js), the
// people who have the same file open see each other's caret, name and selection in their colour
// (presence.js, relayed to the sandbox's writers and stored nowhere; no co-editing), and the tabs
// go in any order (drag, Alt+←/→, the tab's menu).

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, toast, errorToast, confirmDialog, openMenu, openModal, field, closableTabKeys, skeleton } from './dom.js';
import { allows } from './status.js';
import { token, activeTheme, onThemeChange, px, hexToken } from './theme.js';
import { readJson, writeJson } from './store.js';
import { SAVE_KEYS, WRAP_KEYS, WRAP_ARIA } from './shortcuts.js';
import { attachEditorDrop } from './filedrop.js';
import { createSplitter, layoutPrefs } from './splitter.js';
import { createEditorLayer } from './editorlayer.js';
import { createPresence, PRESENCE_SWEEP_MS } from './presence.js';
import { mapPosition, textEdit } from './textlayer.js';
import { createTabDrag, movedBy } from './tabdrag.js';

const MONACO_BASE = '/assets/vendor/monaco/vs';
const LANGUAGE_SERVICES = new Set(['javascript', 'typescript', 'json', 'css', 'scss', 'less', 'html']);
/** Word wrap preference per user in this browser: `true` wraps long lines (default off). */
const WRAP_KEY = 'yulius.editorWordWrap';
/** The explorer's narrowest width, and the room it always leaves the open file (UI-044). */
const EXPLORER_MIN = 140;
const EDITOR_MIN = 240;

/** ⌥Z / Alt+Z, the VS Code word-wrap toggle (`code` for macOS, where ⌥Z types "Ω"). */
function isWrapShortcut(event) {
  if (!event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return false;
  return event.code === 'KeyZ' || (event.key || '').toLowerCase() === 'z';
}

// Keywords for popular languages without a language service in the browser.
const KEYWORDS = {
  python: 'False None True and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield print len range enumerate zip dict list set tuple str int float bool open isinstance super self __init__ __name__ __main__',
  rust: 'as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while Option Some None Result Ok Err Vec String Box Rc Arc println format vec impl derive',
  go: 'break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var nil true false error string int int64 float64 bool byte rune make new len cap append copy delete panic recover fmt',
  java: 'abstract assert boolean break byte case catch char class const continue default do double else enum extends final finally float for if implements import instanceof int interface long native new package private protected public return short static strictfp super switch synchronized this throw throws transient try void volatile while true false null String System Override List Map',
  kotlin: 'as break class continue do else false for fun if in interface is null object package return super this throw true try typealias val var when while by catch constructor data enum finally get import init internal lateinit open override private protected public sealed set suspend',
  c: 'auto break case char const continue default do double else enum extern float for goto if inline int long register restrict return short signed sizeof static struct switch typedef union unsigned void volatile while NULL include define ifdef ifndef endif printf malloc free',
  cpp: 'alignas alignof auto bool break case catch char class const constexpr const_cast continue decltype default delete do double dynamic_cast else enum explicit export extern false float for friend goto if inline int long mutable namespace new noexcept nullptr operator private protected public return short signed sizeof static static_cast struct switch template this throw true try typedef typename union unsigned using virtual void volatile while std vector string include',
  csharp: 'abstract as base bool break byte case catch char checked class const continue decimal default delegate do double else enum event explicit extern false finally fixed float for foreach goto if implicit in int interface internal is lock long namespace new null object operator out override params private protected public readonly ref return sbyte sealed short sizeof static string struct switch this throw true try typeof uint ulong unchecked unsafe ushort using var virtual void volatile while async await',
  php: 'abstract and array as break callable case catch class clone const continue declare default do echo else elseif empty enddeclare endfor endforeach endif endswitch endwhile extends final finally fn for foreach function global goto if implements include instanceof insteadof interface isset list match namespace new or print private protected public readonly require return static switch throw trait try unset use var while yield null true false',
  ruby: 'BEGIN END alias and begin break case class def defined? do else elsif end ensure false for if in module next nil not or redo rescue retry return self super then true undef unless until when while yield puts require attr_accessor',
  swift: 'associatedtype class deinit enum extension fileprivate func import init inout internal let open operator private protocol public rethrows static struct subscript typealias var break case continue default defer do else fallthrough for guard if in repeat return switch where while as Any catch false is nil super self Self throw throws true try async await',
  shell: 'if then else elif fi case esac for while until do done in function select time return exit export local readonly declare echo printf cd ls grep sed awk cat source set unset shift test',
  sql: 'SELECT FROM WHERE INSERT INTO VALUES UPDATE SET DELETE CREATE TABLE ALTER DROP INDEX VIEW JOIN LEFT RIGHT INNER OUTER ON GROUP BY ORDER HAVING LIMIT OFFSET AS AND OR NOT NULL IS IN LIKE BETWEEN DISTINCT COUNT SUM AVG MIN MAX PRIMARY KEY FOREIGN REFERENCES UNIQUE DEFAULT CASE WHEN THEN ELSE END UNION ALL EXISTS',
  lua: 'and break do else elseif end false for function goto if in local nil not or repeat return then true until while require print pairs ipairs table string',
  r: 'if else repeat while function for next break TRUE FALSE NULL Inf NaN NA library require data.frame list vector c print',
  scala: 'abstract case catch class def do else extends false final finally for forSome if implicit import lazy match new null object override package private protected return sealed super this throw trait try true type val var while with yield',
  dart: 'abstract as assert async await break case catch class const continue covariant default deferred do dynamic else enum export extends extension external factory false final finally for Function get hide if implements import in interface is late library mixin new null on operator part required rethrow return set show static super switch sync this throw true try typedef var void while with yield',
  dockerfile: 'FROM RUN CMD LABEL EXPOSE ENV ADD COPY ENTRYPOINT VOLUME USER WORKDIR ARG ONBUILD STOPSIGNAL HEALTHCHECK SHELL AS',
  yaml: 'true false null',
};

let monacoPromise = null;

/** Loads Monaco once (AMD build; workers come from the same origin). */
function loadMonaco() {
  if (window.monaco) return Promise.resolve(window.monaco);
  if (monacoPromise) return monacoPromise;
  monacoPromise = new Promise((resolve, reject) => {
    const script = document.createElement('script');
    script.src = `${MONACO_BASE}/loader.js`;
    script.onload = () => {
      window.require.config({ paths: { vs: MONACO_BASE } });
      window.require(['vs/editor/editor.main'], () => {
        setupMonaco(window.monaco);
        resolve(window.monaco);
      }, (error) => reject(error));
    };
    script.onerror = () => reject(new Error('The editor couldn’t be loaded.'));
    document.head.append(script);
  }).catch((error) => { monacoPromise = null; throw error; });
  return monacoPromise;
}

// Monaco's chrome follows the tokens (brand, theme), not VS Code's blue (VS Code theme colour ids).
function defineEditorTheme(monaco) {
  // Colours as seen where the editor sits: a theme island (e.g. a dark work area on a light page) counts.
  const scope = document.querySelector('.ed-host') || document.body;
  const c = (name) => hexToken(name, scope);
  const well = c('--term-bg');
  const scheme = getComputedStyle(scope).colorScheme || activeTheme();
  monaco.editor.defineTheme('yulius', {
    base: scheme.includes('light') ? 'vs' : 'vs-dark',
    inherit: true,
    rules: [],
    colors: {
      'editor.background': well, 'editorGutter.background': well, 'minimap.background': well,
      'editor.foreground': c('--term-fg'),
      'editorCursor.foreground': c('--accent'),
      'editor.selectionBackground': c('--selection'),
      'editor.inactiveSelectionBackground': c('--accent-soft'),
      'editor.lineHighlightBackground': c('--surface-2'),
      'editorLineNumber.foreground': c('--text-faint'),
      'editorLineNumber.activeForeground': c('--text-2'),
      focusBorder: c('--accent'),
      'editorWidget.background': c('--surface-2'),
      'editorWidget.border': c('--border-strong'),
      'editorSuggestWidget.background': c('--surface-2'),
      'editorSuggestWidget.border': c('--border-strong'),
      'editorSuggestWidget.selectedBackground': c('--accent-soft'),
      'list.activeSelectionBackground': c('--accent-soft'),
      'list.hoverBackground': c('--surface-hover'),
      'input.background': c('--surface'),
      'input.border': c('--border-control'),
      'inputOption.activeBorder': c('--accent'),
      'button.background': c('--accent'),
      'button.foreground': c('--accent-ink'),
      'textLink.foreground': c('--accent-fg'),
      'scrollbarSlider.background': c('--border-strong'),
    },
  });
}

function setupMonaco(monaco) {
  // The editor's colours come from tokens.css and follow theme changes (theme.js).
  defineEditorTheme(monaco);
  onThemeChange(() => { defineEditorTheme(monaco); monaco.editor.setTheme('yulius'); });
  for (const [language, words] of Object.entries(KEYWORDS)) {
    const keywords = words.split(' ');
    monaco.languages.registerCompletionItemProvider(language, {
      provideCompletionItems(model, position) {
        const word = model.getWordUntilPosition(position);
        const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
        const seen = new Set();
        const suggestions = [];
        for (const keyword of keywords) {
          if (seen.has(keyword)) continue;
          seen.add(keyword);
          suggestions.push({ label: keyword, kind: monaco.languages.CompletionItemKind.Keyword, insertText: keyword, range });
        }
        // Identifiers of the open files of this language (Monaco's word suggestions stop
        // once another provider answers).
        for (const other of monaco.editor.getModels()) {
          if (other.getLanguageId() !== language) continue;
          for (const match of other.getValue().matchAll(/[A-Za-z_][A-Za-z0-9_]{2,}/g)) {
            const text = match[0];
            if (seen.has(text) || text === word.word || suggestions.length > 2000) continue;
            seen.add(text);
            suggestions.push({ label: text, kind: monaco.languages.CompletionItemKind.Text, insertText: text, range });
          }
        }
        return { suggestions };
      },
    });
  }
}

/** Monaco language id for a file name. */
function languageFor(monaco, path) {
  const name = path.split('/').pop() || '';
  const lower = name.toLowerCase();
  for (const language of monaco.languages.getLanguages()) {
    if ((language.filenames || []).some((f) => f.toLowerCase() === lower)) return language.id;
  }
  const dot = lower.lastIndexOf('.');
  const ext = dot >= 0 ? lower.slice(dot) : '';
  if (ext) {
    for (const language of monaco.languages.getLanguages()) {
      if ((language.extensions || []).includes(ext)) return language.id;
    }
  }
  if (lower === 'dockerfile' || lower.startsWith('dockerfile.')) return 'dockerfile';
  return 'plaintext';
}

/**
 * Editor panel view. getTask() returns the selected task (for its worktree).
 * Returns {el, headExtra, show, taskChanged}.
 */
export function createEditorView(ctx, sandbox, getTask) {
  const el = h('div', { class: 'panel-view ed' });
  if (!allows(sandbox.access, 'write')) {
    setChildren(el, h('div', { class: 'panel-msg' }, icon('lock', { size: 20 }), h('p', null, 'The editor requires write access on GitHub.')));
    return { el };
  }
  const api = ctx.api;
  const sid = enc(sandbox.id);
  let root = 'sandbox';
  let monaco = null;
  let editor = null;
  let layer = null; // the drawn selections (SPEC-127 EDT-007)
  let started = false;
  // {path, root, model, sha, saved, dirty, viewState, presence}; text instead of model while suspended
  const tabs = [];
  let active = null;
  let suspended = false; // another sandbox is in front: no Monaco models (SPEC-113)
  let shown = false; // the Editor is the tool in front
  const expanded = new Map(); // dir path → entries | null (loading)

  const folder = h('select', { class: 'select select-sm', 'aria-label': 'Folder' });
  // The sandbox, the selected task's worktree, and the folder open now (a terminal link can open
  // another task's worktree).
  function folderOptions() {
    const task = getTask();
    const options = [h('option', { value: 'sandbox' }, 'Sandbox')];
    if (task) options.push(h('option', { value: `task:${task.id}` }, `Task #${task.number} worktree`));
    if (root.startsWith('task:') && (!task || root !== `task:${task.id}`)) {
      const other = (ctx.store.tasksOf(sandbox.id) || []).find((t) => `task:${t.id}` === root);
      options.push(h('option', { value: root }, other ? `Task #${other.number} worktree` : 'Task worktree'));
    }
    setChildren(folder, options);
    folder.value = root;
  }
  folderOptions();
  folder.addEventListener('change', async () => {
    if (tabs.some((t) => t.dirty) && !(await confirmDialog({ title: 'Switch folder?', message: 'Unsaved changes in open files are discarded.', confirmLabel: 'Discard and switch', danger: true }))) {
      folder.value = root;
      return;
    }
    root = folder.value;
    closeAll();
    expanded.clear();
    renderTree();
    loadDir('');
  });

  const tree = h('div', { class: 'ed-tree', role: 'tree', 'aria-label': 'Files' });
  const tabbar = h('div', { class: 'ed-tabs', role: 'tablist', 'aria-label': 'Open files' });
  const host = h('div', { class: 'ed-host' });
  const empty = h('div', { class: 'ed-empty' }, h('p', null, 'Open a file from the explorer.'), h('p', { class: 'muted xs' }, `${SAVE_KEYS} saves, ${WRAP_KEYS} wraps long lines. Your open files and unsaved changes stay in this browser tab only.`));

  // Word wrap: one editor option, so it applies to every open file at once.
  const me = (ctx.store && ctx.store.state.me && ctx.store.state.me.user) || {};
  const wrapKey = `${WRAP_KEY}.${me.id || me.login || 'anonymous'}`;
  let wordWrap = readJson(wrapKey, false) === true;
  const wrapButton = h('button', { type: 'button', class: 'btn-icon ed-wrap', 'aria-label': 'Word wrap', 'aria-keyshortcuts': WRAP_ARIA, onClick: () => setWordWrap(!wordWrap) }, icon('wrap', { size: 16 }));
  function renderWrap() {
    wrapButton.setAttribute('aria-pressed', String(wordWrap));
    wrapButton.title = `Word wrap: ${wordWrap ? 'on' : 'off'} (${WRAP_KEYS})`;
  }
  function setWordWrap(on) {
    wordWrap = on;
    writeJson(wrapKey, on);
    if (editor) editor.updateOptions({ wordWrap: on ? 'on' : 'off' });
    renderWrap();
  }
  renderWrap();
  // Anywhere in the editor (Monaco, explorer, tabs, its toolbar): captured before Monaco, which would
  // otherwise type the character. Elsewhere ⌥Z / Alt+Z keeps its usual meaning (the terminal, fields).
  const wrapKeys = (event) => {
    if (!isWrapShortcut(event)) return;
    event.preventDefault();
    event.stopPropagation();
    if (!event.repeat) setWordWrap(!wordWrap);
  };
  el.addEventListener('keydown', wrapKeys, true);
  wrapButton.addEventListener('keydown', wrapKeys);
  folder.addEventListener('keydown', wrapKeys);
  const status = h('div', { class: 'ed-status' });
  const treeHead = h('div', { class: 'ed-tree-head' },
    h('span', null, 'Explorer'),
    iconButton('plus', 'New file or folder', (event) => newEntryMenu(event.currentTarget, '')),
    iconButton('refresh', 'Refresh', () => refreshTree()));
  const side = h('div', { class: 'ed-side' }, treeHead, tree);
  const edRoot = h('div', { class: 'ed-root' }, side, h('div', { class: 'ed-main' }, tabbar, h('div', { class: 'ed-stage' }, host, empty), status));
  setChildren(el, edRoot);

  // The explorer's width (UI-044): dragged on its right edge and kept per user; the stylesheet keeps
  // it within the panel when the panel narrows (and ignores it on phones).
  const prefs = layoutPrefs(ctx);
  const explorerRange = () => ({ min: EXPLORER_MIN, max: Math.max(EXPLORER_MIN, edRoot.getBoundingClientRect().width - EDITOR_MIN) });
  function setExplorer(px) {
    if (px) edRoot.style.setProperty('--ed-side-w', `${Math.round(px)}px`);
    else edRoot.style.removeProperty('--ed-side-w');
  }
  setExplorer(prefs.get('explorer'));
  const explorerSplitter = createSplitter({
    label: 'Resize explorer',
    className: 'splitter-explorer',
    width: () => side.getBoundingClientRect().width,
    resize: (px) => { const { min, max } = explorerRange(); setExplorer(Math.max(min, Math.min(max, px))); },
    commit: () => prefs.set('explorer', side.getBoundingClientRect().width),
    reset: () => { prefs.clear('explorer'); setExplorer(null); },
    range: explorerRange,
  });
  side.append(explorerSplitter.el);
  // The panel's width changes the explorer's room: its separator keeps saying the true width.
  const sizeWatch = typeof ResizeObserver === 'function' ? new ResizeObserver(() => explorerSplitter.sync()) : null;
  if (sizeWatch) sizeWatch.observe(edRoot);

  const query = (path) => `root=${encodeURIComponent(root)}&path=${encodeURIComponent(path)}`;
  // Files dropped on the explorer or the editor, or picked with its upload button (EDT-006).
  attachEditorDrop({ ctx, sandboxId: sandbox.id, tree, host, head: treeHead, getRoot: () => root, activePath: () => (active ? active.path : null), reveal: (dir) => loadDir(dir), refresh: () => refreshTree(), open: (path) => openFile(path) });

  // ---------------------------------------------------------------- explorer
  async function loadDir(path) {
    expanded.set(path, null);
    renderTree();
    try {
      const listing = await api.get(`/api/sandboxes/${sid}/files?${query(path)}`);
      expanded.set(path, listing.entries || []);
      if (listing.truncated) toast(`Only the first ${listing.entries.length} entries of ${path || 'the folder'} are shown.`, { kind: 'info' });
    } catch (error) {
      expanded.delete(path);
      if (path === '') setChildren(tree, h('div', { class: 'panel-msg' }, error.status === 409 ? 'The editor opens once the sandbox repository has been cloned.' : (error.message || 'Couldn’t list files.')));
      else errorToast(error, 'Couldn’t list the folder');
      return;
    }
    renderTree();
  }

  async function refreshTree() {
    const dirs = [...expanded.keys()];
    for (const dir of dirs) {
      try {
        const listing = await api.get(`/api/sandboxes/${sid}/files?${query(dir)}`);
        expanded.set(dir, listing.entries || []);
      } catch {
        expanded.delete(dir);
      }
    }
    renderTree();
  }

  function renderTree() {
    const rows = [];
    let busy = false;
    const walk = (dir, depth) => {
      const entries = expanded.get(dir);
      if (entries === null) { busy = true; rows.push(h('div', { class: 'ed-row ed-row-loading', style: { paddingLeft: `${8 + depth * 12}px` } }, skeleton('Loading the folder…', 1, { announce: false }))); return; }
      if (!entries) return;
      for (const entry of entries) {
        const isDir = entry.kind === 'dir';
        const open = isDir && expanded.has(entry.path);
        const row = h('div', {
          class: ['ed-row', active && active.path === entry.path && 'active', entry.kind === 'symlink' && 'muted'],
          role: 'treeitem',
          'aria-expanded': isDir ? String(open) : null,
          title: entry.path,
          dataset: { path: entry.path, kind: entry.kind },
          style: { paddingLeft: `${8 + depth * 12}px` },
          tabindex: '0',
        },
        h('span', { class: 'ed-twisty' }, isDir ? (open ? '▾' : '▸') : ''),
        h('span', { class: 'ed-name' }, entry.name));
        const activate = () => {
          if (isDir) {
            if (open) { expanded.delete(entry.path); renderTree(); } else loadDir(entry.path);
          } else if (entry.kind === 'file') openFile(entry.path);
        };
        row.addEventListener('click', activate);
        row.addEventListener('keydown', (event) => { if (event.key === 'Enter') activate(); });
        row.addEventListener('contextmenu', (event) => {
          event.preventDefault();
          openMenu({ x: event.clientX, y: event.clientY }, [
            isDir ? { label: 'New file…', onSelect: () => createEntry(entry.path, 'file') } : null,
            isDir ? { label: 'New folder…', onSelect: () => createEntry(entry.path, 'dir') } : null,
            !isDir ? { label: 'Open', onSelect: () => openFile(entry.path) } : null,
            { label: 'Copy path', onSelect: () => navigator.clipboard && navigator.clipboard.writeText(entry.path) },
            { separator: true },
            { label: 'Delete…', danger: true, onSelect: () => deleteEntry(entry) },
          ].filter(Boolean));
        });
        rows.push(row);
        if (open) walk(entry.path, depth + 1);
      }
    };
    walk('', 0);
    setChildren(tree, rows.length ? rows : h('div', { class: 'ed-row muted' }, 'Empty folder'));
    // A folder still listed shows a shimmering row; the tree says it is busy until it is read (SPEC-092).
    tree.setAttribute('aria-busy', String(busy));
  }

  function newEntryMenu(anchor, dir) {
    openMenu(anchor, [
      { label: 'New file…', onSelect: () => createEntry(dir, 'file') },
      { label: 'New folder…', onSelect: () => createEntry(dir, 'dir') },
    ]);
  }

  async function createEntry(dir, kind) {
    const input = h('input', { class: 'input', type: 'text', placeholder: kind === 'file' ? 'name.ext or sub/dir/name.ext' : 'folder name', autocomplete: 'off' });
    const modal = openModal({
      title: kind === 'file' ? `New file${dir ? ` in ${dir}` : ''}` : `New folder${dir ? ` in ${dir}` : ''}`,
      body: field('Name', input),
      actions: [
        { label: 'Cancel', value: false },
        { label: 'Create', kind: 'primary', value: 'create' },
      ],
    });
    input.addEventListener('keydown', (event) => { if (event.key === 'Enter') { event.preventDefault(); modal.close('create'); } });
    setTimeout(() => input.focus(), 30);
    const name = (await modal.result) === 'create' ? input.value.trim() : '';
    if (!name) return;
    const path = dir ? `${dir}/${name}` : name;
    try {
      await api.post(`/api/sandboxes/${sid}/files`, { root, path, kind });
    } catch (error) {
      errorToast(error, `Couldn’t create ${path}`);
      return;
    }
    await loadDir(dir);
    if (kind === 'file') openFile(path);
  }

  async function deleteEntry(entry) {
    const ok = await confirmDialog({
      title: `Delete ${entry.name}?`,
      message: entry.kind === 'dir' ? `The folder ${entry.path} and everything in it is deleted from the sandbox.` : `${entry.path} is deleted from the sandbox.`,
      confirmLabel: 'Delete',
      danger: true,
    });
    if (!ok) return;
    try {
      await api.del(`/api/sandboxes/${sid}/files?${query(entry.path)}`);
    } catch (error) {
      errorToast(error, `Couldn’t delete ${entry.path}`);
      return;
    }
    for (const tab of [...tabs]) if (tab.path === entry.path || tab.path.startsWith(`${entry.path}/`)) closeTab(tab, true);
    const parent = entry.path.includes('/') ? entry.path.slice(0, entry.path.lastIndexOf('/')) : '';
    expanded.delete(entry.path);
    loadDir(parent);
  }

  // ---------------------------------------------------------------- presence (SPEC-127 EDT-010)
  // Each open file has its presence; only the one in front is shown, while the Editor is shown,
  // its sandbox is in front and the page is visible. Offsets travel in the file's text as saved
  // (`saved`, at `sha`), mapped through the unsaved typing on each side; someone at another `sha`
  // is hidden until both have the same saved text.
  const myId = me.id || null;

  /**
   * The edit from the shown text to the saved one (`toSaved`) or back, or `null` while they are the
   * same (Monaco's version is the saved one's: no whole-text comparison on every key).
   */
  function savedEdit(tab, toSaved) {
    if (tab.model.getAlternativeVersionId() === tab.savedVersion) return null;
    const text = tab.model.getValue();
    return toSaved ? textEdit(text, tab.saved) : textEdit(tab.saved, text);
  }
  const mapThrough = (offset, edit) => (edit ? mapPosition(offset, edit) : offset);

  /** Marks `tab`'s shown text as its saved one, at `version` of its model. */
  function savedAs(tab, text, version = tab.model ? tab.model.getAlternativeVersionId() : null) {
    tab.saved = text;
    tab.savedVersion = version;
  }

  /** Where this person is in `tab`'s saved text, or `null` while it is not the file in front. */
  function ownSelection(tab) {
    if (!editor || tab !== active || !tab.model || editor.getModel() !== tab.model) return null;
    const selection = editor.getSelection();
    if (!selection) return null;
    const anchor = tab.model.getOffsetAt({ lineNumber: selection.selectionStartLineNumber, column: selection.selectionStartColumn });
    const head = tab.model.getOffsetAt({ lineNumber: selection.positionLineNumber, column: selection.positionColumn });
    const edit = savedEdit(tab, true);
    return { anchor: mapThrough(anchor, edit), head: mapThrough(head, edit) };
  }

  /** The others in `tab`, drawn when it is the file in front: those at its `sha`, mapped through the typing. */
  function drawPeers(tab) {
    if (!layer || tab !== active || !tab.model || editor.getModel() !== tab.model) return;
    const list = [];
    // Nobody else here: nothing to map (typing in a large file stays cheap).
    if (tab.presence.present && tab.presence.peers.size) {
      const length = tab.model.getValueLength();
      const edit = savedEdit(tab, false);
      for (const [id, peer] of tab.presence.peers) {
        if (peer.revision !== tab.sha) continue;
        const clamp = (offset) => Math.min(mapThrough(offset, edit), length);
        const anchor = clamp(peer.selection.anchor);
        const head = clamp(peer.selection.head);
        list.push({ id, colourOf: peer.userId, name: peer.name, start: Math.min(anchor, head), end: Math.max(anchor, head), head });
      }
    }
    layer.setPeers(list);
  }

  function withPresence(tab) {
    tab.presence = createPresence({
      put: ({ revision, ...body }, options) => api.put(`/api/sandboxes/${sid}/files/presence`, { root: tab.root, path: tab.path, sha256: revision, ...body }, options),
      state: ({ leaving }) => ({ revision: tab.sha || '', selection: leaving ? null : ownSelection(tab) }),
      draw: () => drawPeers(tab),
      myId,
    });
    return tab;
  }

  /** Shows the file in front to the others and takes every other file away (`going`: the page goes). */
  function syncPresence({ going = false } = {}) {
    const visible = shown && !suspended && document.visibilityState !== 'hidden';
    for (const tab of tabs) tab.presence.setShown(visible && tab === active && Boolean(tab.model), { going });
  }

  /** The text or its saved version moved: redraw the others and tell them. */
  function presenceMoved(tab) {
    if (!tab || !tab.presence) return;
    drawPeers(tab);
    tab.presence.soon();
  }

  const offs = [
    ctx.store.on('file-presence', (event) => {
      if (event.sandbox_id !== sandbox.id) return;
      // A file's saved version is its `sha256`, the revision its offsets are in.
      for (const tab of tabs) if (tab.root === event.root && tab.path === event.path) tab.presence.peer({ ...event, revision: event.sha256 });
    }),
    ctx.store.on('resync', () => { for (const tab of tabs) tab.presence.lost(); }),
  ];
  // People who went quiet fade out; the file in front says it is still there now and then.
  const sweep = setInterval(() => { for (const tab of tabs) tab.presence.tick(); }, PRESENCE_SWEEP_MS);
  // A hidden page has left its file for the others (with a request that outlives a closing page).
  const onVisibility = () => syncPresence({ going: document.visibilityState === 'hidden' });
  const onPageHide = () => { for (const tab of tabs) tab.presence.setShown(false, { going: true }); };
  document.addEventListener('visibilitychange', onVisibility);
  window.addEventListener('pagehide', onPageHide);

  // ---------------------------------------------------------------- files and tabs
  async function ensureEditor() {
    if (editor) return true;
    try {
      monaco = await loadMonaco();
    } catch (error) {
      errorToast(error, 'The editor couldn’t be loaded');
      return false;
    }
    defineEditorTheme(monaco); // the host is in place now: take its theme
    editor = monaco.editor.create(host, {
      theme: 'yulius',
      automaticLayout: true,
      fontFamily: token('--font-mono'),
      fontSize: px('--fs'),
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      renderWhitespace: 'selection',
      wordWrap: wordWrap ? 'on' : 'off',
      tabSize: 2,
      wordBasedSuggestions: 'allDocuments',
      quickSuggestions: { other: true, comments: false, strings: false },
      // One plain box per range and row, copied into rounded, gliding shapes (editorlayer.js).
      roundedSelection: false,
      cursorSmoothCaretAnimation: 'on',
    });
    layer = createEditorLayer(monaco, editor);
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => { if (active) save(active); });
    editor.onDidChangeCursorPosition(() => renderStatus());
    editor.onDidChangeCursorSelection(() => { if (active && active.presence) active.presence.soon(); });
    return true;
  }

  async function openFile(path) {
    const existing = tabs.find((t) => t.path === path);
    if (existing) { activate(existing); return; }
    if (!(await ensureEditor())) return;
    let file;
    try {
      file = await api.get(`/api/sandboxes/${sid}/files/content?${query(path)}`);
    } catch (error) {
      errorToast(error, `Couldn’t open ${path}`);
      return;
    }
    if (file.binary || file.too_large || file.content == null) {
      toast(file.too_large ? `${path} is too large for the editor (${Math.round(file.size / 1024)} KB).` : `${path} is a binary file.`, { kind: 'info' });
      return;
    }
    const tab = withPresence({ path, root, model: null, sha: file.sha256, saved: file.content, dirty: false, viewState: null, saving: false });
    tabs.push(tab);
    // Opened while another sandbox came to the front: it shows when this one is back.
    if (suspended) { tab.text = file.content; active = tab; renderTabs(); return; }
    attach(tab, file.content);
    activate(tab);
  }

  /** The tab's text, from its model, or kept while suspended. */
  const textOf = (tab) => (tab.model ? tab.model.getValue() : tab.text);

  /** Gives `tab` a Monaco model holding `text`; the person's edits mark it modified. */
  function attach(tab, text) {
    tab.model = monaco.editor.createModel(text, languageFor(monaco, tab.path));
    // Monaco keeps one kind of line end: the saved text is compared in its terms.
    // A new model counts its versions afresh: unsaved text is compared in full.
    if (tab.saved === text) savedAs(tab, tab.model.getValue());
    else tab.savedVersion = null;
    tab.model.onDidChangeContent(() => {
      presenceMoved(tab);
      if (tab.ignoreChange) return;
      if (!tab.dirty) { tab.dirty = true; renderTabs(); renderStatus(); }
    });
  }

  /**
   * Opens `path` of folder `root` with the cursor at `line`/`col` (a terminal's file link, UI-018,
   * F 2026-10-10). Another folder is opened first; unsaved changes are asked about, as when the
   * folder is switched by hand.
   */
  async function openAt({ root: wanted = 'sandbox', path, line = null, col = null }) {
    if (!path) return;
    if (wanted !== root) {
      if (tabs.some((t) => t.dirty) && !(await confirmDialog({ title: 'Switch folder?', message: 'Unsaved changes in open files are discarded.', confirmLabel: 'Discard and switch', danger: true }))) return;
      root = wanted;
      folderOptions();
      closeAll();
      expanded.clear();
      renderTree();
      loadDir('');
    }
    await openFile(path);
    if (!line || !editor || !active || !active.model || active.path !== path) return;
    const lineNumber = Math.max(1, Math.min(line, active.model.getLineCount()));
    const column = Math.max(1, Math.min(col || 1, active.model.getLineMaxColumn(lineNumber)));
    editor.setPosition({ lineNumber, column });
    editor.revealLineInCenter(lineNumber);
    editor.focus();
  }

  /** `focus: false` keeps keyboard focus where it is (closing a tab from the strip, #80). */
  function activate(tab, { focus = true } = {}) {
    // Suspended: it shows when the sandbox is back (resume), with the cursor it had.
    if (suspended) { active = tab; renderTabs(); return; }
    if (active && active.model && editor) active.viewState = editor.saveViewState();
    active = tab;
    editor.setModel(tab.model);
    if (tab.viewState) editor.restoreViewState(tab.viewState);
    syncPresence();
    if (focus) editor.focus();
    empty.hidden = true;
    host.hidden = false;
    renderTabs();
    renderTree();
    renderStatus();
    refreshFromDisk(tab);
  }

  /** Picks up changes made on disk (agents) while the tab was in the background. */
  async function refreshFromDisk(tab) {
    if (tab.dirty) return;
    try {
      const file = await api.get(`/api/sandboxes/${sid}/files/content?${query(tab.path)}`);
      if (file.content != null && file.sha256 !== tab.sha && !tab.dirty && !tab.model) {
        // Suspended meanwhile (another sandbox in front): the kept text takes the disk's version.
        tab.text = file.content;
        savedAs(tab, file.content, null);
        tab.sha = file.sha256;
        return;
      }
      if (file.content != null && file.sha256 !== tab.sha && !tab.dirty) {
        const view = active === tab ? editor.saveViewState() : null;
        tab.ignoreChange = true;
        tab.model.setValue(file.content);
        tab.ignoreChange = false;
        tab.sha = file.sha256;
        savedAs(tab, tab.model.getValue());
        presenceMoved(tab);
        if (view) editor.restoreViewState(view);
        toast(`${tab.path} changed on disk and was reloaded.`, { kind: 'info' });
      }
    } catch { /* deleted or unreachable: the next save reports it */ }
  }

  /**
   * `fromKeyboard`: closed with Delete/Backspace on the strip — focus stays on the tabs instead
   * of moving into the next file, where the next press would delete text (#80).
   */
  async function closeTab(tab, force = false, { fromKeyboard = false } = {}) {
    if (tab.dirty && !force && !(await confirmDialog({ title: `Close ${tab.path.split('/').pop()}?`, message: 'Your unsaved changes are lost.', confirmLabel: 'Close without saving', danger: true }))) return;
    const index = tabs.indexOf(tab);
    if (index < 0) return;
    tabs.splice(index, 1);
    tab.presence.destroy({ going: false });
    if (tab.model) tab.model.dispose();
    if (active === tab) {
      active = null;
      const next = tabs[index] || tabs[index - 1];
      if (next && suspended) active = next; // shown by resume()
      else if (next) activate(next, { focus: !fromKeyboard });
      else { editor.setModel(null); host.hidden = true; empty.hidden = false; }
    }
    renderTabs();
    if (fromKeyboard) (tabbar.querySelector('[role="tab"][tabindex="0"]') || tree.querySelector('[role="treeitem"]'))?.focus();
    renderStatus();
  }

  function closeAll() {
    for (const tab of tabs) tab.presence.destroy({ going: false });
    for (const tab of tabs) if (tab.model) tab.model.dispose();
    tabs.length = 0;
    active = null;
    if (editor) editor.setModel(null);
    host.hidden = true;
    empty.hidden = false;
    renderTabs();
    renderStatus();
  }

  async function save(tab, overwrite = false) {
    if (tab.saving) return;
    tab.saving = true;
    renderStatus();
    try {
      let baseSha = tab.sha;
      if (overwrite) {
        const current = await api.get(`/api/sandboxes/${sid}/files/content?${query(tab.path)}`).catch(() => null);
        baseSha = current ? current.sha256 : null;
      }
      const content = textOf(tab);
      const model = tab.model;
      const version = model ? model.getAlternativeVersionId() : null;
      const written = await api.put(`/api/sandboxes/${sid}/files/content`, { root, path: tab.path, content, base_sha256: baseSha });
      tab.sha = written.sha256;
      // The model may have gone meanwhile (another sandbox in front): then compared in full.
      savedAs(tab, content, model && tab.model === model ? version : null);
      tab.dirty = false;
      presenceMoved(tab);
      toast(`Saved ${tab.path}`, { kind: 'ok' });
    } catch (error) {
      if (error.status === 409) {
        const choice = await openModal({
          title: `${tab.path} changed on disk`,
          body: h('p', { class: 'text-2' }, 'Someone (or an agent) changed this file after you opened it. Overwrite it with your version, or reload the version on disk and lose your changes.'),
          actions: [
            { label: 'Cancel', value: 'cancel' },
            { label: 'Reload from disk', value: 'reload' },
            { label: 'Overwrite', kind: 'primary', value: 'overwrite' },
          ],
        }).result;
        tab.saving = false;
        if (choice === 'overwrite') return save(tab, true);
        if (choice === 'reload') { tab.dirty = false; await refreshFromDisk(tab); }
      } else {
        errorToast(error, `Couldn’t save ${tab.path}`);
      }
    } finally {
      tab.saving = false;
      renderTabs();
      renderStatus();
    }
    return undefined;
  }

  /** Puts the open files' tabs in `order` (paths); answers the strip's tabs as drawn again. */
  function reorderTabs(order) {
    tabs.sort((a, b) => order.indexOf(a.path) - order.indexOf(b.path));
    renderTabs();
    return [...tabbar.querySelectorAll('[role="tab"]')];
  }

  /** Moves `tab` one place left (-1) or right (1) (EDT-011); the keyboard goes with it. */
  function moveTab(tab, step) {
    const order = movedBy(tabs.map((t) => t.path), tab.path, step);
    if (order) reorderTabs(order);
  }

  // Dragging a tab along the strip, as in Notes (tabdrag.js).
  let tabsStale = false; // drawn again once the drag ends
  const tabDrag = createTabDrag(tabbar, { keyOf: (el) => el.dataset.path, commit: reorderTabs, cancelled: () => { if (tabsStale) renderTabs(); } });

  function renderTabs() {
    // A save or a close while a tab is dragged: the strip keeps the dragged tabs until it ends.
    if (tabDrag.dragging) { tabsStale = true; return; }
    tabsStale = false;
    const focused = tabbar.contains(document.activeElement) ? document.activeElement.dataset.path : null;
    const items = tabs.map((tab, index) => {
      const name = tab.path.split('/').pop();
      const button = h('div', { class: ['ed-tab', tab === active && 'active'], role: 'tab', tabindex: tab === active ? '0' : '-1', dataset: { path: tab.path }, title: tab.path, 'aria-selected': String(tab === active), 'aria-keyshortcuts': 'Delete Backspace Alt+ArrowLeft Alt+ArrowRight' },
        h('span', { class: 'ed-tab-name' }, name),
        tab.dirty ? h('span', { class: 'sr-only' }, ', modified') : null,
        // Not a button: see closableTabKeys. Delete or Backspace closes the focused tab.
        h('span', { class: 'ed-tab-close', 'aria-hidden': 'true', title: `Close ${name}`, onClick: (event) => { event.stopPropagation(); closeTab(tab); } }, tab.dirty ? '●' : '×'));
      button.addEventListener('click', () => { if (!tabDrag.clickConsumed()) activate(tab); });
      button.addEventListener('auxclick', (event) => { if (event.button === 1) closeTab(tab); });
      button.addEventListener('keydown', (event) => {
        if (event.altKey && !event.ctrlKey && !event.metaKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
          event.preventDefault();
          moveTab(tab, event.key === 'ArrowLeft' ? -1 : 1);
          return;
        }
        closableTabKeys(event, { select: () => activate(tab), close: () => closeTab(tab, false, { fromKeyboard: true }) });
      });
      // Moving a tab without dragging (WCAG 2.5.7), as the notes and sandbox tabs do.
      button.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        openMenu({ x: event.clientX, y: event.clientY }, [
          { label: 'Move left', icon: 'left', disabled: index === 0, onSelect: () => moveTab(tab, -1) },
          { label: 'Move right', icon: 'right', disabled: index === tabs.length - 1, onSelect: () => moveTab(tab, 1) },
          { separator: true },
          { label: 'Close', onSelect: () => closeTab(tab) },
        ]);
      });
      button.addEventListener('pointerdown', (event) => { if (!event.target.closest('.ed-tab-close')) tabDrag.press(event, button); });
      return button;
    });
    if (items.length && !tabs.includes(active)) items[0].tabIndex = 0;
    setChildren(tabbar, items);
    // Focus stays on the strip across re-renders; after a close it moves to the selected tab (#80).
    if (focused) (items.find((item) => item.dataset.path === focused) || items.find((item) => item.tabIndex === 0))?.focus();
  }

  function renderStatus() {
    if (!active || !editor || !active.model) { setChildren(status, h('span', { class: 'muted' }, root === 'sandbox' ? 'Sandbox repository' : 'Task worktree')); return; }
    const position = editor.getPosition();
    setChildren(status,
      h('span', null, active.path),
      h('span', { class: 'muted' }, active.saving ? 'Saving…' : active.dirty ? 'Modified' : 'Saved'),
      h('span', { class: 'muted' }, position ? `Ln ${position.lineNumber}, Col ${position.column}` : ''),
      h('span', { class: 'muted' }, active.model.getLanguageId()),
      LANGUAGE_SERVICES.has(active.model.getLanguageId()) ? h('span', { class: 'muted', title: 'Type-aware suggestions run in the browser' }, 'IntelliSense') : null);
  }

  renderStatus();

  return {
    el,
    headExtra: () => [folder, wrapButton],
    /** `opts.file`: {root, path, line, col} to open (a terminal's file link). */
    show(opts = {}) {
      shown = true;
      syncPresence();
      if (!started) { started = true; if (!(opts && opts.file && opts.file.root !== root)) loadDir(''); }
      if (editor) editor.layout();
      explorerSplitter.sync();
      if (opts && opts.file) openAt(opts.file);
    },
    hide() {
      shown = false;
      syncPresence();
    },
    taskChanged: folderOptions,
    /** Open files with unsaved changes: the panel host never drops this view for its bound. */
    get unsaved() { return tabs.some((tab) => tab.dirty); },
    // Another sandbox in front (SPEC-113 KST-006): the Monaco models go, so this sandbox's files feed
    // no suggestions there; each open file keeps its text, unsaved mark and cursor for resume().
    suspend() {
      if (suspended) return;
      suspended = true;
      syncPresence();
      if (active && active.model && editor) active.viewState = editor.saveViewState();
      if (editor) editor.setModel(null);
      for (const tab of tabs) {
        if (!tab.model) continue;
        tab.text = tab.model.getValue();
        tab.model.dispose();
        tab.model = null;
      }
    },
    resume() {
      if (!suspended) return;
      suspended = false;
      for (const tab of tabs) {
        if (tab.model) continue;
        attach(tab, tab.text);
        tab.text = null;
      }
      const front = active;
      active = null; // its saved cursor is restored, not replaced by the empty editor's
      if (front) activate(front, { focus: false });
    },
    // Called when the panel host drops this view (another sandbox): free Monaco's editor and
    // models, so other sandboxes' files neither leak nor feed suggestions.
    destroy() {
      tabDrag.cancel();
      for (const tab of tabs) tab.presence.destroy({ going: false });
      clearInterval(sweep);
      offs.forEach((off) => off());
      document.removeEventListener('visibilitychange', onVisibility);
      window.removeEventListener('pagehide', onPageHide);
      if (sizeWatch) sizeWatch.disconnect();
      if (layer) layer.destroy();
      layer = null;
      for (const tab of tabs) if (tab.model) tab.model.dispose();
      tabs.length = 0;
      active = null;
      if (editor) editor.dispose();
      editor = null;
    },
  };
}
