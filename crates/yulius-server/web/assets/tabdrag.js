// Dragging a tab along its strip (SPEC-112 NOT-015, SPEC-127 EDT-011), for the notepads' and the
// Editor's tabs. Mouse or pen; touch scrolls the strip and moves tabs from the menu. Past a few
// pixels the tab follows the pointer, the tabs it passes slide aside, and on release it settles
// into its place from where it was let go; Escape puts it back. The click that ends a drag (or an
// Escape) chooses nothing: the strip's click handler asks `clickConsumed()` first.

/** How far a pressed tab moves before it is dragged rather than clicked. */
const DRAG_THRESHOLD_PX = 4;

/**
 * Drag for the tabs of `tablist`: `keyOf(tab)` names a tab element, `commit(order)` puts the tabs
 * in that order (keys) and answers the strip's tab elements as drawn again. Returns `{press,
 * cancel, clickConsumed}`; call `press(event, tab)` on a tab's `pointerdown`.
 */
export function createTabDrag(tablist, { keyOf, commit }) {
  let drag = null; // {key, tab, startX, moved, tabs, keys, pointerId, lefts, widths}
  let dragged = false; // the click that ends a drag chooses nothing

  function press(event, tab) {
    if (event.button !== 0 || event.pointerType === 'touch' || drag) return;
    const tabs = [...tablist.querySelectorAll('[role="tab"]')];
    drag = {
      key: keyOf(tab), tab, startX: event.clientX, moved: false, tabs, pointerId: event.pointerId,
      keys: tabs.map(keyOf), lefts: tabs.map((t) => t.offsetLeft), widths: tabs.map((t) => t.offsetWidth),
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', end);
    window.addEventListener('pointercancel', cancel);
    window.addEventListener('keydown', escape, true);
  }

  /** Where the dragged tab lands moved by `dx`: the order it makes and how far each tab shifts. */
  function layout(state, dx) {
    const from = state.keys.indexOf(state.key);
    const centre = state.lefts[from] + state.widths[from] / 2 + dx;
    const others = state.keys.filter((key) => key !== state.key);
    const slot = others.filter((key) => { const i = state.keys.indexOf(key); return state.lefts[i] + state.widths[i] / 2 < centre; }).length;
    const placed = [...others];
    placed.splice(slot, 0, state.key);
    let left = state.lefts[0];
    const shifts = new Map();
    for (const key of placed) {
      const i = state.keys.indexOf(key);
      shifts.set(key, left - state.lefts[i]);
      left += state.widths[i];
    }
    return { order: placed, shifts };
  }

  function move(event) {
    if (!drag || event.pointerId !== drag.pointerId) return;
    const dx = event.clientX - drag.startX;
    if (!drag.moved) {
      if (Math.abs(dx) < DRAG_THRESHOLD_PX) return;
      drag.moved = true;
      tablist.classList.add('reordering');
      drag.tab.classList.add('dragging');
    }
    const { shifts } = layout(drag, dx);
    drag.tabs.forEach((tab, i) => {
      const key = drag.keys[i];
      tab.style.transform = `translateX(${key === drag.key ? dx : shifts.get(key)}px)`;
    });
  }

  function done() {
    window.removeEventListener('pointermove', move);
    window.removeEventListener('pointerup', end);
    window.removeEventListener('pointercancel', cancel);
    window.removeEventListener('keydown', escape, true);
    tablist.classList.remove('reordering');
    const state = drag;
    drag = null;
    return state;
  }

  function end(event) {
    if (!drag || event.pointerId !== drag.pointerId) return;
    const state = done();
    if (!state.moved) return;
    dragged = true; // the click after this release
    setTimeout(() => { dragged = false; }, 0); // a release outside the tab clicks nothing
    const dx = event.clientX - state.startX;
    const { order } = layout(state, dx);
    const from = state.lefts[state.keys.indexOf(state.key)] + dx;
    const tabs = commit(order);
    const tab = tabs.find((t) => keyOf(t) === state.key);
    if (!tab) return;
    // From where it was let go into its place (FLIP).
    tab.classList.add('dragging');
    tab.style.transform = `translateX(${from - tab.offsetLeft}px)`;
    tab.getBoundingClientRect();
    tab.classList.remove('dragging');
    tab.classList.add('settling');
    tab.style.transform = '';
    tab.addEventListener('transitionend', () => tab.classList.remove('settling'), { once: true });
  }

  function cancel() {
    const state = done();
    if (state) for (const tab of state.tabs) { tab.style.transform = ''; tab.classList.remove('dragging'); }
  }

  function escape(event) {
    if (event.key !== 'Escape' || !drag || !drag.moved) return;
    event.preventDefault();
    event.stopPropagation();
    cancel();
    // The release that follows clicks nothing (it may land back on the pressed tab).
    dragged = true;
    const released = () => {
      window.removeEventListener('pointerup', released, true);
      window.removeEventListener('pointercancel', released, true);
      setTimeout(() => { dragged = false; }, 0);
    };
    window.addEventListener('pointerup', released, true);
    window.addEventListener('pointercancel', released, true);
  }

  return {
    press,
    cancel,
    /** Whether this click ends a drag (and so chooses nothing); it is used up. */
    clickConsumed() {
      if (!dragged) return false;
      dragged = false;
      return true;
    },
  };
}

/** `order` with `key` moved one place by `step` (-1 left, 1 right), or `null` at an end. */
export function movedBy(order, key, step) {
  const from = order.indexOf(key);
  const to = from + step;
  if (from < 0 || to < 0 || to >= order.length) return null;
  const next = [...order];
  next.splice(from, 1);
  next.splice(to, 0, key);
  return next;
}
