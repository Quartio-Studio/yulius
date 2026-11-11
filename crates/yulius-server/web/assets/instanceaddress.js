// The instance page's Address card (SPEC-466 MVC-011): the public URL and whether GitHub accepts
// it for sign-in. After a move to another URL GitHub refuses every sign-in until a person adds the
// new callback URL to the GitHub App (GitHub has no API for it): the card gives the one link and
// the two values to paste, and reads the answer again until it is registered. After a move it says
// where the instance came from and what its first start did with the tasks.

import { h, icon, setChildren, toast, errorToast, skeleton, loadError } from './dom.js';
import { relLong, dateTime, plural } from './format.js';

const API = '/api/admin/instance/address';
/** How often an answer that is not "registered" is read again while the card is shown. */
const REREAD_MS = 5000;

/** What the first start after a move did with the tasks that were live, in one sentence. */
export function movedTasks(moved) {
  if (!moved.queued && !moved.left && !moved.failed) return 'No task was live when it moved.';
  const said = [`put ${plural(moved.queued, 'task')} back in the queue`];
  if (moved.left) said.push(`left ${moved.left} stopped that a person had paused or was stopping`);
  if (moved.failed) said.push(`could not queue ${moved.failed}: ${moved.failed === 1 ? 'its owner finds it' : 'their owners find them'} under Needs you`);
  return `Its first start ${said.length > 1 ? `${said.slice(0, -1).join(', ')}, and ${said[said.length - 1]}` : said[0]}.`;
}

/** A value to paste on GitHub, with its copy button. */
function pasteValue(key, label, value, note) {
  const copy = h('button', { type: 'button', class: 'btn btn-sm', 'aria-label': `Copy the ${label}`, onClick: async () => {
    try {
      await navigator.clipboard.writeText(value);
      toast(`The ${label} is copied.`, { kind: 'ok' });
    } catch {
      toast('The browser blocked the clipboard: select the value and copy it.', { kind: 'warn' });
    }
  } }, icon('copy', { size: 14 }), 'Copy');
  return h('div', { class: 'address-value' },
    h('dt', null, label[0].toUpperCase() + label.slice(1)),
    h('dd', null, h('code', { class: 'mono', dataset: { addressValue: key } }, value), copy, note ? h('span', { class: 'muted xs' }, note) : null));
}

/** The card. */
export function addressCard(ctx) {
  const body = h('div', { class: 'settings-body instance-body' }, skeleton('Loading the address…', 2, { block: true }));
  const check = h('button', { type: 'button', class: 'btn btn-sm', hidden: true, onClick: () => load(true) }, icon('refresh', { size: 14 }), 'Check again');
  const card = h('section', { class: 'settings-section address', 'aria-labelledby': 'address-title' },
    h('div', { class: 'settings-head' }, icon('globe', { size: 16 }), h('h2', { id: 'address-title' }, 'Address'), h('span', { class: 'muted small' }, 'public URL, GitHub sign-in'), h('span', { class: 'spacer' }), check),
    body);
  let timer = null;
  let gone = false;

  function github(g) {
    if (!g) return h('p', { class: 'muted small' }, 'No GitHub App is connected, so there is nothing to register on GitHub: people sign in once the App is connected under GitHub connection.');
    const checked = g.checked_at ? `Checked ${relLong(g.checked_at)}.` : '';
    if (g.state === 'registered') {
      return h('p', { class: 'address-ok' }, icon('check', { size: 16 }), 'GitHub accepts this address for sign-in. ', h('span', { class: 'muted xs' }, checked));
    }
    if (g.state === 'missing') {
      return h('div', { class: 'banner banner-warn address-missing' }, icon('warn', { size: 16 }),
        h('div', { class: 'banner-text' },
          h('strong', null, 'GitHub sign-in doesn’t work at this address yet.'),
          h('p', null, 'The GitHub App doesn’t hold this instance’s callback URL, so GitHub refuses every sign-in from here. Add the two values below in the App’s settings and save; password sign-in works meanwhile.'),
          h('div', null, h('a', { class: 'btn btn-sm btn-primary', href: g.settings_url, target: '_blank', rel: 'noopener noreferrer' }, icon('external', { size: 14 }), 'Open the App’s settings on GitHub')),
          h('dl', { class: 'address-values' },
            pasteValue('callback', 'callback URL', g.callback_url, 'Add it to the callback URLs. An old one may stay until the old host is retired.'),
            pasteValue('homepage', 'homepage URL', g.homepage_url, 'Replace the old homepage.')),
          h('p', { class: 'muted xs' }, `Asked again while this page is open, and said here once GitHub accepts it. ${checked}`)));
    }
    return h('div', { class: 'banner banner-info' }, icon('info', { size: 16 }),
      h('span', { class: 'banner-text' }, `GitHub didn’t say whether it accepts this address: ${g.reason || 'no answer yet'}. Sign-in is not affected by that. ${checked}`));
  }

  function render(view) {
    const moved = view.moved;
    setChildren(body,
      h('div', { class: 'instance-version' }, h('span', { class: 'muted small' }, 'This instance answers at'), h('span', { class: 'mono' }, view.public_url)),
      moved ? h('p', { class: 'small address-moved' },
        `Moved here${moved.from ? ` from ${moved.from}` : ''} ${relLong(moved.at)} (${dateTime(moved.at)}). ${movedTasks(moved)}`) : null,
      // What GitHub answers is news when it changes while the page is open.
      h('div', { role: 'status' }, github(view.github)));
    check.hidden = !view.github;
  }

  /** Reads the address; `ask` makes the server ask GitHub now. */
  async function load(ask = false) {
    clearTimeout(timer);
    check.disabled = true;
    let view = null;
    try {
      view = ask ? await ctx.api.post(`${API}/check`, {}) : await ctx.api.get(API);
    } catch (error) {
      if (ask) {
        errorToast(error, 'Couldn’t ask GitHub');
        // What the card shows stays, and is read again as before.
        if (!gone) timer = setTimeout(() => load(), REREAD_MS);
      } else if (!gone) setChildren(body, loadError(`Couldn’t load the address: ${error.message}`, () => load()));
    }
    check.disabled = false;
    if (gone || !view) return;
    render(view);
    // Until GitHub accepts the address the answer is read again: the server asks GitHub anew.
    if (view.github && view.github.state !== 'registered') timer = setTimeout(() => load(), REREAD_MS);
  }

  load();
  return { el: card, destroy() { gone = true; clearTimeout(timer); } };
}
