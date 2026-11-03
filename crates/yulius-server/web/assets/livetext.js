// Text operations of the Editor's shared text (SPEC-411 EDT-016), as the server's `live_text.rs`:
// an operation is an array of parts over UTF-16 units — a positive number keeps that many, a
// negative one deletes that many, a string inserts itself — covering the whole text it applies to.
// `transform(a, b)` puts `a`'s insert first when both insert at one place; the server passes the
// incoming operation as `a`, this tab its own unconfirmed one, so both order such inserts alike.

const isRetain = (part) => typeof part === 'number' && part > 0;
const isDelete = (part) => typeof part === 'number' && part < 0;
const isInsert = (part) => typeof part === 'string';

/** A normalized operation built part by part (no empty parts, an insert before an adjacent delete). */
export function builder() {
  const ops = [];
  const self = {
    ops,
    retain(n) {
      if (n <= 0) return self;
      if (isRetain(ops[ops.length - 1])) ops[ops.length - 1] += n;
      else ops.push(n);
      return self;
    },
    insert(text) {
      if (!text) return self;
      const last = ops[ops.length - 1];
      if (isInsert(last)) ops[ops.length - 1] += text;
      else if (isDelete(last)) {
        if (isInsert(ops[ops.length - 2])) ops[ops.length - 2] += text;
        else ops.splice(ops.length - 1, 0, text);
      } else ops.push(text);
      return self;
    },
    delete(n) {
      if (n <= 0) return self;
      if (isDelete(ops[ops.length - 1])) ops[ops.length - 1] -= n;
      else ops.push(-n);
      return self;
    },
  };
  return self;
}

/** Length of the text `op` applies to. */
export function baseLength(op) {
  return op.reduce((sum, part) => sum + (isInsert(part) ? 0 : Math.abs(part)), 0);
}

/** Length of the text `op` makes. */
export function targetLength(op) {
  return op.reduce((sum, part) => sum + (isInsert(part) ? part.length : isRetain(part) ? part : 0), 0);
}

/** Whether `op` changes nothing. */
export const isNoop = (op) => op.every(isRetain);

/** The text after `op`; throws when `op` does not fit `text`. */
export function apply(text, op) {
  if (baseLength(op) !== text.length) throw new Error('the operation does not fit the text');
  let out = '';
  let at = 0;
  for (const part of op) {
    if (isRetain(part)) { out += text.slice(at, at + part); at += part; } else if (isInsert(part)) out += part;
    else at -= part;
  }
  return out;
}

/** `[a', b']` for `a` and `b` made on one text: `a'` applies after `b`, `b'` after `a`. */
export function transform(a, b) {
  if (baseLength(a) !== baseLength(b)) throw new Error('the operations are not on one text');
  const a2 = builder();
  const b2 = builder();
  let i = 0;
  let j = 0;
  let x = a[i++];
  let y = b[j++];
  while (x !== undefined || y !== undefined) {
    if (isInsert(x)) { a2.insert(x); b2.retain(x.length); x = a[i++]; continue; }
    if (isInsert(y)) { a2.retain(y.length); b2.insert(y); y = b[j++]; continue; }
    if (x === undefined || y === undefined) throw new Error('the operations are not on one text');
    const n = Math.min(Math.abs(x), Math.abs(y));
    if (isRetain(x) && isRetain(y)) { a2.retain(n); b2.retain(n); } else if (isDelete(x) && isRetain(y)) a2.delete(n);
    else if (isRetain(x) && isDelete(y)) b2.delete(n);
    // Both delete it: gone already for each.
    x = Math.abs(x) > n ? (x > 0 ? x - n : x + n) : a[i++];
    y = Math.abs(y) > n ? (y > 0 ? y - n : y + n) : b[j++];
  }
  return [a2.ops, b2.ops];
}

/** One operation doing `a` then `b`. */
export function compose(a, b) {
  if (targetLength(a) !== baseLength(b)) throw new Error('the operations do not follow each other');
  const out = builder();
  let i = 0;
  let j = 0;
  let x = a[i++];
  let y = b[j++];
  while (x !== undefined || y !== undefined) {
    if (isDelete(x)) { out.delete(-x); x = a[i++]; continue; }
    if (isInsert(y)) { out.insert(y); y = b[j++]; continue; }
    if (x === undefined || y === undefined) throw new Error('the operations do not follow each other');
    if (isRetain(x) && isRetain(y)) {
      const n = Math.min(x, y);
      out.retain(n);
      x = x > n ? x - n : a[i++];
      y = y > n ? y - n : b[j++];
    } else if (isInsert(x) && isDelete(y)) {
      const n = Math.min(x.length, -y);
      x = x.length > n ? x.slice(n) : a[i++];
      y = -y > n ? y + n : b[j++];
    } else if (isInsert(x) && isRetain(y)) {
      const n = Math.min(x.length, y);
      out.insert(x.slice(0, n));
      x = x.length > n ? x.slice(n) : a[i++];
      y = y > n ? y - n : b[j++];
    } else {
      // A kept part deleted.
      const n = Math.min(x, -y);
      out.delete(n);
      x = x > n ? x - n : a[i++];
      y = -y > n ? y + n : b[j++];
    }
  }
  return out.ops;
}

/** Where offset `index` of the text before `op` is after it (an insert at it pushes it on). */
export function transformIndex(index, op) {
  let rest = index;
  let moved = index;
  for (const part of op) {
    if (rest < 0) break;
    if (isRetain(part)) rest -= part;
    else if (isInsert(part)) moved += part.length;
    else { moved -= Math.min(rest, -part); rest += part; }
  }
  return moved;
}

/**
 * The operation of Monaco's `changes` (one content change event: offsets in the text before it,
 * not overlapping) on a text that is `length` long after them.
 */
export function fromChanges(changes, length) {
  const sorted = [...changes].sort((p, q) => p.rangeOffset - q.rangeOffset);
  const before = length - sorted.reduce((sum, c) => sum + c.text.length - c.rangeLength, 0);
  const op = builder();
  let at = 0;
  for (const change of sorted) {
    op.retain(change.rangeOffset - at).delete(change.rangeLength).insert(change.text);
    at = change.rangeOffset + change.rangeLength;
  }
  op.retain(before - at);
  return op.ops;
}

/**
 * The edits of `op` on a text, in the text's offsets before it ({start, end, text}), as Monaco
 * applies several at once.
 */
export function toEdits(op) {
  const edits = [];
  let at = 0;
  for (const part of op) {
    if (isRetain(part)) { at += part; continue; }
    const last = edits[edits.length - 1];
    const joined = last && last.end === at;
    if (isInsert(part)) {
      if (joined) last.text += part;
      else edits.push({ start: at, end: at, text: part });
    } else {
      if (joined) last.end -= part;
      else edits.push({ start: at, end: at - part, text: '' });
      at -= part;
    }
  }
  return edits;
}

/** The operation from `a` to `b`: one replacement of the part that differs, never inside a character. */
export function replacement(a, b) {
  let start = 0;
  const shortest = Math.min(a.length, b.length);
  while (start < shortest && a.charCodeAt(start) === b.charCodeAt(start)) start += 1;
  let tail = 0;
  while (tail < shortest - start && a.charCodeAt(a.length - 1 - tail) === b.charCodeAt(b.length - 1 - tail)) tail += 1;
  const high = (code) => code >= 0xd800 && code <= 0xdbff;
  if (start > 0 && high(a.charCodeAt(start - 1))) start -= 1;
  if (tail > 0 && high(a.charCodeAt(a.length - tail - 1))) tail -= 1;
  return builder().retain(start).delete(a.length - tail - start).insert(b.slice(start, b.length - tail)).retain(tail).ops;
}
