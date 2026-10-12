// Notes (SPEC-041): the rail's notepads, one for this sandbox and one for the team, shared by
// everyone who may read them and saved by themselves while people type. A save sends the whole
// text with the revision it started from; the server refuses a stale one (409), so a teammate's
// edit is never overwritten silently: the person picks Keep mine or Use theirs. A teammate's save
// (`note.updated`) reloads a notepad that has nothing unsaved.

import { enc } from './api.js';
import { h, setChildren } from './dom.js';
import { relLong } from './format.js';
import { attachMarkdownInput } from './mdinput.js';
import { allows } from './status.js';
import { readJson, writeJson } from './store.js';

const TAB_KEY = 'yulius.notesTab';
/** Quiet time after the last keystroke before a save (NOT-002). */
export const SAVE_DELAY_MS = 600;
const RETRY_FIRST_MS = 1000;
const RETRY_MAX_MS = 30000;

/** The rail's Notes view for `sandbox`: `{el, headExtra, show, hide, destroy}` for panels.js. */
export function createNotesView(ctx, sandbox) {
  const status = h('span', { class: 'notes-status', role: 'status', 'aria-live': 'polite', 'aria-label': 'Save state' });
  const tablist = h('div', { class: 'term-tabs notes-tabs', role: 'tablist', 'aria-label': 'Notes' });
  const panes = h('div', { class: 'notes-panes' });
  const el = h('div', { class: 'panel-view notes' }, tablist, panes);
  const pads = [
    notepad(ctx, {
      key: 'sandbox',
      label: 'This sandbox',
      textLabel: 'Sandbox notes',
      path: `/api/sandboxes/${enc(sandbox.id)}/notes`,
      matches: (event) => event.scope === 'sandbox' && event.sandbox_id === sandbox.id,
      readOnly: !allows(sandbox.access, 'write') && 'Read-only. Editing this sandbox’s notes needs write access to the repository on GitHub.',
      placeholder: 'Notes for everyone working in this sandbox: links, decisions, what to try next…',
      onState: () => renderStatus(),
    }),
    notepad(ctx, {
      key: 'team',
      label: 'Team',
      textLabel: 'Team notes',
      path: '/api/notes',
      matches: (event) => event.scope === 'team',
      readOnly: false,
      placeholder: 'Notes for the whole team, the same in every sandbox…',
      onState: () => renderStatus(),
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
        if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
        event.preventDefault();
        const index = pads.indexOf(pad);
        select(pads[(index + (event.key === 'ArrowRight' ? 1 : pads.length - 1)) % pads.length], { focusTab: true });
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
  const onHidden = () => { if (document.visibilityState === 'hidden') for (const pad of pads) pad.flush(); };
  const onPageHide = () => { for (const pad of pads) pad.flush(); };
  document.addEventListener('visibilitychange', onHidden);
  window.addEventListener('pagehide', onPageHide);

  return {
    el,
    headExtra: () => [status],
    show() {
      shown = true;
      current.load();
      renderStatus();
      setTimeout(() => { if (shown) current.focus(); }, 0);
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
    },
  };
}

/** One notepad: its text box, what the server holds, and the save loop. */
function notepad(ctx, { key, label, textLabel, path, matches, readOnly, placeholder, onState }) {
  const textarea = h('textarea', { class: 'notes-text', 'aria-label': textLabel, placeholder, spellcheck: 'true', readOnly: Boolean(readOnly) });
  const banner = h('div', { class: 'notes-banner', hidden: true });
  const meta = h('p', { class: 'notes-meta' });
  const el = h('div', { class: 'notes-pane' },
    readOnly ? h('p', { class: 'notes-readonly' }, readOnly) : null,
    banner, textarea, meta);
  if (!readOnly) attachMarkdownInput(textarea);

  let loaded = false;
  let base = 0; // the revision the text box's text started from
  let saved = ''; // the text stored at `base`
  let lastEdit = null; // {updated_by, updated_at}
  let timer = 0;
  let retryTimer = 0;
  let retryDelay = RETRY_FIRST_MS;
  let saving = null; // the save in flight
  let failed = null; // 'retrying' | message of a refusal that retrying cannot fix
  let conflict = null; // the newer note someone else saved
  let destroyed = false;

  const dirty = () => loaded && !readOnly && textarea.value !== saved;

  function statusText() {
    if (!loaded) return '';
    if (conflict) return 'Not saved';
    if (failed === 'retrying') return 'Not saved, retrying…';
    if (failed) return `Not saved: ${failed}`;
    if (saving || timer || dirty()) return 'Saving…';
    return readOnly ? 'Read-only' : 'Saved';
  }
  const statusTone = () => (conflict || failed ? 'warn' : '');

  function renderMeta() {
    meta.textContent = lastEdit && lastEdit.updated_by ? `Edited by ${lastEdit.updated_by} · ${relLong(lastEdit.updated_at)}` : '';
  }

  /** Puts `note` into the text box, keeping the caret where it was. */
  function adopt(note) {
    const { selectionStart, selectionEnd } = textarea;
    const focused = document.activeElement === textarea;
    textarea.value = note.body;
    saved = note.body;
    base = note.revision;
    lastEdit = note.updated_by ? { updated_by: note.updated_by, updated_at: note.updated_at } : null;
    if (focused) textarea.setSelectionRange(Math.min(selectionStart, note.body.length), Math.min(selectionEnd, note.body.length));
    renderMeta();
  }

  let loading = null;
  /** Fetches the note unless the text box holds unsaved typing. */
  function load() {
    if (loading || dirty() || saving || conflict) return loading;
    loading = ctx.api.get(path).then((note) => {
      if (destroyed || dirty() || saving || conflict) return;
      if (!loaded || note.revision !== base) adopt(note);
      loaded = true;
      failed = null;
    }).catch((error) => {
      if (!loaded) failed = `couldn’t load the notes (${error.message})`;
    }).finally(() => { loading = null; onState(); });
    return loading;
  }

  function schedule() {
    clearTimeout(timer);
    timer = setTimeout(() => { timer = 0; save(); }, SAVE_DELAY_MS);
    onState();
  }

  /** Saves now when there is something to save (also the flush on blur, hide and pagehide). */
  function flush() {
    if (timer) { clearTimeout(timer); timer = 0; }
    save();
  }

  async function save() {
    if (saving || conflict || !dirty() || (failed && failed !== 'retrying')) { onState(); return; }
    clearTimeout(retryTimer);
    retryTimer = 0;
    const body = textarea.value;
    saving = ctx.api.put(path, { body, revision: base });
    onState();
    try {
      const note = await saving;
      saving = null;
      saved = note.body;
      base = note.revision;
      lastEdit = { updated_by: note.updated_by, updated_at: note.updated_at };
      failed = null;
      retryDelay = RETRY_FIRST_MS;
      renderMeta();
      // Typed while the save was on its way: that is saved next.
      if (dirty() && !timer) schedule();
    } catch (error) {
      saving = null;
      if (destroyed) return;
      if (error.status === 409) await resolveConflict();
      else if (error.status === 403 || error.status === 404 || error.status === 422) failed = error.message;
      else {
        failed = 'retrying';
        retryTimer = setTimeout(() => { retryTimer = 0; save(); }, retryDelay);
        retryDelay = Math.min(retryDelay * 2, RETRY_MAX_MS);
      }
    }
    onState();
  }

  /** A save was refused as stale: the same text is adopted, a different one asks the person. */
  async function resolveConflict() {
    let note;
    try {
      note = await ctx.api.get(path);
    } catch {
      failed = 'retrying';
      retryTimer = setTimeout(() => { retryTimer = 0; save(); }, retryDelay);
      return;
    }
    if (destroyed) return;
    if (note.body === textarea.value) {
      adopt(note);
      return;
    }
    conflict = note;
    renderBanner();
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
  }

  /** Someone saved (`event`), or events were lost (`null`): reload when nothing is unsaved. */
  function announced(event) {
    if (!loaded) return;
    if (event && event.revision <= base) return;
    load();
  }

  textarea.addEventListener('input', () => {
    if (failed && failed !== 'retrying') failed = null; // a refusal is retried after the next edit
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
    focus() { textarea.focus(); },
    destroy() {
      flush(); // the save goes on after the view is gone
      destroyed = true;
      clearTimeout(timer);
      clearTimeout(retryTimer);
    },
  };
}
