// Watches page (SPEC-630 WCH-013): a project's bash scripts on a schedule, run by Yulius in the
// sandbox without an agent. What a script prints becomes a notice for its owner, keyed
// `watch:<name>`. Each watch shows its state, next and last run, last exit, failure and output,
// with Run now, Pause/Resume, Edit and Remove for those who may.

import { enc } from '../api.js';
import { h, badge, itemCard, iconButton, openModal, toast, confirmDialog, errorToast, field, emptyState } from '../dom.js';
import { relLong, dateTime } from '../format.js';
import { allows } from '../status.js';
import { pageFrame, loader, createButton } from './common.js';

const STATE_TONE = { active: 'ok', running: 'warn', paused: '', ended: '' };
const STATE_LABEL = { active: 'Active', running: 'Running', paused: 'Paused', ended: 'Ended' };

/** How often it runs, in words. */
export function scheduleText(w) {
  if (w.every) return `every ${w.every}`;
  const offset = w.utc_offset_minutes || 0;
  const sign = offset < 0 ? '−' : '+';
  const abs = Math.abs(offset);
  const utc = offset ? ` (UTC${sign}${Math.floor(abs / 60)}${abs % 60 ? `:${String(abs % 60).padStart(2, '0')}` : ''})` : ' (UTC)';
  return `cron ${w.cron}${utc}`;
}

/** The body a form's fields make: `{name?, script, every|cron, utc_offset_minutes, expiry, timeout_seconds, secrets}`. */
export function watchBody(values, { withName }) {
  const body = {
    script: values.script,
    utc_offset_minutes: Number(values.offset) || 0,
    secrets: values.secrets.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean),
  };
  if (withName) body.name = values.name.trim();
  if (values.kind === 'cron') body.cron = values.when.trim();
  else body.every = values.when.trim();
  if (values.expiry.trim()) body.expiry = values.expiry.trim();
  if (String(values.timeout).trim()) body.timeout_seconds = Number(values.timeout);
  return body;
}

/** Mount the Watches page. */
export function mountWatchesPage(ctx, sandbox) {
  const write = allows(sandbox.access, 'write');
  const create = () => editor(null);
  const frame = pageFrame({
    title: 'Watches',
    sub: 'scripts on a schedule: no agent, no tokens; what they print becomes your notice',
    actions: [createButton('New watch', create, { disabled: !write, title: write ? null : 'Requires write access' })],
  });
  const host = h('div');
  frame.body.append(host);

  const reload = loader(host, 'the watches', () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/watches`), (list) => {
    const items = Array.isArray(list) ? list : [];
    if (!items.length) {
      return emptyState({ iconName: 'eye', title: 'No watches yet', text: 'Watch a URL, a release, a CI number or a date without spending an agent turn: exit 0 with output posts it to you, no output posts nothing, exit 3 posts and stops the watch.', action: write ? createButton('New watch', create) : null });
    }
    return h('div', { class: 'item-list' }, items.map(watchCard));
  }, { shape: 'items' });

  function lastRun(w) {
    if (!w.last_ended_at) return w.state === 'running' ? 'running now' : 'never ran';
    const exit = w.last_exit === null || w.last_exit === undefined ? '' : ` · exit ${w.last_exit}`;
    return [h('span', { title: dateTime(w.last_ended_at) }, `last run ${relLong(w.last_ended_at)}`), exit];
  }

  function watchCard(w) {
    const meta = h('div', { class: 'item-meta' },
      w.next_run_at ? [h('span', { title: dateTime(w.next_run_at) }, `Next run ${relLong(w.next_run_at)}`), ' · '] : null,
      lastRun(w),
      ` · ${w.runs} run${w.runs === 1 ? '' : 's'}, ${w.posts} post${w.posts === 1 ? '' : 's'}`,
      w.pending_posts ? ` · ${w.pending_posts} waiting to be posted` : null,
      ` · ${w.owner}`);
    const extra = [meta];
    if (w.ended_reason) extra.push(h('div', { class: 'item-meta' }, `Ended: ${w.ended_reason}`));
    if (w.last_failure) extra.push(h('div', { class: 'field-error' }, `Last run failed: ${w.last_failure}${w.failures > 1 ? ` (${w.failures} in a row)` : ''}`));
    if (w.last_output) extra.push(h('details', { class: 'watch-output' }, h('summary', null, 'Last output'), h('pre', { class: 'mono' }, w.last_output)));
    if (w.last_errors) extra.push(h('details', { class: 'watch-output' }, h('summary', null, 'Error output'), h('pre', { class: 'mono' }, w.last_errors)));
    const pausable = w.state === 'active' || w.state === 'running';
    const canToggle = pausable ? (w.can_edit || w.can_remove) : w.can_edit;
    return itemCard({
      iconName: 'eye',
      title: w.name,
      badges: [badge(STATE_LABEL[w.state] || w.state, STATE_TONE[w.state] || ''), w.failures >= 3 ? badge('Failing', 'danger') : null],
      description: [scheduleText(w), h('code', { class: 'muted' }, ` watch:${w.name}`)],
      extra,
      controls: [
        h('button', { type: 'button', class: 'btn btn-sm', disabled: !w.can_edit || w.state === 'running', title: w.can_edit ? 'Run it once now (it posts as usual)' : 'Only its owner runs it', onClick: () => runNow(w) }, 'Run now'),
        h('button', { type: 'button', class: 'btn btn-sm', disabled: !canToggle, title: canToggle ? null : 'Only its owner (or, to pause, a project administrator)', onClick: () => setPaused(w, pausable) }, pausable ? 'Pause' : 'Resume'),
        iconButton('pencil', `Edit ${w.name}`, () => editor(w), { disabled: !w.can_edit }),
        iconButton('trash', `Remove ${w.name}`, () => remove(w), { class: 'btn-quiet-danger', disabled: !w.can_remove }),
      ],
      dim: !pausable,
    });
  }

  async function runNow(w) {
    try {
      await ctx.api.post(`/api/watches/${enc(w.id)}/run`);
      toast(`${w.name}: running now.`, { kind: 'ok' });
      reload();
    } catch (error) {
      errorToast(error, 'Run refused');
    }
  }

  async function setPaused(w, paused) {
    try {
      if (paused) await ctx.api.post(`/api/watches/${enc(w.id)}/pause`);
      else await ctx.api.post(`/api/watches/${enc(w.id)}/resume`);
      toast(`${w.name} ${paused ? 'paused' : 'resumed'}.`, { kind: 'ok' });
      reload();
    } catch (error) {
      errorToast(error, paused ? 'Couldn’t pause the watch' : 'Couldn’t resume the watch');
    }
  }

  async function remove(w) {
    if (!(await confirmDialog({ title: `Remove watch “${w.name}”?`, message: 'It stops, a run that goes is cancelled, and its notices are removed.', confirmLabel: 'Remove', danger: true }))) return;
    try {
      await ctx.api.del(`/api/watches/${enc(w.id)}`);
      toast('Watch removed.', { kind: 'ok' });
      reload();
    } catch (error) {
      errorToast(error, 'Couldn’t remove the watch');
    }
  }

  /** New watch (`w` null) or Edit: one dialog. A name is given once and never changes. */
  function editor(w) {
    const name = h('input', { class: 'input mono', name: 'name', value: w ? w.name : '', disabled: Boolean(w), placeholder: 'release-check', autofocus: !w });
    const script = h('textarea', { class: 'textarea mono', name: 'script', rows: 8, placeholder: 'curl -fsS https://example.com/version | grep -q 2.0 || exit 0\necho "2.0 is out"\nexit 3' });
    script.value = w ? w.script : '';
    const kind = h('select', { class: 'select', name: 'kind', 'aria-label': 'Schedule kind' },
      h('option', { value: 'every' }, 'Every'),
      h('option', { value: 'cron' }, 'Cron'));
    kind.value = w && w.cron ? 'cron' : 'every';
    const when = h('input', { class: 'input mono', name: 'when', value: w ? (w.cron || w.every || '') : '1h', placeholder: '15m, 2h, 1d or 0 9 * * *' });
    const offset = h('input', { class: 'input mono', name: 'offset', type: 'number', value: w ? String(w.utc_offset_minutes || 0) : '0' });
    const expiry = h('input', { class: 'input mono', name: 'expiry', value: w && w.expiry ? w.expiry : '', placeholder: '1d (default), 2h, never' });
    const timeout = h('input', { class: 'input mono', name: 'timeout', type: 'number', value: w ? String(w.timeout_seconds) : '', placeholder: '300' });
    const secrets = h('input', { class: 'input mono', name: 'secrets', value: w ? w.secrets.join(', ') : '', placeholder: 'API_TOKEN, OTHER_KEY' });
    const body = h('div', { class: 'stack' },
      field('Name', name, 'Lowercase letters, digits and dashes. Its posts are keyed watch:<name>, so each replaces the last.'),
      field('Script (bash)', script, 'Exit 0 with output posts it; no output posts nothing; exit 3 posts and stops the watch; other exits fail (3 in a row warn you once).'),
      h('div', { class: 'row' }, field('Schedule', kind), field('Interval or cron', when), field('UTC offset (minutes, cron)', offset)),
      h('div', { class: 'row' }, field('Posts expire after', expiry), field('Timeout (seconds)', timeout)),
      field('Agent secrets', secrets, 'Names the script uses as $NAME; they reach it only through the egress proxy.'));
    openModal({
      title: w ? `Edit ${w.name}` : 'New watch',
      wide: true,
      body,
      actions: [
        { label: 'Cancel' },
        { label: w ? 'Save' : 'Create', kind: 'primary', run: async () => {
          const values = { name: name.value, script: script.value, kind: kind.value, when: when.value, offset: offset.value, expiry: expiry.value, timeout: timeout.value, secrets: secrets.value };
          const sent = watchBody(values, { withName: !w });
          if (w) await ctx.api.put(`/api/watches/${enc(w.id)}`, sent);
          else await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/watches`, sent);
          toast(w ? 'Watch saved.' : 'Watch created; it runs at its first time.', { kind: 'ok' });
          reload();
        } },
      ],
    });
  }

  const off = ctx.store.on('watches', (event) => {
    if (!event || !event.sandbox_id || event.sandbox_id === sandbox.id) reload();
  });
  return { el: frame.el, key: `watches:${sandbox.id}`, destroy: off };
}
