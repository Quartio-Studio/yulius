// Dropped and pasted files (SPEC-001 EDT-006, UI-044; F 2026-10-10). Files or folders dropped on
// the editor's explorer go into the folder under the pointer, files dropped on the editor into
// the open file's folder (text files then open); files dropped on a terminal — or an image
// pasted into it — are uploaded as attachments and their paths typed into it as a paste, the way
// a local terminal hands a dragged file to Claude Code. Uploads stream one file per request
// (docs/api.md), one after another, with progress; an existing file is replaced only after
// asking. Phones have no drag and drop: the explorer's upload button picks files instead.
// Text pastes stay with the terminal (terminal-core.js): only the paste event's files are taken.

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, openModal } from './dom.js';

/** The server's limit when it does not say (`/api/me` → `instance.upload_max_bytes`). */
const DEFAULT_LIMIT = 25 * 1024 * 1024;
/** Files one drop may add to the editor (a folder's files count). */
const MAX_EDITOR_FILES = 1000;
/** Files one drop may hand to a terminal. */
const MAX_TERMINAL_FILES = 20;
/** Text files opened after a drop on the editor. */
const MAX_OPENED = 5;
/** Folders whose listing is read to find existing files before an upload. */
const MAX_LISTED_FOLDERS = 50;
/** Tries of an upload the server refused as one too many at once (429), with backoff. */
const RETRIES = 3;
/** An upload that makes no progress for this long fails. */
const IDLE_MS = 60000;
/** How long a finished upload's summary stays. */
const SUMMARY_MS = 4000;

/** Whether a drag carries files from the computer (not text or a part of the page). */
export function dragHasFiles(event) {
  const types = event.dataTransfer && event.dataTransfer.types;
  return Boolean(types) && [...types].includes('Files');
}

/** `512 KB`, `3.4 MB` (binary units, as the server's limit). */
export function formatSize(bytes) {
  if (bytes >= 1024 * 1024) {
    const mb = bytes / (1024 * 1024);
    return `${mb >= 10 || Number.isInteger(mb) ? Math.round(mb) : mb.toFixed(1)} MB`;
  }
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} bytes`;
}

/** A path as typed into a shell: as it is when it needs no quoting, else in single quotes. */
export function shellQuote(path) {
  return /^[\p{L}\p{N}_./@%+=:,-]+$/u.test(path) ? path : `'${path.replace(/'/g, "'\\''")}'`;
}

function uploadLimit(ctx) {
  const instance = ctx.store && ctx.store.state.me && ctx.store.state.me.instance;
  return (instance && instance.upload_max_bytes) || DEFAULT_LIMIT;
}

/** `3 files`, or `2 of 3 files` when some failed. */
const counted = (done, total) => `${done === total ? '' : `${done} of `}${total} ${total === 1 ? 'file' : 'files'}`;

const parentOf = (path) => (path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '');
const joinPath = (dir, path) => (dir ? `${dir}/${path}` : path);

let guarded = false;
/**
 * A file dropped outside every drop zone would make the browser open it in this tab, leaving the
 * app: such a drop is refused instead (installed once).
 */
function guardWindow() {
  if (guarded) return;
  guarded = true;
  for (const type of ['dragover', 'drop']) {
    window.addEventListener(type, (event) => {
      if (!dragHasFiles(event) || event.defaultPrevented) return;
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = 'none';
    });
  }
}

/** What a drop carries, taken while the drop event lasts (its entries vanish afterwards). */
function dropped(dataTransfer) {
  const items = [...(dataTransfer.items || [])].filter((item) => item.kind === 'file');
  const entries = items.map((item) => (item.webkitGetAsEntry ? item.webkitGetAsEntry() : null));
  return { entries, files: [...(dataTransfer.files || [])] };
}

const entryFile = (entry) => new Promise((resolve, reject) => { entry.file(resolve, reject); });
const readBatch = (reader) => new Promise((resolve, reject) => { reader.readEntries(resolve, reject); });

/**
 * The files of a drop as [{file, path}] (paths relative to the drop). With `folders`, folders are
 * walked (`.git` skipped); without, they are only counted. At most `max` files.
 */
async function collect({ entries, files }, { folders, max }) {
  const out = [];
  const skipped = [];
  let folderCount = 0;
  let truncated = false;
  if (!entries.length || !entries.every(Boolean)) {
    for (const file of files) out.push({ file, path: file.name });
    return { files: out.slice(0, max), truncated: out.length > max, folders: 0, skipped };
  }
  const walk = async (entry, prefix) => {
    if (out.length >= max) { truncated = true; return; }
    if (entry.isFile) {
      out.push({ file: await entryFile(entry), path: prefix + entry.name });
      return;
    }
    if (!entry.isDirectory) return;
    folderCount += 1;
    if (!folders) return;
    if (entry.name === '.git') { skipped.push(prefix + entry.name); return; }
    const reader = entry.createReader();
    for (;;) {
      const batch = await readBatch(reader);
      if (!batch.length) return;
      for (const child of batch) await walk(child, `${prefix}${entry.name}/`);
      if (out.length >= max) { truncated = true; return; }
    }
  };
  for (const entry of entries) await walk(entry, '');
  return { files: out, truncated, folders: folderCount, skipped };
}

/** Sends one file; an upload refused as one too many at once (429) is tried again. */
async function sendWithRetry(send, file, onProgress) {
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await send(file, { onProgress, timeoutMs: IDLE_MS });
    } catch (error) {
      if (!error || error.code !== 'capacity' || attempt >= RETRIES) throw error;
      await new Promise((resolve) => { setTimeout(resolve, 1000 * 2 ** attempt); });
    }
  }
}

/** The server's sentence for a failed upload (a proxy's 413 has none). */
function failure(error) {
  if (error && error.status === 413) return 'larger than the server accepts';
  return (error && error.message) || 'the upload failed';
}

/**
 * The progress of a batch of uploads and its errors: inside the explorer, or floating over a
 * terminal. Errors stay until dismissed; a clean batch says what it did and goes.
 */
function createStatus({ floating, label }) {
  const text = h('span', { class: 'upload-text' });
  const fill = h('div', { class: 'upload-bar-fill' });
  const bar = h('div', { class: 'upload-bar', role: 'progressbar', 'aria-label': label, 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': 0 }, fill);
  const errors = h('ul', { class: 'upload-errors' });
  const close = iconButton('x', 'Dismiss', () => hide(), { class: 'upload-close' });
  const el = h('div', { class: ['upload-status', floating && 'upload-status-float'], role: 'status', 'aria-live': 'polite', hidden: true },
    h('div', { class: 'upload-head' }, icon('upload', { size: 14 }), text, close), bar, errors);
  let timer = 0;
  function hide() {
    clearTimeout(timer);
    el.hidden = true;
    setChildren(errors, null);
  }
  return {
    el,
    /** A batch starts: forget the last one's errors. */
    begin(message) {
      clearTimeout(timer);
      setChildren(errors, null);
      el.hidden = false;
      el.classList.remove('upload-status-error');
      bar.hidden = false;
      close.hidden = true;
      text.textContent = message;
      this.progress(0);
    },
    progress(percent, message) {
      const value = Math.max(0, Math.min(100, Math.round(percent)));
      fill.style.width = `${value}%`;
      bar.setAttribute('aria-valuenow', String(value));
      if (message) text.textContent = message;
    },
    error(message) {
      el.hidden = false;
      el.classList.add('upload-status-error');
      errors.append(h('li', { class: 'upload-error' }, icon('warn', { size: 12 }), h('span', null, message)));
    },
    /** The batch ended: its summary; errors keep the status open until dismissed. */
    end(message) {
      bar.hidden = true;
      el.hidden = false;
      text.textContent = message;
      close.hidden = false;
      clearTimeout(timer);
      if (!errors.children.length) timer = setTimeout(hide, SUMMARY_MS);
    },
    hide,
  };
}

/**
 * Uploads `jobs` ([{file, path, send}]) one after another into `status`; `title(index, job)` names
 * the current one. Returns [{job, result}] for the files stored.
 */
async function uploadAll(jobs, status, title) {
  const total = jobs.reduce((sum, job) => sum + job.file.size, 0) || 1;
  let before = 0;
  const stored = [];
  for (const [index, job] of jobs.entries()) {
    const heading = title(index, job);
    status.progress((before / total) * 100, heading);
    try {
      const result = await sendWithRetry(job.send, job.file, (sent) => {
        status.progress(((before + Math.min(sent, job.file.size)) / total) * 100, `${heading} · ${Math.round((Math.min(sent, job.file.size) / (job.file.size || 1)) * 100)}%`);
      });
      stored.push({ job, result });
    } catch (error) {
      status.error(`${job.path}: ${failure(error)}`);
    }
    before += job.file.size;
  }
  status.progress(100);
  return stored;
}

/** One file input for the explorer's upload button (phones have no drag and drop). */
function filePicker(onFiles) {
  const input = h('input', { type: 'file', multiple: true, hidden: true, class: 'upload-input', tabindex: '-1', 'aria-hidden': 'true' });
  input.addEventListener('change', () => {
    const files = [...input.files];
    input.value = '';
    if (files.length) onFiles(files);
  });
  return input;
}

/**
 * Makes `el` a drop zone for files: an overlay over `overlayHost()` says what a drop does
 * (`label(event)`) or why it cannot (`refused()` returns a sentence while dropping is not
 * allowed); `onDrop(event, carried)` gets the drop's files. Captured before the zone's own
 * children (Monaco, xterm) so they never see a file drag.
 */
function dropZone(el, { overlayHost, label, refused = () => null, onOver, onLeave, onDrop }) {
  guardWindow();
  const labelEl = h('span', { class: 'drop-overlay-label' });
  const overlay = h('div', { class: 'drop-overlay', 'aria-hidden': 'true' }, labelEl);
  let depth = 0;
  const show = (event) => {
    const reason = refused();
    const host = overlayHost();
    if (overlay.parentElement !== host) host.append(overlay);
    overlay.classList.toggle('drop-overlay-refused', Boolean(reason));
    setChildren(labelEl, icon(reason ? 'lock' : 'upload', { size: 16 }), h('span', null, reason || label(event)));
    if (event.dataTransfer) event.dataTransfer.dropEffect = reason ? 'none' : 'copy';
  };
  const hide = () => {
    depth = 0;
    overlay.remove();
    if (onLeave) onLeave();
  };
  el.addEventListener('dragenter', (event) => {
    if (!dragHasFiles(event)) return;
    event.preventDefault();
    depth += 1;
    show(event);
  }, true);
  el.addEventListener('dragover', (event) => {
    if (!dragHasFiles(event)) return;
    event.preventDefault();
    event.stopPropagation();
    show(event);
    if (onOver && !refused()) onOver(event);
  }, true);
  el.addEventListener('dragleave', (event) => {
    if (!dragHasFiles(event)) return;
    depth -= 1;
    if (depth <= 0 || (event.relatedTarget && !el.contains(event.relatedTarget))) hide();
  }, true);
  el.addEventListener('drop', (event) => {
    if (!dragHasFiles(event)) return;
    event.preventDefault();
    event.stopPropagation();
    const blocked = refused();
    const carried = blocked ? null : dropped(event.dataTransfer);
    hide();
    if (carried) onDrop(event, carried);
  }, true);
  return { hide };
}

/** A media type or name that the editor can open as text. */
function looksLikeText(file) {
  const type = file.type || '';
  if (/^text\//.test(type) || /json|javascript|typescript|xml|yaml|toml|x-sh|markdown|csv|sql/.test(type)) return true;
  if (type) return false;
  return !/\.(png|jpe?g|gif|webp|bmp|ico|pdf|zip|gz|tgz|xz|7z|rar|tar|mp3|mp4|mov|wav|woff2?|ttf|otf|exe|dll|so|dylib|wasm|bin)$/i.test(file.name);
}

/**
 * Drop files onto the built-in editor (EDT-006). Options:
 *   ctx, sandboxId     the app context (api, store) and the sandbox
 *   tree, host, head   the explorer's tree, Monaco's host and the explorer's header
 *   getRoot()          `sandbox` or `task:<id>` (the folder the editor shows)
 *   activePath()       the open file's path, or null
 *   reveal(dir)        lists `dir` (expanded) again; refresh() lists every expanded folder again
 *   open(path)         opens a file in a tab
 */
export function attachEditorDrop({ ctx, sandboxId, tree, host, head, getRoot, activePath, reveal, refresh, open }) {
  const api = ctx.api;
  const side = tree.closest('.ed-side') || tree.parentElement;
  const stage = host.closest('.ed-stage') || host.parentElement;
  const status = createStatus({ floating: false, label: 'Upload progress' });
  side.append(status.el);
  let queue = Promise.resolve();
  let marked = null;

  const where = (dir) => (dir ? `${dir}/` : getRoot() === 'sandbox' ? 'the repository' : 'the task worktree');
  const selectedFolder = () => {
    const row = tree.querySelector('.ed-row.active');
    const path = row && (row.dataset.path || row.title);
    return path ? parentOf(path) : '';
  };
  /** The folder under the pointer: a folder row, a file row's folder, else the selected row's. */
  const explorerTarget = (event) => {
    const row = event.target instanceof Element ? event.target.closest('.ed-row') : null;
    const path = row && (row.dataset.path || row.title);
    if (!path) return { dir: selectedFolder(), row: null };
    const isDir = row.dataset.kind ? row.dataset.kind === 'dir' : row.hasAttribute('aria-expanded');
    return isDir ? { dir: path, row } : { dir: parentOf(path), row: null };
  };
  const mark = (row) => {
    if (marked === row) return;
    if (marked) marked.classList.remove('ed-drop-target');
    marked = row;
    if (row) row.classList.add('ed-drop-target');
  };

  /** Which of `paths` exist, read from their folders' listings (`kind` per path). */
  async function existing(paths) {
    const root = getRoot();
    const found = new Map();
    const folders = [...new Set(paths.map(parentOf))].slice(0, MAX_LISTED_FOLDERS);
    for (const dir of folders) {
      try {
        const listing = await api.get(`/api/sandboxes/${enc(sandboxId)}/files?root=${encodeURIComponent(root)}&path=${encodeURIComponent(dir)}`);
        for (const entry of listing.entries || []) found.set(entry.path, entry.kind);
      } catch { /* a folder that does not exist yet holds nothing */ }
    }
    return found;
  }

  async function askReplace(names) {
    const shown = names.slice(0, 8);
    const body = h('div', null,
      h('p', { class: 'text-2' }, names.length === 1 ? `${names[0]} already exists. Replace it with the file you dropped?` : `${names.length} files already exist. Replace them with the files you dropped?`),
      names.length > 1 ? h('ul', { class: 'upload-conflicts' }, shown.map((name) => h('li', null, name)), names.length > shown.length ? h('li', { class: 'muted' }, `and ${names.length - shown.length} more`) : null) : null);
    return openModal({
      title: names.length === 1 ? `Replace ${names[0].split('/').pop()}?` : `Replace ${names.length} files?`,
      body,
      actions: [
        { label: 'Cancel', value: 'cancel' },
        { label: 'Keep existing', value: 'skip' },
        { label: 'Replace', kind: 'primary', value: 'replace' },
      ],
    }).result;
  }

  async function run(carried, dir, { openText }) {
    const root = getRoot();
    const { files, truncated, skipped } = await collect(carried, { folders: true, max: MAX_EDITOR_FILES });
    status.begin(`Adding to ${where(dir)}…`);
    if (truncated) status.error(`Only the first ${MAX_EDITOR_FILES} files of the drop are added.`);
    for (const path of skipped) status.error(`${path}: .git folders are not added.`);
    const limit = uploadLimit(ctx);
    let jobs = [];
    for (const item of files) {
      const path = joinPath(dir, item.path);
      if (item.file.size > limit) status.error(`${path}: ${formatSize(item.file.size)} is larger than the upload limit of ${formatSize(limit)}.`);
      else jobs.push({ file: item.file, path, overwrite: false });
    }
    if (!jobs.length) { status.end(files.length ? 'Nothing was added.' : 'The drop held no files.'); return; }
    const found = await existing(jobs.map((job) => job.path));
    const folders = jobs.filter((job) => found.get(job.path) && found.get(job.path) !== 'file');
    for (const job of folders) status.error(`${job.path}: a folder or link with that name exists.`);
    jobs = jobs.filter((job) => !folders.includes(job));
    const clashes = jobs.filter((job) => found.get(job.path) === 'file');
    if (clashes.length) {
      const choice = await askReplace(clashes.map((job) => job.path));
      if (choice !== 'replace' && choice !== 'skip') { status.end('Nothing was added.'); return; }
      if (choice === 'skip') jobs = jobs.filter((job) => !clashes.includes(job));
      else for (const job of clashes) job.overwrite = true;
    }
    for (const job of jobs) {
      job.send = (file, options) => api.put(`/api/sandboxes/${enc(sandboxId)}/files/upload?root=${encodeURIComponent(root)}&path=${encodeURIComponent(job.path)}&overwrite=${job.overwrite}`, file, options);
    }
    const stored = await uploadAll(jobs, status, (index, job) => `Uploading ${index + 1} of ${jobs.length}: ${job.path.split('/').pop()}`);
    if (getRoot() !== root) { status.end(`Added ${counted(stored.length, jobs.length)} to the folder shown before.`); return; }
    await reveal(dir);
    await refresh();
    status.end(stored.length === 1 && jobs.length === 1 ? `Added ${stored[0].result.path}` : `Added ${counted(stored.length, jobs.length)} to ${where(dir)}`);
    if (openText) {
      for (const { job, result } of stored.filter(({ job: j }) => looksLikeText(j.file) && j.file.size <= 2 * 1024 * 1024).slice(0, MAX_OPENED)) {
        if (job.path === result.path) await open(result.path);
      }
    }
  }

  const enqueue = (carried, dir, options) => {
    queue = queue.then(() => run(carried, dir, options)).catch((error) => {
      status.error(failure(error));
      status.end('The upload stopped.');
    });
  };

  dropZone(side, {
    overlayHost: () => side,
    label: (event) => `Drop to add to ${where(explorerTarget(event).dir)}`,
    onOver: (event) => mark(explorerTarget(event).row),
    onLeave: () => mark(null),
    onDrop: (event, carried) => enqueue(carried, explorerTarget(event).dir, { openText: false }),
  });
  dropZone(stage, {
    overlayHost: () => stage,
    label: () => `Drop to add to ${where(parentOf(activePath() || ''))}`,
    onDrop: (_event, carried) => enqueue(carried, parentOf(activePath() || ''), { openText: true }),
  });

  const input = filePicker((files) => enqueue({ entries: [], files }, selectedFolder(), { openText: false }));
  const button = iconButton('upload', 'Upload files', () => input.click());
  const refreshButton = head.querySelector('.btn-icon:last-of-type');
  head.insertBefore(button, refreshButton && refreshButton.parentElement === head ? refreshButton : null);
  head.append(input);
  return { button, input };
}

/** Uploads a terminal attachment of a task (whoever may type into its terminal). */
export function taskAttachments(ctx, taskId) {
  return (file, options) => ctx.api.post(`/api/tasks/${enc(taskId)}/attachments?name=${encodeURIComponent(file.name)}`, file, options);
}

/** Uploads an attachment of one of the user's shells. */
export function shellAttachments(ctx, sandboxId, shellId) {
  return (file, options) => ctx.api.post(`/api/sandboxes/${enc(sandboxId)}/shells/${enc(shellId)}/attachments?name=${encodeURIComponent(file.name)}`, file, options);
}

/** `pasted-20261010-153012.png` for a clipboard image (always named `image.png`). */
function pastedName(file) {
  const now = new Date();
  const pad = (n) => String(n).padStart(2, '0');
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  const ext = (file.type.split('/')[1] || 'png').replace(/[^a-z0-9]/gi, '').slice(0, 8) || 'png';
  return `pasted-${stamp}.${ext}`;
}

/** The files of a paste that carries no text of its own (an image, a copied file). */
function pastedFiles(data) {
  if (!data) return [];
  const files = [...(data.items || [])].filter((item) => item.kind === 'file').map((item) => item.getAsFile()).filter(Boolean);
  if (!files.length) return [];
  const text = (data.getData('text/plain') || '').trim();
  if (text && !files.some((file) => file.name === text)) return [];
  return files.map((file) => (/^image\.\w+$/.test(file.name) || !file.name ? new File([file], pastedName(file), { type: file.type }) : file));
}

/**
 * Drop files onto a terminal, or paste an image into it (UI-044): each file is uploaded with
 * `upload(file, options)` (taskAttachments, shellAttachments) and the stored paths are pasted into
 * the terminal's input (bracketed paste when the program asked for it), space-separated, quoted
 * as needed. Folders are refused; read-only viewers cannot drop. `view` is a terminal view
 * (terminal.js), `label` the overlay's sentence ("Drop to send to Claude").
 */
export function attachTerminalDrop(ctx, view, { label, upload }) {
  const zone = view.el;
  const overlayHost = () => zone.querySelector('.term') || zone;
  const status = createStatus({ floating: true, label: 'Upload progress' });
  let queue = Promise.resolve();
  const place = () => {
    const host = overlayHost();
    if (status.el.parentElement !== host) host.append(status.el);
  };

  function insert(paths) {
    if (!paths.length) return;
    const text = `${paths.map(shellQuote).join(' ')} `;
    if (view.term && typeof view.term.paste === 'function') view.term.paste(text);
    else view.sendKeys(text);
    view.focus();
  }

  async function run(carried) {
    place();
    const { files, truncated, folders } = await collect(carried, { folders: false, max: MAX_TERMINAL_FILES });
    status.begin('Preparing…');
    if (folders) status.error(folders === 1 ? 'Folders cannot be sent; drop the files inside it.' : `${folders} folders were left out; drop the files inside them.`);
    if (truncated) status.error(`Only the first ${MAX_TERMINAL_FILES} files are sent.`);
    const limit = uploadLimit(ctx);
    const jobs = [];
    for (const item of files) {
      if (item.file.size > limit) status.error(`${item.path}: ${formatSize(item.file.size)} is larger than the upload limit of ${formatSize(limit)}.`);
      else jobs.push({ file: item.file, path: item.path, send: upload });
    }
    if (!jobs.length) { status.end(files.length || folders ? 'Nothing was sent.' : 'The drop held no files.'); return; }
    const stored = await uploadAll(jobs, status, (index, job) => `Uploading ${jobs.length > 1 ? `${index + 1} of ${jobs.length}: ` : ''}${job.path}`);
    if (!view.canWrite) {
      if (stored.length) status.error('The terminal became read-only; the paths were not typed.');
    } else {
      insert(stored.map(({ result }) => result.path));
    }
    status.end(stored.length === 1 && jobs.length === 1 ? `Added ${stored[0].result.name}` : `Added ${counted(stored.length, jobs.length)}`);
  }

  const enqueue = (carried) => {
    queue = queue.then(() => run(carried)).catch((error) => {
      status.error(failure(error));
      status.end('The upload stopped.');
    });
  };
  const readOnly = () => (view.canWrite ? null : 'Read-only: files cannot be added here');

  dropZone(zone, {
    overlayHost,
    label: () => label,
    refused: readOnly,
    onDrop: (_event, carried) => enqueue(carried),
  });
  // Images and copied files pasted into the terminal; text pastes are left to the terminal.
  zone.addEventListener('paste', (event) => {
    const files = pastedFiles(event.clipboardData);
    if (!files.length) return;
    event.preventDefault();
    event.stopPropagation();
    if (readOnly()) {
      place();
      status.begin('');
      status.error(readOnly());
      status.end('Nothing was sent.');
      return;
    }
    enqueue({ entries: [], files });
  }, true);
  return { status };
}
