// Drawn selections behind a text box (SPEC-112 NOT-017): a <textarea> paints its own selection
// in square boxes that jump, so the notepads hide it and draw it here instead, one rounded shape
// per line that glides (a CSS transition) from one selection to the next.
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

/**
 * The layer for `textarea`: `{el, refresh, destroy}`. `el` goes right before the text box in a
 * positioned parent; `refresh()` redraws after the text changed from code.
 */
export function createTextLayer(textarea) {
  const own = h('div', { class: 'tl-group tl-own' });
  const content = h('div', { class: 'tl-content' }, own);
  const mirror = h('div', { class: 'tl-mirror' });
  const el = h('div', { class: 'text-layer', 'aria-hidden': 'true' }, mirror, content);
  textarea.classList.add('drawn-selection');
  const shapes = new Map([[own, []]]);
  let frame = 0;

  /** The mirror holds `text` laid out like the text box; returns its text node. */
  function layOut() {
    const style = getComputedStyle(textarea);
    for (const key of MIRRORED) mirror.style[key] = style[key];
    const padding = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
    mirror.style.width = `${Math.max(0, textarea.clientWidth - padding)}px`;
    // A zero-width space keeps a last empty line a line.
    const text = `${textarea.value}​`;
    if (mirror.textContent !== text) mirror.textContent = text;
    return { node: mirror.firstChild, lineHeight: parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.5 };
  }

  /** One box per line of `start..end` in text box coordinates (unscrolled), lines whole height. */
  function lineBoxes(start, end) {
    const length = textarea.value.length;
    const from = Math.max(0, Math.min(start, length));
    const to = Math.max(from, Math.min(end, length));
    if (from === to) return [];
    const { node, lineHeight } = layOut();
    const range = document.createRange();
    range.setStart(node, from);
    range.setEnd(node, to);
    const origin = mirror.getBoundingClientRect();
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

  function draw() {
    frame = 0;
    if (!textarea.isConnected || textarea.clientWidth === 0) return; // hidden: drawn when shown
    content.style.transform = `translate(${-textarea.scrollLeft}px, ${-textarea.scrollTop}px)`;
    own.classList.toggle('tl-blurred', document.activeElement !== textarea);
    const { selectionStart, selectionEnd } = textarea;
    place(own, lineBoxes(Math.min(selectionStart, selectionEnd), Math.max(selectionStart, selectionEnd)));
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
    destroy() {
      if (frame) cancelAnimationFrame(frame);
      for (const name of events) textarea.removeEventListener(name, schedule);
      textarea.removeEventListener('scroll', onScroll);
      document.removeEventListener('selectionchange', onSelection);
      if (resize) resize.disconnect();
    },
  };
}
