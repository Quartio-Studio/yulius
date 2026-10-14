// Presence in a shared text (SPEC-112 NOT-020, SPEC-127 EDT-010): where this person is goes to the
// others who have the same notepad or file open, and theirs comes back to be drawn. One loop for
// Notes and the Editor, so they behave alike: updates at most every 100 ms (the last one always),
// again every 15 s while shown, `hello` when shown and after lost events (the others answer at
// once, at most once a second), `null` when hidden or when the page goes (with a request that
// outlives it), a refused or lost update again after 1 s, doubling up to 15 s, and one update on
// its way at a time so they arrive in order. Someone not heard of for 45 s has gone.
//
// Nothing goes out before the source has something to say: `state({leaving})` answers `null` until
// it can speak (a notepad not loaded yet), else `{revision, selection}` (selection `null`: not
// placed; not needed when `leaving`). A leave still goes out after `destroy()`, once the update on
// its way is answered, so a closed file never lingers for the others.

/** The least time between two updates of one's own selection. */
const GAP_MS = 100;
/** …sent again this often while shown, so the others know one is still there. */
const HEARTBEAT_MS = 15000;
/** Someone not heard of for this long has gone. */
const EXPIRY_MS = 45000;
/** How often people who went quiet are looked for (`tick`). */
export const PRESENCE_SWEEP_MS = 1000;
/** A refused or lost update is sent again after this, doubling up to the heartbeat. */
const RETRY_MS = 1000;
/** Hellos are answered at most this often, however many people open the text at once. */
const ANSWER_GAP_MS = 1000;
/** This browser tab, so one person's tabs are told apart. */
export const CLIENT_ID = Array.from(crypto.getRandomValues(new Uint8Array(8)), (b) => b.toString(16).padStart(2, '0')).join('');

/** A refusal worth trying again (lost, too many, the server busy); access or input refusals are not. */
const retryable = (error) => !error || !error.status || error.status === 408 || error.status === 429 || error.status >= 500;

/**
 * Presence for one shared text. `put(body, {keepalive})` sends `{revision, selection, hello,
 * client}` (the caller adds what names the text); `state()` is what to say (see above); `draw()`
 * redraws the others from `peers` (`id` → `{userId, name, revision, selection, seen}`) and
 * `present`; `myId` is this person, whose own updates (any tab) are not drawn.
 */
export function createPresence({ put, state, draw, myId }) {
  const peers = new Map();
  let present = false; // the text is shown
  let destroyed = false;
  let timer = 0;
  let lastAt = 0;
  let last = null; // the last update sent, as JSON
  let wasThere = false; // the last update sent had a selection
  let helloDue = false;
  let forceDue = false;
  let retryMs = RETRY_MS;
  let lastAnswerAt = 0;
  let answerTimer = 0;
  let inFlight = false;
  let queued = false; // an update waits for the one on its way

  /** Sends the selection now; `leaving` sends `null`, `going` with a request that outlives the page. */
  function send({ leaving = false, going = false } = {}) {
    clearTimeout(timer);
    timer = 0;
    // Gone (a closed file): only the leave that waited for the update on its way.
    const away = leaving || !present || destroyed;
    const now = state({ leaving: away });
    if (!now) return;
    // One update on its way at a time, so they reach the others in order: the newest goes when it
    // is answered (hidden by then, that is `null`). A page going away cannot wait.
    if (inFlight && !going) {
      queued = true;
      return;
    }
    const selection = away ? null : now.selection;
    const update = JSON.stringify({ revision: now.revision, selection });
    // Nothing new, or nothing to take back: nothing to say, unless asked.
    if (!helloDue && !forceDue && (update === last || (!selection && !wasThere))) return;
    const hello = helloDue && !leaving;
    helloDue = false;
    forceDue = false;
    last = update;
    lastAt = Date.now();
    wasThere = Boolean(selection);
    inFlight = true;
    const answered = () => {
      inFlight = false;
      if (queued) {
        queued = false;
        send();
      }
    };
    put({ revision: now.revision, selection, hello, client: CLIENT_ID }, { keepalive: going }).then(() => {
      retryMs = RETRY_MS;
      answered();
    }, (error) => {
      answered();
      // Refused (429) or lost: sent again later, unless something newer went out meanwhile.
      if (last !== update || leaving || destroyed || !retryable(error)) return;
      last = null;
      if (hello) helloDue = true;
      clearTimeout(timer);
      timer = setTimeout(send, retryMs);
      retryMs = Math.min(retryMs * 2, HEARTBEAT_MS);
    });
  }

  /** Sends where this person is soon: at most one update per GAP_MS, the last always. */
  function soon() {
    if (!present || timer || destroyed) return;
    timer = setTimeout(send, Math.max(0, lastAt + GAP_MS - Date.now()));
  }

  /** Someone's update; this person's own, from any tab, is not drawn. */
  function peer(event) {
    if (destroyed || event.user_id === myId) return;
    const id = `${event.user_id}:${event.client || ''}`;
    if (event.selection) peers.set(id, { userId: event.user_id, name: event.name, revision: event.revision, selection: event.selection, seen: Date.now() });
    else peers.delete(id);
    // Someone opened the text: tell them where this person is, even if it did not change (once a
    // second at most, however many open it at once: a later hello is answered then).
    if (event.hello && present && wasThere && !answerTimer) {
      answerTimer = setTimeout(() => {
        answerTimer = 0;
        if (!present || !wasThere) return;
        lastAnswerAt = Date.now();
        forceDue = true;
        send();
      }, Math.max(0, lastAnswerAt + ANSWER_GAP_MS - Date.now()));
    }
    draw();
  }

  /** Events were lost (a reconnect): who is here is asked again, and nobody stays from before. */
  function lost() {
    if (!present || destroyed) return;
    peers.clear();
    helloDue = true;
    soon();
    draw();
  }

  /** Shown (says hello) or hidden (leaves; `going`: the page goes, so the request outlives it). */
  function setShown(next, { going = false } = {}) {
    if (next === present || destroyed) return;
    present = next;
    if (next) {
      helloDue = true;
      send();
    } else {
      helloDue = false; // a hello not sent yet has nobody to greet
      forceDue = false;
      clearTimeout(answerTimer);
      answerTimer = 0;
      send({ leaving: true, going });
      peers.clear(); // drawn again from their answers to the next hello
    }
    draw();
  }

  /** Once a second: people gone quiet fade out; a present person says so now and then. */
  function tick() {
    if (destroyed) return;
    const now = Date.now();
    let gone = false;
    for (const [id, other] of peers) if (now - other.seen > EXPIRY_MS) { peers.delete(id); gone = true; }
    if (gone) draw();
    if (present && wasThere && now - lastAt >= HEARTBEAT_MS) {
      forceDue = true;
      send();
    }
  }

  return {
    peers,
    get present() { return present; },
    soon,
    peer,
    lost,
    setShown,
    tick,
    /** Leaves (with a request that outlives the page when `going`) and stops. */
    destroy({ going = true } = {}) {
      setShown(false, { going });
      destroyed = true;
      clearTimeout(answerTimer);
    },
  };
}
