// Playgrounds (SPEC-099 PLY-009): a team tries Yulius on its own code before it connects GitHub.
// A project folder (or a repository) dropped or chosen here becomes a playground: the browser
// packs it into a gzip tar itself (`files/…` the working tree, `git/…` the history part of its
// `.git`), sends it to `POST /api/playgrounds`, and the server keeps its Git on the server.
//
// Packing reads nothing into memory: the archive is a Blob of headers and the chosen files, and
// `CompressionStream` gzips it as it is read. Dependency and build folders stay behind, and of
// `.git` only HEAD, packed-refs, shallow, objects/ and refs/ go (the server takes nothing else).

import { h, icon, setChildren } from './dom.js';
import { bytes } from './format.js';

/** Folders that are rebuilt, never part of a project's own code. */
export const SKIPPED_DIRS = new Set(['node_modules', '.venv', 'venv', '__pycache__', 'target', '.next', '.nuxt', '.turbo', '.cache']);
/** The server refuses a larger archive (`MAX_FOLDER_UPLOAD_BYTES`). */
export const MAX_ARCHIVE_BYTES = 1024 ** 3;
/** The server's bound on what the files of a folder hold in all (`MAX_IMPORT_TOTAL_BYTES` for
 * the working tree and again for the history), and so on any one file. */
export const MAX_FOLDER_BYTES = 4 * 1024 ** 3;
export const MAX_FILE_BYTES = 2 * 1024 ** 3;
/** The server's bound on archive entries (`MAX_SNAPSHOT_ENTRIES`). */
export const MAX_FILES = 200000;
/** Waiting for the server: from the last byte sent, cloning and committing take a while. */
const SETUP_TIMEOUT_MS = 15 * 60 * 1000;

const HISTORY_FILES = new Set(['HEAD', 'packed-refs', 'shallow']);
const HISTORY_DIRS = new Set(['objects', 'refs']);
const encoder = new TextEncoder();

/**
 * Where a file of the folder (`rel`, slash-separated, relative to the folder) goes in the archive:
 * `files/<rel>`, `git/<rest>` for the history part of the folder's own `.git`, or null when it
 * stays behind (a dependency or build folder, a nested `.git`, `.DS_Store`, the rest of `.git`).
 */
export function archivePath(rel) {
  const parts = String(rel).split('/').filter((part) => part && part !== '.');
  if (!parts.length || parts.includes('..')) return null;
  if (parts[0] === '.git') {
    const rest = parts.slice(1);
    if (rest.length === 1 && HISTORY_FILES.has(rest[0])) return `git/${rest[0]}`;
    // objects/info/ holds alternates (another repository's objects): the server refuses them.
    if (rest.length > 1 && HISTORY_DIRS.has(rest[0]) && !(rest[0] === 'objects' && rest[1] === 'info')) return `git/${rest.join('/')}`;
    return null;
  }
  const dirs = parts.slice(0, -1);
  if (dirs.some((dir) => dir === '.git' || SKIPPED_DIRS.has(dir))) return null;
  if (parts[parts.length - 1] === '.DS_Store' || parts[parts.length - 1] === '.git') return null;
  return `files/${parts.join('/')}`;
}

/** Whether a folder met while walking a dropped directory is worth entering (`rel` its path). */
function walkInto(rel) {
  const parts = rel.split('/');
  const name = parts[parts.length - 1];
  if (parts[0] === '.git') return parts.length === 1 || (HISTORY_DIRS.has(parts[1]) && !(parts[1] === 'objects' && parts[2] === 'info'));
  return name !== '.git' && !SKIPPED_DIRS.has(name);
}

function putString(block, offset, text, length) {
  block.set(encoder.encode(text).subarray(0, length), offset);
}

function putOctal(block, offset, value, length) {
  putString(block, offset, value.toString(8).padStart(length - 1, '0'), length - 1);
}

/** One 512-byte ustar header. */
function header(name, size, type) {
  const block = new Uint8Array(512);
  putString(block, 0, name, 100);
  putOctal(block, 100, 0o644, 8);
  putOctal(block, 108, 0, 8);
  putOctal(block, 116, 0, 8);
  putOctal(block, 124, size, 12);
  putOctal(block, 136, Math.floor(Date.now() / 1000), 12);
  block.fill(0x20, 148, 156);
  putString(block, 156, type, 1);
  putString(block, 257, 'ustar\u000000', 8);
  let sum = 0;
  for (const byte of block) sum += byte;
  putString(block, 148, `${sum.toString(8).padStart(6, '0')}\u0000 `, 8);
  return block;
}

const padding = (size) => new Uint8Array((512 - (size % 512)) % 512);

/** A PAX record `"<len> path=<path>\n"`, whose length counts its own digits. */
function paxRecord(path) {
  const body = encoder.encode(` path=${path}\n`).length;
  let length = body + 1;
  while (String(length).length + body !== length) length = String(length).length + body;
  return encoder.encode(`${length} path=${path}\n`);
}

/**
 * The tar archive (a Blob, nothing read yet) of `entries` (`{path, file}`, `path` relative to the
 * folder), with what stays behind left out; names longer than ustar's 100 bytes get a PAX header.
 * Returns `{blob, files, size}`.
 */
export function tarOf(entries) {
  const parts = [];
  let files = 0;
  let size = 0;
  for (const { path, file } of entries) {
    const name = archivePath(path);
    if (!name) continue;
    if (encoder.encode(name).length > 100) {
      const record = paxRecord(name);
      parts.push(header('PaxHeader', record.length, 'x'), record, padding(record.length));
    }
    parts.push(header(name, file.size, '0'), file, padding(file.size));
    files += 1;
    size += file.size;
  }
  parts.push(new Uint8Array(1024));
  return { blob: new Blob(parts), files, size };
}

/** Gzips `blob` as it is read, reporting the bytes read so far. */
async function gzip(blob, onProgress) {
  let read = 0;
  const count = new TransformStream({
    transform(chunk, controller) {
      read += chunk.byteLength;
      onProgress(read, blob.size);
      controller.enqueue(chunk);
    },
  });
  return new Response(blob.stream().pipeThrough(count).pipeThrough(new CompressionStream('gzip'))).blob();
}

/** `{name, entries}` of the folder chosen with `<input webkitdirectory>`. */
export function entriesOfInput(fileList) {
  const list = Array.from(fileList || []);
  const first = list.find((file) => file.webkitRelativePath);
  const name = first ? first.webkitRelativePath.split('/')[0] : '';
  const entries = list.map((file) => ({
    path: file.webkitRelativePath ? file.webkitRelativePath.split('/').slice(1).join('/') : file.name,
    file,
  }));
  return { name, entries };
}

const readBatch = (reader) => new Promise((resolve, reject) => reader.readEntries(resolve, reject));
const fileOf = (entry) => new Promise((resolve, reject) => entry.file(resolve, reject));

async function walk(directory, prefix, out) {
  const reader = directory.createReader();
  for (;;) {
    const batch = await readBatch(reader);
    if (!batch.length) return;
    for (const entry of batch) {
      const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory) {
        if (walkInto(rel)) await walk(entry, rel, out);
      } else if (archivePath(rel)) {
        if (out.length >= MAX_FILES) throw new Error(`The folder has more than ${MAX_FILES.toLocaleString()} files.`);
        out.push({ path: rel, file: await fileOf(entry) });
      }
    }
  }
}

/**
 * `{name, entries}` of what was dropped: one folder (walked, skipping what stays behind), or loose
 * files as the project's top level.
 */
export async function entriesOfDrop(dataTransfer) {
  const items = Array.from(dataTransfer.items || []);
  const roots = items.map((item) => (item.webkitGetAsEntry ? item.webkitGetAsEntry() : null)).filter(Boolean);
  if (roots.length === 1 && roots[0].isDirectory) {
    const entries = [];
    await walk(roots[0], '', entries);
    return { name: roots[0].name, entries };
  }
  const entries = [];
  for (const root of roots) {
    if (root.isDirectory) await walk(root, root.name, entries);
    else entries.push({ path: root.name, file: await fileOf(root) });
  }
  if (!roots.length) for (const file of Array.from(dataTransfer.files || [])) entries.push({ path: file.name, file });
  return { name: '', entries };
}

/**
 * Packs `entries`, uploads them as the playground `name` and returns the new sandbox.
 * `report(stage, done, total)` follows it: `packing` (bytes read), `uploading` (bytes sent),
 * `setting-up`.
 */
export async function startFolderPlayground(api, { name, entries }, report = () => {}) {
  const tar = tarOf(entries);
  if (!tar.files) throw new Error('The folder has no files to start a playground with.');
  // Refused here, before minutes of packing and uploading end in the server's refusal.
  const huge = entries.find(({ path, file }) => archivePath(path) && file.size > MAX_FILE_BYTES);
  if (huge) throw new Error(`${huge.path} is ${bytes(huge.file.size)}; a playground takes files up to 2 GB.`);
  if (tar.size > MAX_FOLDER_BYTES) throw new Error(`The folder holds ${bytes(tar.size)}; a playground takes up to 4 GB. Leave dependencies and build output out.`);
  report('packing', 0, tar.blob.size);
  const archive = await gzip(tar.blob, (done, total) => report('packing', done, total));
  if (archive.size > MAX_ARCHIVE_BYTES) {
    throw new Error(`The folder is ${bytes(archive.size)} once packed; a playground takes up to 1 GB. Leave dependencies and build output out.`);
  }
  report('uploading', 0, archive.size);
  return api.post(`/api/playgrounds?name=${encodeURIComponent(name || 'project')}`, archive, {
    timeoutMs: SETUP_TIMEOUT_MS,
    onProgress: (sent, total) => report(sent >= total ? 'setting-up' : 'uploading', sent, total),
  });
}

const STAGE_TEXT = {
  packing: (done, total) => `Packing the folder… ${total ? `${Math.floor((done / total) * 100)}%` : ''}`,
  uploading: (done, total) => `Uploading ${bytes(done)} of ${bytes(total)}…`,
  'setting-up': () => 'Setting up the playground…',
};

/**
 * The drop zone for a project folder: drop one or choose it, and it becomes a playground at once;
 * `onCreated(sandbox)` follows. It shows the progress and, on failure, why, in place.
 */
export function folderDropZone(ctx, { onCreated }) {
  const input = h('input', { type: 'file', class: 'sr-only', tabindex: -1, 'aria-hidden': 'true', multiple: true, webkitdirectory: true });
  const choose = h('button', { type: 'button', class: 'btn btn-primary', onClick: () => input.click() }, icon('folder', { size: 14 }), 'Choose a folder…');
  const status = h('div', { class: 'playground-drop-status', role: 'status', 'aria-live': 'polite' });
  const zone = h('div', { class: 'playground-drop', role: 'group', 'aria-label': 'Project folder' },
    h('span', { class: 'playground-drop-icon', 'aria-hidden': 'true' }, icon('upload', { size: 22 })),
    h('p', { class: 'playground-drop-title' }, 'Drop a project folder here'),
    h('p', { class: 'muted small' }, 'A repository keeps its branches and history. Dependencies and build output stay behind.'),
    h('div', { class: 'row-wrap playground-drop-actions' }, choose),
    status,
    input);
  let busy = false;

  const show = (stage, done, total) => {
    const pct = stage === 'setting-up' || !total ? null : Math.min(100, Math.floor((done / total) * 100));
    setChildren(status,
      h('div', { class: ['update-track', pct === null && 'indeterminate'], role: 'progressbar', 'aria-label': 'Playground progress', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': pct === null ? undefined : pct },
        h('div', { class: 'update-fill', style: pct === null ? null : { width: `${pct}%` } })),
      h('span', { class: 'small' }, STAGE_TEXT[stage](done, total)));
  };

  async function start(collect) {
    if (busy) return;
    busy = true;
    zone.classList.add('busy');
    choose.disabled = true;
    try {
      show('packing', 0, 0);
      const folder = await collect();
      const sandbox = await startFolderPlayground(ctx.api, folder, show);
      setChildren(status, h('span', { class: 'small' }, 'The playground is ready.'));
      onCreated(sandbox);
    } catch (error) {
      setChildren(status, h('span', { class: 'field-error', role: 'alert' }, error && error.message ? error.message : String(error)));
    } finally {
      busy = false;
      zone.classList.remove('busy', 'drop-over');
      choose.disabled = false;
      input.value = '';
    }
  }

  input.addEventListener('change', () => { if (input.files && input.files.length) start(async () => entriesOfInput(input.files)); });
  zone.addEventListener('dragover', (event) => {
    if (!event.dataTransfer || !Array.from(event.dataTransfer.types || []).includes('Files')) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = 'copy';
    zone.classList.add('drop-over');
  });
  zone.addEventListener('dragleave', (event) => { if (!zone.contains(event.relatedTarget)) zone.classList.remove('drop-over'); });
  zone.addEventListener('drop', (event) => {
    if (!event.dataTransfer) return;
    event.preventDefault();
    zone.classList.remove('drop-over');
    const data = event.dataTransfer;
    // Entries must be taken during the event; their files are read afterwards.
    const items = Array.from(data.items || []).map((item) => (item.webkitGetAsEntry ? item.webkitGetAsEntry() : null));
    const files = Array.from(data.files || []);
    start(() => entriesOfDrop({ items: items.map((entry) => ({ webkitGetAsEntry: () => entry })), files }));
  });
  return zone;
}
