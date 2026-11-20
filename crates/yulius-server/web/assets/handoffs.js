// Tasks hand work to another sandbox (SPEC-512 part 5, #1106): on a task's page, where it was
// handed off from and its own handoffs, the owner answering a stored request (ask mode) with
// Approve or Decline; in Settings, the instance's switch (instance administrators).

import { h, icon, setChildren, toast, errorToast } from './dom.js';
import { href } from './router.js';

const STATE_TEXT = { created: 'Created', declined: 'Declined', refused: 'Refused' };

function routeOf(link) {
  try {
    return new URL(link).pathname;
  } catch {
    return null;
  }
}

function linkTo(link, label) {
  const route = link && routeOf(link);
  return route ? h('a', { href: href(route), dataset: { route } }, label) : h('span', null, label);
}

/** A task's handoffs panel: provenance and handoffs; `isOwner()` says whether the viewer answers. */
export function handoffsPanel(ctx, task, isOwner) {
  const el = h('div', { class: 'handoffs', hidden: true, 'aria-live': 'polite' });
  let state = { from: null, handoffs: [] };

  async function answer(handoff, approve, buttons) {
    buttons.forEach((b) => { b.disabled = true; });
    try {
      state = await ctx.api.post(`/api/tasks/${encodeURIComponent(task().id)}/handoffs/${encodeURIComponent(handoff.id)}`, { approve });
      toast(approve ? `Handed off to ${handoff.target_sandbox}` : 'Handoff declined', { kind: 'ok' });
      render();
    } catch (error) {
      buttons.forEach((b) => { b.disabled = false; });
      errorToast(error, 'Couldn’t answer the handoff');
    }
  }

  function render() {
    const rows = [];
    if (state.from) {
      rows.push(h('div', { class: 'handoff-from' }, icon('git-branch', { size: 14 }), h('span', null, 'Handed off from '), linkTo(state.from.link, `${state.from.sandbox} #${state.from.number}`)));
    }
    for (const handoff of state.handoffs.slice(0, 5)) {
      if (handoff.state === 'requested') {
        const options = ['harness', 'model', 'effort', 'mode'].filter((k) => handoff[k]).map((k) => `${k}: ${handoff[k]}`).join(' · ');
        const approve = h('button', { type: 'button', class: 'btn btn-sm btn-primary' }, 'Approve');
        const decline = h('button', { type: 'button', class: 'btn btn-sm' }, 'Decline');
        approve.addEventListener('click', () => answer(handoff, true, [approve, decline]));
        decline.addEventListener('click', () => answer(handoff, false, [approve, decline]));
        rows.push(h('section', { class: 'handoff-request', 'aria-label': `Handoff to ${handoff.target_sandbox}` },
          h('div', null, h('strong', null, `This task asks to hand work to ${handoff.target_sandbox}: `), handoff.title),
          h('pre', { class: 'handoff-prompt' }, handoff.prompt),
          options ? h('div', { class: 'muted xs' }, options) : null,
          isOwner() ? h('div', { class: 'row' }, approve, decline, h('span', { class: 'muted xs' }, 'Approving creates exactly this task there, as you.'))
            : h('div', { class: 'muted xs' }, 'Waits for the task’s owner.')));
      } else {
        const what = STATE_TEXT[handoff.state] || handoff.state;
        rows.push(h('div', { class: 'handoff-line muted small' },
          `${what}: ${handoff.target_sandbox} · ${handoff.title}`,
          handoff.target_link ? [' · ', linkTo(handoff.target_link, `#${handoff.target_number}`)] : null,
          handoff.reason ? ` · ${handoff.reason}` : null));
      }
    }
    el.hidden = rows.length === 0;
    setChildren(el, ...rows);
  }

  async function load() {
    try {
      state = await ctx.api.get(`/api/tasks/${encodeURIComponent(task().id)}/handoffs`);
      render();
    } catch {
      // No handoffs shown: the rest of the page stands.
    }
  }

  const off = ctx.store.on('handoffs', (taskId) => { if (taskId === task().id) load(); });
  load();
  return { el, destroy: off, load };
}

/** Settings: the instance's handoff switch (instance administrators), off by default. */
export function handoffSettingCard(ctx) {
  const card = h('section', { class: 'settings-section', 'aria-labelledby': 'handoffs-title' });
  const body = h('div', { class: 'settings-body instance-body' });
  setChildren(card,
    h('div', { class: 'settings-head' }, icon('git-branch', { size: 16 }), h('h2', { id: 'handoffs-title' }, 'Handoffs between sandboxes'), h('span', { class: 'muted small' }, 'tasks starting work in another repository')),
    body);

  function render(state) {
    const mode = h('select', { class: 'input', id: 'handoffs-mode' },
      [['off', 'Off'], ['ask', 'Ask: the task’s owner approves each one'], ['automatic', 'Automatic, within the bounds']].map(([value, label]) => h('option', { value, selected: state.mode === value }, label)));
    const depth = h('input', { class: 'input', type: 'number', id: 'handoffs-depth', min: 1, max: 5, value: state.max_depth });
    const perDay = h('input', { class: 'input', type: 'number', id: 'handoffs-per-day', min: 1, max: 100, value: state.per_day });
    const save = h('button', { type: 'submit', class: 'btn btn-primary btn-sm' }, 'Save');
    const form = h('form', { class: 'jev-form', novalidate: true },
      h('div', { class: 'field' }, h('label', { class: 'field-label', for: 'handoffs-mode' }, 'Mode'), mode),
      h('div', { class: 'field' }, h('label', { class: 'field-label', for: 'handoffs-depth' }, 'Deepest chain'), depth),
      h('div', { class: 'field' }, h('label', { class: 'field-label', for: 'handoffs-per-day' }, 'Handoffs per owner a day'), perDay),
      h('p', { class: 'muted xs' }, 'Off by default. A sandbox’s writers can close it to handoffs on its Settings page. Every handoff is audited on both sandboxes, and its work reaches dev only through its own pull request.'),
      h('div', { class: 'row' }, save));
    form.addEventListener('submit', async (event) => {
      event.preventDefault();
      save.disabled = true;
      try {
        render(await ctx.api.put('/api/admin/handoffs', { mode: mode.value, max_depth: Number(depth.value), per_day: Number(perDay.value) }));
        toast(`Handoffs: ${mode.value}`, { kind: 'ok' });
      } catch (error) {
        errorToast(error);
        save.disabled = false;
      }
    });
    setChildren(body, form);
  }

  ctx.api.get('/api/admin/handoffs').then(render, (error) => {
    setChildren(body, h('div', { class: 'banner banner-danger', role: 'alert' }, icon('warn'), h('span', { class: 'banner-text' }, `Couldn’t load the handoff setting: ${error.message}`)));
  });
  return card;
}

/** A sandbox's opt-out (HOF-002): its writers accept or refuse other sandboxes' handoffs. */
export function sandboxHandoffsCard(ctx, sandbox, canWrite) {
  const card = h('section', { class: 'settings-section', 'aria-labelledby': 'sandbox-handoffs-title' });
  const box = h('input', { type: 'checkbox', id: 'sandbox-handoffs', disabled: true });
  setChildren(card,
    h('div', { class: 'settings-head' }, icon('git-branch', { size: 16 }), h('h2', { id: 'sandbox-handoffs-title' }, 'Handoffs from other sandboxes')),
    h('div', { class: 'settings-body' },
      h('label', { class: 'telegram-choice' }, box, h('span', null, 'Accept handoffs from other sandboxes', h('span', { class: 'muted xs' }, ' · when this instance allows them, tasks of other sandboxes may start tasks here as their owners'))),
      canWrite ? null : h('p', { class: 'muted xs' }, 'Its writers change this.')));
  box.addEventListener('change', async () => {
    const accept = box.checked;
    box.disabled = true;
    try {
      await ctx.api.put(`/api/sandboxes/${encodeURIComponent(sandbox.id)}/handoffs`, { accept });
      toast(accept ? 'This sandbox accepts handoffs' : 'This sandbox refuses handoffs', { kind: 'ok' });
    } catch (error) {
      box.checked = !accept;
      errorToast(error, 'Couldn’t save');
    } finally {
      box.disabled = !canWrite;
    }
  });
  ctx.api.get(`/api/sandboxes/${encodeURIComponent(sandbox.id)}/handoffs`).then((state) => {
    box.checked = Boolean(state.accept);
    box.disabled = !canWrite;
  }, () => { card.hidden = true; });
  return card;
}
