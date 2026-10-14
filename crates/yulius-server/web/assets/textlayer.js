// Drawn selections behind a text box (SPEC-112 NOT-017, NOT-020): a <textarea> paints its own
// selection in square boxes that jump, so the notepads hide it and draw it here instead, one
// rounded shape per line that glides (a CSS transition) from one selection to the next. The same
// layer draws teammates: their selection in their colour, their caret and their name.
//
// Where the text lies is measured on a hidden mirror: a div with the text box's font, padding,
// width and wrapping, holding the same text, over which a DOM Range gives one box per line. The
// shapes sit in a layer under the (transparent) text box, moved with its scroll.

import { h } from './dom.js';

/** Styles the mirror copies so its text wraps exactly like the text box's. */
const MIRRORED = ['fontFamily', 'fontSize', 'fontWeight', 'fontStyle', 'fontVariant', 'fontStretch', 'letterSpacing', 'wordSpacing',
  'lineHeight', 'textTransform', 'textIndent', 'tabSize', 'paddingTop', 'paddingRight', 'paddingBottom', 'paddingLeft'];
/** A blank line inside a selection still shows, this wide. */
const BLANK_LINE_PX = 6;
/** A caret's width; it is centred on its place between two characters. */
const CARET_PX = 2;
/** Teammates' colours (tokens `--peer-1` … `--peer-8`). */
const PEER_COLOURS = 8;
/** How long a leaving teammate takes to fade before their marks are removed. */
const LEAVE_MS = 400;

/**
 * The one edit that turns text `a` into `b`: their common start and end kept, `a.slice(start,
 * endA)` replaced by `b.slice(start, endB)`.
 */
export function textEdit(a, b) {
  let start = 0;
  const shortest = Math.min(a.length, b.length);
  while (start < shortest && a.charCodeAt(start) === b.charCodeAt(start)) start += 1;
  let tail = 0;
  while (tail < shortest - start && a.charCodeAt(a.length - 1 - tail) === b.charCodeAt(b.length - 1 - tail)) tail += 1;
  return { start, endA: a.length - tail, endB: b.length - tail };
}

/** Offset `pos` of the text before `edit` in the text after it; inside the edit, its start. */
export function mapPosition(pos, edit) {
  if (pos <= edit.start) return pos;
  if (pos >= edit.endA) return pos + edit.endB - edit.endA;
  return edit.start;
}

/** A teammate's colour, the same for them everywhere: one of the eight, from their user id. */
export function peerColour(id) {
  let hash = 0;
  for (const char of String(id)) hash = (hash * 31 + char.codePointAt(0)) >>> 0;
  return `var(--peer-${(hash % PEER_COLOURS) + 1})`;
}

/**
 * The layer for `textarea`: `{el, refresh, setPeers, destroy}`. `el` goes right before the text
 * box in a positioned parent; `refresh()` redraws after the text changed from code;
 * `setPeers([{id, name, start, end, head}])` shows those teammates (in offsets of the text box's
 * text) and lets the others fade out.
 */
export function createTextLayer(textarea) {
  const own = h('div', { class: 'tl-group tl-own' });
  const content = h('div', { class: 'tl-content' }, own);
  const mirror = h('div', { class: 'tl-mirror' });
  const el = h('div', { class: 'text-layer', 'aria-hidden': 'true' }, mirror, content);
  textarea.classList.add('drawn-selection');
  const shapes = new Map([[own, []]]);
  const peerMarks = new Map(); // id → {group, caret, name}
  let peers = [];
  let frame = 0;

  /** Lays the mirror out like the text box, with its text: the text node and the line height. */
  function layOut() {
    const style = getComputedStyle(textarea);
    for (const key of MIRRORED) mirror.style[key] = style[key];
    const padding = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
    mirror.style.width = `${Math.max(0, textarea.clientWidth - padding)}px`;
    // A zero-width space keeps a last empty line a line and gives the end of the text a place.
    const text = `${textarea.value}​`;
    if (mirror.textContent !== text) mirror.textContent = text;
    return { node: mirror.firstChild, origin: mirror.getBoundingClientRect(), lineHeight: parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.5 };
  }

  const clamp = (pos) => Math.max(0, Math.min(pos, textarea.value.length));

  /** One box per line of `start..end` in text box coordinates (unscrolled), lines whole height. */
  function lineBoxes(layout, start, end) {
    const from = clamp(start);
    const to = Math.max(from, clamp(end));
    if (from === to) return [];
    const { node, origin, lineHeight } = layout;
    const range = document.createRange();
    range.setStart(node, from);
    range.setEnd(node, to);
    const lines = [];
    for (const rect of range.getClientRects()) {
      const middle = rect.top + rect.height / 2 - origin.top;
      const line = lines.find((l) => Math.abs(l.middle - middle) < lineHeight / 2);
      if (line) {
        line.left = Math.min(line.left, rect.left - origin.left);
        line.right = Math.max(line.right, rect.right - origin.left);
      } else {
        lines.push({ middle, left: rect.left - origin.left, right: rect.right - origin.left });
      }
    }
    lines.sort((a, b) => a.middle - b.middle);
    // A selection ending at a line's start or starting at a line's end draws nothing there; a
    // blank line inside it shows as a sliver.
    return lines
      .filter((line, i) => line.right - line.left >= 1 || (i > 0 && i < lines.length - 1))
      .map((line) => ({
        x: line.left,
        y: line.middle - lineHeight / 2,
        width: Math.max(line.right - line.left, BLANK_LINE_PX),
        height: lineHeight,
      }));
  }

  /** Where a caret at `pos` stands: the left edge of the character after it, on its line. */
  function caretBox(layout, pos) {
    const { node, origin, lineHeight } = layout;
    const at = clamp(pos);
    const range = document.createRange();
    // The character after the caret (the zero-width space at the very end) says where it is.
    range.setStart(node, at);
    range.setEnd(node, at + 1);
    const rects = [...range.getClientRects()];
    // A line break's box may sit at the end of the line before; the last box is the character's.
    const rect = textarea.value[at] === '\n' ? rects[0] : rects[rects.length - 1];
    if (!rect) return { x: 0, y: 0, height: lineHeight };
    const middle = rect.top + rect.height / 2 - origin.top;
    return { x: rect.left - origin.left, y: middle - lineHeight / 2, height: lineHeight };
  }

  /** Shows `boxes` in `group`, reusing its shapes in order so a changing selection glides. */
  function place(group, boxes) {
    const pool = shapes.get(group);
    boxes.forEach((box, i) => {
      let shape = pool[i];
      const fresh = !shape;
      if (fresh) {
        // A new shape starts where it belongs and fades in, instead of flying in from the corner.
        shape = h('div', { class: 'tl-shape tl-gone tl-still' });
        // Faded out and not taken back meanwhile: gone for good (the pool's tail).
        shape.addEventListener('transitionend', () => {
          if (!shape.classList.contains('tl-gone') || !shape.isConnected) return;
          pool.splice(pool.indexOf(shape), 1);
          shape.remove();
        });
        pool.push(shape);
        group.append(shape);
      }
      shape.style.transform = `translate(${box.x}px, ${box.y}px)`;
      shape.style.width = `${box.width}px`;
      shape.style.height = `${box.height}px`;
      if (fresh) shape.getBoundingClientRect(); // the start of its fade
      shape.classList.remove('tl-gone', 'tl-still');
    });
    for (const shape of pool.slice(boxes.length)) shape.classList.add('tl-gone');
  }

  /** A teammate's marks: a group in their colour with their selection, caret and name. */
  function marksOf(peer) {
    let marks = peerMarks.get(peer.id);
    if (!marks) {
      const name = h('span', { class: 'tl-name' });
      const caret = h('div', { class: 'tl-caret tl-still' }, name);
      const group = h('div', { class: 'tl-group tl-peer tl-leaving' }, caret);
      group.style.setProperty('--tl-colour', peerColour(peer.id));
      shapes.set(group, []);
      content.append(group);
      marks = { group, caret, name, fresh: true };
      peerMarks.set(peer.id, marks);
    }
    return marks;
  }

  function drawPeers(layout) {
    const showing = new Set(peers.map((peer) => peer.id));
    for (const [id, marks] of peerMarks) {
      if (showing.has(id) || marks.group.classList.contains('tl-leaving')) continue;
      marks.group.classList.add('tl-leaving');
      marks.leaving = setTimeout(() => {
        marks.group.remove();
        shapes.delete(marks.group);
        peerMarks.delete(id);
      }, LEAVE_MS);
    }
    for (const peer of peers) {
      const marks = marksOf(peer);
      clearTimeout(marks.leaving);
      const { group, caret, name } = marks;
      Object.assign(group.dataset, { start: String(peer.start), end: String(peer.end), head: String(peer.head) });
      if (name.textContent !== peer.name) name.textContent = peer.name;
      place(group, lineBoxes(layout, peer.start, peer.end));
      const at = caretBox(layout, peer.head);
      caret.style.transform = `translate(${at.x - CARET_PX / 2}px, ${at.y}px)`;
      caret.style.height = `${at.height}px`;
      // On the first line the name hangs below the caret, where the layer does not cut it off.
      caret.classList.toggle('tl-below', at.y < at.height);
      if (marks.fresh) {
        caret.getBoundingClientRect(); // in place before it fades in
        marks.fresh = false;
      }
      caret.classList.remove('tl-still');
      group.classList.remove('tl-leaving');
    }
  }

  function draw() {
    frame = 0;
    if (!textarea.isConnected || textarea.clientWidth === 0) return; // hidden: drawn when shown
    content.style.transform = `translate(${-textarea.scrollLeft}px, ${-textarea.scrollTop}px)`;
    own.classList.toggle('tl-blurred', document.activeElement !== textarea);
    const layout = layOut();
    const { selectionStart, selectionEnd } = textarea;
    place(own, lineBoxes(layout, Math.min(selectionStart, selectionEnd), Math.max(selectionStart, selectionEnd)));
    drawPeers(layout);
  }

  const schedule = () => { if (!frame) frame = requestAnimationFrame(draw); };
  // The scroll moves the shapes with the text at once, never late by a frame.
  const onScroll = () => { content.style.transform = `translate(${-textarea.scrollLeft}px, ${-textarea.scrollTop}px)`; };
  const onSelection = () => { if (document.activeElement === textarea) schedule(); };
  const events = ['input', 'select', 'selectionchange', 'focus', 'blur', 'keyup', 'pointerup'];
  for (const name of events) textarea.addEventListener(name, schedule);
  textarea.addEventListener('scroll', onScroll);
  document.addEventListener('selectionchange', onSelection);
  const resize = typeof ResizeObserver === 'function' ? new ResizeObserver(schedule) : null;
  if (resize) resize.observe(textarea);

  return {
    el,
    refresh: schedule,
    setPeers(list) {
      peers = list;
      schedule();
    },
    destroy() {
      if (frame) cancelAnimationFrame(frame);
      for (const marks of peerMarks.values()) clearTimeout(marks.leaving);
      for (const name of events) textarea.removeEventListener(name, schedule);
      textarea.removeEventListener('scroll', onScroll);
      document.removeEventListener('selectionchange', onSelection);
      if (resize) resize.disconnect();
    },
  };
}
