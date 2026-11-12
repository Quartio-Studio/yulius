// Claude Code (superadmin, Settings → Instance; SPEC-461 CCM-008): which Claude Code sandboxes
// get and where it comes from, npm's newest releases at the last check and what that check did,
// loudly when the version in use does not know the default model, with the fix; "Check now"; the
// release channel, changed here without editing the unit; and the tasks whose running session
// still runs an older Claude Code, which move when their turn ends.

import { h, icon, setChildren, toast, errorToast, badge, skeleton } from './dom.js';
import { relLong } from './format.js';
import { modelLabel } from './models.js';
import { BRAND } from './brand.js';

const API = '/api/admin/claude-code';
/** While a check runs the card asks again this often (ms); after a failed read, it tries again. */
const POLL_MS = 2000;
const RETRY_MS = 10000;

/** The channels a superadmin chooses from, and what each means. */
const CHANNELS = [
  ['latest', 'Latest', 'Every release, within the hour of its publication.'],
  ['stable', 'Stable', 'About a week behind Latest; takes Latest’s release when it does not know the default model.'],
  ['off', 'Off', 'Sandboxes run the host’s own install; nothing is checked or installed.'],
];

const channelName = (id) => (CHANNELS.find(([key]) => key === id) || [id, id])[1];

/** "Fable 5.2 (claude-fable-5-2)" for each id. */
function models(ids) {
  return ids.map((id) => `${modelLabel(id)} (${id})`).join(', ');
}

/** The version sandboxes run, where it comes from and what it knows. */
function versionLine(view) {
  const knows = !view.models_read ? null
    : view.problem ? badge('default model unknown', 'danger')
      : view.unknown_models.length ? badge('an offered model unknown', 'warn')
        : badge('knows every offered model', 'ok');
  const from = view.source === 'yulius'
    ? `kept current by ${BRAND.name}${view.host_version ? ` · the host has ${view.host_version}` : ''}`
    : view.source === 'host' ? 'the host’s install' : '';
  return h('div', { class: 'instance-version' },
    h('span', { class: 'muted small' }, 'Sandboxes run'),
    h('span', { class: 'mono' }, view.version ? `Claude Code ${view.version}` : 'no Claude Code'),
    knows,
    from ? h('span', { class: 'muted small' }, from) : null);
}

/** What the last check saw and did. */
function checkLine(view) {
  if (view.checking) {
    return h('p', { class: 'muted small claude-check', role: 'status' }, h('span', { class: 'spinner', 'aria-hidden': 'true' }), ' Checking npm for a newer release…');
  }
  // Nothing reads npm with updates off: what an earlier check saw is not said as today's.
  if (view.channel === 'off') return h('p', { class: 'muted xs' }, 'Updates are off: nothing is checked.');
  const newest = [view.latest ? `latest ${view.latest}` : null, view.stable ? `stable ${view.stable}` : null].filter(Boolean).join(' · ');
  const when = view.checked_at ? `Checked ${relLong(view.checked_at)}` : null;
  if (!when && !newest) {
    return h('p', { class: 'muted xs' }, 'Checked two minutes after the server starts, then every hour.');
  }
  // A check that failed read nothing: the releases are the ones an earlier check saw.
  const has = !newest ? null : view.result === 'failed' ? `npm had ${newest} when it was last read` : `npm has ${newest}`;
  return h('p', { class: 'muted small claude-check' }, [when, has].filter(Boolean).join(': '), '. ', view.result === 'installed' || view.result === 'up_to_date' ? view.result_text : null);
}

/** The warnings below the danger banner: another gap, a refused release, a failed check. */
function warnings(view) {
  const items = [];
  const others = view.unknown_models.filter((id) => id !== view.default_model);
  if (!view.problem && others.length) {
    items.push(`Claude Code ${view.version} does not know ${models(others)}, which the composer offers: a task that asks for it still gets it by id, but a session cannot pick it by itself.`);
  }
  if (view.result === 'refused' || view.result === 'failed') items.push(view.result_text);
  if (view.moves_stopped) items.push(view.moves_stopped);
  if (!items.length) return null;
  return h('div', { class: 'banner banner-warn', role: 'status' }, icon('warn', { size: 16 }),
    h('div', { class: 'banner-text' }, items.length === 1 ? items[0] : h('ul', { class: 'claude-notes' }, items.map((text) => h('li', null, text)))));
}

/** The tasks whose running session still runs an older Claude Code (CCM-013). */
function behind(view) {
  if (!view.behind.length) return null;
  return h('div', { class: 'stack-sm claude-behind' },
    h('h3', { class: 'small', id: 'claude-behind-title' }, `Still on an older Claude Code (${view.behind.length})`),
    h('p', { class: 'muted xs' }, `Each moves to ${view.version} when its turn ends, in the same session, or at its next start. A session with background work waits for it.`),
    h('ul', { class: 'small claude-behind-list', 'aria-labelledby': 'claude-behind-title' }, view.behind.map((task) => {
      const route = `/s/${encodeURIComponent(task.sandbox)}/t/${task.number}`;
      return h('li', null, h('a', { href: route, dataset: { route } }, `#${task.number} ${task.title}`), ' ',
        h('span', { class: 'muted' }, `${task.sandbox} · Claude Code ${task.version}`));
    })));
}

/** The card. */
export function claudeCodeCard(ctx) {
  const body = h('div', { class: 'settings-body instance-body' }, skeleton('Reading Claude Code…', 3, { block: true }));
  // The page has another "Check now" (Yulius's own updates): this one names what it checks.
  const checkButton = h('button', { type: 'button', class: 'btn btn-sm', 'aria-label': 'Check now for a Claude Code release', onClick: () => checkNow() }, icon('refresh', { size: 14 }), 'Check now');
  const card = h('section', { class: 'settings-section claude-code', 'aria-labelledby': 'claude-code-title' },
    h('div', { class: 'settings-head' }, icon('terminal', { size: 16 }), h('h2', { id: 'claude-code-title' }, 'Claude Code'),
      h('span', { class: 'muted small' }, 'the version sandboxes run'), h('span', { class: 'spacer' }), checkButton),
    body);
  let timer = null;
  let gone = false;
  let failed = false;
  /** A channel change is being saved: its controls stay off however the card is drawn meanwhile. */
  let saving = false;
  /** A check is being asked for: "Check now" stays off likewise. */
  let asking = false;
  /** What is drawn: a poll that reads the same leaves the card (its focus, its alert) alone. */
  let drawn = '';
  /** How many changes this card asked for have had their outcome (see `change`). */
  let changes = 0;

  /** The channel choice, and where the channel in effect is set. */
  function channel(view) {
    const select = h('select', { class: 'select', id: 'claude-channel', 'aria-describedby': 'claude-channel-hint', disabled: saving,
      onChange: (event) => setChannel(event.target.value, select) },
    CHANNELS.map(([id, name]) => h('option', { value: id, selected: id === view.channel }, name)));
    const chosen = view.channel_source === 'settings';
    const what = (CHANNELS.find(([id]) => id === view.channel) || [])[2] || '';
    const by = chosen ? `Chosen here${view.channel_set_by ? ` by ${view.channel_set_by}` : ''}${view.channel_set_at ? ` ${relLong(view.channel_set_at)}` : ''}; it wins over the server’s configuration.`
      : 'From the server’s configuration (YULIUS_CLAUDE_UPDATES). A channel chosen here wins over it and needs no restart.';
    return h('div', { class: 'claude-channel' },
      h('label', { class: 'small', for: 'claude-channel' }, 'Release channel'),
      h('div', { class: 'row' }, select,
        chosen ? h('button', { type: 'button', class: 'btn btn-sm btn-ghost', disabled: saving, onClick: () => setChannel(null, select) }, `Use the server’s (${channelName(view.config_channel)})`) : null),
      h('p', { class: 'muted xs', id: 'claude-channel-hint' }, `${what} ${by}`));
  }

  function render(view) {
    clearTimeout(timer);
    if (!gone && view.checking) timer = setTimeout(refresh, POLL_MS);
    const shown = JSON.stringify([view, saving]);
    if (shown === drawn) return;
    drawn = shown;
    checkButton.disabled = asking || !view.managed || view.checking || view.channel === 'off';
    checkButton.classList.toggle('busy', Boolean(view.checking));
    checkButton.title = !view.managed ? `${BRAND.name} keeps nothing current on this server` : view.channel === 'off' ? 'Updates are off' : '';
    const parts = [
      view.problem ? h('div', { class: 'banner banner-danger claude-problem', role: 'alert' }, icon('warn', { size: 16 }),
        h('div', { class: 'banner-text' }, h('strong', null, view.problem), ' ', view.fix)) : null,
      versionLine(view),
      view.managed ? null : h('p', { class: 'small claude-unmanaged' }, view.unmanaged.charAt(0).toUpperCase() + view.unmanaged.slice(1), '.'),
      view.managed ? checkLine(view) : null,
      warnings(view),
      view.managed ? channel(view) : null,
      behind(view),
    ];
    setChildren(body, ...parts.filter(Boolean));
  }

  async function refresh() {
    const asked = changes;
    try {
      const view = await ctx.api.get(API);
      if (gone || asked !== changes) return;
      failed = false;
      render(view);
    } catch (error) {
      if (gone || asked !== changes) return;
      drawn = '';
      setChildren(body, h('p', { class: 'muted small' }, 'Couldn’t read which Claude Code sandboxes run.'));
      // One toast per outage.
      if (!failed) errorToast(error, 'Couldn’t read Claude Code');
      failed = true;
      clearTimeout(timer);
      timer = setTimeout(refresh, RETRY_MS);
    }
  }

  /**
   * Sends a change this card asks for (a check, a channel change) and marks its outcome, taken
   * or refused. From then on the card is drawn again whatever the next view says, because the
   * change's controls were switched off by hand. And a read asked before the outcome may say
   * what was true before the change, so `refresh` drops its answer: the outcome's own draw, or
   * the read after a refusal, is the newer word.
   */
  async function change(send) {
    try {
      return await send();
    } finally {
      changes += 1;
      drawn = '';
    }
  }

  async function checkNow() {
    asking = true;
    checkButton.disabled = true;
    try {
      const view = await change(() => ctx.api.post(`${API}/check`, {}));
      asking = false;
      render(view);
    } catch (error) {
      asking = false;
      errorToast(error, 'Couldn’t check for a Claude Code release');
      refresh();
    }
  }

  /**
   * Follows `id` (null: the server's configuration again); the select goes back when refused.
   * While the change is saved neither the select nor "Use the server's" takes another one.
   */
  async function setChannel(id, select) {
    saving = true;
    for (const control of select.closest('.claude-channel').querySelectorAll('select, button')) control.disabled = true;
    try {
      const view = await change(() => ctx.api.put(`${API}/channel`, { channel: id }));
      saving = false;
      toast(view.channel === 'off' ? 'Claude Code updates are off: sandboxes run the host’s install from their next start.'
        : `Claude Code follows ${channelName(view.channel)} now. Checking for a release…`, { kind: 'ok' });
      render(view);
    } catch (error) {
      saving = false;
      errorToast(error, 'Couldn’t change the channel');
      // The select shows the refused choice: the card is drawn again from what the server says.
      refresh();
    }
  }

  refresh();
  return {
    el: card,
    refresh,
    destroy() {
      gone = true;
      clearTimeout(timer);
    },
  };
}
