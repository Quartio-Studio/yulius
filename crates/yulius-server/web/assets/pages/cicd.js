// CI/CD page (SPEC-151, ADR-0125): CI (tests) and CD (deployment) as two sections of one page,
// chosen with the Secrets page's tabs. The section is kept in `?section=cd` (CI is the plain
// address, the one the `yulius/ci` commit status links to); `/s/<slug>/cd` opens the CD tab too.

import { h, icon, setChildren } from '../dom.js';
import { replaceQuietly } from '../router.js';
import { pageFrame } from './common.js';
import { mountCiSection, CI_SUB } from './ci.js';
import { mountCdSection, CD_SUB } from './cd.js';

const SECTIONS = [
  { value: '', label: 'CI (tests)', icon: 'check', sub: CI_SUB, mount: mountCiSection },
  { value: 'cd', label: 'CD (deployment)', icon: 'upload', sub: CD_SUB, mount: mountCdSection },
];

/** Mount the CI/CD page: `/s/<slug>/ci`, `?section=cd` for the CD tab. */
export function mountCiCdPage(ctx, sandbox, route) {
  const frame = pageFrame({ title: 'CI/CD' });
  const path = `/s/${encodeURIComponent(sandbox.slug)}/ci`;
  let current = route.query.get('section') === 'cd' ? 'cd' : '';
  // The CD page's own address (agents' messages, older links) says the canonical one from now on.
  if (route.path !== path) replaceQuietly(route.query.toString() ? `${path}?${route.query}` : path);
  const host = h('div');
  const tabs = h('div', { class: 'tl-tabs ftabs', role: 'radiogroup', 'aria-label': 'Section' });
  let teardown = null;

  function renderTabs() {
    const focused = tabs.contains(document.activeElement);
    setChildren(tabs, SECTIONS.map((o) => h('button', {
      type: 'button', role: 'radio', 'aria-checked': String(o.value === current), tabindex: o.value === current ? '0' : '-1', class: ['tl-tab', o.value === current && 'active'],
      onClick: () => {
        if (current === o.value) return;
        current = o.value;
        // The shown section goes first: its run dialog writes its own address as it closes.
        stop();
        replaceQuietly(current ? `${path}?section=${current}` : path);
        renderTabs();
        show(null);
      },
      onKeydown: (event) => {
        if (!['ArrowRight', 'ArrowLeft', 'ArrowUp', 'ArrowDown'].includes(event.key)) return;
        event.preventDefault();
        const other = [...tabs.children].find((b) => b !== event.currentTarget);
        if (other) other.click();
      },
    }, h('span', { class: 'tl-tab-icon' }, icon(o.icon, { size: 14 })), o.label)));
    if (focused) tabs.querySelector('[aria-checked="true"]')?.focus();
  }

  function stop() {
    if (teardown) teardown();
    teardown = null;
  }

  /** Mounts the current section in a part of its own, so a late answer for the other one lands nowhere. */
  function show(shownRoute) {
    const section = SECTIONS.find((o) => o.value === current);
    const part = h('div', { class: 'stack' });
    setChildren(host, part);
    frame.sub.textContent = section.sub;
    teardown = section.mount(ctx, sandbox, shownRoute, part, frame.actions);
  }

  renderTabs();
  frame.body.append(tabs, host);
  // `?run=<id>` opens that run of the section the address names.
  show(route);
  return { el: frame.el, key: `ci:${sandbox.id}`, destroy: stop };
}
