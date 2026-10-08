// Built-in code editor (SPEC-001 EDT-001..004): a small VS Code-styled editor for quick changes
// and reviews — explorer, tabs and Monaco (VS Code's editor component) with syntax colors and
// suggestions (language services for JavaScript/TypeScript, JSON, CSS and HTML; keywords and
// words of the open files for other popular languages). Each user's view (open files, unsaved
// text) lives only in this browser tab: nothing is stored on the server or shared. Saving
// writes the file into the sandbox; a file changed on disk since it was opened (agents edit
// the same files) is not overwritten silently.

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, toast, errorToast, confirmDialog, openMenu, openModal, field } from './dom.js';
import { allows } from './status.js';
import { token, activeTheme, onThemeChange, px } from './theme.js';

const MONACO_BASE = '/assets/vendor/monaco/vs';
const LANGUAGE_SERVICES = new Set(['javascript', 'typescript', 'json', 'css', 'scss', 'less', 'html']);

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
    script.onerror = () => reject(new Error('The editor could not be loaded.'));
    document.head.append(script);
  }).catch((error) => { monacoPromise = null; throw error; });
  return monacoPromise;
}

function defineEditorTheme(monaco) {
  const well = token('--term-bg');
  monaco.editor.defineTheme('yulius', {
    base: activeTheme() === 'light' ? 'vs' : 'vs-dark',
    inherit: true,
    rules: [],
    colors: { 'editor.background': well, 'editorGutter.background': well, 'minimap.background': well },
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
  let started = false;
  const tabs = []; // {path, model, sha, dirty, viewState, external}
  let active = null;
  const expanded = new Map(); // dir path → entries | null (loading)

  const folder = h('select', { class: 'select select-sm', 'aria-label': 'Folder' });
  function folderOptions() {
    const task = getTask();
    const current = folder.value;
    setChildren(folder,
      h('option', { value: 'sandbox' }, 'Sandbox'),
      task ? h('option', { value: `task:${task.id}` }, `Task #${task.number} worktree`) : null);
    folder.value = [...folder.options].some((o) => o.value === current) ? current : 'sandbox';
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
  const tabbar = h('div', { class: 'ed-tabs', role: 'tablist' });
  const host = h('div', { class: 'ed-host' });
  const empty = h('div', { class: 'ed-empty' }, h('p', null, 'Open a file from the explorer.'), h('p', { class: 'muted xs' }, '⌘S / Ctrl+S saves. Your open files and unsaved changes stay in this browser tab only.'));
  const status = h('div', { class: 'ed-status' });
  const treeHead = h('div', { class: 'ed-tree-head' },
    h('span', null, 'Explorer'),
    iconButton('plus', 'New file or folder', (event) => newEntryMenu(event.currentTarget, '')),
    iconButton('refresh', 'Refresh', () => refreshTree()));
  setChildren(el, h('div', { class: 'ed-root' },
    h('div', { class: 'ed-side' }, treeHead, tree),
    h('div', { class: 'ed-main' }, tabbar, h('div', { class: 'ed-stage' }, host, empty), status)));

  const query = (path) => `root=${encodeURIComponent(root)}&path=${encodeURIComponent(path)}`;

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
      if (path === '') setChildren(tree, h('div', { class: 'panel-msg' }, error.status === 409 ? 'The editor opens once the sandbox repository has been cloned.' : (error.message || 'Could not list files.')));
      else errorToast(error, 'Could not list the folder');
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
    const walk = (dir, depth) => {
      const entries = expanded.get(dir);
      if (entries === null) { rows.push(h('div', { class: 'ed-row muted', style: { paddingLeft: `${8 + depth * 12}px` } }, 'Loading…')); return; }
      if (!entries) return;
      for (const entry of entries) {
        const isDir = entry.kind === 'dir';
        const open = isDir && expanded.has(entry.path);
        const row = h('div', {
          class: ['ed-row', active && active.path === entry.path && 'active', entry.kind === 'symlink' && 'muted'],
          role: 'treeitem',
          'aria-expanded': isDir ? String(open) : null,
          title: entry.path,
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
      errorToast(error, `Could not create ${path}`);
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
      errorToast(error, `Could not delete ${entry.path}`);
      return;
    }
    for (const tab of [...tabs]) if (tab.path === entry.path || tab.path.startsWith(`${entry.path}/`)) closeTab(tab, true);
    const parent = entry.path.includes('/') ? entry.path.slice(0, entry.path.lastIndexOf('/')) : '';
    expanded.delete(entry.path);
    loadDir(parent);
  }

  // ---------------------------------------------------------------- files and tabs
  async function ensureEditor() {
    if (editor) return true;
    try {
      monaco = await loadMonaco();
    } catch (error) {
      errorToast(error, 'The editor could not be loaded');
      return false;
    }
    editor = monaco.editor.create(host, {
      theme: 'yulius',
      automaticLayout: true,
      fontFamily: token('--font-mono'),
      fontSize: px('--fs'),
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      renderWhitespace: 'selection',
      tabSize: 2,
      wordBasedSuggestions: 'allDocuments',
      quickSuggestions: { other: true, comments: false, strings: false },
    });
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => { if (active) save(active); });
    editor.onDidChangeCursorPosition(() => renderStatus());
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
      errorToast(error, `Could not open ${path}`);
      return;
    }
    if (file.binary || file.too_large || file.content == null) {
      toast(file.too_large ? `${path} is too large for the editor (${Math.round(file.size / 1024)} KB).` : `${path} is a binary file.`, { kind: 'info' });
      return;
    }
    const model = monaco.editor.createModel(file.content, languageFor(monaco, path));
    const tab = { path, model, sha: file.sha256, dirty: false, viewState: null, saving: false };
    model.onDidChangeContent(() => {
      if (tab.ignoreChange) return;
      if (!tab.dirty) { tab.dirty = true; renderTabs(); renderStatus(); }
    });
    tabs.push(tab);
    activate(tab);
  }

  function activate(tab) {
    if (active && editor) active.viewState = editor.saveViewState();
    active = tab;
    editor.setModel(tab.model);
    if (tab.viewState) editor.restoreViewState(tab.viewState);
    editor.focus();
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
      if (file.content != null && file.sha256 !== tab.sha && !tab.dirty) {
        const view = active === tab ? editor.saveViewState() : null;
        tab.ignoreChange = true;
        tab.model.setValue(file.content);
        tab.ignoreChange = false;
        tab.sha = file.sha256;
        if (view) editor.restoreViewState(view);
        toast(`${tab.path} changed on disk and was reloaded.`, { kind: 'info' });
      }
    } catch { /* deleted or unreachable: the next save reports it */ }
  }

  async function closeTab(tab, force = false) {
    if (tab.dirty && !force && !(await confirmDialog({ title: `Close ${tab.path.split('/').pop()}?`, message: 'Your unsaved changes are lost.', confirmLabel: 'Close without saving', danger: true }))) return;
    const index = tabs.indexOf(tab);
    if (index < 0) return;
    tabs.splice(index, 1);
    tab.model.dispose();
    if (active === tab) {
      active = null;
      const next = tabs[index] || tabs[index - 1];
      if (next) activate(next);
      else { editor.setModel(null); host.hidden = true; empty.hidden = false; }
    }
    renderTabs();
    renderStatus();
  }

  function closeAll() {
    for (const tab of tabs) tab.model.dispose();
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
      const written = await api.put(`/api/sandboxes/${sid}/files/content`, { root, path: tab.path, content: tab.model.getValue(), base_sha256: baseSha });
      tab.sha = written.sha256;
      tab.dirty = false;
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
        errorToast(error, `Could not save ${tab.path}`);
      }
    } finally {
      tab.saving = false;
      renderTabs();
      renderStatus();
    }
    return undefined;
  }

  function renderTabs() {
    setChildren(tabbar, tabs.map((tab) => {
      const name = tab.path.split('/').pop();
      const button = h('div', { class: ['ed-tab', tab === active && 'active'], role: 'tab', title: tab.path, 'aria-selected': String(tab === active) },
        h('span', { class: 'ed-tab-name' }, name),
        h('button', { type: 'button', class: 'ed-tab-close', 'aria-label': `Close ${name}`, onClick: (event) => { event.stopPropagation(); closeTab(tab); } }, tab.dirty ? '●' : '×'));
      button.addEventListener('click', () => activate(tab));
      button.addEventListener('auxclick', (event) => { if (event.button === 1) closeTab(tab); });
      return button;
    }));
  }

  function renderStatus() {
    if (!active || !editor) { setChildren(status, h('span', { class: 'muted' }, root === 'sandbox' ? 'Sandbox repository' : 'Task worktree')); return; }
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
    headExtra: () => [folder],
    show() {
      if (!started) { started = true; loadDir(''); }
      if (editor) editor.layout();
    },
    taskChanged: folderOptions,
    // Called when the panel host drops this view (another sandbox): free Monaco's editor and
    // models, so other sandboxes' files neither leak nor feed suggestions.
    destroy() {
      for (const tab of tabs) tab.model.dispose();
      tabs.length = 0;
      active = null;
      if (editor) editor.dispose();
      editor = null;
    },
  };
}
