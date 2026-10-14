// Notes (SPEC-041): the rail's notepads, one for this sandbox and one for the team, shared by
// everyone who may read them, and the person's own ("Me", SPEC-103), which only they see and
// which is the same in every sandbox. All of them save by themselves while people type. A save
// sends the whole text with the revision it started from; the server refuses a stale one (409),
// so a teammate's edit is never overwritten silently: the person picks Keep mine or Use theirs.
// A teammate's save (`note.updated`) reloads a notepad that has nothing unsaved.
//
// Typed text is not lost on the way (review round 1): a notepad is read-only until it is loaded;
// a hidden page saves at once with a request that outlives it (`keepalive`) and keeps the unsaved
// text in sessionStorage, which the next load offers again; leaving with unsaved text asks first.
//
// The tabs go in any order (SPEC-112): dragged along the strip, Alt+←/→ or the context menu; the
// order is remembered per browser and is Team, This sandbox, Me until the person moves one. Each
// tab shows a grip and says it can be clicked or dragged (SPEC-136).

import { enc } from './api.js';
import { h, icon, openMenu, setChildren, skeleton } from './dom.js';
import { relLong } from './format.js';
import { attachMarkdownInput } from './mdinput.js';
import { allows } from './status.js';
import { createTextLayer, mapPosition, textEdit } from './textlayer.js';
import { readJson, writeJson } from './store.js';

const TAB_KEY = 'yulius.notesTab';
const ORDER_KEY = 'yulius.notesOrder';
/** The tabs' order until the person moves one: Team first (SPEC-112 NOT-015). */
export const DEFAULT_ORDER = ['team', 'sandbox', 'me'];
/** What a tab says under the pointer: the grip before its name shows it drags (SPEC-136). The keys
 *  are announced once, through `aria-keyshortcuts`, not again in this description. */
const TAB_HINT = 'Click to open, drag to reorder';
/** How far a pressed tab moves before it is dragged rather than clicked. */
const DRAG_THRESHOLD_PX = 4;
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
/** Presence (SPEC-112 NOT-020): the least time between two updates of one's own selection. */
const PRESENCE_GAP_MS = 100;
/** …sent again this often while the notepad is shown, so the others know one is still there. */
const PRESENCE_HEARTBEAT_MS = 15000;
/** A teammate not heard of for this long has gone. */
const PRESENCE_EXPIRY_MS = 45000;
/** How often teammates who went quiet are looked for. */
const PRESENCE_SWEEP_MS = 1000;
/** A refused or lost update is sent again after this, doubling up to the heartbeat. */
const PRESENCE_RETRY_MS = 1000;
/** Hellos are answered at most this often, however many people open the notepad at once. */
const PRESENCE_ANSWER_GAP_MS = 1000;
/** This browser tab, so one person's tabs are told apart (SPEC-112 NOT-020). */
const CLIENT_ID = Array.from(crypto.getRandomValues(new Uint8Array(8)), (b) => b.toString(16).padStart(2, '0')).join('');

const retryable = (error) => !error.status || error.status === 408 || error.status === 429 || error.status >= 500;
const byteLength = (text) => new TextEncoder().encode(text).length;

/** Sign-out: no unsaved notes stay in this browser tab. */
export function forgetUnsavedNotes() {
  try {
    for (const key of Object.keys(window.sessionStorage)) if (key.startsWith(UNSAVED_PREFIX)) window.sessionStorage.removeItem(key);
  } catch { /* storage unavailable: nothing kept */ }
}

/** A stored tab order when it names each notepad once, else the default one. */
export function tabOrder(stored) {
  const valid = Array.isArray(stored) && stored.length === DEFAULT_ORDER.length
    && DEFAULT_ORDER.every((key) => stored.filter((item) => item === key).length === 1);
  return valid ? [...stored] : [...DEFAULT_ORDER];
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
  const all = [
    notepad(ctx, {
      key: 'sandbox',
      label: 'This sandbox',
      textLabel: 'Sandbox notes',
      path: `/api/sandboxes/${enc(sandbox.id)}/notes`,
      putPresence: (body, options) => ctx.api.put(`/api/sandboxes/${enc(sandbox.id)}/notes/presence`, body, options),
      unsavedKey: `${UNSAVED_PREFIX}${userId}.sandbox:${sandbox.id}`,
      matches: (event) => event.scope === 'sandbox' && event.sandbox_id === sandbox.id,
      readOnly: !allows(sandbox.access, 'write') && 'Read-only. Editing this sandbox’s notes needs write access to the repository on GitHub.',
      placeholder: 'Notes for everyone working in this sandbox: links, decisions, what to try next…',
      onState: () => renderStatus(),
      isMine,
      myId: userId,
    }),
    notepad(ctx, {
      key: 'team',
      label: 'Team',
      textLabel: 'Team notes',
      path: '/api/notes',
      putPresence: (body, options) => ctx.api.put('/api/notes/presence', body, options),
      unsavedKey: `${UNSAVED_PREFIX}${userId}.team`,
      matches: (event) => event.scope === 'team',
      readOnly: false,
      placeholder: 'Notes for the whole team, the same in every sandbox…',
      onState: () => renderStatus(),
      isMine,
      myId: userId,
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
      myId: userId,
      // Nobody else saves it, whatever name an older save carries (a renamed GitHub login).
      personal: true,
    }),
  ];
  let pads = tabOrder(readJson(ORDER_KEY, null)).map((key) => all.find((pad) => pad.key === key));
  let current = pads.find((pad) => pad.key === readJson(TAB_KEY, pads[0].key)) || pads[0];
  let shown = false;
  let drag = null; // the pressed tab: {pad, tab, startX, moved, tabs, pointerId, lefts, widths, order}
  let dragged = false; // the click that ends a drag chooses nothing

  /** Draws the strip; the keyboard stays in it, on `focusPad` when given, else on the chosen tab. */
  function renderTabs(focusPad = null) {
    const focused = tablist.contains(document.activeElement);
    const tabs = pads.map((pad) => {
      const selected = pad === current;
      const tab = h('button', {
        type: 'button', class: ['term-tab', selected && 'active'], role: 'tab', id: `notes-tab-${pad.key}`,
        'aria-selected': String(selected), 'aria-controls': `notes-pane-${pad.key}`, tabindex: selected ? '0' : '-1',
        'aria-keyshortcuts': 'Alt+ArrowLeft Alt+ArrowRight', dataset: { key: pad.key }, title: TAB_HINT,
        onClick: () => { if (dragged) { dragged = false; return; } select(pad, { focusTab: true }); },
      }, h('span', { class: 'notes-tab-grip', 'aria-hidden': 'true' }, icon('grip', { size: 12 })), pad.label);
      tab.addEventListener('keydown', (event) => {
        const index = pads.indexOf(pad);
        if (event.altKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
          event.preventDefault();
          move(pad, event.key === 'ArrowLeft' ? -1 : 1);
          return;
        }
        if (event.altKey || event.ctrlKey || event.metaKey) return;
        const next = { ArrowRight: index + 1, ArrowLeft: index - 1 + pads.length, Home: 0, End: pads.length - 1 }[event.key];
        if (next === undefined) return;
        event.preventDefault();
        select(pads[next % pads.length], { focusTab: true });
      });
      // Moving a tab without dragging (WCAG 2.5.7), as the sandbox tabs do.
      tab.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        const index = pads.indexOf(pad);
        openMenu({ x: event.clientX, y: event.clientY }, [
          { label: 'Move left', icon: 'left', disabled: index === 0, onSelect: () => move(pad, -1) },
          { label: 'Move right', icon: 'right', disabled: index === pads.length - 1, onSelect: () => move(pad, 1) },
        ]);
      });
      tab.addEventListener('pointerdown', (event) => pressTab(event, pad, tab));
      return tab;
    });
    setChildren(tablist, tabs);
    if (focused || focusPad) tabs[pads.indexOf(focusPad || current)].focus();
    return tabs;
  }

  /** Puts the tabs in `order` (notepad keys) and remembers it. */
  function reorder(order, focusPad = null) {
    pads = order.map((key) => all.find((pad) => pad.key === key));
    writeJson(ORDER_KEY, order);
    return renderTabs(focusPad);
  }

  /** Moves `pad` one place left (-1) or right (1); the keyboard goes with it. */
  function move(pad, step) {
    const order = pads.map((p) => p.key);
    const from = order.indexOf(pad.key);
    const to = from + step;
    if (to < 0 || to >= order.length) return;
    order.splice(from, 1);
    order.splice(to, 0, pad.key);
    reorder(order, tablist.contains(document.activeElement) ? pad : null);
  }

  // ---- dragging a tab (mouse or pen; touch scrolls the strip and moves tabs from the menu): past
  // a few pixels the tab follows the pointer, the tabs it passes slide aside, and on release it
  // settles into its place from where it was let go.
  function pressTab(event, pad, tab) {
    if (event.button !== 0 || event.pointerType === 'touch' || drag) return;
    const tabs = [...tablist.children];
    drag = {
      pad, tab, startX: event.clientX, moved: false, tabs, pointerId: event.pointerId,
      lefts: tabs.map((t) => t.offsetLeft), widths: tabs.map((t) => t.offsetWidth), order: pads.map((p) => p.key),
    };
    window.addEventListener('pointermove', dragMove);
    window.addEventListener('pointerup', dragEnd);
    window.addEventListener('pointercancel', dragCancel);
    window.addEventListener('keydown', dragKey, true);
  }

  /** Where `state`'s tab lands moved by `dx`: the order it makes and how far each tab shifts. */
  function dragLayout(state, dx) {
    const from = pads.indexOf(state.pad);
    const centre = state.lefts[from] + state.widths[from] / 2 + dx;
    const others = pads.filter((p) => p !== state.pad);
    const slot = others.filter((p) => { const i = pads.indexOf(p); return state.lefts[i] + state.widths[i] / 2 < centre; }).length;
    const placed = [...others];
    placed.splice(slot, 0, state.pad);
    let left = state.lefts[0];
    const shifts = new Map();
    for (const p of placed) {
      const i = pads.indexOf(p);
      shifts.set(p, left - state.lefts[i]);
      left += state.widths[i];
    }
    return { order: placed.map((p) => p.key), shifts };
  }

  function dragMove(event) {
    if (!drag || event.pointerId !== drag.pointerId) return;
    const dx = event.clientX - drag.startX;
    if (!drag.moved) {
      if (Math.abs(dx) < DRAG_THRESHOLD_PX) return;
      drag.moved = true;
      tablist.classList.add('reordering');
      drag.tab.classList.add('dragging');
    }
    const { shifts } = dragLayout(drag, dx);
    drag.tabs.forEach((tab, i) => {
      tab.style.transform = `translateX(${pads[i] === drag.pad ? dx : shifts.get(pads[i])}px)`;
    });
  }

  function dragDone() {
    window.removeEventListener('pointermove', dragMove);
    window.removeEventListener('pointerup', dragEnd);
    window.removeEventListener('pointercancel', dragCancel);
    window.removeEventListener('keydown', dragKey, true);
    tablist.classList.remove('reordering');
    const state = drag;
    drag = null;
    return state;
  }

  function dragEnd(event) {
    if (!drag || event.pointerId !== drag.pointerId) return;
    const state = dragDone();
    if (!state.moved) return;
    dragged = true; // the click after this release
    setTimeout(() => { dragged = false; }, 0); // a release outside the tab clicks nothing
    const dx = event.clientX - state.startX;
    const { order } = dragLayout(state, dx);
    const from = state.lefts[state.order.indexOf(state.pad.key)] + dx;
    const tabs = reorder(order);
    const tab = tabs[order.indexOf(state.pad.key)];
    // From where it was let go into its place (FLIP).
    tab.classList.add('dragging');
    tab.style.transform = `translateX(${from - tab.offsetLeft}px)`;
    tab.getBoundingClientRect();
    tab.classList.remove('dragging');
    tab.classList.add('settling');
    tab.style.transform = '';
    tab.addEventListener('transitionend', () => tab.classList.remove('settling'), { once: true });
  }

  function dragCancel() {
    const state = dragDone();
    if (state) for (const tab of state.tabs) { tab.style.transform = ''; tab.classList.remove('dragging'); }
  }

  function dragKey(event) {
    if (event.key !== 'Escape' || !drag || !drag.moved) return;
    event.preventDefault();
    event.stopPropagation();
    dragCancel();
    // The release that follows clicks nothing (it may land back on the pressed tab).
    dragged = true;
    const released = () => {
      window.removeEventListener('pointerup', released, true);
      window.removeEventListener('pointercancel', released, true);
      setTimeout(() => { dragged = false; }, 0);
    };
    window.addEventListener('pointerup', released, true);
    window.addEventListener('pointercancel', released, true);
  }

  function select(pad, { focusTab = false } = {}) {
    if (current !== pad) {
      current.flush();
      current.setShown(false);
    }
    current = pad;
    if (shown) pad.setShown(true);
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

  for (const pad of all) {
    pad.el.id = `notes-pane-${pad.key}`;
    pad.el.setAttribute('role', 'tabpanel');
    pad.el.setAttribute('aria-labelledby', `notes-tab-${pad.key}`);
    pad.el.hidden = pad !== current;
    panes.append(pad.el);
  }
  renderTabs();

  const offs = [
    ctx.store.on('note', (event) => { for (const pad of pads) if (pad.matches(event)) pad.announced(event); }),
    ctx.store.on('resync', () => { for (const pad of pads) { pad.announced(null); pad.presenceLost(); } }),
    ctx.store.on('note-presence', (event) => { for (const pad of pads) if (pad.matches(event)) pad.peerPresence(event); }),
  ];
  // Teammates who went quiet fade out; the shown notepad says it is still there now and then.
  const sweep = setInterval(() => { for (const pad of pads) pad.tick(); }, PRESENCE_SWEEP_MS);
  // A hidden page (another tab, a closed laptop, a reload) saves what is typed at once.
  // The notepads share the browser's allowance for requests that outlive the page (64 KiB).
  const leave = () => {
    const budget = { left: KEEPALIVE_MAX_BYTES };
    for (const pad of pads) pad.flush({ leaving: true, budget });
    for (const pad of pads) pad.setShown(false, { going: true });
  };
  // A hidden page has left the notepad for its teammates; shown again, it says hello.
  const onHidden = () => {
    if (document.visibilityState === 'hidden') leave();
    else if (shown) current.setShown(true);
  };
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
      current.setShown(true);
      current.load();
      renderStatus();
      // Shown again because the page changed (panels.js `{ context: true }`): the keyboard stays
      // with the page; opened by the person, the notepad takes it (SPEC-056 WUI-103).
      if (!opts.context) setTimeout(() => { if (shown) current.focus(); }, 0);
    },
    hide() {
      shown = false;
      for (const pad of pads) pad.flush();
      for (const pad of pads) pad.setShown(false);
    },
    destroy() {
      dragCancel();
      clearInterval(sweep);
      for (const pad of pads) pad.destroy();
      offs.forEach((off) => off());
      document.removeEventListener('visibilitychange', onHidden);
      window.removeEventListener('pagehide', onPageHide);
      window.removeEventListener('beforeunload', onBeforeUnload);
    },
  };
}

/** One notepad: its text box, what the server holds, and the save loop. */
function notepad(ctx, { key, label, textLabel, path, putPresence = null, unsavedKey, matches, readOnly, placeholder, onState, isMine, myId, personal = false }) {
  const hintId = `notes-readonly-${key}`;
  const textarea = h('textarea', { class: 'notes-text', 'aria-label': textLabel, 'aria-describedby': readOnly ? hintId : null, placeholder, spellcheck: 'true', readOnly: true });
  const banner = h('div', { class: 'notes-banner', hidden: true });
  const meta = h('p', { class: 'notes-meta' });
  // Until the first read, shimmering fields cover the empty box (SPEC-092); the box keeps focus.
  const fields = skeleton('Loading the notes…', 3, { block: true });
  // The selection is drawn, rounded and gliding (SPEC-112 NOT-017).
  const layer = createTextLayer(textarea);
  const el = h('div', { class: 'notes-pane notes-loading' },
    readOnly ? h('p', { class: 'notes-readonly', id: hintId }, readOnly) : null,
    banner, h('div', { class: 'notes-box' }, layer.el, textarea, fields), meta);
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
    layer.refresh();
    settled();
    if (!readOnly) dropUnsaved(); // a reader who lost write access keeps theirs for later
    renderMeta();
    presenceMoved();
  }

  /** Goes on from `note`, this person's own stored save, keeping the text box as it is. */
  function rebase(note) {
    saved = note.body;
    base = note.revision;
    lastEdit = { updated_by: note.updated_by, updated_at: note.updated_at };
    settled();
    renderMeta();
    presenceMoved();
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
    presenceMoved();
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
    presenceMoved();
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
      presenceMoved();
      if (dirty()) { if (!timer && !destroyed) schedule(); } // typed while the save was on its way
      else { dropUnsaved(); catchUp(); }
    } catch (error) {
      saving = null;
      if (error.status === 409) {
        // No other save starts while the refusal is looked into: its own look would no longer
        // know a lost save this look goes on from (review of SPEC-112). Typing meanwhile is
        // saved after it.
        saving = resolveConflict();
        try { await saving; } finally { saving = null; }
      } else if (!retryable(error)) failed = error.message;
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
    // The person's own other tab or device (always so for "Me", SPEC-103) is not a teammate.
    const own = personal || isMine(conflict);
    const who = own
      ? 'You changed these notes in another tab or on another device'
      : `${conflict.updated_by || 'Someone'} changed these notes`;
    setChildren(banner,
      h('p', null, `${who} while you were typing. Your text is not saved yet.`),
      h('div', { class: 'row' },
        h('button', { type: 'button', class: 'btn btn-primary btn-sm', onClick: keepMine }, 'Keep mine'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: useTheirs },
          own ? 'Use that version' : 'Use theirs')));
  }

  function keepMine() {
    if (!conflict) return;
    base = conflict.revision;
    saved = conflict.body;
    conflict = null;
    renderBanner();
    presenceMoved();
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

  // ---- presence (SPEC-112 NOT-020): where this person is in a shared notepad goes to its other
  // readers, and theirs is drawn here. Offsets travel in the saved text at `base`, mapped through
  // the unsaved typing on each side; someone at another revision is hidden until both meet again.
  // Nothing goes out before the text box first has the keyboard (opening Notes gives it; Notes
  // shown again because the page changed does not), so nobody shows at the start unasked.
  const peers = new Map(); // `user id:tab` → {userId, name, revision, selection, seen}
  let present = false; // the notepad is shown
  let touched = false; // the caret was put in it
  let presenceTimer = 0;
  let lastPresenceAt = 0;
  let lastPresence = null; // the last update sent, as JSON
  let wasThere = false; // the last update sent had a selection
  let helloDue = false;
  let forceDue = false;
  let retryMs = PRESENCE_RETRY_MS;
  let lastAnswerAt = 0;
  let answerTimer = 0;
  let presenceInFlight = false;
  let presenceQueued = false; // an update waits for the one on its way

  /** The text box's own text and the saved one are the same document, unless a conflict is open. */
  const inStep = () => loaded && !conflict;

  function ownSelection() {
    if (!present || !touched || !inStep()) return null;
    const { selectionStart, selectionEnd, selectionDirection } = textarea;
    const [anchor, head] = selectionDirection === 'backward' ? [selectionEnd, selectionStart] : [selectionStart, selectionEnd];
    const edit = textEdit(textarea.value, saved);
    return { anchor: mapPosition(anchor, edit), head: mapPosition(head, edit) };
  }

  /** Sends the selection now; `leaving` sends `null`, `going` with a request that outlives the page. */
  function sendPresence({ leaving = false, going = false } = {}) {
    clearTimeout(presenceTimer);
    presenceTimer = 0;
    if (!putPresence || !loaded) return;
    // One update on its way at a time, so they reach the others in order: the newest goes when it
    // is answered (hidden by then, that is `null`). A page going away cannot wait.
    if (presenceInFlight && !going) {
      presenceQueued = true;
      return;
    }
    const selection = leaving ? null : ownSelection();
    const update = JSON.stringify({ revision: base, selection });
    // Nothing new, or nothing to take back: nothing to say, unless asked.
    if (!helloDue && !forceDue && (update === lastPresence || (!selection && !wasThere))) return;
    const hello = helloDue && !leaving;
    helloDue = false;
    forceDue = false;
    lastPresence = update;
    lastPresenceAt = Date.now();
    wasThere = Boolean(selection);
    presenceInFlight = true;
    const answered = () => {
      presenceInFlight = false;
      if (presenceQueued && !destroyed) {
        presenceQueued = false;
        sendPresence();
      }
    };
    putPresence({ revision: base, selection, hello, client: CLIENT_ID }, { keepalive: going }).then(() => {
      retryMs = PRESENCE_RETRY_MS;
      answered();
    }, () => {
      answered();
      // Refused (429) or lost: sent again later, unless something newer went out meanwhile.
      if (lastPresence !== update || leaving || destroyed) return;
      lastPresence = null;
      if (hello) helloDue = true;
      clearTimeout(presenceTimer);
      presenceTimer = setTimeout(sendPresence, retryMs);
      retryMs = Math.min(retryMs * 2, PRESENCE_HEARTBEAT_MS);
    });
  }

  /** Sends where this person is soon: at most one update per PRESENCE_GAP_MS, the last always. */
  function presenceSoon() {
    if (!putPresence || !present || presenceTimer) return;
    presenceTimer = setTimeout(sendPresence, Math.max(0, lastPresenceAt + PRESENCE_GAP_MS - Date.now()));
  }

  /** Teammates drawn in the text box's text: those at this revision, mapped through the typing. */
  function drawPeers() {
    if (!putPresence) return;
    const list = [];
    if (present && inStep()) {
      const edit = textEdit(saved, textarea.value);
      for (const [id, peer] of peers) {
        if (peer.revision !== base) continue;
        const anchor = mapPosition(peer.selection.anchor, edit);
        const head = mapPosition(peer.selection.head, edit);
        list.push({ id, colourOf: peer.userId, name: peer.name, start: Math.min(anchor, head), end: Math.max(anchor, head), head });
      }
    }
    layer.setPeers(list);
  }

  /** The text, its revision or the selection moved: redraw the others and tell them. */
  function presenceMoved() {
    drawPeers();
    presenceSoon();
  }

  /** A teammate's update (`note.presence`); this person's own, from any tab, is not drawn. */
  function peerPresence(event) {
    if (!putPresence || event.user_id === myId) return;
    const id = `${event.user_id}:${event.client || ''}`;
    if (event.selection) peers.set(id, { userId: event.user_id, name: event.name, revision: event.revision, selection: event.selection, seen: Date.now() });
    else peers.delete(id);
    // Someone opened the notepad: tell them where this person is, even if it did not change
    // (once a second at most, however many open it at once: a later hello is answered then).
    if (event.hello && present && wasThere && !answerTimer) {
      answerTimer = setTimeout(() => {
        answerTimer = 0;
        if (!present || !wasThere) return;
        lastAnswerAt = Date.now();
        forceDue = true;
        sendPresence();
      }, Math.max(0, lastAnswerAt + PRESENCE_ANSWER_GAP_MS - Date.now()));
    }
    drawPeers();
  }

  /** Events were lost (a reconnect): who is here is asked again, and nobody stays from before. */
  function presenceLost() {
    if (!putPresence || !present) return;
    peers.clear();
    helloDue = true;
    presenceSoon();
    drawPeers();
  }

  /** Shown (says hello) or hidden (leaves; `going`: the page goes, so the request outlives it). */
  function setShown(next, { going = false } = {}) {
    if (!putPresence || next === present) return;
    present = next;
    if (next) {
      helloDue = true;
      if (loaded) sendPresence();
    } else {
      helloDue = false; // a hello not sent yet has nobody to greet
      forceDue = false;
      clearTimeout(answerTimer);
      answerTimer = 0;
      sendPresence({ leaving: true, going });
      peers.clear(); // drawn again from their answers to the next hello
    }
    drawPeers();
  }

  /** Once a second: teammates gone quiet fade out; a present person says so now and then. */
  function tick() {
    if (!putPresence) return;
    const now = Date.now();
    let gone = false;
    for (const [id, peer] of peers) if (now - peer.seen > PRESENCE_EXPIRY_MS) { peers.delete(id); gone = true; }
    if (gone) drawPeers();
    if (present && wasThere && now - lastPresenceAt >= PRESENCE_HEARTBEAT_MS) {
      forceDue = true;
      sendPresence();
    }
  }

  textarea.addEventListener('input', () => {
    if (failed && failed !== 'retrying' && failed !== 'loading') failed = null; // a refusal is retried after the next edit
    if (!conflict) schedule();
    presenceMoved();
  });
  textarea.addEventListener('blur', () => flush());
  textarea.addEventListener('focus', () => { touched = true; presenceSoon(); });
  const onSelect = () => { if (document.activeElement === textarea) presenceSoon(); };
  for (const name of ['select', 'selectionchange', 'keyup', 'pointerup']) textarea.addEventListener(name, onSelect);
  document.addEventListener('selectionchange', onSelect);

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
    setShown,
    peerPresence,
    presenceLost,
    tick,
    destroy() {
      flush({ leaving: true }); // kept for the next load; the save goes on after the view is gone
      setShown(false, { going: true });
      destroyed = true;
      document.removeEventListener('selectionchange', onSelect);
      layer.destroy();
      clearTimeout(timer);
    },
  };
}
