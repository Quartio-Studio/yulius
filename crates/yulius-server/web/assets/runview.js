// A CI run's or a deploy's view inside the CI/CD page (SPEC-238, ADR-0180), in place of the tab's
// list rather than in a dialog over it: a region named for it, "All runs" back to the list, and
// its head line. The run is the page's address, so Back, Forward and the sidebar's remembered
// place (SPEC-113) come back to it.

import { h, icon, setChildren } from './dom.js';
import { href } from './router.js';

/**
 * The view's frame: `name` names the region ("CI run", "Deploy"); "All runs" goes to `list` (a
 * page address), or calls `onBack` instead while the view has no address of its own (a deploy
 * still starting, SPEC-189). Returns the region `el`, its `head` line and its `body`, shown by
 * `ready()`; until then `loading(node)` stands in for both (the run's loading shape).
 */
export function runView({ name, list, onBack }) {
  const attrs = { class: 'btn btn-sm run-back', href: href(list) };
  if (onBack) attrs.onClick = (event) => { event.preventDefault(); onBack(); };
  else attrs.dataset = { route: list };
  const back = h('a', attrs, icon('back', { size: 14 }), 'All runs');
  const head = h('div', { class: 'row-wrap run-head' });
  const body = h('div', { class: 'stack' });
  const summary = h('div', { class: 'run-summary' }, head);
  const content = h('div', { class: 'stack run-content' });
  const el = h('section', { class: 'run-view stack', 'aria-label': name }, back, content);
  return {
    el, head, body, back,
    loading: (node) => setChildren(content, node),
    ready: () => { if (!summary.isConnected) setChildren(content, summary, body); },
  };
}
