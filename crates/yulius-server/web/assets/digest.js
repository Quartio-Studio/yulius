// The end-of-day digest (SPEC-512 part 3, #1413): the instance's setting (instance
// administrators, Settings) and the person's latest digest at /digest, which the bell opens.

import { h, icon, setChildren, toast, errorToast } from './dom.js';
import { href } from './router.js';
import { dateTime } from './format.js';
import { pageFrame } from './pages/common.js';

/** "2 wait for you · 1 parked · 3 still working": what a digest holds, in one line. */
export function digestSummary(digest) {
  const parts = [];
  if (digest.waiting.length) parts.push(`${digest.waiting.length} ${digest.waiting.length === 1 ? 'waits' : 'wait'} for you`);
  if (digest.parked.length) parts.push(`${digest.parked.length} parked`);
  if (digest.working_count) parts.push(`${digest.working_count} still working`);
  return parts.join(' · ');
}

/** The task's page inside the app, from the digest's absolute link. */
function routeOf(link) {
  try {
    return new URL(link).pathname;
  } catch {
    return null;
  }
}

function itemRow(item) {
  const route = routeOf(item.link);
  const label = `${item.project} #${item.number} ${item.title}`;
  return h('li', { class: 'digest-item' },
    route ? h('a', { href: href(route), dataset: { route } }, label) : h('span', null, label),
    item.detail ? h('div', { class: 'muted small digest-detail' }, item.detail) : null);
}

function digestSection(id, title, items, extra = null) {
  if (!items.length && !extra) return null;
  return h('section', { class: 'settings-section', 'aria-labelledby': id },
    h('div', { class: 'settings-head' }, h('h2', { id }, title)),
    h('div', { class: 'settings-body' }, extra, items.length ? h('ul', { class: 'digest-list' }, items.map(itemRow)) : null));
}

/** `/digest`: the person's latest digest, what waits for them first. */
export function mountDigestPage(ctx) {
  const frame = pageFrame({ title: 'Digest', sub: 'what waits for you, what was parked and what still works' });
  const host = h('div', { class: 'stack' }, h('p', { class: 'muted' }, 'Loading…'));
  frame.body.append(host);
  // A digest that arrives while the page is open replaces the one shown.
  const off = ctx.store.on('digest', (digest) => show(digest));
  ctx.api.get('/api/me/digest').then((digest) => show(digest), (error) => {
    setChildren(host, h('div', { class: 'banner banner-danger', role: 'alert' }, icon('warn'), h('span', { class: 'banner-text' }, `Couldn’t load the digest: ${error.message}`)));
  });
  function show(digest) {
    if (!digest) {
      setChildren(host, h('p', { class: 'muted' }, 'No digest yet. When an administrator turns it on, one comes at the end of each day with something to report.'));
      return;
    }
    frame.sub.textContent = `made ${dateTime(digest.created_at)}`;
    setChildren(host,
      digestSection('digest-waiting', `Waiting for you (${digest.waiting.length})`, digest.waiting),
      digestSection('digest-parked', `Parked (${digest.parked.length})`, digest.parked),
      digestSection('digest-working', `Still working (${digest.working_count})`, digest.working,
        digest.working_count > digest.working.length ? h('p', { class: 'muted small' }, `The first ${digest.working.length} of ${digest.working_count}.`) : null));
  }
  return { el: frame.el, destroy: off };
}

/** Settings: the instance's digest (instance administrators), off by default. */
export function digestSettingCard(ctx) {
  const card = h('section', { class: 'settings-section', 'aria-labelledby': 'digest-title' });
  const body = h('div', { class: 'settings-body instance-body' });
  setChildren(card,
    h('div', { class: 'settings-head' }, icon('bell', { size: 16 }), h('h2', { id: 'digest-title' }, 'End-of-day digest'), h('span', { class: 'muted small' }, 'what waits for each person, once a day')),
    body);

  function render(state) {
    const enabled = h('input', { type: 'checkbox', id: 'digest-enabled', checked: state.enabled });
    const at = h('input', { class: 'input', type: 'time', id: 'digest-at', value: state.at, required: true });
    const hours = h('input', { class: 'input', type: 'number', id: 'digest-park-hours', min: 1, max: 720, step: 1, value: state.park_idle_hours ?? '', placeholder: 'Off', 'aria-describedby': 'digest-park-hint' });
    const save = h('button', { type: 'submit', class: 'btn btn-primary btn-sm' }, 'Save');
    const form = h('form', { class: 'jev-form', novalidate: true },
      h('label', { class: 'telegram-choice' }, enabled, h('span', null, 'Send each person a digest: what waits for them, what was parked, what still works')),
      h('div', { class: 'field' }, h('label', { class: 'field-label', for: 'digest-at' }, 'At (UTC)'), at),
      h('div', { class: 'field' }, h('label', { class: 'field-label', for: 'digest-park-hours' }, 'Park tasks quiet for (hours)'), hours),
      h('p', { class: 'muted xs', id: 'digest-park-hint' }, 'Empty: the digest parks nothing. Tasks that wait for a person or are Done are never parked by it; a parked task resumes when someone writes to it.'),
      h('div', { class: 'row' }, save));
    form.addEventListener('submit', async (event) => {
      event.preventDefault();
      const value = hours.value.trim();
      const body = { enabled: enabled.checked, at: at.value, park_idle_hours: value ? Number(value) : null };
      save.disabled = true;
      try {
        render(await ctx.api.put('/api/admin/digest', body));
        toast(body.enabled ? `Digest on, at ${body.at} UTC` : 'Digest off', { kind: 'ok' });
      } catch (error) {
        errorToast(error);
        save.disabled = false;
      }
    });
    setChildren(body, form);
  }

  ctx.api.get('/api/admin/digest').then(render, (error) => {
    setChildren(body, h('div', { class: 'banner banner-danger', role: 'alert' }, icon('warn'), h('span', { class: 'banner-text' }, `Couldn’t load the digest setting: ${error.message}`)));
  });
  return card;
}
