// Column splitters (SPEC-001 UI-044, F 2026-10-10): the sidebar, the editor's explorer and the
// tool panel resize by their edge. One control for all three: a quiet grip that is also a focusable
// separator (WAI-ARIA window splitter: ←/→, Shift for bigger steps, Home/End), dragged with any
// pointer, reset with a double-click. Widths are kept per user in this browser.

import { h } from './dom.js';
import { readJson, writeJson } from './store.js';

const STEP = 16;
const BIG_STEP = 64;

/**
 * A vertical splitter for a column. The owner keeps the width; the splitter only asks for changes:
 * - `width()` → the column's current width in px;
 * - `resize(px)` applies a requested width (the owner clamps it);
 * - `commit()` remembers the width after a drag or a key;
 * - `reset()` returns to the default width (double-click);
 * - `range()` → {min, max} in px, for the separator's value and Home/End.
 * `grow` is +1 when moving the edge right widens the column (its right edge), −1 for a left edge.
 * Returns {el, sync}: sync() refreshes the accessible value after the owner changed the width.
 */
export function createSplitter({ label, className, grow = 1, width, resize, commit, reset, range }) {
  const el = h('div', {
    class: ['splitter', className],
    role: 'separator',
    'aria-orientation': 'vertical',
    'aria-label': label,
    tabindex: '0',
    title: `${label}: drag, use the arrow keys, or double-click to reset`,
  });

  function sync() {
    const { min, max } = range();
    const now = Math.round(width());
    el.setAttribute('aria-valuemin', String(Math.round(min)));
    el.setAttribute('aria-valuemax', String(Math.round(Math.max(min, max))));
    el.setAttribute('aria-valuenow', String(now));
    el.setAttribute('aria-valuetext', `${now} pixels wide`);
  }

  el.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    el.setPointerCapture(event.pointerId);
    el.classList.add('dragging');
    document.body.classList.add('resizing');
    const startX = event.clientX;
    const start = width();
    let lastX = startX;
    let frame = 0;
    // One resize per frame: a drag never lays the page out more often than it paints.
    const apply = () => { frame = 0; resize(start + grow * (lastX - startX)); };
    const move = (e) => { lastX = e.clientX; if (!frame) frame = requestAnimationFrame(apply); };
    const up = (e) => {
      if (frame) cancelAnimationFrame(frame);
      apply();
      try { el.releasePointerCapture(e.pointerId); } catch { /* already released */ }
      el.classList.remove('dragging');
      document.body.classList.remove('resizing');
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      el.removeEventListener('pointercancel', up);
      commit();
      sync();
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  });

  // A double-click resets: a way to size the column without dragging (WCAG 2.5.7), like the keys.
  el.addEventListener('dblclick', () => { reset(); sync(); });

  el.addEventListener('keydown', (event) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const step = event.shiftKey ? BIG_STEP : STEP;
    const { min, max } = range();
    let next = null;
    if (event.key === 'ArrowLeft') next = width() - grow * step;
    else if (event.key === 'ArrowRight') next = width() + grow * step;
    else if (event.key === 'Home') next = min;
    else if (event.key === 'End') next = max;
    if (next === null) return;
    event.preventDefault();
    resize(next);
    commit();
    sync();
  });

  return { el, sync };
}

/**
 * Column widths of the signed-in user in this browser (`localStorage` `yulius.layout.<user id>`):
 * {sidebar: px, explorer: px, panel: share of the work area}. Unreadable or blocked storage reads as
 * nothing stored (store.js readJson/writeJson catch every storage error).
 */
export function layoutPrefs(ctx) {
  const user = (ctx.store && ctx.store.state.me && ctx.store.state.me.user) || {};
  const key = `yulius.layout.${user.id || user.login || 'anonymous'}`;
  const all = () => readJson(key, {}) || {};
  return {
    get(name) {
      const value = Number(all()[name]);
      return Number.isFinite(value) && value > 0 ? value : null;
    },
    set(name, value) {
      const next = all();
      next[name] = Math.round(value * 10000) / 10000;
      writeJson(key, next);
    },
    clear(name) {
      const next = all();
      delete next[name];
      writeJson(key, next);
    },
  };
}
