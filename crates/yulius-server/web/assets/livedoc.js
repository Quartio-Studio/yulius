// One file's shared text in this browser tab (SPEC-411 EDT-016..022): the Jupiter client of
// ot.js. The server orders every edit; this tab keeps one operation on its way (`outstanding`) and
// composes what is typed meanwhile (`buffer`), sent at most every SEND_GAP_MS. Steps come back on
// the event stream as `file.live` in revision order: its own step (its person's, its tab id) is the
// acknowledgement, a teammate's is transformed past what is still unconfirmed and applied
// (`onRemote`). A gap in revisions, a lost stream or an edit unconfirmed for ACK_MS is caught up
// (`GET …/files/live`); an edit is sent again only once a catch-up showed it did not land (the
// server answers an edit sent twice with its step). A document the server no longer has is joined
// again with this tab's own text, and typing not confirmed yet is put on top of whatever text the
// document then has (transformed past the change from the text this tab last had from it).
//
// Autosave (EDT-019): the server writes the shared text when asked. This tab asks AUTOSAVE_MS
// after its person's last keystroke, at once on `flush()` (focus or the file left, Ctrl+S), and
// FALLBACK_SAVE_MS after a teammate's last step (a closed browser's typing). Never while the disk
// moved on under unsaved text (a conflict, EDT-020): then only `save({force})` (Keep mine) or
// `disk({force})` (Take disk version), and never for a folder it may not change. `close()` keeps
// the document until this tab's typing is confirmed and saved (a closed file, a switched folder).

import { apply, compose, replacement, transform, transformIndex, isNoop } from './livetext.js';

/** The least time between two edits sent. */
export const SEND_GAP_MS = 50;
/** An edit unconfirmed this long is caught up. */
export const ACK_MS = 5000;
/** The text is saved this long after the person's last keystroke. */
export const AUTOSAVE_MS = 1000;
/** …and this long after a teammate's last step, when nobody saved it meanwhile. */
export const FALLBACK_SAVE_MS = 3000;
/** A closed file's document waits at most this long for its typing to be confirmed and saved. */
export const CLOSE_MS = 10_000;
/** A refused or lost request is tried again after this. */
const RETRY_MS = 1000;
/** Steps remembered for placing teammates who are a little behind. */
const LOG_STEPS = 200;
/** An edit this small also goes with `keepalive`, so it outlives a closing page (64 KiB budget). */
const KEEPALIVE_CHARS = 16 * 1024;
/** Documents opened in this page: each gets a tab id of its own (`<page id>d<n>`). */
let documents = 0;

/**
 * `api`, `sid` (encoded), `root`, `path`, `client` (this tab), `user` (this person's id);
 * `canWrite()`; `getText()` this tab's text now; `onRemote(op)` applies a teammate's (transformed)
 * step to it; `onReset(text, eol)` replaces it (a new document or a lost one); `onState()` the saved
 * state or the conflict changed; `onSynced()` nothing of this tab's is unconfirmed any more
 * (presence may speak).
 */
export function createLiveDoc({ api, sid, root, path, client: page, user, canWrite, getText, onRemote, onReset, onState, onSynced }) {
  // Its own id, and a new one once typing was given up (an edit of the old one landing late is
  // then nobody's acknowledgement, and never taken for a new edit sent twice).
  const fresh = () => `${page}d${(documents += 1).toString(36)}`;
  let client = fresh();
  let session = null;
  let revision = 0;
  let shadow = ''; // the document's text at `revision`, as this tab has it
  let outstanding = null;
  let buffer = null;
  let sentAt = 0;
  let sendTimer = 0;
  let ackTimer = 0;
  let retryTimer = 0;
  let saveTimer = 0;
  let saveIsFallback = false; // the timer is a teammate's fallback, not this person's own
  let saveWanted = false; // a save waits for this tab's edits to be confirmed
  let saving = null;
  let catching = null;
  let catchAgain = false;
  let joining = null;
  let destroyed = false;
  let readOnly = false;
  let closing = null; // {resolve, promise, timer}: close() waits for this tab's typing to be saved
  const log = []; // {revision, op}: the steps, as the server made them
  const state = { saved: true, sha: '', diskSha: null, eol: '\n' };

  const base = () => ({ root, path });
  const pending = () => Boolean(outstanding || buffer);
  const unconfirmed = () => (outstanding && buffer ? compose(outstanding, buffer) : outstanding || buffer);

  /** A closing document is done once nothing of this tab's waits to be confirmed or written. */
  function settle() {
    if (!closing || pending() || saving || saveWanted) return;
    if (!(state.saved || state.diskSha != null || readOnly || !canWrite())) return;
    clearTimeout(closing.timer);
    const { resolve } = closing;
    closing = null;
    resolve();
  }

  function takeState(doc) {
    const changed = state.saved !== doc.saved || state.sha !== doc.sha256 || state.diskSha !== (doc.disk_sha256 ?? null);
    state.saved = doc.saved;
    state.sha = doc.sha256;
    state.diskSha = doc.disk_sha256 ?? null;
    if (changed) onState();
    // Unsaved text nobody is saving (left by a browser that closed): this tab saves it.
    if (!state.saved && state.diskSha == null && !saveTimer && !saving) schedule(FALLBACK_SAVE_MS, true);
    settle();
  }

  /**
   * A whole document: this tab's text becomes its text, with this tab's typing on top where it is
   * known not to be in it. `sent`: the text a join sent (the document may have started from it:
   * then only the typing since); else the typing not confirmed yet goes on top, transformed past
   * the change from `shadow`, unless an edit of it is on its way and may have landed already
   * (`keep: false`, or a catch-up's whole text): that typing is given up.
   */
  function takeWhole(doc, { sent = null, keep = true } = {}) {
    let mine = null;
    let before = shadow;
    if (doc.text != null && doc.text !== getText()) {
      if (sent != null && doc.text === sent) {
        before = sent;
        mine = replacement(sent, getText());
      } else if (sent != null || (keep && !outstanding)) {
        mine = unconfirmed();
      }
    }
    if (!mine && pending()) client = fresh();
    session = doc.session;
    revision = doc.revision;
    outstanding = null;
    buffer = null;
    log.length = 0;
    state.eol = doc.eol || '\n';
    clearTimeout(ackTimer);
    if (doc.text != null) shadow = doc.text;
    let text = doc.text;
    if (mine) {
      try {
        const [kept] = transform(mine, replacement(before, doc.text));
        text = apply(doc.text, kept);
        buffer = isNoop(kept) ? null : kept;
      } catch { buffer = null; }
    }
    if (text != null && text !== getText()) onReset(text, state.eol);
    takeState(doc);
    if (buffer) send();
    else onSynced();
  }

  /** Joins the file's document (`own`: start it from this tab's text, when nobody edits it). */
  function join(own = null) {
    if (joining) return joining;
    const body = { ...base(), client };
    // Its own text in its own line ending; else the server takes the disk's.
    if (own) Object.assign(body, own, { eol: state.eol });
    joining = api.post(`/api/sandboxes/${sid}/files/live`, body).then((doc) => {
      if (!destroyed) takeWhole(doc, { sent: own ? own.text : null });
      return doc;
    }).finally(() => { joining = null; });
    return joining;
  }

  /** The server lost the document (restart, idle): joined again with this tab's text. */
  function rejoin() {
    if (destroyed) return;
    session = null;
    join({ text: getText(), sha256: state.sha || undefined, saved: state.saved && !pending() }).catch(() => {
      if (!destroyed) retryTimer = setTimeout(rejoin, RETRY_MS);
    });
  }

  /** A step of the server's, in order. */
  function step({ revision: made, user_id: by, client: from, op }) {
    if (made <= revision) return;
    if (from === client && (user == null || by === user) && outstanding) {
      outstanding = null;
      clearTimeout(ackTimer);
    } else {
      let theirs = op;
      if (outstanding) [outstanding, theirs] = transform(outstanding, theirs);
      if (buffer) [buffer, theirs] = transform(buffer, theirs);
      if (!isNoop(theirs)) onRemote(theirs);
      if (!saveTimer || saveIsFallback) schedule(FALLBACK_SAVE_MS, true);
    }
    try {
      shadow = apply(shadow, op);
    } catch {
      // Out of step with the server: its whole text again.
      resync();
      return;
    }
    revision = made;
    log.push({ revision: made, op });
    if (log.length > LOG_STEPS) log.shift();
    if (!outstanding) {
      if (buffer) send();
      else {
        onSynced();
        if (saveWanted) save();
      }
    }
    settle();
  }

  function send() {
    clearTimeout(sendTimer);
    sendTimer = 0;
    if (destroyed || outstanding || !buffer || !session || readOnly) return;
    const wait = sentAt + SEND_GAP_MS - Date.now();
    if (wait > 0) { sendTimer = setTimeout(send, wait); return; }
    outstanding = buffer;
    buffer = null;
    sentAt = Date.now();
    post();
  }

  /** Catches up, then sends `sent` again if it is still unconfirmed (it did not land). */
  function recheck(sent) {
    catchUp().then((caught) => {
      if (destroyed || outstanding !== sent) return;
      if (caught) post();
      else retryTimer = setTimeout(() => recheck(sent), RETRY_MS);
    });
  }

  /** Sends `outstanding` (`save`: the server saves it too, as the page goes). */
  function post({ save: andSave = false } = {}) {
    const sent = outstanding;
    const at = session;
    clearTimeout(ackTimer);
    ackTimer = setTimeout(() => { if (outstanding === sent) recheck(sent); }, ACK_MS);
    const keepalive = JSON.stringify(sent).length < KEEPALIVE_CHARS;
    const body = { ...base(), session: at, client, revision, op: sent };
    if (andSave) body.save = true;
    api.post(`/api/sandboxes/${sid}/files/live/edit`, body, keepalive ? { keepalive: true } : undefined).catch((error) => {
      if (destroyed || outstanding !== sent || session !== at) return;
      const status = error && error.status;
      if (status === 409) rejoin();
      else if (status === 403) { readOnly = true; onState(); settle(); } else if (status === 422) resync();
      // Lost or refused for now: the steps say whether it landed; sent again if not.
      else retryTimer = setTimeout(() => recheck(sent), RETRY_MS);
    });
  }

  /** This tab's text is out of step: the server's text replaces it. */
  function resync() {
    api.get(`/api/sandboxes/${sid}/files/live?${new URLSearchParams({ ...base(), session: '-', since: '0' })}`)
      .then((doc) => { if (!destroyed) takeWhole(doc, { keep: false }); }, () => rejoin());
  }

  /**
   * Catches up the steps after `revision` (one catch-up at a time; one more when asked meanwhile).
   * Answers whether it got the server's answer.
   */
  function catchUp() {
    if (catching) { catchAgain = true; return catching; }
    if (!session) return Promise.resolve(false);
    catching = api.get(`/api/sandboxes/${sid}/files/live?${new URLSearchParams({ ...base(), session, since: String(revision) })}`).then((doc) => {
      if (destroyed) return false;
      if (doc.session !== session) { rejoin(); return false; }
      if (!doc.steps) { takeWhole(doc, { keep: false }); return true; }
      for (const each of doc.steps) step(each);
      takeState(doc);
      return true;
    }, (error) => {
      if (error && error.status === 409) rejoin();
      return false;
    }).finally(() => {
      catching = null;
      if (catchAgain) { catchAgain = false; catchUp(); }
    });
    return catching;
  }

  function schedule(ms, fallback = false) {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => { saveTimer = 0; save(); }, ms);
    saveIsFallback = fallback;
  }

  /** Writes the shared text (`force`: over a disk that moved on, Keep mine). */
  function save({ force = false, keepalive = false } = {}) {
    clearTimeout(saveTimer);
    saveTimer = 0;
    if (destroyed || !session || readOnly || !canWrite()) { saveWanted = false; settle(); return Promise.resolve(); }
    if (state.diskSha != null && !force) { saveWanted = false; settle(); return Promise.resolve(); }
    // This tab's typing first: the save waits until the server has it.
    if (pending()) { saveWanted = true; return Promise.resolve(); }
    saveWanted = false;
    if (state.saved && !force) { settle(); return Promise.resolve(); }
    if (saving && !force) { saveWanted = true; return saving; }
    const at = session;
    const request = api.post(`/api/sandboxes/${sid}/files/live/save`, { ...base(), session: at, force }, keepalive ? { keepalive: true } : undefined).then((done) => {
      if (!destroyed && session === at && done.revision >= revision) takeState(done);
      return done;
    }, (error) => {
      if (destroyed || session !== at) return;
      if (error && error.status === 409) { rejoin(); return; }
      throw error;
    }).finally(() => {
      if (saving === request) { saving = null; onState(); }
      if (saveWanted && !pending()) save();
      settle();
    });
    saving = request;
    onState();
    return request;
  }

  /**
   * Takes the disk's newer version (`force`: over unsaved text, Take disk version). A refusal
   * other than an ended document (the file no longer text) is the caller's to tell.
   */
  function disk({ force = false } = {}) {
    if (destroyed || !session) return Promise.resolve();
    const at = session;
    return api.post(`/api/sandboxes/${sid}/files/live/disk`, { ...base(), session: at, force }).then((done) => {
      if (destroyed || session !== at) return;
      // Its step comes on the stream; a newer revision than this tab's is caught up there.
      if (done.revision > revision) catchUp();
      else takeState(done);
    }, (error) => {
      if (destroyed) return;
      if (error && error.status === 409) { rejoin(); return; }
      throw error;
    });
  }

  return {
    get session() { return session; },
    get revision() { return revision; },
    get state() { return state; },
    get readOnly() { return readOnly; },
    /** A save is on its way. */
    get saving() { return Boolean(saving); },
    /** This tab's edits are all confirmed (its text is the server's at `revision`). */
    get synced() { return !pending(); },
    /** Saved for everyone and nothing of this tab's on its way. */
    get saved() { return state.saved && !pending(); },
    /** Something of this tab's is not confirmed or written yet (a page leaving would lose it). */
    get busy() { return pending() || Boolean(saving) || saveWanted; },
    join,
    /** The person typed `op` (on this tab's text before it). */
    local(op) {
      if (destroyed || isNoop(op)) return;
      buffer = buffer ? compose(buffer, op) : op;
      if (canWrite()) schedule(AUTOSAVE_MS);
      onState();
      send();
    },
    /** A `file.live` event for this file. */
    event(event) {
      if (destroyed || !session) return;
      if (event.session !== session) { rejoin(); return; }
      // A step announced without its operation (too large to carry), or one after a gap.
      if (event.revision > revision + (event.op ? 1 : 0)) { catchUp(); return; }
      // The disk's line endings changed: the whole text again, in them.
      if (event.op && event.eol && event.eol !== state.eol) { resync(); return; }
      if (event.op) step(event);
      if (event.revision === revision) takeState({ saved: event.saved, sha256: event.sha256, disk_sha256: event.disk_sha256 });
    },
    /** This tab's text is out of step with the shared text: the server's replaces it. */
    resync,
    /** Events may have been lost. */
    lost() { if (session) catchUp(); },
    /**
     * Saves now (focus or the file left, Ctrl+S). As the page goes (`keepalive`) it cannot wait for
     * an answer: the typing goes at once and the server saves it with it (an edit on its way is
     * sent again for that; the server applies it once). Typing behind an edit on its way is lost
     * then, unless a teammate's Editor or the next one to open the file saves it.
     */
    flush({ keepalive = false } = {}) {
      if (keepalive && session && !readOnly && canWrite() && pending()) {
        if (!outstanding) { outstanding = buffer; buffer = null; sentAt = Date.now(); }
        post({ save: true });
        return Promise.resolve();
      }
      if (buffer && !outstanding) send();
      return save({ keepalive });
    },
    save,
    disk,
    /** Saves what this tab typed and resolves once it is confirmed and written (or CLOSE_MS). */
    close() {
      if (closing) return closing.promise;
      let resolve;
      const promise = new Promise((done) => { resolve = done; });
      const timer = setTimeout(() => { if (closing) { const done = closing.resolve; closing = null; done(); } }, CLOSE_MS);
      closing = { resolve, promise, timer };
      if (buffer && !outstanding) send();
      save();
      settle();
      return promise;
    },
    /**
     * Where offset `offset` of the text at `at` (a teammate's revision) is in this tab's text;
     * `null` when this tab has not had that revision yet or no longer remembers the steps since.
     */
    place(offset, at) {
      if (at > revision) return null;
      if (at < revision && (!log.length || log[0].revision > at + 1)) return null;
      let moved = offset;
      for (const each of log) if (each.revision > at) moved = transformIndex(moved, each.op);
      if (outstanding) moved = transformIndex(moved, outstanding);
      if (buffer) moved = transformIndex(moved, buffer);
      return moved;
    },
    destroy() {
      destroyed = true;
      for (const timer of [sendTimer, ackTimer, retryTimer, saveTimer]) clearTimeout(timer);
      if (closing) { clearTimeout(closing.timer); closing.resolve(); closing = null; }
    },
  };
}
