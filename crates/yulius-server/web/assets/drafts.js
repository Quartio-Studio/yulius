// The new-task composer's drafts (SPEC-026): what a person put in a sandbox's composer (the
// prompt, the attached files and the choices on its edge) waits here while they visit other
// pages or reload, and is cleared when the task starts.
//
// A draft is `{ text, mode, workflowId, afterTaskId, startRule, attached }`; `mode` and
// `workflowId` are null until chosen (the composer's defaults apply). Each attachment is
// `{ path, name, type, at, thumb?, file? }`: the file was uploaded at once (SPEC-014), so only its
// server path is needed to start the task; `file` (this page only) draws its thumbnail, `thumb` (a
// small data URL) draws it after a reload.
//
// Stored in `localStorage` under `yulius.draft.<user id>.<sandbox id>`, written shortly after each
// change, at once when the composer closes and when the page is hidden or goes away. Sign-out
// removes every stored draft (`forgetDrafts`). Bounds (DRF-007): attachments older than the
// server keeps draft files are dropped when read, at most MAX_THUMBS thumbnails of at most
// THUMB_PX are stored, and a full storage keeps the draft without thumbnails, or in memory only.

const PREFIX = 'yulius.draft.';
/** The server removes a composer's uploaded files after a week (`DRAFT_MAX_AGE`, uploads.rs). */
const MAX_AGE_MS = 7 * 24 * 3600 * 1000;
const MAX_THUMBS = 12;
const THUMB_PX = 160;
/** Typing is stored at most this often. */
const SAVE_DELAY_MS = 300;

/** storage key → the draft, for this page. */
const drafts = new Map();
/** storage key → its pending write. */
const timers = new Map();

function emptyDraft() {
  return { text: '', mode: null, workflowId: null, afterTaskId: '', startRule: '', attached: [] };
}

const keyOf = (userId, sandboxId) => `${PREFIX}${userId || 'anonymous'}.${sandboxId}`;

/** The live draft of a sandbox's composer for this user: the composer changes it in place. */
export function draftFor(userId, sandboxId) {
  const key = keyOf(userId, sandboxId);
  let draft = drafts.get(key);
  if (!draft) {
    draft = Object.assign(emptyDraft(), restore(key));
    drafts.set(key, draft);
  }
  return draft;
}

/** Records a change of the draft: stored shortly after, or at once with `now`. */
export function saveDraft(userId, sandboxId, { now = false } = {}) {
  const key = keyOf(userId, sandboxId);
  clearTimeout(timers.get(key));
  timers.delete(key);
  if (now) store(key);
  else timers.set(key, setTimeout(() => { timers.delete(key); store(key); }, SAVE_DELAY_MS));
}

/**
 * Adds uploaded files (`[{path, name, file}]`) to the draft and stores it. An image gets a small
 * stored thumbnail a moment later; `onThumb` is called then, to redraw.
 */
export function attachToDraft(userId, sandboxId, items, onThumb) {
  const draft = draftFor(userId, sandboxId);
  const at = Date.now();
  const added = items.map(({ path, name, file }) => ({ path, name, type: file.type || '', at, file }));
  draft.attached.push(...added);
  saveDraft(userId, sandboxId);
  for (const item of added) {
    if (!/^image\//.test(item.type)) continue;
    thumbnail(item.file).then((thumb) => {
      if (!thumb || !draft.attached.includes(item)) return;
      item.thumb = thumb;
      saveDraft(userId, sandboxId);
      if (onThumb) onThumb();
    });
  }
}

/** Empties a sandbox's draft in place and forgets the stored one (its task started). */
export function clearDraft(userId, sandboxId) {
  const key = keyOf(userId, sandboxId);
  Object.assign(draftFor(userId, sandboxId), emptyDraft());
  clearTimeout(timers.get(key));
  timers.delete(key);
  try { window.localStorage.removeItem(key); } catch { /* storage unavailable */ }
}

/** Forgets every draft, in memory and stored (sign-out: drafts are private words). */
export function forgetDrafts() {
  for (const timer of timers.values()) clearTimeout(timer);
  timers.clear();
  drafts.clear();
  try {
    for (const key of Object.keys(window.localStorage)) if (key.startsWith(PREFIX)) window.localStorage.removeItem(key);
  } catch { /* storage unavailable: nothing kept */ }
}

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
  try {
    // Nothing typed or attached: nothing to keep.
    if (!draft.text.trim() && !draft.attached.length) { window.localStorage.removeItem(key); return; }
  } catch { return; }
  const shape = (withThumbs) => JSON.stringify({
    text: draft.text,
    mode: draft.mode,
    workflowId: draft.workflowId,
    afterTaskId: draft.afterTaskId,
    startRule: draft.startRule,
    attached: draft.attached.map((item, index) => ({
      path: item.path, name: item.name, type: item.type, at: item.at,
      thumb: withThumbs && index < MAX_THUMBS ? item.thumb : undefined,
    })),
  });
  try {
    window.localStorage.setItem(key, shape(true));
  } catch {
    // Full (or unavailable): without thumbnails, else in memory only.
    try { window.localStorage.setItem(key, shape(false)); } catch { /* memory only */ }
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
    afterTaskId: text(stored.afterTaskId), startRule: text(stored.startRule), attached,
  };
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
    return canvas.toDataURL('image/webp', 0.8);
  } catch {
    return null;
  }
}
