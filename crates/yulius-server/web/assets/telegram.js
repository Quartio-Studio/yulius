// The personal Telegram channel (SPEC-498): the instance's bot, set and removed by an instance
// administrator (Settings → Administration, TGC-001), and each person's own chat, linked with a
// single-use code and unlinked in Settings → Notifications (TGC-010..TGC-020). The token goes to
// the server once and never comes back.

import { h, icon, field, setChildren, toast, errorToast, confirmDialog } from './dom.js';
import { relLong } from './format.js';
import { allows } from './status.js';

/** How often a shown code asks whether the chat was linked meanwhile. */
const LINK_CHECK_MS = 3000;
const CHOICES = [
  ['needs_you', 'Needs you', 'A task waits for you'],
  ['done', 'Done', 'A task finished'],
  ['failed', 'Failed or stopped', 'A task failed to start or stopped abnormally'],
  ['account', 'Subscription problems', 'A subscription must sign in again or hit its limit, or your task moved off a nearly used-up one'],
  ['digest', 'End-of-day digest', 'What waits for you, once a day, when this instance sends one'],
];
const PRIVACY = 'Telegram bot chats are not end-to-end encrypted: notification text passes Telegram’s servers. Agent secrets are removed first.';

function errorBanner(what, error) {
  return h('div', { class: 'banner banner-danger', role: 'alert' }, icon('warn'), h('span', { class: 'banner-text' }, `Couldn’t load ${what}: ${error.message}`));
}

/** The instance's bot (instance administrators): state, token form and Remove. */
export function telegramBotCard(ctx) {
  const card = h('section', { class: 'settings-section', 'aria-labelledby': 'telegram-bot-title' });
  const body = h('div', { class: 'settings-body instance-body' });
  setChildren(card,
    h('div', { class: 'settings-head' }, icon('bell', { size: 16 }), h('h2', { id: 'telegram-bot-title' }, 'Telegram bot'), h('span', { class: 'muted small' }, 'notifications on people’s phones')),
    body);
  let state = null;
  let statusBox = null;
  let refresh = null;
  // Each render (also the one after a Save or a Remove) is a generation: a refresh read started
  // before it is dropped (review round 3: a read answered after Remove brought "is receiving"
  // back).
  let generation = 0;

  function statusText() {
    if (!state.configured) return 'No bot: nobody can link a Telegram chat.';
    const who = state.set_by ? ` by ${state.set_by}` : '';
    const when = state.set_at ? ` ${relLong(state.set_at)}` : '';
    const linked = `${state.linked} ${state.linked === 1 ? 'person' : 'people'} linked`;
    if (state.state === 'off') return `@${state.bot_username} is saved but not running: save its token again.`;
    if (state.state === 'starting') return `@${state.bot_username} is starting: waiting for Telegram's first answer. Set${who}${when}, ${linked}.`;
    if (state.state === 'retrying') return `@${state.bot_username} cannot reach Telegram (${state.last_error || 'error'}); retrying. Set${who}${when}, ${linked}.`;
    return `@${state.bot_username} is receiving. Set${who}${when}, ${linked}.`;
  }

  function render() {
    generation += 1;
    const input = h('input', { class: 'input mono', type: 'password', id: 'settings-telegram-token', autocomplete: 'off', spellcheck: 'false', placeholder: 'Paste the bot token from @BotFather', 'aria-describedby': 'settings-telegram-hint' });
    const save = h('button', { type: 'submit', class: 'btn btn-primary btn-sm', disabled: true }, state.configured ? 'Replace' : 'Save');
    input.addEventListener('input', () => { save.disabled = !input.value.trim(); });
    const form = h('form', { class: 'jev-form', novalidate: true }, field(state.configured ? 'New bot token' : 'Bot token', input, null), h('div', { class: 'row' }, save));
    form.addEventListener('submit', async (event) => {
      event.preventDefault();
      const token = input.value.trim();
      if (!token) return;
      save.disabled = true;
      save.classList.add('busy');
      try {
        state = await ctx.api.put('/api/admin/telegram', { token });
        input.value = '';
        toast(`Telegram bot @${state.bot_username} saved. People can link their chats now.`, { kind: 'ok' });
        render();
      } catch (error) {
        errorToast(error, 'Couldn’t save the bot token');
        save.disabled = false;
      } finally {
        save.classList.remove('busy');
      }
    });
    const remove = state.configured
      ? h('button', { type: 'button', class: 'btn btn-sm btn-danger', onClick: () => removeBot() }, 'Remove')
      : null;
    statusBox = h('div');
    setChildren(body,
      h('p', { class: 'text-2' }, 'One bot serves the whole instance. Each person links their own chat in Settings → Notifications and chooses what reaches it. The server asks Telegram for messages, so no inbound port or public URL is needed. Create a bot with @BotFather and paste its token here.'),
      statusBox,
      remove ? h('div', { class: 'row' }, remove) : null,
      form,
      h('p', { class: 'muted xs', id: 'settings-telegram-hint' }, 'Checked with Telegram, stored encrypted on this server and never shown again. Another bot’s token unlinks every chat.'));
    showStatus();
  }

  // The status line alone, so a token being typed stays. While the bot starts or retries, the
  // state is read again every 2 s until it settles or the card is gone (review round 2).
  function showStatus() {
    const on = state.configured && state.state === 'polling';
    statusBox.className = ['jev-status', on ? 'jev-on' : 'jev-off'].join(' ');
    setChildren(statusBox, icon(on ? 'check' : 'key', { size: 14 }), h('span', { role: 'status' }, statusText()));
    clearTimeout(refresh);
    if (!['starting', 'retrying'].includes(state.state)) return;
    const asked = generation;
    refresh = setTimeout(async () => {
      if (!card.isConnected || asked !== generation) return;
      try {
        const fresh = await ctx.api.get('/api/admin/telegram');
        if (asked !== generation) return;
        state = fresh;
      } catch {
        // Read again at the next turn.
      }
      if (card.isConnected && asked === generation) showStatus();
    }, 2000);
  }

  async function removeBot() {
    if (!(await confirmDialog({ title: 'Remove the Telegram bot?', message: 'Notifications stop at once and every linked chat is unlinked.', confirmLabel: 'Remove', danger: true }))) return;
    try {
      state = await ctx.api.del('/api/admin/telegram');
      toast('Telegram bot removed.', { kind: 'ok' });
      render();
    } catch (error) {
      errorToast(error, 'Couldn’t remove the bot');
    }
  }

  (async () => {
    try {
      state = await ctx.api.get('/api/admin/telegram');
    } catch (error) {
      setChildren(body, errorBanner('the Telegram bot', error));
      return;
    }
    render();
  })();
  return card;
}

/** The person's own chat: Link Telegram with its code, or the linked state with choices. */
export function telegramLink(ctx) {
  const box = h('div', { class: 'telegram-link', 'aria-live': 'polite' });
  let state = null;
  let pending = null; // the shown code: { code, link, expires_at }
  let timer = null;

  const stopChecking = () => { if (timer) clearTimeout(timer); timer = null; };

  async function check() {
    timer = null;
    if (!box.isConnected) return;
    if (pending && Date.parse(pending.expires_at) <= Date.now()) { pending = null; render(); return; }
    try {
      const next = await ctx.api.get('/api/me/telegram');
      if (next.linked) {
        state = next;
        pending = null;
        toast('Telegram linked. Notifications reach your chat now.', { kind: 'ok' });
        render();
        return;
      }
    } catch {
      // A failed check is tried again; the code stays shown.
    }
    if (pending) timer = setTimeout(check, LINK_CHECK_MS);
  }

  async function startLink(button) {
    button.disabled = true;
    try {
      pending = await ctx.api.post('/api/me/telegram/link', {});
      render();
      stopChecking();
      timer = setTimeout(check, LINK_CHECK_MS);
    } catch (error) {
      errorToast(error, 'Couldn’t start linking');
      button.disabled = false;
    }
  }

  async function unlink() {
    if (!(await confirmDialog({ title: 'Unlink Telegram?', message: 'Your chat gets no more notifications. You can link it again any time.', confirmLabel: 'Unlink', danger: true }))) return;
    try {
      state = await ctx.api.del('/api/me/telegram');
      toast('Telegram unlinked.', { kind: 'ok' });
      render();
    } catch (error) {
      errorToast(error, 'Couldn’t unlink Telegram');
    }
  }

  // SPEC-512 TGI-001: replies and new tasks from the chat, off by default, with the project a
  // new message starts its task in (one the person may work in).
  async function saveIntake(enabled, sandboxId, revert) {
    try {
      state = await ctx.api.put('/api/me/telegram/intake', { enabled, sandbox_id: sandboxId || null });
      toast(enabled ? 'Your Telegram chat now answers tasks and starts new ones.' : 'Your Telegram chat only gets notifications now.', { kind: 'ok' });
      render();
    } catch (error) {
      revert();
      errorToast(error, 'Couldn’t save Telegram intake');
    }
  }

  function intakeFieldset() {
    const writable = (ctx.store.state.sandboxes || []).filter((sb) => allows(sb.access, 'write'));
    const intake = state.intake || { enabled: false, sandbox_id: null };
    const current = intake.sandbox_id || (writable[0] && writable[0].id) || '';
    const select = h('select', { class: 'input', id: 'telegram-intake-project', disabled: writable.length === 0 },
      writable.map((sb) => h('option', { value: sb.id, selected: sb.id === current }, sb.name)));
    // Turning it off always works, even with no project left to write in (review round 1).
    const on = h('input', { type: 'checkbox', id: 'telegram-intake', checked: Boolean(intake.enabled), disabled: writable.length === 0 && !intake.enabled });
    on.addEventListener('change', () => saveIntake(on.checked, select.value, () => { on.checked = !on.checked; }));
    select.addEventListener('change', () => { if (on.checked) saveIntake(true, select.value, () => { select.value = intake.sandbox_id || ''; }); });
    return h('fieldset', { class: 'telegram-choices' }, h('legend', { class: 'muted xs' }, 'Work from this chat'),
      h('label', { class: 'telegram-choice' }, on, h('span', null, 'Answer tasks and start new ones', h('span', { class: 'muted xs' }, ' · reply to a notification to answer it; send a message, photo or file to start a task; /status, /task'))),
      h('div', { class: 'field' }, h('label', { class: 'field-label', for: 'telegram-intake-project' }, 'New messages start tasks in'), select),
      h('p', { class: 'muted xs' }, writable.length ? 'Off by default. Tasks start as you, with your access; a reply follows the owner rule.' : 'You need write access to a project to start tasks from Telegram.'));
  }

  async function choose(key, checked, input) {
    const choices = { ...state.choices, [key]: checked };
    try {
      state = await ctx.api.put('/api/me/telegram/choices', choices);
    } catch (error) {
      input.checked = !checked;
      errorToast(error, 'Couldn’t save your choice');
    }
  }

  function render() {
    if (!state.available) {
      setChildren(box, h('p', { class: 'muted xs' }, 'No Telegram bot is set up on this instance yet. An instance administrator adds one in Settings → Administration.'));
      return;
    }
    if (state.linked) {
      stopChecking();
      const choices = CHOICES.filter(([key]) => key !== 'account' || state.account_choice).map(([key, name, about]) => h('label', { class: 'telegram-choice' },
        h('input', { type: 'checkbox', name: `telegram-${key}`, checked: Boolean(state.choices[key]), onChange: (e) => choose(key, e.target.checked, e.target) }),
        h('span', null, name, h('span', { class: 'muted xs' }, ` · ${about}`))));
      setChildren(box,
        h('div', { class: 'row' },
          h('span', { class: 'jev-status jev-on' }, icon('check', { size: 14 }), h('span', null, `Linked to @${state.bot_username}${state.linked_at ? ` ${relLong(state.linked_at)}` : ''}`)),
          h('button', { type: 'button', class: 'btn btn-sm', onClick: unlink }, 'Unlink')),
        h('fieldset', { class: 'telegram-choices' }, h('legend', { class: 'muted xs' }, 'Send me'), choices),
        intakeFieldset(),
        h('p', { class: 'muted xs' }, `Only tasks of sandboxes you can see. At most 5 messages a minute; the rest come as one summary. ${PRIVACY}`));
      return;
    }
    if (pending) {
      setChildren(box,
        h('p', null, 'Open the bot and press Start, or send it this code. The code works once, for 10 minutes.'),
        h('div', { class: 'row' },
          h('a', { class: 'btn btn-primary btn-sm', href: pending.link, target: '_blank', rel: 'noopener noreferrer' }, `Open @${state.bot_username}`),
          h('code', { class: 'mono telegram-code', 'aria-label': 'Link code' }, `/start ${pending.code}`)),
        h('p', { class: 'muted xs', role: 'status' }, 'Waiting for your chat…'));
      return;
    }
    const button = h('button', { type: 'button', class: 'btn btn-sm', onClick: () => startLink(button) }, 'Link Telegram');
    setChildren(box,
      h('div', { class: 'row' }, button),
      h('p', { class: 'muted xs' }, `Needs you, and what else you choose, on your phone even with no tab open. ${PRIVACY}`));
  }

  (async () => {
    try {
      state = await ctx.api.get('/api/me/telegram');
    } catch (error) {
      setChildren(box, errorBanner('your Telegram link', error));
      return;
    }
    render();
  })();
  return box;
}
