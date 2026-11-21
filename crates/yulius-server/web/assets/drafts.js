// The new-task composer's drafts (SPEC-026): what a person put in a sandbox's composer (the
// prompt, the attached files and the choices on its edge) waits here while they visit other
// pages or reload, and is cleared when the task starts.
//
// A draft is `{ text, mode, workflowId, afterTaskId, startRule, supertask, prepare, attached, starting }`; `mode` and
// `workflowId` are null until chosen (the composer's defaults apply); `prepare` is null, or
// `{ at }` for a prepared task (SPEC-105: `at` an ISO time, or null for "when I decide"); `starting` (this page only)
// is true while its task is being created, so a composer opened meanwhile does not start it again. Each attachment is
// `{ path, name, type, at, thumb?, file? }`: the file was uploaded at once (SPEC-014), so only its
// server path is needed to start the task; `text` holds each file where it sits as `@'<path>'`
// (SPEC-124), and `attached` every uploaded file, the tray's, in order (SPEC-150: one without a
// place in the text is sent after it); `file` (this page only) draws its thumbnail, `thumb` (a
// small data URL) draws it after a reload.
//
// Stored in `localStorage` under `yulius.draft.<user id>.<sandbox id>` for the original draft, with a unique
// suffix for each additional draft (SPEC-594). Written shortly after each
// change, at once when the composer closes and when the page is hidden or goes away. Sign-out
// removes every stored draft (`forgetDrafts`). Bounds (DRF-007): attachments older than the
// server keeps draft files are dropped when read, at most MAX_THUMBS thumbnails of at most
// THUMB_PX are stored, and a full storage keeps the draft without thumbnails, or in memory only.

const PREFIX = 'yulius.draft.';
/** Written at sign-out, so the other tabs of this browser forget their drafts too. */
const FORGOTTEN_KEY = 'yulius.draftsForgotten';
/** Explicit discard/start can retire even a draft whose first upload is still in flight. */
const RETIRED_KEY = 'yulius.draftRetired';
let retirements;
try { retirements = new BroadcastChannel(RETIRED_KEY); } catch { /* use storage events */ }
/** The server removes a composer's uploaded files after a week (`DRAFT_MAX_AGE`, uploads.rs). */
const MAX_AGE_MS = 7 * 24 * 3600 * 1000;
const MAX_THUMBS = 12;
/** As many conditions as the rule editor allows (RULE_MAX_CONDITIONS, ruleeditor.js). */
const MAX_RULE_CONDITIONS = 16;
const THUMB_PX = 160;
/** Typing is stored at most this often. */
const SAVE_DELAY_MS = 300;

/** storage key → the draft, for this page. */
const drafts = new Map();
/** storage key → its pending write. */
const timers = new Map();
/** storage key → the callbacks told when attachments change outside the composer's own edits. */
const listeners = new Map();

function emptyDraft() {
  return { text: '', mode: null, workflowId: null, afterTaskId: '', startRule: null, supertask: false, prepare: null, attached: [], starting: false };
}

const keyOf = (userId, sandboxId) => `${PREFIX}${userId || 'anonymous'}.${sandboxId}`;

/** The selected draft in each sandbox in this tab. Only its key goes in sessionStorage. */
const selected = new Map();

function draftKeys(scope) {
  const keys = new Set(drafts.keys());
  try { for (const key of Object.keys(window.localStorage)) keys.add(key); } catch { /* memory only */ }
  return [...keys].filter((key) => key === scope || key.startsWith(scope + '.'));
}

function hasContent(draft) {
  return draft.starting || draft.text.trim() || draft.attached.length;
}

function loadDraft(scope, key) {
  if (!drafts.has(key)) drafts.set(key, { ...emptyDraft(), ...restore(key), scope, key });
  return drafts.get(key);
}

export function selectDraft(draft) {
  selected.set(draft.scope, draft.key);
  try { window.sessionStorage.setItem(draft.scope, draft.key); } catch { /* memory only */ }
}

/** The original single draft uses the bare scope key and opens without migration. */
export function draftFor(userId, sandboxId) {
  const scope = keyOf(userId, sandboxId);
  let key = selected.get(scope);
  if (!key) {
    try { key = window.sessionStorage.getItem(scope); } catch { /* memory only */ }
    if (key !== scope && !key?.startsWith(scope + '.')) {
      key = draftKeys(scope).find((candidate) => hasContent(loadDraft(scope, candidate))) || scope;
    }
    selectDraft(loadDraft(scope, key));
  }
  return loadDraft(scope, key);
}

export function otherDrafts(draft) {
  return draftKeys(draft.scope).filter((key) => key !== draft.key)
    .map((key) => loadDraft(draft.scope, key))
    .filter(hasContent);
}

export function newDraft(draft) {
  const id = Array.from(crypto.getRandomValues(new Uint8Array(12)), (byte) => byte.toString(16).padStart(2, '0')).join('');
  selectDraft(loadDraft(draft.scope, draft.scope + '.' + id));
}

/** Records a change of the draft: stored shortly after, or at once with `now`. */
export function saveDraft(draft, { now = false } = {}) {
  const { key } = draft;
  if (drafts.get(key) !== draft || (now && !timers.has(key))) return;
  draft.remoteChange = false;
  clearTimeout(timers.get(key));
  timers.delete(key);
  if (now) store(key);
  else timers.set(key, setTimeout(() => { timers.delete(key); store(key); }, SAVE_DELAY_MS));
}

/**
 * Calls `callback` whenever this draft gains an attachment or a thumbnail, or another tab removes it
 * (an upload can end after the composer that started it is gone). Returns the unsubscribe.
 */
export function onDraftChange(draft, callback) {
  const { key } = draft;
  if (!listeners.has(key)) listeners.set(key, new Set());
  const set = listeners.get(key);
  set.add(callback);
  return () => {
    set.delete(callback);
    if (!set.size) {
      listeners.delete(key);
      reconcile(draft);
    }
  };
}

function changed(key, kind = 'attachments') {
  for (const callback of [...(listeners.get(key) || [])]) callback(kind);
}

export function setDraftStarting(draft, starting) {
  if (drafts.get(draft.key) !== draft || draft.starting === starting) return;
  draft.starting = starting;
  changed(draft.key, 'starting');
  if (!starting) reconcile(draft);
}

/**
 * Adds uploaded files (`[{path, name, file}]`) to `draft`, the one the upload started from, and
 * stores it; an image gets a small stored thumbnail a moment later. A draft forgotten meanwhile
 * (sign-out) is left alone, so a late upload never stores it again.
 */
export function attachToDraft(draft, items) {
  const { key } = draft;
  if (drafts.get(key) !== draft) return;
  const at = Date.now();
  const added = items.map(({ path, name, file }) => ({ path, name, type: file.type || '', at, file }));
  draft.attached.push(...added);
  saveDraft(draft);
  changed(key);
  for (const item of added) {
    if (!/^image\//.test(item.type)) continue;
    thumbnail(item.file).then((thumb) => {
      // Also for an upload out of the tray now: undo or redo may bring it back (SPEC-150).
      if (!thumb || drafts.get(key) !== draft) return;
      item.thumb = thumb;
      saveDraft(draft);
      changed(key);
    });
  }
}

/** Retires the originating draft so a late upload, thumbnail or unmount cannot write it back. */
export function clearDraft(draft) {
  if (drafts.get(draft.key) !== draft) return;
  retire(draft);
  if (retirements) retirements.postMessage(draft.key);
  else {
    try {
      window.localStorage.setItem(RETIRED_KEY, draft.key);
      window.localStorage.removeItem(RETIRED_KEY);
    } catch { /* storage unavailable */ }
  }
}

function retire(draft) {
  const { key, scope } = draft;
  drafts.delete(key);
  try { window.localStorage.removeItem(key); } catch { /* storage unavailable */ }
  clearTimeout(timers.get(key));
  timers.delete(key);
  if (selected.get(scope) === key) {
    const next = otherDrafts(draft)[0];
    if (next) selectDraft(next);
    else newDraft(draft);
  }
}

/** Forgets every draft, in memory and stored, here and in this browser's other tabs (sign-out:
 * drafts are private words). */
export function forgetDrafts() {
  forgetInMemory();
  try {
    for (const key of Object.keys(window.localStorage)) if (key.startsWith(PREFIX)) window.localStorage.removeItem(key);
    window.localStorage.setItem(FORGOTTEN_KEY, String(Date.now()));
  } catch { /* storage unavailable: nothing kept */ }
}

/** Drops this page's drafts: a composer still open holds a draft no longer stored or written. */
function forgetInMemory() {
  for (const timer of timers.values()) clearTimeout(timer);
  timers.clear();
  drafts.clear();
  selected.clear();
  try {
    for (const key of Object.keys(window.sessionStorage)) if (key.startsWith(PREFIX)) window.sessionStorage.removeItem(key);
  } catch { /* memory only */ }
}
function retiredElsewhere(key) {
  const draft = drafts.get(key);
  if (!draft) return;
  retire(draft);
  changed(key, 'retired');
}
if (retirements) retirements.onmessage = (event) => retiredElsewhere(event.data);

// A remote save waits while this tab is editing or submitting. A later local save wins.
function reconcile(draft) {
  if (!draft.remoteChange || draft.starting || listeners.has(draft.key) || drafts.get(draft.key) !== draft) return;
  draft.remoteChange = false;
  const stored = restore(draft.key);
  // Keep the live File and thumbnail work for attachments which still belong to the draft.
  const live = new Map(draft.attached.map((item) => [item.path, item]));
  if (stored.attached) stored.attached = stored.attached.map((item) => {
    const previous = live.get(item.path);
    return previous?.at === item.at ? Object.assign(previous, item) : item;
  });
  Object.assign(draft, emptyDraft(), stored);
}

window.addEventListener('storage', (event) => {
  if (event.key === FORGOTTEN_KEY) { forgetInMemory(); return; }
  if (event.key === RETIRED_KEY) { if (event.newValue) retiredElsewhere(event.newValue); return; }
  const draft = drafts.get(event.key);
  // Clearing the prompt also removes its storage entry. Its editor and held uploads stay live.
  if (draft && !draft.memoryOnly && !timers.has(event.key)) {
    draft.remoteChange = true;
    reconcile(draft);
  }
});

/** Stores every pending draft now (the page is hidden or goes away). */
function flushAll() {
  for (const [key, timer] of timers) { clearTimeout(timer); store(key); }
  timers.clear();
}
window.addEventListener('pagehide', flushAll);
document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden') flushAll(); });

function store(key) {
  const draft = drafts.get(key);
  if (!draft) return;
  draft.memoryOnly = false;
  try {
    // Nothing typed or attached: nothing to keep.
    if (!draft.text.trim() && !draft.attached.length) { window.localStorage.removeItem(key); return; }
  } catch { draft.memoryOnly = true; return; }
  const shape = (withThumbs) => {
    let thumbs = 0; // the first MAX_THUMBS images keep theirs
    return JSON.stringify({
      text: draft.text,
      mode: draft.mode,
      workflowId: draft.workflowId,
      afterTaskId: draft.afterTaskId,
      startRule: draft.startRule,
      supertask: draft.supertask,
      prepare: draft.prepare,
      attached: draft.attached.map((item) => ({
        path: item.path, name: item.name, type: item.type, at: item.at,
        thumb: withThumbs && item.thumb && thumbs++ < MAX_THUMBS ? item.thumb : undefined,
      })),
    });
  };
  try {
    window.localStorage.setItem(key, shape(true));
  } catch {
    // Full (or unavailable): without thumbnails, else in memory only.
    try { window.localStorage.setItem(key, shape(false)); } catch { draft.memoryOnly = true; }
  }
}

function restore(key) {
  let stored;
  try { stored = JSON.parse(window.localStorage.getItem(key)); } catch { return {}; }
  if (!stored || typeof stored !== 'object') return {};
  const text = (value) => (typeof value === 'string' ? value : '');
  const choice = (value) => (typeof value === 'string' ? value : null);
  const now = Date.now();
  const attached = (Array.isArray(stored.attached) ? stored.attached : [])
    .filter((item) => item && typeof item.path === 'string' && typeof item.name === 'string'
      && Number.isFinite(item.at) && now - item.at < MAX_AGE_MS)
    .map((item) => ({
      path: item.path, name: item.name, type: text(item.type), at: item.at,
      thumb: typeof item.thumb === 'string' && item.thumb.startsWith('data:image/') ? item.thumb : undefined,
    }));
  return {
    text: text(stored.text), mode: choice(stored.mode), workflowId: choice(stored.workflowId),
    afterTaskId: text(stored.afterTaskId), startRule: rule(stored.startRule), supertask: stored.supertask === true, prepare: prepare(stored.prepare), attached,
  };
}

/** A stored prepared choice (SPEC-105): `{ at }` with an ISO time or null; null otherwise. */
function prepare(value) {
  if (!value || typeof value !== 'object') return null;
  return { at: typeof value.at === 'string' && Number.isFinite(Date.parse(value.at)) ? value.at : null };
}

/** A stored start rule (SPEC-022: `{ join, conditions: [{ number, state }] }`) when well formed. */
function rule(value) {
  if (!value || typeof value !== 'object' || !Array.isArray(value.conditions)) return null;
  const conditions = value.conditions.filter((c) => c && Number.isInteger(c.number) && c.number > 0
    && ['done', 'merged', 'ended'].includes(c.state)).map((c) => ({ number: c.number, state: c.state }));
  if (!conditions.length || conditions.length !== value.conditions.length || conditions.length > MAX_RULE_CONDITIONS) return null;
  return { join: value.join === 'or' ? 'or' : 'and', conditions };
}

/** A data URL of the image scaled to fit THUMB_PX, or null when it cannot be drawn. */
async function thumbnail(file) {
  try {
    const bitmap = await createImageBitmap(file);
    const scale = Math.min(1, THUMB_PX / Math.max(bitmap.width, bitmap.height));
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, Math.round(bitmap.width * scale));
    canvas.height = Math.max(1, Math.round(bitmap.height * scale));
    canvas.getContext('2d').drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    bitmap.close();
    // A browser without WebP encoding (Safari) answers PNG, several times larger: JPEG then.
    const webp = canvas.toDataURL('image/webp', 0.8);
    return webp.startsWith('data:image/webp') ? webp : canvas.toDataURL('image/jpeg', 0.8);
  } catch {
    return null;
  }
}
