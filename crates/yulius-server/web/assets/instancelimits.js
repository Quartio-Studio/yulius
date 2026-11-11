// The instance page's Resource limits card (SPEC-459 HSL-008): this host's size and, per limit,
// the value in force, where it comes from (host-sized, the service unit, a setting saved here)
// and the host-sized formula. A superadmin saves a number (it wins over the unit), goes back to
// host-sized or removes the setting; numbers pinned on a host of another size are listed in a
// notice that takes the host-sized values or keeps them.

import { enc } from './api.js';
import { h, icon, setChildren, toast, errorToast, badge, skeleton, loadError } from './dom.js';
import { relLong, plural } from './format.js';

const API = '/api/admin/limits';
const GIB = 1024 ** 3;
const SOURCES = { default: 'host-sized', unit: 'unit', settings: 'settings' };
const SOURCE_TITLES = {
  default: 'Sized from this host',
  unit: 'Set by the variable in the server’s service unit',
  settings: 'A number saved here',
};

/** "64 CPUs · 755 GiB memory · 3.5 TB data disk", as far as the host reports it. */
export function hostSize(host) {
  const parts = [];
  if (host.cpus) parts.push(plural(host.cpus, 'CPU'));
  if (host.memory_bytes) parts.push(`${Math.floor(host.memory_bytes / GIB)} GiB memory`);
  if (host.disk_bytes) {
    const gb = host.disk_bytes / 1e9;
    parts.push(`${gb >= 1000 ? `${(gb / 1000).toFixed(1)} TB` : `${Math.floor(gb)} GB`} data disk`);
  }
  return parts.length ? parts.join(' · ') : 'a host of unknown size';
}

/** A limit's number with what it counts; no sandbox disk quota at 0. */
export function amount(limit, value) {
  if (limit.key === 'sandbox_disk_gb' && value === 0) return 'no quota';
  return limit.suffix ? `${value} ${limit.suffix}` : String(value);
}

/** Where a limit's value comes from and what else is known about it, in sentences. */
export function limitNote(limit) {
  const said = [];
  const saved = limit.setting;
  const savedBy = saved ? `${saved.set_by === 'migration' ? 'by the move to this host' : `by ${saved.set_by}`} ${relLong(saved.set_at)}` : '';
  const sized = `${amount(limit, limit.host_sized)} (${limit.formula})`;
  if (limit.source === 'settings') said.push(`Saved here ${savedBy}. Host-sized would be ${sized}.`);
  else if (limit.source === 'unit') said.push(`Set by ${limit.env} in the service unit. Host-sized would be ${sized}.`);
  else said.push(`Host-sized: ${limit.formula}.${saved ? ` Chosen here ${savedBy}.` : ''}`);
  if (limit.unit_value !== null && limit.unit_value !== undefined && limit.source !== 'unit') {
    said.push(`The unit’s ${limit.env}=${limit.unit_value} is overruled.`);
  }
  if (limit.stale && limit.configured_on) said.push(`Configured on ${hostSize(limit.configured_on)}.`);
  if (limit.restart_needed) said.push('Applies at the next restart of the server.');
  else if (!limit.live) said.push('A change applies at the next restart of the server.');
  return said.join(' ');
}

/** The card. */
export function limitsCard(ctx) {
  const body = h('div', { class: 'settings-body instance-body' }, skeleton('Loading the resource limits…', 4, { block: true }));
  const sub = h('span', { class: 'muted small' }, 'sized from this host');
  const card = h('section', { class: 'settings-section limits', 'aria-labelledby': 'limits-title' },
    h('div', { class: 'settings-head' }, icon('cpu', { size: 16 }), h('h2', { id: 'limits-title' }, 'Resource limits'), sub),
    body);
  let view = null;
  /** What a save was refused with, per limit. */
  const refused = new Map();
  /** What is typed and not saved yet, per limit. */
  const typed = new Map();

  /** One request that answers the new view; `key` names the limit a refusal belongs to. */
  async function change(request, done, key = null) {
    for (const button of card.querySelectorAll('button')) button.disabled = true;
    let failed = false;
    try {
      view = await request();
      if (key) { refused.delete(key); typed.delete(key); }
      toast(done, { kind: 'ok' });
    } catch (error) {
      failed = true;
      if (key && error && error.status === 422) refused.set(key, error.message);
      else {
        errorToast(error, 'Couldn’t change the limits');
        // The change may be in force all the same (its audit entry failed): show what is.
        try { view = await ctx.api.get(API); } catch { /* the view from before stays */ }
      }
    }
    // The pressed button is drawn anew: after a failure the focus goes to the limit's field.
    if (card.isConnected) render(failed ? key : null);
  }

  function notice(stale) {
    if (!stale.length) return null;
    return h('div', { class: 'banner banner-warn limits-notice', role: 'status', 'aria-label': 'Limits sized for another host' },
      icon('warn'),
      h('div', { class: 'banner-text' },
        h('strong', null, `${plural(stale.length, 'limit')} ${stale.length === 1 ? 'was' : 'were'} sized for a host of another size.`),
        h('ul', { class: 'limits-stale' }, stale.map((l) => h('li', null,
          `${l.label}: ${amount(l, l.value)}, configured on ${hostSize(l.configured_on || {})}; this host would get ${amount(l, l.host_sized)}.`))),
        h('div', { class: 'row' },
          h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: () => change(() => ctx.api.post(`${API}/adapt`, {}), 'The limits are sized for this host now.') }, 'Use host-sized values'),
          h('button', { type: 'button', class: 'btn btn-sm', onClick: () => change(() => ctx.api.post(`${API}/keep`, {}), 'The numbers stay, recorded for this host.') }, 'Keep these numbers'))));
  }

  function limitRow(l) {
    const error = refused.get(l.key);
    const errorId = `limit-error-${l.key}`;
    const input = h('input', {
      class: 'input limit-input', type: 'number', inputmode: 'numeric', min: String(l.min), max: String(l.max), step: '1',
      value: typed.has(l.key) ? typed.get(l.key) : String(l.value), 'aria-label': l.label,
      'aria-invalid': error ? 'true' : null, 'aria-describedby': error ? errorId : null,
    });
    const save = h('button', { type: 'submit', class: 'btn btn-sm btn-primary', 'aria-label': `Save ${l.label}` }, 'Save');
    const unchanged = () => input.value.trim() === '' || (Number(input.value) === l.value && l.source === 'settings');
    save.disabled = unchanged();
    const said = error ? h('div', { class: 'field-error', role: 'alert', id: errorId }, error) : null;
    input.addEventListener('input', () => {
      typed.set(l.key, input.value);
      save.disabled = unchanged();
      // A refusal was about the number before this one.
      if (refused.delete(l.key) && said) {
        said.remove();
        input.removeAttribute('aria-invalid');
        input.removeAttribute('aria-describedby');
      }
    });
    const hostSized = l.source === 'default' ? null : h('button', {
      type: 'button', class: 'btn btn-sm', 'aria-label': `Host-sized ${l.label}`, title: `Use ${amount(l, l.host_sized)}, whatever the unit says`,
      onClick: () => change(() => ctx.api.put(`${API}/${enc(l.key)}`, { value: null }), `${l.label} is host-sized now.`, l.key),
    }, 'Host-sized');
    const remove = l.setting ? h('button', {
      type: 'button', class: 'btn btn-sm', 'aria-label': `Remove setting of ${l.label}`, title: 'The unit’s value applies again, else the host-sized one',
      onClick: () => change(() => ctx.api.del(`${API}/${enc(l.key)}`), `The setting of ${l.label} is removed.`, l.key),
    }, 'Remove setting') : null;
    const form = h('form', { class: 'row limit-form', novalidate: true }, input, l.suffix ? h('span', { class: 'muted small' }, l.suffix) : null, save, hostSized, remove);
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      if (input.value.trim() === '') return;
      const value = Number(input.value);
      // The server takes whole numbers only: anything else is said here, at the field.
      if (!Number.isSafeInteger(value) || value < 0) {
        typed.set(l.key, input.value);
        refused.set(l.key, `${l.label} must be a whole number between ${l.min} and ${l.max}${l.suffix ? ` ${l.suffix}` : ''}`);
        render(l.key);
        return;
      }
      change(() => ctx.api.put(`${API}/${enc(l.key)}`, { value }), `${l.label} saved${l.live ? ': in force now' : ': it applies at the next restart'}.`, l.key);
    });
    return h('li', { class: 'limit', 'data-limit': l.key },
      h('div', { class: 'row limit-head' },
        h('h3', { class: 'limit-name' }, l.label),
        h('span', { class: 'limit-value' }, amount(l, l.value)),
        badge(SOURCES[l.source] || l.source, l.stale ? 'warn' : '', SOURCE_TITLES[l.source])),
      h('p', { class: 'muted xs limit-note' }, limitNote(l)),
      form,
      said);
  }

  /** Draws the card; `focus` names the limit whose field takes the focus (the one refused). */
  function render(focus = null) {
    sub.textContent = hostSize(view.host);
    setChildren(body,
      h('p', { class: 'text-2 limits-intro' }, 'How much work this server takes at once. Without a number each limit is sized from this host and follows it; a number saved here wins over the service unit’s YULIUS_* variable.'),
      notice(view.limits.filter((l) => l.stale)),
      h('ul', { class: 'limits-list' }, view.limits.map(limitRow)));
    const field = focus ? body.querySelector(`[data-limit="${focus}"] input`) : null;
    if (field) field.focus();
  }

  async function load() {
    try {
      view = await ctx.api.get(API);
    } catch (error) {
      if (card.isConnected) setChildren(body, loadError(`Couldn’t load the resource limits: ${error.message}`, load));
      return;
    }
    if (card.isConnected) render();
  }

  load();
  return card;
}
