// Notes (SPEC-041): the rail's notepads, one for this sandbox and one for the team, shared by
// everyone who may read them, and the person's own ("Me", SPEC-103), which only they see and which
// is the same in every sandbox. All of them save by themselves while people type. A save sends the whole
// text with the revision it started from; the server refuses a stale one (409), so a teammate's
// edit is never overwritten silently: the person picks Keep mine or Use theirs. A teammate's save
// (`note.updated`) reloads a notepad that has nothing unsaved.
//
// Typed text is not lost on the way (review round 1): a notepad is read-only until it is loaded;
// a hidden page saves at once with a request that outlives it (`keepalive`) and keeps the unsaved
// text in sessionStorage, which the next load offers again; leaving with unsaved text asks first.

import { enc } from './api.js';
import { h, setChildren, skeleton } from './dom.js';
import { relLong } from './format.js';
import { attachMarkdownInput } from './mdinput.js';
import { allows } from './status.js';
import { readJson, writeJson } from './store.js';

const TAB_KEY = 'yulius.notesTab';
/** Unsaved text per user and notepad, in sessionStorage (this browser tab only). */
const UNSAVED_PREFIX = 'yulius.notes.unsaved.';
/** Quiet time after the last keystroke before a save (NOT-002). */
export const SAVE_DELAY_MS = 600;
const RETRY_FIRST_MS = 1000;
const RETRY_MAX_MS = 30000;
/** The server's limit (NOT-006): a larger text is refused here, without sending it. */
const MAX_NOTE_BYTES = 256 * 1024;
/** Browsers bound the bodies of requests that outlive the page (64 KiB in all). */
const KEEPALIVE_MAX_BYTES = 60 * 1024;

const retryable = (error) => !error.status || error.status === 408 || error.status === 429 || error.status >= 500;
const byteLength = (text) => new TextEncoder().encode(text).length;

/** Sign-out: no unsaved notes stay in this browser tab. */
export function forgetUnsavedNotes() {
  try {
    for (const key of Object.keys(window.sessionStorage)) if (key.startsWith(UNSAVED_PREFIX)) window.sessionStorage.removeItem(key);
  } catch { /* storage unavailable: nothing kept */ }
}

/** The rail's Notes view for `sandbox`: `{el, headExtra, show, hide, destroy}` for panels.js. */
export function createNotesView(ctx, sandbox) {
  const status = h('span', { class: 'notes-status', role: 'status', 'aria-live': 'polite', 'aria-label': 'Save state' });
  const tablist = h('div', { class: 'term-tabs notes-tabs', role: 'tablist', 'aria-label': 'Notes' });
  const panes = h('div', { class: 'notes-panes' });
  const el = h('div', { class: 'panel-view notes' }, tablist, panes);
  const userId = ctx.store.state.me && ctx.store.state.me.user ? ctx.store.state.me.user.id : 'anonymous';
  const me = ctx.store.state.me && ctx.store.state.me.user ? ctx.store.state.me.user : null;
  // Saves name the GitHub login, else the display name (the superadmin without GitHub).
  const isMine = (note) => Boolean(me && note.updated_by && (note.updated_by === me.login || (!me.github_linked && note.updated_by === me.display_name)));
  const pads = [
    notepad(ctx, {
      key: 'sandbox',
      label: 'This sandbox',
      textLabel: 'Sandbox notes',
      path: `/api/sandboxes/${enc(sandbox.id)}/notes`,
      unsavedKey: `${UNSAVED_PREFIX}${userId}.sandbox:${sandbox.id}`,
      matches: (event) => event.scope === 'sandbox' && event.sandbox_id === sandbox.id,
      readOnly: !allows(sandbox.access, 'write') && 'Read-only. Editing this sandbox’s notes needs write access to the repository on GitHub.',
      placeholder: 'Notes for everyone working in this sandbox: links, decisions, what to try next…',
      onState: () => renderStatus(),
      isMine,
    }),
    notepad(ctx, {
      key: 'team',
      label: 'Team',
      textLabel: 'Team notes',
      path: '/api/notes',
      unsavedKey: `${UNSAVED_PREFIX}${userId}.team`,
      matches: (event) => event.scope === 'team',
      readOnly: false,
      placeholder: 'Notes for the whole team, the same in every sandbox…',
      onState: () => renderStatus(),
      isMine,
    }),
    notepad(ctx, {
      key: 'me',
      label: 'Me',
      textLabel: 'My notes',
      path: '/api/me/notes',
      unsavedKey: `${UNSAVED_PREFIX}${userId}.me`,
      // The server sends a personal note's event to its owner alone.
      matches: (event) => event.scope === 'me',
      readOnly: false,
      placeholder: 'Only you see these notes, the same in every sandbox…',
      onState: () => renderStatus(),
      isMine,
    }),
  ];
  let current = pads.find((pad) => pad.key === readJson(TAB_KEY, 'sandbox')) || pads[0];
  let shown = false;

  function renderTabs() {
    const focused = tablist.contains(document.activeElement);
    const tabs = pads.map((pad) => {
      const selected = pad === current;
      const tab = h('button', { type: 'button', class: ['term-tab', selected && 'active'], role: 'tab', id: `notes-tab-${pad.key}`, 'aria-selected': String(selected), 'aria-controls': `notes-pane-${pad.key}`, tabindex: selected ? '0' : '-1', onClick: () => select(pad, { focusTab: true }) }, pad.label);
      tab.addEventListener('keydown', (event) => {
        const index = pads.indexOf(pad);
        const next = { ArrowRight: index + 1, ArrowLeft: index - 1 + pads.length, Home: 0, End: pads.length - 1 }[event.key];
        if (next === undefined) return;
        event.preventDefault();
        select(pads[next % pads.length], { focusTab: true });
      });
      return tab;
    });
    setChildren(tablist, tabs);
    if (focused) tabs[pads.indexOf(current)].focus();
  }

  function select(pad, { focusTab = false } = {}) {
    if (current !== pad) current.flush();
    current = pad;
    writeJson(TAB_KEY, pad.key);
    for (const other of pads) other.el.hidden = other !== pad;
    renderTabs();
    if (!focusTab) pad.focus();
    pad.load();
    renderStatus();
  }

  function renderStatus() {
    status.textContent = current.statusText();
    status.dataset.tone = current.statusTone();
    current.renderMeta();
  }

  for (const pad of pads) {
    pad.el.id = `notes-pane-${pad.key}`;
    pad.el.setAttribute('role', 'tabpanel');
    pad.el.setAttribute('aria-labelledby', `notes-tab-${pad.key}`);
    pad.el.hidden = pad !== current;
    panes.append(pad.el);
  }
  renderTabs();

  const offs = [
    ctx.store.on('note', (event) => { for (const pad of pads) if (pad.matches(event)) pad.announced(event); }),
    ctx.store.on('resync', () => { for (const pad of pads) pad.announced(null); }),
  ];
  // A hidden page (another tab, a closed laptop, a reload) saves what is typed at once.
  // The notepads share the browser's allowance for requests that outlive the page (64 KiB).
  const leave = () => {
    const budget = { left: KEEPALIVE_MAX_BYTES };
    for (const pad of pads) pad.flush({ leaving: true, budget });
  };
  const onHidden = () => { if (document.visibilityState === 'hidden') leave(); };
  const onPageHide = () => leave();
  // Unsaved text makes the browser ask before leaving the page.
  const onBeforeUnload = (event) => {
    if (!pads.some((pad) => pad.unsaved())) return;
    event.preventDefault();
    event.returnValue = '';
  };
  document.addEventListener('visibilitychange', onHidden);
  window.addEventListener('pagehide', onPageHide);
  window.addEventListener('beforeunload', onBeforeUnload);

  return {
    el,
    headExtra: () => [status],
    show(opts = {}) {
      shown = true;
      current.load();
      renderStatus();
      // Shown again because the page changed (panels.js `{ context: true }`): the keyboard stays
      // with the page; opened by the person, the notepad takes it (SPEC-056 WUI-103).
      if (!opts.context) setTimeout(() => { if (shown) current.focus(); }, 0);
    },
    hide() {
      shown = false;
      for (const pad of pads) pad.flush();
    },
    destroy() {
      for (const pad of pads) pad.destroy();
      offs.forEach((off) => off());
      document.removeEventListener('visibilitychange', onHidden);
      window.removeEventListener('pagehide', onPageHide);
      window.removeEventListener('beforeunload', onBeforeUnload);
    },
  };
}

/** One notepad: its text box, what the server holds, and the save loop. */
function notepad(ctx, { key, label, textLabel, path, unsavedKey, matches, readOnly, placeholder, onState, isMine }) {
  const hintId = `notes-readonly-${key}`;
  const textarea = h('textarea', { class: 'notes-text', 'aria-label': textLabel, 'aria-describedby': readOnly ? hintId : null, placeholder, spellcheck: 'true', readOnly: true });
  const banner = h('div', { class: 'notes-banner', hidden: true });
  const meta = h('p', { class: 'notes-meta' });
  // Until the first read, shimmering fields cover the empty box (SPEC-092); the box keeps focus.
  const fields = skeleton('Loading the notes…', 3, { block: true });
  const el = h('div', { class: 'notes-pane notes-loading' },
    readOnly ? h('p', { class: 'notes-readonly', id: hintId }, readOnly) : null,
    banner, h('div', { class: 'notes-box' }, textarea, fields), meta);
  if (!readOnly) attachMarkdownInput(textarea);

  let loaded = false;
  let base = 0; // the revision the text box's text started from
  let saved = ''; // the text stored at `base`
  let unconfirmed = []; // texts sent since the last answered save (an answer may be lost)
  let announcedRevision = 0; // the newest revision an event told of
  let lastEdit = null; // {updated_by, updated_at}
  let timer = 0;
  let retryTimer = 0;
  let retryDelay = RETRY_FIRST_MS;
  let saving = null; // the save in flight
  let failed = null; // 'retrying' | 'loading' (a load is retried) | message of a refusal
  let conflict = null; // the newer note someone else saved
  let checking = false; // undone after a lost answer: the server may hold the undone text
  let reloadAfter = false; // an event arrived while a load was on its way
  let destroyed = false;

  const dirty = () => loaded && !readOnly && textarea.value !== saved;

  function statusText() {
    if (failed === 'loading') return 'Couldn’t load the notes, retrying…';
    if (!loaded) return ''; // the fields say it
    if (conflict) return 'Not saved';
    if (failed === 'retrying') return 'Not saved, retrying…';
    if (failed) return `Not saved: ${failed}`;
    if (saving || timer || checking || dirty()) return 'Saving…';
    return readOnly ? 'Read-only' : 'Saved';
  }
  const statusTone = () => (conflict || (failed && failed !== 'loading') ? 'warn' : '');

  function renderMeta() {
    meta.textContent = lastEdit && lastEdit.updated_by ? `Edited by ${lastEdit.updated_by} · ${relLong(lastEdit.updated_at)}` : '';
  }

  function retryLater(fn) {
    clearTimeout(retryTimer);
    retryTimer = setTimeout(() => { retryTimer = 0; fn(); }, retryDelay);
    retryDelay = Math.min(retryDelay * 2, RETRY_MAX_MS);
  }

  function settled() {
    unconfirmed = [];
    checking = false; // an answered save, adopt or rebase: the server's text is known
    failed = null;
    retryDelay = RETRY_FIRST_MS;
    clearTimeout(retryTimer);
    retryTimer = 0;
  }

  // ---- unsaved text kept for the next load (a reload, a sandbox switch, a closed panel): the
  // text, the revision it started from (also while a conflict is open: nothing chose yet) and
  // the texts sent whose answers did not arrive.
  function keepUnsaved() {
    if (!loaded || readOnly) return;
    if (!dirty() && !conflict && !checking && !failed) { dropUnsaved(); return; }
    try { window.sessionStorage.setItem(unsavedKey, JSON.stringify({ body: textarea.value, base, sent: unconfirmed })); } catch { /* memory only */ }
  }
  function dropUnsaved() {
    try { window.sessionStorage.removeItem(unsavedKey); } catch { /* nothing kept */ }
  }
  function takeUnsaved() {
    try {
      const kept = JSON.parse(window.sessionStorage.getItem(unsavedKey) || 'null');
      if (!kept || typeof kept.body !== 'string' || !Number.isInteger(kept.base)) return null;
      return { body: kept.body, base: kept.base, sent: Array.isArray(kept.sent) ? kept.sent.filter((text) => typeof text === 'string') : [] };
    } catch { return null; }
  }

  /** A stored note that is this person's own save whose answer was lost. */
  const ownLostSave = (note, sent) => isMine(note) && sent.includes(note.body);

  /** Puts `note` into the text box, keeping the caret and the scroll where they were. */
  function adopt(note) {
    const { selectionStart, selectionEnd, scrollTop } = textarea;
    const focused = document.activeElement === textarea;
    textarea.value = note.body;
    saved = note.body;
    base = note.revision;
    lastEdit = note.updated_by ? { updated_by: note.updated_by, updated_at: note.updated_at } : null;
    if (focused) textarea.setSelectionRange(Math.min(selectionStart, note.body.length), Math.min(selectionEnd, note.body.length));
    textarea.scrollTop = scrollTop;
    settled();
    if (!readOnly) dropUnsaved(); // a reader who lost write access keeps theirs for later
    renderMeta();
  }

  /** Goes on from `note`, this person's own stored save, keeping the text box as it is. */
  function rebase(note) {
    saved = note.body;
    base = note.revision;
    lastEdit = { updated_by: note.updated_by, updated_at: note.updated_at };
    settled();
    renderMeta();
    if (dirty()) schedule();
    else dropUnsaved();
  }

  /** The first load: the note, and the text this tab could not save last time, if any. */
  function first(note) {
    const kept = readOnly ? null : takeUnsaved();
    loaded = true;
    el.classList.remove('notes-loading');
    fields.remove();
    textarea.readOnly = Boolean(readOnly);
    adopt(note);
    if (!kept || kept.body === note.body) return;
    textarea.value = kept.body;
    // Nobody saved since, or the newer save is this person's own: it simply goes on.
    if (kept.base === note.revision || ownLostSave(note, kept.sent)) {
      unconfirmed = [...kept.sent, kept.body].slice(-8); // a save of it may still arrive
      schedule();
      return;
    }
    // Someone else did: the person decides, and until then the text keeps its own revision.
    base = kept.base;
    conflict = note;
    keepUnsaved();
    renderBanner();
  }

  /** Loads again when an event told of a newer revision and nothing is unsaved. */
  function catchUp() {
    if (loaded && announcedRevision > base && !dirty() && !saving && !conflict) load();
  }

  let loading = null;
  /** Fetches the note unless the text box holds unsaved typing. */
  function load() {
    if (loading || dirty() || saving || conflict || destroyed) return loading;
    let again = false;
    reloadAfter = false;
    loading = ctx.api.get(path).then((note) => {
      if (destroyed) return;
      if (!loaded) { first(note); return; }
      const wasChecking = checking;
      checking = false;
      if (saving || conflict || note.revision < base) return;
      if (note.revision > base && ownLostSave(note, unconfirmed)) { rebase(note); return; }
      if (dirty()) return;
      if (note.revision !== base) { adopt(note); again = true; } // moved on: newer may exist
      else settled();
      if (wasChecking) dropUnsaved();
    }).catch((error) => {
      if (destroyed) return;
      if (!loaded) { failed = 'loading'; retryLater(load); return; }
      // Behind a teammate's save, or checking an undo: try again later, never at once.
      if (retryable(error) && (checking || announcedRevision > base)) retryLater(load);
      else if (checking) { failed = error.message; checking = false; } // not Saved: unknown
    }).finally(() => {
      loading = null;
      onState();
      if (again || reloadAfter) catchUp();
    });
    return loading;
  }

  function schedule() {
    clearTimeout(timer);
    timer = setTimeout(() => { timer = 0; save(); }, SAVE_DELAY_MS);
    onState();
  }

  /**
   * Saves now when there is something to save (blur, hide, another tab). Leaving the page also
   * keeps the text for the next load; with a `budget` (the page is going) the save is a request
   * that outlives the page, while the browser's shared allowance lasts.
   */
  function flush({ leaving = false, budget = null } = {}) {
    if (timer) { clearTimeout(timer); timer = 0; }
    save({ budget }); // first: the stash then names the text on its way among those sent
    if (leaving) keepUnsaved();
  }

  async function save({ budget = null } = {}) {
    // Not loaded yet: nothing typed here, and a kept text waits for the first load (round 3).
    if (!loaded || readOnly) return;
    if (!dirty()) {
      if (failed === 'retrying') {
        // Undone back to the stored text: a lost answer may have stored the undone text, so
        // look before saying Saved (the load goes on from it); else it is simply saved.
        failed = null;
        retryDelay = RETRY_FIRST_MS;
        clearTimeout(retryTimer);
        retryTimer = 0;
        if (unconfirmed.length) { checking = true; load(); }
      }
      if (!conflict && !checking && !failed) dropUnsaved();
      catchUp();
      onState();
      return;
    }
    if (saving || conflict || (failed && failed !== 'retrying')) { onState(); return; }
    const body = textarea.value;
    if (byteLength(body) > MAX_NOTE_BYTES) {
      failed = `notes hold at most ${MAX_NOTE_BYTES / 1024} KiB`;
      onState();
      return;
    }
    clearTimeout(retryTimer);
    retryTimer = 0;
    const payload = { body, revision: base };
    const size = budget ? byteLength(JSON.stringify(payload)) : 0;
    const keepalive = Boolean(budget) && size <= budget.left;
    if (keepalive) budget.left -= size;
    unconfirmed = [...unconfirmed.slice(-7), body];
    saving = ctx.api.put(path, payload, { keepalive });
    onState();
    try {
      const note = await saving;
      saving = null;
      saved = note.body;
      base = note.revision;
      lastEdit = { updated_by: note.updated_by, updated_at: note.updated_at };
      settled();
      renderMeta();
      if (dirty()) { if (!timer && !destroyed) schedule(); } // typed while the save was on its way
      else { dropUnsaved(); catchUp(); }
    } catch (error) {
      saving = null;
      if (error.status === 409) await resolveConflict();
      else if (!retryable(error)) failed = error.message;
      else {
        failed = 'retrying';
        // Bounded after the view is gone: the text is also kept for the next load.
        if (!destroyed || retryDelay < RETRY_MAX_MS) retryLater(save);
      }
    }
    onState();
  }

  /** A save was refused as stale: our own lost save or the same text goes on; else the person picks. */
  async function resolveConflict() {
    let note;
    try {
      note = await ctx.api.get(path);
    } catch {
      failed = 'retrying';
      if (!destroyed || retryDelay < RETRY_MAX_MS) retryLater(save);
      return;
    }
    if (note.body === textarea.value) { adopt(note); return; }
    if (ownLostSave(note, unconfirmed)) {
      // Our save was stored but its answer was lost: go on from it.
      rebase(note);
      if (destroyed) keepUnsaved();
      return;
    }
    conflict = note;
    keepUnsaved();
    if (!destroyed) renderBanner();
  }

  function renderBanner() {
    banner.hidden = !conflict;
    if (!conflict) { setChildren(banner, null); banner.removeAttribute('role'); return; }
    banner.setAttribute('role', 'alert');
    setChildren(banner,
      h('p', null, `${conflict.updated_by || 'Someone'} changed these notes while you were typing. Your text is not saved yet.`),
      h('div', { class: 'row' },
        h('button', { type: 'button', class: 'btn btn-primary btn-sm', onClick: keepMine }, 'Keep mine'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: useTheirs }, 'Use theirs')));
  }

  function keepMine() {
    if (!conflict) return;
    base = conflict.revision;
    saved = conflict.body;
    conflict = null;
    renderBanner();
    textarea.focus();
    save();
  }

  function useTheirs() {
    if (!conflict) return;
    const note = conflict;
    conflict = null;
    renderBanner();
    adopt(note);
    textarea.focus();
    onState();
    load(); // anything saved while the choice was open
  }

  /** Someone saved (`event`), or events were lost (`null`): reload when nothing is unsaved. */
  function announced(event) {
    if (event) announcedRevision = Math.max(announcedRevision, event.revision);
    if (loaded && event && event.revision <= base) return;
    if (!loaded && retryTimer) return; // the first load is already retrying with its pauses
    if (loading) { reloadAfter = true; return; }
    load();
  }

  textarea.addEventListener('input', () => {
    if (failed && failed !== 'retrying' && failed !== 'loading') failed = null; // a refusal is retried after the next edit
    if (!conflict) schedule();
  });
  textarea.addEventListener('blur', () => flush());

  return {
    key,
    label,
    el,
    matches,
    load,
    flush,
    announced,
    statusText,
    statusTone,
    renderMeta,
    unsaved: () => Boolean(conflict || saving || timer || checking || dirty()),
    focus() { textarea.focus(); },
    destroy() {
      flush({ leaving: true }); // kept for the next load; the save goes on after the view is gone
      destroyed = true;
      clearTimeout(timer);
    },
  };
}
