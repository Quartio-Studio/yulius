// Drawn selections in the Editor (SPEC-127 EDT-007, EDT-010): the notepads' rounded, gliding
// shapes, placed where Monaco itself draws a selection. Monaco renders each selection as boxes
// (`.selected-text`, one per range and visual line) in `.view-overlays`, and rebuilds them on every
// render, so they cannot be animated; they are made transparent and copied, after each render, into
// shapes that persist and glide (textlayer.js `createMarks`). Teammates' selections are Monaco
// decorations copied the same way, so word wrap, folding, tabs and fonts are Monaco's business.
//
// The layer sits inside `.lines-content`, right after `.view-overlays` (behind the text), so a
// scroll moves it with the text in the same frame; a scroll's new lines appear without gliding.

import { h } from './dom.js';
import { createMarks } from './textlayer.js';

/** Class of a teammate's selection decoration; `-<slot>` tells teammates apart. */
const PEER_RANGE = 'ed-peer-range';

/**
 * The layer for a Monaco `editor`: `{refresh, setPeers, destroy}`. `setPeers([{id, colourOf,
 * name, start, end, head}])` (offsets in the shown model) shows those teammates and lets the
 * others fade out.
 */
export function createEditorLayer(monaco, editor) {
  let root = null; // Monaco's `.monaco-editor` of the shown model
  const content = h('div', { class: 'tl-content' });
  const el = h('div', { class: 'ed-layer', 'aria-hidden': 'true' }, content);
  const marks = createMarks(content);
  const decorations = editor.createDecorationsCollection();
  const slots = new Map(); // teammate id → slot number of their decoration class
  let overlays = null; // Monaco's `.view-overlays` of the shown model
  let peers = [];
  let frame = 0;

  /** Boxes of the rendered elements matching `selector`, in the layer's coordinates, top to bottom. */
  function boxes(selector, origin) {
    return [...overlays.querySelectorAll(selector)]
      .map((node) => {
        const rect = node.getBoundingClientRect();
        return { x: rect.left - origin.left, y: rect.top - origin.top, width: rect.width, height: rect.height };
      })
      .filter((box) => box.width > 0 && box.height > 0)
      .sort((a, b) => a.y - b.y || a.x - b.x);
  }

  /** Where a teammate's caret stands, or `null` while its line is not on screen. */
  function caretAt(offset, origin) {
    const model = editor.getModel();
    if (!model) return null;
    const position = model.getPositionAt(offset);
    const visible = editor.getVisibleRanges().some((range) => range.startLineNumber <= position.lineNumber && position.lineNumber <= range.endLineNumber);
    const at = visible && editor.getScrolledVisiblePosition(position);
    if (!at) return null;
    const box = root.getBoundingClientRect();
    return { x: box.left + at.left - origin.left, y: box.top + at.top - origin.top, height: at.height, top: at.top };
  }

  /** Copies what Monaco rendered into the shapes. */
  function draw() {
    frame = 0;
    if (!overlays || !el.isConnected || root.clientWidth === 0) return; // hidden: drawn when shown
    const origin = el.getBoundingClientRect();
    marks.own.classList.toggle('tl-blurred', !editor.hasTextFocus());
    marks.place(marks.own, boxes('.selected-text', origin));
    marks.drawPeers(peers.map((peer) => {
      const caret = caretAt(peer.head, origin);
      return {
        id: peer.id,
        colourOf: peer.colourOf,
        name: peer.name,
        boxes: boxes(`.${PEER_RANGE}-${slots.get(peer.id)}`, origin),
        caret,
        // On the first line in view the name hangs below the caret, where the editor does not cut it off.
        below: Boolean(caret && caret.top < caret.height),
        data: { start: String(peer.start), end: String(peer.end), head: String(peer.head) },
      };
    }));
  }

  const schedule = () => { if (!frame) frame = requestAnimationFrame(draw); };
  // Monaco has just rendered (a selection, a scroll, a layout or wrap change): copy it before the
  // frame is painted.
  const rendered = new MutationObserver(() => {
    if (frame) cancelAnimationFrame(frame);
    draw();
  });
  // Monaco builds its view when a model is shown and again for each new one: the layer follows.
  function attach() {
    rendered.disconnect();
    root = editor.getModel() && editor.getDomNode();
    const linesContent = root && root.querySelector('.lines-content');
    overlays = linesContent && linesContent.querySelector(':scope > .view-overlays');
    if (!overlays) { el.remove(); return; }
    overlays.after(el);
    // Another file: its selection appears in place.
    content.classList.add('tl-jump');
    requestAnimationFrame(() => requestAnimationFrame(() => content.classList.remove('tl-jump')));
    rendered.observe(overlays, { childList: true, subtree: true, attributes: true, attributeFilter: ['style', 'class'] });
    schedule();
  }
  attach();
  const subscriptions = [
    editor.onDidChangeModel(attach),
    // Lines scrolled into view appear in place instead of gliding in from where others were.
    editor.onDidScrollChange(() => {
      content.classList.add('tl-jump');
      requestAnimationFrame(() => requestAnimationFrame(() => content.classList.remove('tl-jump')));
    }),
    editor.onDidFocusEditorText(schedule),
    editor.onDidBlurEditorText(schedule),
    editor.onDidLayoutChange(schedule),
  ];

  return {
    el,
    refresh: schedule,
    setPeers(list) {
      peers = list;
      for (const id of [...slots.keys()]) if (!list.some((peer) => peer.id === id)) slots.delete(id);
      for (const peer of list) {
        if (slots.has(peer.id)) continue;
        let slot = 0;
        while ([...slots.values()].includes(slot)) slot += 1;
        slots.set(peer.id, slot);
      }
      const model = editor.getModel();
      decorations.set(!model ? [] : list.filter((peer) => peer.end > peer.start).map((peer) => {
        const start = model.getPositionAt(peer.start);
        const end = model.getPositionAt(peer.end);
        return {
          range: new monaco.Range(start.lineNumber, start.column, end.lineNumber, end.column),
          options: { className: `${PEER_RANGE} ${PEER_RANGE}-${slots.get(peer.id)}`, stickiness: monaco.editor.TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges },
        };
      }));
      // Monaco renders the decorations in its next frame (copied then); a caret alone moves now.
      schedule();
    },
    destroy() {
      if (frame) cancelAnimationFrame(frame);
      rendered.disconnect();
      for (const subscription of subscriptions) subscription.dispose();
      decorations.clear();
      marks.destroy();
      el.remove();
    },
  };
}
