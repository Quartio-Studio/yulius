// The Jev key (SPEC-001 ADM-001, ADM-002): the optional step of the setup wizard and the card on
// the instance page where a superadmin sets, replaces, tests and removes it. The key goes to the
// server once and never comes back: the API answers with its state only.

import { h, icon, field, setChildren, toast, errorToast, confirmDialog } from './dom.js';
import { relLong } from './format.js';
import { BRAND } from './brand.js';

/** What Jev does here, in two sentences (setup and settings say the same). */
export function jevExplanation() {
  return h('p', { class: 'text-2' },
    'Jev (by TypeSafe) gives task intake its judgement: it suggests splitting a new task that mixes separate pieces of work, and it holds a prompt typed into a running task when it changes the task’s scope, until someone decides. ',
    `Without a key both checks are off and ${BRAND.name} works as usual.`);
}

function keyInput(id) {
  return h('input', { class: 'input mono', type: 'password', id, autocomplete: 'off', spellcheck: 'false', placeholder: 'Paste the Jev API key', 'aria-describedby': `${id}-hint` });
}

/** The result of a test request, in words, with its tone. */
function testLine(result) {
  const tone = result.ok ? 'ok' : result.result === 'refused' ? 'danger' : 'warn';
  const latency = result.ok && typeof result.latency_ms === 'number' ? ` (${result.latency_ms} ms)` : '';
  return h('div', { class: ['jev-test', `jev-test-${tone}`], role: 'status' }, icon(result.ok ? 'check' : 'warn', { size: 14 }), h('span', null, `${result.message}${latency}`));
}

/**
 * The setup wizard's optional step: save (and test) a key, or skip. `onDone()` continues to the
 * app either way.
 */
export function jevSetupCard(ctx, { onDone, header = null }) {
  const input = keyInput('setup-jev');
  const result = h('div');
  const save = h('button', { type: 'submit', class: 'btn btn-primary', disabled: true }, 'Save and test');
  const skip = h('button', { type: 'button', class: 'btn btn-ghost', onClick: () => onDone() }, 'Skip for now');
  const next = h('button', { type: 'button', class: 'btn btn-primary', hidden: true, onClick: () => onDone() }, 'Continue');
  input.addEventListener('input', () => { save.disabled = !input.value.trim(); });
  const form = h('form', { class: 'auth-card', novalidate: true },
    header,
    h('div', { class: 'auth-step' }, 'Optional · last step'),
    h('h1', { class: 'auth-title' }, 'Task intake with Jev'),
    jevExplanation(),
    field('Jev API key', input, null),
    h('p', { class: 'muted xs', id: 'setup-jev-hint' }, 'Stored encrypted on this server and never shown again. You can add, replace or remove it later in Settings → Instance.'),
    result,
    h('div', { class: 'row' }, save, next, skip));
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (!input.value.trim()) return;
    save.disabled = true;
    save.classList.add('busy');
    setChildren(result);
    try {
      await ctx.api.put('/api/admin/jev', { key: input.value.trim() });
      input.value = '';
      const tested = await ctx.api.post('/api/admin/jev/test', {});
      setChildren(result, testLine(tested));
      if (tested.ok) {
        save.hidden = true;
        skip.hidden = true;
        next.hidden = false;
        next.focus();
      } else {
        // Saved but not working: keep the choice with the person.
        save.disabled = false;
        skip.textContent = 'Continue anyway';
      }
    } catch (error) {
      setChildren(result, h('div', { class: 'field-error', role: 'alert' }, error.message || 'Couldn’t save the key.'));
      save.disabled = false;
    } finally {
      save.classList.remove('busy');
    }
  });
  setTimeout(() => input.focus(), 30);
  return form;
}

/** The instance page's card: the key's state and the actions on it. */
export function jevSettingsCard(ctx) {
  const card = h('section', { class: 'settings-section', 'aria-labelledby': 'jev-title' });
  const body = h('div', { class: 'settings-body instance-body' });
  setChildren(card,
    h('div', { class: 'settings-head' }, icon('key', { size: 16 }), h('h2', { id: 'jev-title' }, 'Jev key'), h('span', { class: 'muted small' }, 'task intake')),
    body);
  let state = null;
  const result = h('div');

  async function load() {
    try {
      state = await ctx.api.get('/api/admin/jev');
    } catch (error) {
      setChildren(body, h('div', { class: 'banner banner-danger', role: 'alert' }, icon('warn'), h('span', { class: 'banner-text' }, `Couldn’t load the Jev key: ${error.message}`)));
      return;
    }
    render();
  }

  function statusText() {
    if (!state.configured) return 'No key: task intake checks nothing.';
    if (state.source === 'config') return 'In use: the key from the server’s configuration (YULIUS_JEV_API_KEY_FILE).';
    const when = state.set_at ? ` ${relLong(state.set_at)}` : '';
    return `In use: saved here${state.set_by ? ` by ${state.set_by}` : ''}${when}.`;
  }

  function render() {
    const input = keyInput('settings-jev');
    const save = h('button', { type: 'submit', class: 'btn btn-primary btn-sm', disabled: true }, state.source === 'settings' ? 'Replace' : 'Save');
    input.addEventListener('input', () => { save.disabled = !input.value.trim(); });
    const form = h('form', { class: 'jev-form', novalidate: true }, field(state.source === 'settings' ? 'New key' : 'Key', input, null), h('div', { class: 'row' }, save));
    form.addEventListener('submit', async (event) => {
      event.preventDefault();
      const key = input.value.trim();
      if (!key) return;
      save.disabled = true;
      save.classList.add('busy');
      try {
        state = await ctx.api.put('/api/admin/jev', { key });
        input.value = '';
        toast('Jev key saved. Task intake uses it now.', { kind: 'ok' });
        render();
        test();
      } catch (error) {
        errorToast(error, 'Couldn’t save the key');
        save.disabled = false;
      } finally {
        save.classList.remove('busy');
      }
    });
    const testButton = h('button', { type: 'button', class: 'btn btn-sm', disabled: !state.configured, onClick: () => test() }, 'Test');
    const remove = state.source === 'settings'
      ? h('button', { type: 'button', class: 'btn btn-sm btn-danger', onClick: () => removeKey() }, 'Remove')
      : null;
    const adopt = state.source === 'config'
      ? h('button', { type: 'button', class: 'btn btn-sm', onClick: () => adoptKey() }, 'Save it here')
      : null;
    setChildren(body,
      jevExplanation(),
      h('div', { class: ['jev-status', state.configured ? 'jev-on' : 'jev-off'] }, icon(state.configured ? 'check' : 'key', { size: 14 }), h('span', null, statusText())),
      state.source === 'config'
        ? h('p', { class: 'muted xs' }, '“Save it here” stores that key encrypted in settings, so the file can be removed from the service. A key saved here always wins over the file.')
        : null,
      state.config_error ? h('p', { class: 'muted xs' }, `YULIUS_JEV_API_KEY_FILE is set but not usable: ${state.config_error}`) : null,
      h('div', { class: 'row' }, testButton, adopt, remove),
      result,
      form,
      h('p', { class: 'muted xs', id: 'settings-jev-hint' }, 'Stored encrypted on this server and never shown again.'));
  }

  async function test() {
    setChildren(result, h('div', { class: 'jev-test muted', role: 'status' }, h('span', { class: 'spinner', 'aria-hidden': 'true' }), h('span', null, 'Asking Jev one small question…')));
    try {
      setChildren(result, testLine(await ctx.api.post('/api/admin/jev/test', {})));
    } catch (error) {
      setChildren(result, h('div', { class: 'field-error', role: 'alert' }, error.message || 'The test failed.'));
    }
  }

  async function removeKey() {
    const fallback = state.config_present ? ' The key from the server’s configuration is used again.' : ' Task intake then checks nothing.';
    if (!(await confirmDialog({ title: 'Remove the Jev key?', message: `The saved key is deleted.${fallback}`, confirmLabel: 'Remove', danger: true }))) return;
    try {
      state = await ctx.api.del('/api/admin/jev');
      setChildren(result);
      toast('Jev key removed.', { kind: 'ok' });
      render();
    } catch (error) {
      errorToast(error, 'Couldn’t remove the key');
    }
  }

  async function adoptKey() {
    try {
      state = await ctx.api.post('/api/admin/jev/adopt', {});
      toast('The configured key is now saved here.', { kind: 'ok' });
      render();
    } catch (error) {
      errorToast(error, 'Couldn’t save the key');
    }
  }

  setChildren(body, h('div', { class: 'placeholder placeholder-loading' }, 'Loading…'));
  load();
  return card;
}
