// Mouse-tracking filter for terminal output. TUIs such as Claude Code switch on mouse reporting
// (DECSET 1000/1002/1003/1006 …); xterm.js then sends clicks to the program and a plain drag no
// longer selects text. A viewer who can write hands the mouse to the program, as a native terminal
// does (SPEC-098); a read-only viewer cannot send anything, so for it the filter removes those
// switches from the output stream, remembering what the program asked for, and its mouse keeps
// selecting text (F 2026-10-07). Sequences split across chunks are carried over (bounded). Pure
// module: no DOM access.

const ESC = 0x1b;
const LBRACKET = 0x5b; // [
const QUESTION = 0x3f; // ?
const SEMICOLON = 0x3b;
const H = 0x68;
const L = 0x6c;
/** Private modes that turn mouse reporting on or change its encoding. */
const MOUSE_MODES = new Set([9, 1000, 1001, 1002, 1003, 1005, 1006, 1015, 1016]);
/** Longest private-mode sequence considered (anything longer passes through untouched). */
const MAX_SEQUENCE = 64;
const EMPTY = new Uint8Array(0);
const encoder = new TextEncoder();

function concat(parts) {
  let length = 0;
  for (const part of parts) length += part.length;
  const out = new Uint8Array(length);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

/**
 * Scans `ESC [ ? Pm h|l` at `start`. Returns {status: 'no'} when it is something else,
 * {status: 'partial'} when the data ends inside a possible match, or
 * {status: 'match', end, params, set} with the decimal parameters.
 */
function scan(data, start) {
  if (start + 1 >= data.length) return { status: 'partial' };
  if (data[start + 1] !== LBRACKET) return { status: 'no' };
  if (start + 2 >= data.length) return { status: 'partial' };
  if (data[start + 2] !== QUESTION) return { status: 'no' };
  const params = [];
  let current = '';
  for (let i = start + 3; i < data.length; i++) {
    if (i - start > MAX_SEQUENCE) return { status: 'no' };
    const byte = data[i];
    if (byte >= 0x30 && byte <= 0x39) {
      current += String.fromCharCode(byte);
    } else if (byte === SEMICOLON) {
      if (!current) return { status: 'no' };
      params.push(Number(current));
      current = '';
    } else if (byte === H || byte === L) {
      if (!current) return { status: 'no' };
      params.push(Number(current));
      return { status: 'match', end: i + 1, params, set: byte === H };
    } else {
      return { status: 'no' };
    }
  }
  return { status: 'partial' };
}

function sequence(params, set) {
  return encoder.encode(`\x1b[?${params.join(';')}${set ? 'h' : 'l'}`);
}

/**
 * A stateful filter. feed(bytes) → bytes for xterm; reset() after a snapshot;
 * setAppMouse(on) → bytes that switch xterm's mouse reporting to match.
 */
export function createMouseFilter() {
  let carry = EMPTY;
  let appMouse = false;
  const requested = new Set();

  function feed(chunk) {
    const data = carry.length ? concat([carry, chunk]) : chunk;
    carry = EMPTY;
    const parts = [];
    let copyFrom = 0;
    let i = 0;
    while (i < data.length) {
      if (data[i] !== ESC) {
        i += 1;
        continue;
      }
      const found = scan(data, i);
      if (found.status === 'partial') {
        parts.push(data.subarray(copyFrom, i));
        carry = data.slice(i);
        copyFrom = data.length;
        break;
      }
      if (found.status === 'no') {
        i += 1;
        continue;
      }
      const mouse = found.params.filter((p) => MOUSE_MODES.has(p));
      if (mouse.length) {
        for (const mode of mouse) {
          if (found.set) requested.add(mode);
          else requested.delete(mode);
        }
        parts.push(data.subarray(copyFrom, i));
        const kept = appMouse ? found.params : found.params.filter((p) => !MOUSE_MODES.has(p));
        if (kept.length) parts.push(sequence(kept, found.set));
        copyFrom = found.end;
      }
      i = found.end;
    }
    if (copyFrom < data.length) parts.push(data.subarray(copyFrom));
    return parts.length === 1 ? parts[0] : concat(parts);
  }

  return {
    feed,
    /** The screen was replaced by a snapshot, which carries the current modes again (#78). */
    reset() { carry = EMPTY; requested.clear(); },
    appMouse: () => appMouse,
    /** Hands the mouse to the program (true) or keeps it for text selection (false). */
    setAppMouse(on) {
      appMouse = on;
      if (!requested.size) return EMPTY;
      return sequence([...requested].sort((a, b) => a - b), on);
    },
  };
}
