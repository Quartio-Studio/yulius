// One file's shared text in this browser tab (SPEC-411 EDT-016..022): the Jupiter client of
// ot.js. The server orders every edit; this tab keeps one operation on its way (`outstanding`) and
// composes what is typed meanwhile (`buffer`), sent at most every SEND_GAP_MS. Steps come back on
// the event stream as `file.live` in revision order: its own step is the acknowledgement, a
// teammate's is transformed past what is still unconfirmed and applied (`onRemote`). A gap in
// revisions, a lost stream or an edit unconfirmed for ACK_MS is caught up (`GET …/files/live`); a
// document the server no longer has is joined again with this tab's own text.
//
// Autosave (EDT-019): the server writes the shared text when asked. This tab asks AUTOSAVE_MS
// after its person's last keystroke, at once on `flush()` (focus or the file left, Ctrl+S), and
// FALLBACK_SAVE_MS after a teammate's last step (a closed browser's typing). Never while the disk
// moved on under unsaved text (a conflict, EDT-020): then only `save({force})` (Keep mine) or
// `disk({force})` (Take disk version), and never for a folder it may not change.

import { compose, transform, transformIndex, isNoop } from './livetext.js';

/** The least time between two edits sent. */
export const SEND_GAP_MS = 50;
/** An edit unconfirmed this long is caught up. */
export const ACK_MS = 5000;
/** The text is saved this long after the person's last keystroke. */
export const AUTOSAVE_MS = 1000;
/** …and this long after a teammate's last step, when nobody saved it meanwhile. */
export const FALLBACK_SAVE_MS = 3000;
/** A refused or lost request is tried again after this. */
const RETRY_MS = 1000;
/** Steps remembered for placing teammates who are a little behind. */
const LOG_STEPS = 200;

/**
 * `api`, `sid` (encoded), `root`, `path`, `client` (this tab); `canWrite()`; `getText()` this
 * tab's text now; `onRemote(op)` applies a teammate's (transformed) step to it; `onReset(text,
 * eol)` replaces it (a new document or a lost one); `onState()` the saved state or the conflict
 * changed; `onSynced()` nothing of this tab's is unconfirmed any more (presence may speak).
 */
export function createLiveDoc({ api, sid, root, path, client, canWrite, getText, onRemote, onReset, onState, onSynced }) {
  let session = null;
  let revision = 0;
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
  const log = []; // {revision, op}: the steps, as the server made them
  const state = { saved: true, sha: '', diskSha: null, eol: '\n' };

  const base = () => ({ root, path });
  const pending = () => Boolean(outstanding || buffer);

  function takeState(doc) {
    const changed = state.saved !== doc.saved || state.sha !== doc.sha256 || state.diskSha !== (doc.disk_sha256 ?? null);
    state.saved = doc.saved;
    state.sha = doc.sha256;
    state.diskSha = doc.disk_sha256 ?? null;
    if (changed) onState();
    // Unsaved text nobody is saving (left by a browser that closed): this tab saves it.
    if (!state.saved && state.diskSha == null && !saveTimer && !saving) schedule(FALLBACK_SAVE_MS, true);
  }

  /** A whole document: this tab's text becomes its text. */
  function takeWhole(doc) {
    session = doc.session;
    revision = doc.revision;
    outstanding = null;
    buffer = null;
    log.length = 0;
    state.eol = doc.eol || '\n';
    clearTimeout(ackTimer);
    if (doc.text != null && doc.text !== getText()) onReset(doc.text, state.eol);
    takeState(doc);
    onSynced();
  }

  /** Joins the file's document (`own`: start it from this tab's text, when nobody edits it). */
  function join(own = null) {
    if (joining) return joining;
    const body = { ...base(), client, eol: state.eol };
    if (own) Object.assign(body, own);
    joining = api.post(`/api/sandboxes/${sid}/files/live`, body).then((doc) => {
      if (!destroyed) takeWhole(doc);
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
  function step({ revision: made, client: from, op }) {
    if (made <= revision) return;
    if (from === client && outstanding) {
      outstanding = null;
      clearTimeout(ackTimer);
    } else {
      let theirs = op;
      if (outstanding) [outstanding, theirs] = transform(outstanding, theirs);
      if (buffer) [buffer, theirs] = transform(buffer, theirs);
      if (!isNoop(theirs)) onRemote(theirs);
      if (!saveTimer || saveIsFallback) schedule(FALLBACK_SAVE_MS, true);
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

  function post({ keepalive = false } = {}) {
    const sent = outstanding;
    const at = session;
    clearTimeout(ackTimer);
    ackTimer = setTimeout(() => { if (outstanding === sent) catchUp(); }, ACK_MS);
    api.post(`/api/sandboxes/${sid}/files/live/edit`, { ...base(), session: at, client, revision, op: sent }, keepalive ? { keepalive: true } : undefined).catch((error) => {
      if (destroyed || outstanding !== sent || session !== at) return;
      const status = error && error.status;
      if (status === 409) rejoin();
      else if (status === 403) { readOnly = true; onState(); } else if (status === 422) resync();
      // Lost or refused for now: the steps say whether it landed; sent again if not.
      else retryTimer = setTimeout(() => catchUp().then(() => { if (outstanding === sent) post(); }), RETRY_MS);
    });
  }

  /** This tab's text is out of step: the server's text replaces it. */
  function resync() {
    api.get(`/api/sandboxes/${sid}/files/live?${new URLSearchParams({ ...base(), session: '-', since: '0' })}`)
      .then((doc) => { if (!destroyed) takeWhole(doc); }, () => rejoin());
  }

  /** Catches up the steps after `revision` (one catch-up at a time; one more when asked meanwhile). */
  function catchUp() {
    if (catching) { catchAgain = true; return catching; }
    if (!session) return Promise.resolve();
    catching = api.get(`/api/sandboxes/${sid}/files/live?${new URLSearchParams({ ...base(), session, since: String(revision) })}`).then((doc) => {
      if (destroyed) return;
      if (doc.session !== session) { rejoin(); return; }
      if (!doc.steps) { takeWhole(doc); return; }
      for (const each of doc.steps) step(each);
      takeState(doc);
    }, (error) => { if (error && error.status === 409) rejoin(); }).finally(() => {
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
    if (destroyed || !session || readOnly || !canWrite()) return Promise.resolve();
    if (state.diskSha != null && !force) return Promise.resolve();
    // This tab's typing first: the save waits until the server has it.
    if (pending() && !keepalive) { saveWanted = true; return Promise.resolve(); }
    saveWanted = false;
    if (state.saved && !force) return Promise.resolve();
    if (saving && !force && !keepalive) { saveWanted = true; return saving; }
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
    });
    saving = request;
    onState();
    return request;
  }

  /** Takes the disk's newer version (`force`: over unsaved text, Take disk version). */
  function disk({ force = false } = {}) {
    if (destroyed || !session) return Promise.resolve();
    const at = session;
    return api.post(`/api/sandboxes/${sid}/files/live/disk`, { ...base(), session: at, force }).then((done) => {
      if (destroyed || session !== at) return;
      // Its step comes on the stream; a newer revision than this tab's is caught up there.
      if (done.revision > revision) catchUp();
      else takeState(done);
    }, (error) => {
      if (!destroyed && error && error.status === 409) rejoin();
      if (force) throw error;
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
      if (event.revision > revision + (event.op ? 1 : 0)) { catchUp(); return; }
      if (event.op) step(event);
      if (event.revision === revision) takeState({ saved: event.saved, sha256: event.sha256, disk_sha256: event.disk_sha256 });
    },
    /** This tab's text is out of step with the shared text: the server's replaces it. */
    resync,
    /** Events may have been lost. */
    lost() { if (session) catchUp(); },
    /** Saves now (focus or the file left, Ctrl+S, the page goes: `keepalive`). */
    flush({ keepalive = false } = {}) {
      if (keepalive && buffer && !outstanding) { outstanding = buffer; buffer = null; post({ keepalive: true }); }
      return save({ keepalive });
    },
    save,
    disk,
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
    },
  };
}
