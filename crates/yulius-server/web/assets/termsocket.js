// WebSocket protocol clients (docs/api.md "Terminal WebSocket", SPEC-002 §7.7).
// No DOM here: the terminal view and browser panel render what these clients decode.

/** Binary frame kinds. */
export const FRAME_OUTPUT = 1;
export const FRAME_SNAPSHOT = 2;
export const FRAME_BROWSER = 3;

/** Lag close code: the server dropped a slow viewer; reconnect with last_seq. */
export const CLOSE_LAGGED = 4008;
/** Access revoked (permission re-check every 60 s): reconnecting cannot succeed. */
export const CLOSE_REVOKED = 4003;
/** The server-side resource failed (e.g. the sandbox browser could not start). */
export const CLOSE_SERVER_ERROR = 1011;

/** Close codes after which reconnecting is pointless; the socket reports `failed`. */
const FINAL_CLOSE_CODES = new Set([CLOSE_REVOKED]);

/**
 * True for a status that means the session is over: task end states (closed, stopped_*,
 * failed_*), the login terminal's ok/error/cancelled and a finished shell (closed).
 */
export function isEndedStatus(status) {
  const s = String(status || '');
  return s === 'closed' || s === 'ok' || s === 'error' || s === 'cancelled' || s.startsWith('stopped') || s.startsWith('failed');
}

/** Decode a terminal binary frame: u8 kind + u64 big-endian seq + bytes. */
export function parseTerminalFrame(buffer) {
  if (!(buffer instanceof ArrayBuffer) || buffer.byteLength < 9) return null;
  const view = new DataView(buffer);
  const kind = view.getUint8(0);
  if (kind !== FRAME_OUTPUT && kind !== FRAME_SNAPSHOT) return null;
  const seq = Number(view.getBigUint64(1, false));
  return { kind, seq, bytes: new Uint8Array(buffer, 9) };
}

/** Decode a browser frame: u8 3 + 32-byte ASCII tab id (space padded) + JPEG bytes. */
export function parseBrowserFrame(buffer) {
  if (!(buffer instanceof ArrayBuffer) || buffer.byteLength < 33) return null;
  const head = new Uint8Array(buffer, 0, 33);
  if (head[0] !== FRAME_BROWSER) return null;
  let tabId = '';
  for (let i = 1; i < 33; i += 1) tabId += String.fromCharCode(head[i]);
  return { tabId: tabId.replace(/[\s\0]+$/, ''), bytes: new Uint8Array(buffer, 33) };
}

/** Exponential reconnect delay 0.5 s → 10 s with ±20 % jitter and a bounded attempt count. */
export class Backoff {
  constructor({ baseMs = 500, maxMs = 10000, maxAttempts = 40, random = Math.random } = {}) {
    this.baseMs = baseMs;
    this.maxMs = maxMs;
    this.maxAttempts = maxAttempts;
    this.random = random;
    this.attempt = 0;
  }

  nextDelay() {
    const exp = Math.min(this.maxMs, this.baseMs * 2 ** this.attempt);
    this.attempt += 1;
    const jitter = exp * 0.2 * (this.random() * 2 - 1);
    return Math.round(Math.min(this.maxMs, Math.max(this.baseMs, exp + jitter)));
  }

  get exhausted() {
    return this.attempt >= this.maxAttempts;
  }

  reset() {
    this.attempt = 0;
  }
}

/**
 * A WebSocket that reconnects with backoff. Status values:
 * connecting | open | reconnecting | failed | closed.
 * `path` is a string or a function called at each connect (e.g. to add the current terminal
 * size); `onOpen(socket, isReconnect)` sends handshakes; `onMessage(data)` receives frames.
 * `staleMs` (for a server that beats, as the event stream does): an open connection that heard
 * nothing for that long is taken for dead, closed and replaced (SPEC-191 LIV-002).
 */
export class ReconnectingSocket {
  constructor({ transport, path, onOpen, onMessage, onStatus, backoff, staleMs = 0 }) {
    this.transport = transport;
    this.path = path;
    this.onOpen = onOpen || (() => {});
    this.onMessage = onMessage || (() => {});
    this.onStatus = onStatus || (() => {});
    this.backoff = backoff || new Backoff();
    this.ws = null;
    this.timer = null;
    this.stopped = true;
    this.connectedOnce = false;
    this.status = 'closed';
    this.healthy = false;
    this.staleMs = staleMs;
    this.heardAt = 0;
    this.watch = null;
  }

  start() {
    this.stopped = false;
    this.backoff.reset();
    this.connect();
  }

  /** Reconnect now (e.g. "Retry" button), resetting the backoff. */
  retry() {
    this.stopped = false;
    this.backoff.reset();
    clearTimeout(this.timer);
    if (this.ws) {
      const old = this.ws;
      this.ws = null;
      try { old.close(1000); } catch { /* already closed */ }
    }
    this.connect();
  }

  connect() {
    clearTimeout(this.timer);
    this.timer = null;
    if (this.stopped) return;
    this.setStatus(this.connectedOnce ? 'reconnecting' : 'connecting');
    let ws;
    try {
      ws = this.transport.openSocket(typeof this.path === 'function' ? this.path() : this.path);
    } catch {
      this.scheduleReconnect(0);
      return;
    }
    ws.binaryType = 'arraybuffer';
    this.ws = ws;
    this.healthy = false;
    ws.onopen = () => {
      if (this.ws !== ws) return;
      const reconnect = this.connectedOnce;
      this.connectedOnce = true;
      this.heardAt = Date.now();
      if (this.staleMs > 0 && !this.watch) {
        this.watch = setInterval(() => this.checkAlive(this.staleMs), Math.max(50, Math.round(this.staleMs / 4)));
      }
      this.setStatus('open');
      this.onOpen(this, reconnect);
    };
    ws.onmessage = (event) => {
      if (this.ws !== ws) return;
      this.heardAt = Date.now();
      if (!this.healthy) {
        this.healthy = true;
        this.backoff.reset();
      }
      this.onMessage(event.data);
    };
    ws.onerror = () => { /* a close event always follows */ };
    ws.onclose = (event) => {
      if (this.ws !== ws) return;
      this.ws = null;
      if (this.stopped) {
        this.setStatus('closed');
        return;
      }
      if (event.code === CLOSE_LAGGED) this.backoff.reset();
      if (FINAL_CLOSE_CODES.has(event.code)) {
        this.stopped = true;
        this.setStatus('failed', { code: event.code, reason: event.reason || '' });
        return;
      }
      this.scheduleReconnect(event.code);
    };
  }

  scheduleReconnect(code) {
    if (this.stopped) return;
    if (this.backoff.exhausted) {
      this.setStatus('failed', { code });
      return;
    }
    const delay = this.backoff.nextDelay();
    this.setStatus('reconnecting', { delay, code });
    if (this.stopped) return; // a status listener may stop the socket
    this.timer = setTimeout(() => this.connect(), delay);
  }

  setStatus(status, detail = {}) {
    this.status = status;
    this.onStatus(status, detail);
  }

  get isOpen() {
    return Boolean(this.ws && this.ws.readyState === 1);
  }

  /**
   * Replaces an open connection that heard nothing for `silentMs` (a dead one looks open until a
   * close that may never come); true when it did. Only for a server that beats (`staleMs`).
   */
  checkAlive(silentMs = this.staleMs) {
    if (!this.staleMs || this.stopped || !this.isOpen || Date.now() - this.heardAt <= silentMs) return false;
    this.retry();
    return true;
  }

  /** Send a JSON object (or raw string). Returns false when not connected. */
  send(message) {
    if (!this.isOpen) return false;
    this.ws.send(typeof message === 'string' ? message : JSON.stringify(message));
    return true;
  }

  /** Stop reconnecting and close. */
  stop(code = 1000) {
    this.stopped = true;
    clearTimeout(this.timer);
    this.timer = null;
    clearInterval(this.watch);
    this.watch = null;
    if (this.ws) {
      const ws = this.ws;
      this.ws = null;
      try { ws.close(code); } catch { /* ignore */ }
    }
    this.setStatus('closed');
  }
}

// In-band error frames that end the session for this viewer. The server refuses
// unauthorized connections during the HTTP handshake; an in-band `forbidden` is a refused
// input (e.g. write access withdrawn) and only produces a notice.
const FATAL_ERRORS = new Set(['unauthenticated', 'not_found']);

/**
 * Terminal protocol client shared by task terminals, sandbox shells and the account
 * login terminal. Handlers: getSize() → {cols, rows}, onOutput(bytes), onSnapshot(bytes),
 * onState(state), onNotice(text), onExit(exit), onError(error), onStatus(status, detail).
 * With `sizeInPath`, each connect asks for `path?cols=&rows=` (the size from getSize()): the
 * server applies it before the first state and snapshot, so the terminal never appears at
 * another size first (docs/api.md, F 2026-10-09).
 */
export class TerminalSocket {
  constructor({ transport, path, handlers = {}, backoff, sizeInPath = false }) {
    this.h = handlers;
    this.lastSeq = null;
    this.inputSeq = 0;
    this.ended = false;
    this.fatal = null;
    const sized = () => {
      const size = this.h.getSize && this.h.getSize();
      if (!size || !size.cols || !size.rows) return path;
      return `${path}${path.includes('?') ? '&' : '?'}cols=${Math.round(size.cols)}&rows=${Math.round(size.rows)}`;
    };
    this.socket = new ReconnectingSocket({
      transport,
      path: sizeInPath ? sized : path,
      backoff,
      onOpen: (socket) => {
        const size = (this.h.getSize && this.h.getSize()) || { cols: 120, rows: 40 };
        socket.send({ type: 'hello', last_seq: this.lastSeq, cols: size.cols, rows: size.rows });
      },
      onMessage: (data) => (typeof data === 'string' ? this.onText(data) : this.onBinary(data)),
      onStatus: (status, detail) => {
        // After an exit frame or a fatal error, a closing socket is final, not a reconnect.
        if ((this.ended || this.fatal) && status === 'reconnecting') {
          this.socket.stop();
          return;
        }
        if (this.h.onStatus) this.h.onStatus(status, detail);
      },
    });
  }

  start() {
    this.ended = false;
    this.fatal = null;
    this.socket.start();
  }

  retry() {
    this.ended = false;
    this.fatal = null;
    this.socket.retry();
  }

  onBinary(buffer) {
    const frame = parseTerminalFrame(buffer);
    if (!frame) return;
    if (frame.kind === FRAME_SNAPSHOT) {
      this.lastSeq = frame.seq;
      if (this.h.onSnapshot) this.h.onSnapshot(frame.bytes);
      return;
    }
    // Output already applied before a reconnect is skipped (no duplicated output).
    if (this.lastSeq !== null && frame.seq <= this.lastSeq) return;
    this.lastSeq = frame.seq;
    if (this.h.onOutput) this.h.onOutput(frame.bytes);
  }

  onText(text) {
    let message;
    try { message = JSON.parse(text); } catch { return; }
    if (!message || typeof message !== 'object') return;
    switch (message.type) {
      case 'state':
        if (message.status && !isEndedStatus(message.status)) this.ended = false;
        if (this.h.onState) this.h.onState(message);
        break;
      case 'notice':
        if (this.h.onNotice) this.h.onNotice(String(message.text || ''));
        break;
      case 'exit':
        this.ended = true;
        if (this.h.onExit) this.h.onExit(message);
        break;
      case 'error':
        if (FATAL_ERRORS.has(message.code)) this.fatal = message;
        if (this.h.onError) this.h.onError(message);
        break;
      default:
        break;
    }
  }

  /** Send keyboard/paste input with a monotonically increasing client sequence. */
  sendInput(data) {
    if (!this.socket.isOpen || !data) return false;
    this.inputSeq += 1;
    return this.socket.send({ type: 'input', data, seq: this.inputSeq });
  }

  /** The size this viewer's window fits (the terminal takes it while this viewer controls it). */
  resize(cols, rows) {
    return this.socket.send({ type: 'resize', cols, rows });
  }

  /** The viewer clicked into the terminal: it controls the size now. */
  takeControl() {
    return this.socket.send({ type: 'take_control' });
  }

  /** The viewer is active (visible, focused, used recently) or idle (F 2026-10-10). */
  presence(active) {
    return this.socket.send({ type: active ? 'active' : 'idle' });
  }

  get status() {
    return this.socket.status;
  }

  close() {
    this.socket.stop();
  }
}
