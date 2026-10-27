// The instance's service keys, Jev (SPEC-001 ADM-001, ADM-002) and Vidoc (SPEC-195 VDK-003,
// VDK-004): optional steps of the setup wizard and cards on the instance page where a superadmin
// sets, replaces, tests and removes them. A key goes to the server once and never comes back: the
// API answers with its state only. The Vidoc card also switches each project's scans off or on
// (SPEC-338 VOF-005).

import { enc } from './api.js';
import { h, icon, field, setChildren, toast, errorToast, confirmDialog, skeleton, toggle } from './dom.js';
import { relLong } from './format.js';
import { BRAND } from './brand.js';

/** What each key is for, and how the card speaks about it. */
const SERVICES = {
  jev: {
    name: 'Jev',
    api: '/api/admin/jev',
    title: 'Task intake with Jev',
    cardTitle: 'Jev key',
    cardSub: 'task intake',
    placeholder: 'Paste the Jev API key',
    configFile: 'YULIUS_JEV_API_KEY_FILE',
    off: 'No key: task intake checks nothing.',
    offAfterRemove: ' Task intake then checks nothing.',
    savedToast: 'Jev key saved. Task intake uses it now.',
    testing: 'Asking Jev one small question…',
    explanation: () => [
      'Jev (by TypeSafe) gives task intake its judgement: it suggests splitting a new task that mixes separate pieces of work, and it holds a prompt typed into a running task when it changes the task’s scope, until someone decides. ',
      `Without a key both checks are off and ${BRAND.name} works as usual.`],
  },
  vidoc: {
    name: 'Vidoc',
    api: '/api/admin/vidoc',
    title: 'Security scans with Vidoc',
    cardTitle: 'Vidoc key',
    cardSub: 'security scans',
    placeholder: 'Paste the Vidoc API key',
    configFile: 'YULIUS_VIDOC_API_KEY_FILE',
    off: 'No key: security scans are off and report “not run”.',
    offAfterRemove: ' Security scans then report “not run”.',
    savedToast: 'Vidoc key saved. Agents and CI jobs scan with it now.',
    testing: 'Asking Vidoc for its models…',
    explanation: () => [
      'Vidoc’s vidoc-1 model reviews code for security vulnerabilities: agents scan their changes before delivery, and a project’s security CI job scans every pull request. The code being scanned is sent to Vidoc; agents never see the key. ',
      h('span', null, 'Get a key at ', h('a', { href: 'https://model.vidocsecurity.com/#key', target: '_blank', rel: 'noopener noreferrer' }, 'model.vidocsecurity.com'), '. '),
      `Without a key the scan is off and ${BRAND.name} works as usual.`],
  },
};

/** What a service's key does here, in a few sentences (setup and settings say the same). */
export function keyExplanation(service) {
  return h('p', { class: 'text-2' }, ...SERVICES[service].explanation());
}

function keyInput(service, id) {
  return h('input', { class: 'input mono', type: 'password', id, autocomplete: 'off', spellcheck: 'false', placeholder: SERVICES[service].placeholder, 'aria-describedby': `${id}-hint` });
}

/**
 * Turns `sandbox`'s Vidoc scans on or off (SPEC-338 VOF-001). Answers the updated sandbox, or null
 * after saying why it failed; a switch `input` goes back then. `quiet` skips the success toast.
 */
export async function setVidocScans(ctx, sandbox, on, { input = null, quiet = false } = {}) {
  if (input) input.disabled = true;
  try {
    const updated = await ctx.api.patch(`/api/sandboxes/${enc(sandbox.id)}`, { vidoc: on });
    ctx.store.upsertSandbox(updated);
    if (!quiet) toast(on ? `Vidoc scans are on for ${sandbox.name}.` : `Vidoc scans are off for ${sandbox.name}: it scans nothing until you switch them on.`, { kind: 'ok' });
    return updated;
  } catch (error) {
    if (input) input.checked = !on;
    errorToast(error, 'Couldn’t change the Vidoc scans');
    return null;
  } finally {
    if (input) input.disabled = false;
  }
}

/** The Vidoc card's projects, each with its switch (SPEC-338 VOF-005); playgrounds scan nothing. */
function vidocProjects(ctx) {
  const projects = (ctx.store.state.sandboxes || []).filter((sb) => !sb.playground);
  if (!projects.length) return null;
  return h('div', { class: 'vidoc-projects', role: 'group', 'aria-labelledby': 'vidoc-projects-title' },
    h('h3', { class: 'small', id: 'vidoc-projects-title' }, 'Projects'),
    h('p', { class: 'muted xs' }, 'A project switched off scans nothing: its agents and CI report the scan as disabled, and nobody is asked about the key or its quota, until you switch it on again. Use it while the key’s quota is used up.'),
    h('ul', { class: 'vidoc-project-list' }, projects.map((sb) => h('li', { class: 'row vidoc-project' },
      h('span', { class: 'vidoc-project-name' }, sb.name),
      toggle({ checked: !sb.profile || sb.profile.vidoc !== false, label: `Vidoc scans for ${sb.name}`, onChange: (on, input) => setVidocScans(ctx, sb, on, { input }) })))));
}

/** The result of a test request, in words, with its tone. */
function testLine(result) {
  const tone = result.ok ? 'ok' : result.result === 'refused' ? 'danger' : 'warn';
  const latency = result.ok && typeof result.latency_ms === 'number' ? ` (${result.latency_ms} ms)` : '';
  return h('div', { class: ['jev-test', `jev-test-${tone}`], role: 'status' }, icon(result.ok ? 'check' : 'warn', { size: 14 }), h('span', null, `${result.message}${latency}`));
}

/**
 * An optional step of the setup wizard: save (and test) a key, or skip. `onDone()` continues
 * either way; `step` labels it (for example 'Step 3 of 5 · Optional') and `next` says what follows.
 */
export function keySetupCard(ctx, service, { onDone, header = null, step = 'Optional', next: after = null }) {
  const s = SERVICES[service];
  const input = keyInput(service, `setup-${service}`);
  const result = h('div');
  const save = h('button', { type: 'submit', class: 'btn btn-primary', disabled: true }, 'Save and test');
  const skip = h('button', { type: 'button', class: 'btn btn-ghost', onClick: () => onDone() }, 'Skip for now');
  const next = h('button', { type: 'button', class: 'btn btn-primary', hidden: true, onClick: () => onDone() }, 'Continue');
  input.addEventListener('input', () => { save.disabled = !input.value.trim(); });
  const form = h('form', { class: 'auth-card', novalidate: true },
    header,
    h('div', { class: 'auth-step' }, step),
    h('h1', { class: 'auth-title' }, s.title),
    keyExplanation(service),
    field(`${s.name} API key`, input, null),
    h('p', { class: 'muted xs', id: `setup-${service}-hint` }, 'Stored encrypted on this server and never shown again. You can add, replace or remove it later in Settings → Instance.'),
    result,
    h('div', { class: 'row' }, save, next, skip),
    after ? h('p', { class: 'auth-next muted small' }, after) : null);
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (!input.value.trim()) return;
    save.disabled = true;
    save.classList.add('busy');
    setChildren(result);
    try {
      await ctx.api.put(s.api, { key: input.value.trim() });
      input.value = '';
      const tested = await ctx.api.post(`${s.api}/test`, {});
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

/** The instance page's card for a service's key: its state and the actions on it. */
export function keySettingsCard(ctx, service) {
  const s = SERVICES[service];
  const card = h('section', { class: 'settings-section', 'aria-labelledby': `${service}-title` });
  const body = h('div', { class: 'settings-body instance-body' });
  setChildren(card,
    h('div', { class: 'settings-head' }, icon('key', { size: 16 }), h('h2', { id: `${service}-title` }, s.cardTitle), h('span', { class: 'muted small' }, s.cardSub)),
    body);
  let state = null;
  const result = h('div');

  async function load() {
    try {
      state = await ctx.api.get(s.api);
    } catch (error) {
      setChildren(body, h('div', { class: 'banner banner-danger', role: 'alert' }, icon('warn'), h('span', { class: 'banner-text' }, `Couldn’t load the ${s.name} key: ${error.message}`)));
      return;
    }
    render();
  }

  function statusText() {
    if (!state.configured) return s.off;
    if (state.source === 'config') return `In use: the key from the server’s configuration (${s.configFile}).`;
    const when = state.set_at ? ` ${relLong(state.set_at)}` : '';
    return `In use: saved here${state.set_by ? ` by ${state.set_by}` : ''}${when}.`;
  }

  function render() {
    const input = keyInput(service, `settings-${service}`);
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
        state = await ctx.api.put(s.api, { key });
        input.value = '';
        toast(s.savedToast, { kind: 'ok' });
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
      keyExplanation(service),
      h('div', { class: ['jev-status', state.configured ? 'jev-on' : 'jev-off'] }, icon(state.configured ? 'check' : 'key', { size: 14 }), h('span', null, statusText())),
      state.source === 'config'
        ? h('p', { class: 'muted xs' }, '“Save it here” stores that key encrypted in settings, so the file can be removed from the service. A key saved here always wins over the file.')
        : null,
      state.config_error ? h('p', { class: 'muted xs' }, `${s.configFile} is set but not usable: ${state.config_error}`) : null,
      h('div', { class: 'row' }, testButton, adopt, remove),
      result,
      form,
      h('p', { class: 'muted xs', id: `settings-${service}-hint` }, 'Stored encrypted on this server and never shown again.'),
      service === 'vidoc' ? vidocProjects(ctx) : null);
  }

  async function test() {
    setChildren(result, h('div', { class: 'jev-test muted', role: 'status' }, h('span', { class: 'spinner', 'aria-hidden': 'true' }), h('span', null, s.testing)));
    try {
      setChildren(result, testLine(await ctx.api.post(`${s.api}/test`, {})));
    } catch (error) {
      setChildren(result, h('div', { class: 'field-error', role: 'alert' }, error.message || 'The test failed.'));
    }
  }

  async function removeKey() {
    const fallback = state.config_present ? ' The key from the server’s configuration is used again.' : s.offAfterRemove;
    if (!(await confirmDialog({ title: `Remove the ${s.name} key?`, message: `The saved key is deleted.${fallback}`, confirmLabel: 'Remove', danger: true }))) return;
    try {
      state = await ctx.api.del(s.api);
      setChildren(result);
      toast(`${s.name} key removed.`, { kind: 'ok' });
      render();
    } catch (error) {
      errorToast(error, 'Couldn’t remove the key');
    }
  }

  async function adoptKey() {
    try {
      state = await ctx.api.post(`${s.api}/adopt`, {});
      toast('The configured key is now saved here.', { kind: 'ok' });
      render();
    } catch (error) {
      errorToast(error, 'Couldn’t save the key');
    }
  }

  setChildren(body, skeleton(`Loading the ${s.name} key…`, 3, { block: true }));
  load();
  return card;
}
