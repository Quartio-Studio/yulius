// The instance page's "Move to another host" card (SPEC-470 MVB-030..MVB-037): a superadmin moves
// the whole instance to another host from here. Root's helper runs the same command as on the
// host (`yulius instance migrate`, SPEC-460): the plan first, with everything a person has to do
// and the key line to add on the new host when nobody can sign in for it, then the move behind a
// typed confirmation, its checklist as it grows, and an abort that says what it does. While the
// instance is frozen for the copy the card says so and keeps trying; when this address redirects
// to the new one it links there, and on the new instance it shows the finished checklist.

import { h, icon, setChildren, toast, errorToast, confirmDialog, badge, skeleton, loadError, invalid, keepFocus } from './dom.js';
import { relLong, dateTime, plural } from './format.js';
import { BRAND } from './brand.js';

const API = '/api/admin/instance/migration';
/** While a request waits or a run is in flight, the view is read again this often. */
const POLL_MS = 3000;
/** A read while a move runs gives up this early: a frozen server answers nothing at all. */
const FOLLOW_TIMEOUT_MS = 8000;
/** The route of this page, for the link to it at the new address. */
const PAGE = '/admin/instance';

const PHASES = [
  ['Plan', 'Plan', 'Both hosts are checked again. Nothing is changed.'],
  ['Prepare', 'Prepare', `The new host is set up while ${BRAND.name} keeps working here.`],
  ['Cutover', 'Cutover', `${BRAND.name} is frozen, its data is copied and proven, and the new instance is started.`],
  ['Finish', 'Finish', 'This address is redirected and the move’s key is removed.'],
];

const OUTCOMES = {
  ok: ['ok', 'ok', 'check'],
  changed: ['changed', 'accent', 'check'],
  manual: ['yours', 'warn', 'user'],
  failed: ['failed', 'danger', 'x'],
};

/** The host of `root@host`. */
export function hostOf(target) {
  const text = String(target || '');
  const at = text.indexOf('@');
  return at < 0 ? text : text.slice(at + 1);
}

/** The lines of a checklist a person has to act on, and the failed ones. */
export function attention(lines) {
  const all = lines || [];
  return { yours: all.filter((l) => l.outcome === 'manual'), failed: all.filter((l) => l.outcome === 'failed') };
}

/** "changed 3, left alone 12, manual 2, failed 0", as the command's own summary counts. */
export function summary(lines) {
  const count = (outcome) => (lines || []).filter((l) => l.outcome === outcome).length;
  return `changed ${count('changed')}, left alone ${count('ok')}, manual ${count('manual')}, failed ${count('failed')}`;
}

/** What the plan said about the time frozen, if it measured the link. */
export function estimate(lines) {
  const line = (lines || []).find((l) => l.name === 'downtime' && l.outcome !== 'failed');
  return line ? line.detail : '';
}

/** A new public URL as the move decides one (`https://host`), or null: nothing else is linked. */
export function linkable(url) {
  return /^https:\/\/[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$/.test(String(url || '')) ? url : null;
}

/** What a request names, in words. */
function valuesText(run) {
  const said = [run.target || 'the recorded move'];
  if (run.ssh_port && run.ssh_port !== 22) said.push(`SSH port ${run.ssh_port}`);
  if (run.public_url) said.push(`public URL ${run.public_url}`);
  if (run.data_disks && run.data_disks.length) said.push(`data ${plural(run.data_disks.length, 'disk')} ${run.data_disks.join(' and ')}`);
  return said.join(', ');
}

let fieldIds = 0;
/**
 * A field whose label is its name alone: the hint describes the control (`aria-describedby`)
 * instead of being read as part of its name.
 */
function labelled(label, control, hint) {
  fieldIds += 1;
  const id = `move-field-${fieldIds}`;
  control.id = id;
  const note = hint instanceof Node ? hint : hint ? h('span', { class: 'field-hint' }, hint) : null;
  if (note) {
    note.id = `${id}-hint`;
    control.setAttribute('aria-describedby', note.id);
  }
  return h('div', { class: 'field' }, h('label', { class: 'field-label', for: id }, label), control, note);
}

/** Says what is wrong under its field, and keeps the field's hint as its description. */
function complain(control, message) {
  const hint = document.getElementById(`${control.id}-hint`);
  invalid(control, message);
  if (!hint) return false;
  control.setAttribute('aria-describedby', `${control.getAttribute('aria-describedby')} ${hint.id}`);
  // After the next edit removed the message, the hint describes the field again.
  const restore = () => control.setAttribute('aria-describedby', hint.id);
  control.addEventListener('input', restore, { once: true });
  control.addEventListener('change', restore, { once: true });
  return false;
}

/** One line of a checklist. */
function lineItem(line) {
  const [word, tone, mark] = OUTCOMES[line.outcome] || [line.outcome, '', 'info'];
  return h('li', { class: ['move-line', `move-line-${line.outcome}`] },
    h('span', { class: 'move-line-mark' }, icon(mark, { size: 14 }), badge(word, tone)),
    h('span', { class: 'move-line-text' }, h('strong', null, line.name), ' ', h('span', { class: 'move-line-detail' }, line.detail)));
}

/** A checklist: its lines in order. */
function checklist(lines, label) {
  return h('ol', { class: 'move-lines', 'aria-label': label }, (lines || []).map(lineItem));
}

/**
 * The lines a person has to act on and the failed ones, repeated before the whole checklist;
 * `keyShown`: the key to add has its own box, so its line is not said twice.
 */
function attentionBlocks(lines, keyShown = false) {
  const found = attention(lines);
  const failed = found.failed;
  const yours = keyShown ? found.yours.filter((l) => l.name !== 'one-time key') : found.yours;
  return [
    failed.length ? h('div', { class: 'banner banner-danger move-attention', role: 'alert' }, icon('warn', { size: 16 }),
      h('div', { class: 'banner-text' }, h('strong', null, `Failed (${failed.length})`),
        h('ul', { class: 'move-attention-list' }, failed.map((l) => h('li', null, h('strong', null, l.name), ': ', l.detail))))) : null,
    yours.length ? h('div', { class: 'banner banner-warn move-attention' }, icon('user', { size: 16 }),
      h('div', { class: 'banner-text' }, h('strong', null, `Yours to do (${yours.length})`),
        h('ul', { class: 'move-attention-list' }, yours.map((l) => h('li', null, h('strong', null, l.name), ': ', l.detail))))) : null,
  ];
}

/** The phases of a running move, as the update's stepper shows its own. */
function stepper(phase) {
  const index = Math.max(0, PHASES.findIndex(([name]) => name === phase));
  return h('ol', { class: 'update-steps move-phases', 'aria-label': 'Phases of the move' }, PHASES.map(([, title, text], i) => {
    const status = i < index ? 'done' : i === index ? 'current' : 'next';
    return h('li', { class: ['update-step', `update-step-${status}`], 'aria-current': status === 'current' ? 'step' : null },
      h('span', { class: 'update-step-mark', 'aria-hidden': 'true' }, status === 'done' ? icon('check', { size: 12 }) : status === 'current' ? h('span', { class: 'spinner' }) : String(i + 1)),
      h('div', { class: 'update-step-body' },
        h('div', { class: 'update-step-title' }, title, status === 'current' ? h('span', { class: 'sr-only' }, ' (in progress)') : null),
        h('div', { class: 'muted xs' }, text)));
  }));
}

/** The card. */
export function moveCard(ctx) {
  const body = h('div', { class: 'settings-body instance-body' }, skeleton('Loading the move…', 3, { block: true }));
  const card = h('section', { class: 'settings-section move', 'aria-labelledby': 'move-title' },
    h('div', { class: 'settings-head' }, icon('forward', { size: 16 }), h('h2', { id: 'move-title' }, 'Move to another host'), h('span', { class: 'muted small' }, 'plan, run migration, abort')),
    body);

  // The form's controls live as long as the card: what is typed survives every read.
  const target = h('input', { type: 'text', class: 'input mono', name: 'move-target', autocomplete: 'off', spellcheck: 'false', placeholder: 'root@new-host' });
  const port = h('input', { type: 'number', class: 'input', name: 'move-port', min: 1, max: 65535, value: '22', inputmode: 'numeric' });
  const url = h('input', { type: 'text', class: 'input mono', name: 'move-url', autocomplete: 'off', spellcheck: 'false' });
  const disks = h('input', { type: 'text', class: 'input mono', name: 'move-disks', autocomplete: 'off', spellcheck: 'false', placeholder: '/dev/nvme0n1, /dev/nvme1n1' });
  const planButton = h('button', { type: 'submit', class: 'btn btn-primary' }, icon('search', { size: 14 }), 'Show the plan');
  const urlHint = h('span', { class: 'field-hint' });
  const form = h('form', { class: 'move-form', 'aria-label': 'The new host', onSubmit: (event) => { event.preventDefault(); plan(); } },
    h('div', { class: 'move-fields' },
      labelled('New host', target, `As ssh takes it. ${BRAND.name} reaches it as root with a one-time key that it removes at the end.`),
      labelled('SSH port', port),
      labelled('New public URL', url, urlHint),
      labelled('Data disks', disks, 'Optional: one blank disk of the new host for the data directory, or two for a mirror. The plan lists the blank disks when space is short.')),
    h('div', { class: 'row' }, planButton, h('span', { class: 'muted small' }, 'The plan changes nothing and freezes nothing.')));

  /** How often a move in flight is read (the mock's `movepoll=` shortens it for tests). */
  const pollMs = (ctx.api.transport && ctx.api.transport.movePollMs) || POLL_MS;
  let timer = null;
  let gone = false;
  /** The number of the last read or ask: an earlier one that answers later is dropped. */
  let latest = 0;
  /** The view the card shows; null before the first answer. */
  let shown = null;
  /** Whether the form was filled from the view once. */
  let filled = false;
  /** An ask is on its way: its buttons wait. */
  let asking = false;
  /** The move this page follows across the freeze: its id, host, new URL and estimate. */
  let followed = null;
  /** Reads fail while the followed move runs: the instance is frozen for the copy. */
  let frozen = false;
  /** This address answers with a redirect: the instance runs at its new URL. */
  let redirected = false;

  const inFlight = (view) => Boolean(view && ((view.request && view.request.state === 'requested') || (view.run && view.run.state === 'running')));

  /** What is typed, as the API takes it. */
  function typedValues() {
    return {
      target: target.value.trim(),
      ssh_port: Number(port.value) || 0,
      public_url: url.value.trim() || null,
      data_disks: disks.value.split(/[\s,]+/).filter(Boolean),
    };
  }

  /** The values of the helper's last run: a move is the plan that was shown. */
  function runValues(run) {
    return { target: run.target, ssh_port: run.ssh_port, public_url: run.public_url, data_disks: run.data_disks || [] };
  }

  function helperNote() {
    return h('div', { class: 'banner banner-info', role: 'note' }, icon('shield', { size: 16 }),
      h('div', { class: 'banner-text' },
        h('strong', null, 'Moving from this page needs the move helper on this server.'),
        ' Its two units (yulius-migrate.path and yulius-migrate.service) are in the operations guide; they let this page ask root for a move, so root enables them on purpose. Without them, run ',
        h('code', null, 'sudo yulius instance migrate root@new-host'), ' over SSH on this host: it shows the same plan and asks the same things.'));
  }

  function keyBox(view) {
    const key = view.move.public_key;
    const copy = h('button', { type: 'button', class: 'btn btn-sm', 'aria-label': 'Copy the key line', onClick: async () => {
      try {
        await navigator.clipboard.writeText(key);
        toast('The key line is copied.', { kind: 'ok' });
      } catch {
        toast('The browser blocked the clipboard: select the line and copy it.', { kind: 'warn' });
      }
    } }, icon('copy', { size: 14 }), 'Copy');
    const host = view.run && view.run.target ? view.run.target : view.move.target;
    return h('div', { class: 'banner banner-info move-key' }, icon('key', { size: 16 }),
      h('div', { class: 'banner-text' },
        h('strong', null, 'Add the move’s key on the new host, then show the plan again.'),
        h('p', null, `${BRAND.name} reaches ${host} with a one-time key and removes it at the end. Nobody can sign in for it from this page, so add this line to root’s ~/.ssh/authorized_keys there. The plan ended here; nothing was changed.`),
        h('div', { class: 'move-key-line' }, h('code', { class: 'mono', dataset: { moveKey: '' } }, key), copy)));
  }

  /** How the helper's last run stands, in a sentence and a tone. */
  function runBlock(view) {
    const { run, request } = view;
    const move = view.move;
    if (request && request.state === 'unanswered') {
      return h('div', { class: 'banner banner-warn', role: 'status' }, icon('warn', { size: 16 }),
        h('div', { class: 'banner-text' }, h('strong', null, 'The move helper did not take the request.'),
          ' Its path unit may not be enabled on this server: ', h('code', null, 'sudo systemctl enable --now yulius-migrate.path'), ', then ask again.'));
    }
    if (request && request.state === 'requested' && !(run && run.state === 'running')) {
      const what = { plan: 'the plan', move: 'the move', abort: 'the abort' }[request.action];
      return h('div', { class: 'row move-waiting', role: 'status' }, h('span', { class: 'spinner', 'aria-hidden': 'true' }), h('span', null, `Asking the move helper for ${what}…`));
    }
    if (!run) return null;
    const of = valuesText(run);
    if (run.state === 'running') {
      const stopping = request && request.action === 'abort';
      if (run.action === 'move') {
        return h('div', { class: 'update-run move-run', role: 'status' },
          h('div', { class: 'row' }, h('strong', null, stopping ? 'Stopping the move' : `Moving to ${hostOf(run.target)}`), h('span', { class: 'muted small' }, of)),
          stepper(move ? move.phase : 'Plan'));
      }
      const text = stopping ? 'Stopping it…' : run.action === 'abort' ? 'Aborting the move…' : `Planning the move to ${of}…`;
      return h('div', { class: 'row move-waiting', role: 'status' }, h('span', { class: 'spinner', 'aria-hidden': 'true' }), h('span', null, text));
    }
    if (run.state === 'refused') {
      return h('div', { class: 'banner banner-danger', role: 'alert' }, icon('warn', { size: 16 }),
        h('div', { class: 'banner-text' }, h('strong', null, 'The move helper refused the request.'), ` ${run.error || ''} Nothing was changed.`));
    }
    if (run.state === 'lost') {
      return h('div', { class: 'banner banner-warn', role: 'status' }, icon('warn', { size: 16 }),
        h('div', { class: 'banner-text' }, h('strong', null, 'The move helper stopped without a word.'),
          ` It was killed, or this host restarted, ${relLong(run.updated_at)}. The checklist below is how far it got${run.action === 'move' ? ': continue the move, or abort it' : ''}.`));
    }
    // Ended.
    const when = run.ended_at ? relLong(run.ended_at) : '';
    const outcome = (tone, title, text) => h('div', { class: ['update-outcome', tone && `update-outcome-${tone}`, 'move-outcome'], role: 'status' },
      h('div', { class: 'row' }, icon(tone === 'ok' ? 'check' : tone === 'danger' ? 'warn' : 'info', { size: 16 }), h('strong', null, title), h('span', { class: 'muted small' }, when)),
      text ? h('p', { class: 'small' }, text) : null,
      run.error ? h('p', { class: 'small move-error' }, run.error) : null);
    const failed = move ? attention(move.lines).failed.length : 0;
    if (run.action === 'plan') {
      if (move && move.public_key) return null; // the key box says it
      if (run.exit_code === 0 && !failed) return outcome('ok', `The plan for ${of}`, `Nothing is frozen, and nothing was changed but the move’s own files. Its lines: ${summary(move ? move.lines : [])}.`);
      if (failed) return outcome('danger', `The plan for ${of} found problems`, 'Nothing is frozen, and nothing was changed but the move’s own files. Fix what failed, then show the plan again.');
      return outcome('danger', 'The plan could not be shown', run.exit_code === 2 ? 'The command refused before it changed anything.' : 'The command ended before its plan.');
    }
    if (run.action === 'abort') {
      if (run.exit_code === 0) return outcome('', 'The move was aborted', `${BRAND.name} runs on this host as before.`);
      return outcome('danger', 'The abort did not complete', run.exit_code === 2 ? 'Nothing was changed.' : 'The checklist says what is left; abort again once that is done.');
    }
    if (run.exit_code === 0) return outcome('ok', `Moved to ${hostOf(run.target)}`, null);
    return outcome('danger', 'The move did not complete',
      run.exit_code === null
        ? `It was stopped. ${BRAND.name} answers here; continue the move with the same values, or abort it.`
        : `The checklist says where it stopped. ${BRAND.name} answers here; continue the move with the same values, or abort it.`);
  }

  /** The checklist of the command, with what needs a person first. */
  function moveBlock(view) {
    const move = view.move;
    if (!move) return [];
    const head = move.by_command
      ? h('p', { class: 'small move-recorded' }, icon('terminal', { size: 14 }),
        ` A move to ${move.target} is recorded on this host by the command line (yulius instance migrate), phase ${move.phase}, last line ${relLong(move.updated_at)}. Whether it still runs is not known here.`)
      : null;
    const all = move.lines || [];
    return [
      head,
      ...attentionBlocks(all, Boolean(move.public_key)),
      all.length ? h('details', { class: 'move-checklist', open: true },
        h('summary', null, `Checklist (${plural(all.length, 'line')})`),
        checklist(all, 'Checklist')) : null,
    ];
  }

  function arrivedBlock(view) {
    const arrived = view.arrived;
    if (!arrived) return null;
    const { yours, failed } = attention(arrived.lines);
    const just = followed && followed.id === arrived.id;
    return h('div', { class: ['move-arrived', just && 'move-arrived-now'] },
      just ? h('div', { class: 'update-outcome update-outcome-ok', role: 'status' },
        h('div', { class: 'row' }, icon('check', { size: 16 }), h('strong', null, `The move finished: ${BRAND.name} now runs on the new host.`)),
        h('p', { class: 'small' }, 'This page is answered by the new instance. The old host keeps a frozen copy.')) : null,
      h('p', { class: 'small move-arrived-head' },
        `${BRAND.name} was moved to this host ${relLong(arrived.finished_at)} (${dateTime(arrived.finished_at)})${arrived.from ? `, from ${arrived.from}` : ''}. ${summary(arrived.lines)}.`),
      failed.length || yours.length ? h('div', { class: 'stack-sm' },
        h('strong', { class: 'small' }, 'Still for a person, as the move left it:'),
        h('ul', { class: 'move-attention-list small' }, [...failed, ...yours].map((l) => h('li', null, h('strong', null, l.name), ': ', l.detail)))) : null,
      h('details', { class: 'move-checklist', open: just ? true : null },
        h('summary', null, `The finished checklist (${plural(arrived.lines.length, 'line')})`),
        checklist(arrived.lines, 'The finished checklist')));
  }

  function frozenBlock() {
    const host = hostOf(followed.target);
    if (redirected) {
      const there = followed.newUrl ? `${followed.newUrl}${PAGE}` : null;
      return h('div', { class: 'update-outcome update-outcome-ok move-moved', role: 'status' },
        h('div', { class: 'row' }, icon('check', { size: 16 }), h('strong', null, followed.newUrl ? `${BRAND.name} now runs at ${followed.newUrl}.` : `${BRAND.name} now runs on the new host.`)),
        h('p', { class: 'small' }, 'This address redirects there. Sign in again at the new address (a sign-in belongs to one address); this page there shows the finished checklist.'),
        h('div', { class: 'row' }, there
          ? h('a', { class: 'btn btn-primary', href: there }, icon('external', { size: 14 }), `Open ${BRAND.name} at its new address`)
          : h('button', { type: 'button', class: 'btn btn-primary', onClick: () => window.location.reload() }, icon('refresh', { size: 14 }), 'Reload to follow the redirect')));
    }
    return h('div', { class: 'banner banner-info move-frozen', role: 'status' }, h('span', { class: 'spinner', 'aria-hidden': 'true' }),
      h('div', { class: 'banner-text' },
        h('strong', null, `${BRAND.name} is frozen while its data is copied to ${host}.`),
        h('p', null, `Nothing answers here meanwhile. ${followed.estimate ? `The plan said: ${followed.estimate}. ` : ''}This page keeps trying and shows what answers: ${BRAND.name} on the new host, or here again if the move did not complete.`)));
  }

  function actions(view) {
    const run = view.run;
    const can = view.can || {};
    const buttons = [];
    if (can.move && run) {
      const again = run.action === 'move';
      buttons.push(h('button', { type: 'button', class: 'btn btn-primary', disabled: asking, onClick: () => runMigration(view) }, icon('forward', { size: 14 }), again ? 'Continue the move…' : 'Run migration…'));
    }
    if (can.abort) {
      buttons.push(h('button', { type: 'button', class: 'btn btn-danger', disabled: asking, onClick: () => abort(view) }, icon('x', { size: 14 }), 'Abort…'));
    }
    return buttons.length ? h('div', { class: 'row move-actions' }, ...buttons) : null;
  }

  function render() {
    const view = shown;
    if (!view) return;
    if (frozen && followed) {
      // The last checklist stays under the word about the freeze.
      keepFocus(body, () => setChildren(body, ...[frozenBlock(), ...moveBlock({ move: followed.move })].filter(Boolean)));
      return;
    }
    if (!filled) {
      filled = true;
      const from = view.run && view.run.target ? view.run : null;
      if (from && !target.value) {
        target.value = from.target;
        port.value = String(from.ssh_port || 22);
        url.value = from.public_url || '';
        disks.value = (from.data_disks || []).join(', ');
      }
    }
    urlHint.textContent = `Leave empty to keep ${view.public_url}, or its form for the new host’s address when it is made of this host’s address.`;
    const busy = inFlight(view);
    for (const control of [target, port, url, disks]) control.disabled = busy || !view.can.plan;
    planButton.disabled = asking || !view.can.plan;
    const parts = [
      h('p', { class: 'muted small move-intro' }, `${BRAND.name} checks and sets up the new host, copies every repository, secret and setting, starts there and redirects this address. It asks for everything a person has to do in the plan, before anything freezes.`),
      arrivedBlock(view),
      view.update_running ? h('p', { class: 'muted small', role: 'note' }, 'An update or restart is running: a move can be asked once it has finished.') : null,
      view.helper_installed ? null : helperNote(),
      runBlock(view),
      view.move && view.move.public_key && !view.move.by_command && !busy ? keyBox(view) : null,
      ...moveBlock(view),
      actions(view),
      // The form is for the next plan: not while a run is in flight.
      view.helper_installed && !busy ? form : null,
    ];
    // A click re-renders the card: focus stays on the control that had it.
    keepFocus(body, () => setChildren(body, ...parts.filter(Boolean)));
  }

  /** Remembers the move a run is of, so the card can speak for it while nothing answers. */
  function follow(view) {
    const { run, request } = view;
    const moving = (run && run.action === 'move' && run.state === 'running')
      || (request && request.action === 'move' && request.state === 'requested');
    if (!moving) {
      // A move that ended while this address answers is no longer followed; one that finished
      // on the new host is said once more there (its checklist arrived with it).
      if (followed && !(view.arrived && view.arrived.id === followed.id)) followed = null;
      return;
    }
    const move = view.move;
    followed = {
      id: move ? move.id : followed ? followed.id : null,
      target: (run && run.target) || (move && move.target) || (followed && followed.target) || '',
      newUrl: linkable(move && move.new_url) || (followed && followed.newUrl) || null,
      estimate: (move && estimate(move.lines)) || (followed && followed.estimate) || '',
      move: move || (followed && followed.move) || null,
    };
  }

  function show(view) {
    shown = view;
    frozen = false;
    redirected = false;
    follow(view);
    render();
  }

  function schedule() {
    clearTimeout(timer);
    timer = null;
    if (gone) return;
    if (frozen || inFlight(shown)) timer = setTimeout(() => load(), pollMs);
  }

  /** Whether this address answers with a redirect now: the old URL leads to the new instance. */
  async function redirects() {
    try {
      const response = await ctx.api.transport.fetch('/healthz', { method: 'GET', redirect: 'manual', cache: 'no-store', credentials: 'omit' });
      return Boolean(response && response.type === 'opaqueredirect');
    } catch {
      return false;
    }
  }

  async function load() {
    const mine = ++latest;
    clearTimeout(timer);
    let view = null;
    let failure = null;
    try {
      view = await ctx.api.get(API, followed ? { timeoutMs: FOLLOW_TIMEOUT_MS } : undefined);
    } catch (error) {
      failure = error;
    }
    if (gone || mine !== latest) return;
    if (!failure) {
      show(view);
    } else if (followed && (failure.status === 0 || failure.status >= 502)) {
      // Nothing answers while a move runs: the instance is frozen for the copy, or already
      // answers elsewhere.
      frozen = true;
      redirected = await redirects();
      if (gone || mine !== latest) return;
      render();
    } else if (!shown) {
      setChildren(body, loadError(`Couldn’t load the move: ${failure.message}`, () => load()));
      return;
    }
    schedule();
  }

  /** Asks for a plan, a move or an abort; the answer is the new view. */
  async function ask(payload, failed) {
    const mine = ++latest;
    clearTimeout(timer);
    asking = true;
    render();
    let view = null;
    let failure = null;
    try {
      view = await ctx.api.post(API, payload);
    } catch (error) {
      failure = error;
    }
    asking = false;
    if (gone) return false;
    if (failure) {
      render();
      failed(failure);
      // It may have moved on meanwhile (a 409): show where it stands.
      load();
      return false;
    }
    if (mine === latest) show(view);
    schedule();
    // What was asked is answered at the top of the card, which may be a long way up.
    card.scrollIntoView({ block: 'start' });
    return true;
  }

  function plan() {
    const values = typedValues();
    if (!values.target) return complain(target, 'Name the new host: root@new-host.');
    return ask({ action: 'plan', ...values }, (error) => {
      const message = error.message || '';
      if (error.status === 422 && /new host/.test(message)) complain(target, message);
      else if (error.status === 422 && /SSH port/.test(message)) complain(port, message);
      else if (error.status === 422 && /public URL/.test(message)) complain(url, message);
      else if (error.status === 422 && /disk/.test(message)) complain(disks, message);
      else errorToast(error, 'Couldn’t ask for the plan');
    });
  }

  async function runMigration(view) {
    const run = view.run;
    const values = runValues(run);
    const host = hostOf(values.target);
    const lines = view.move ? view.move.lines : [];
    const said = estimate(lines);
    const yours = attention(lines).yours.length;
    const again = run.action === 'move';
    const ok = await confirmDialog({
      title: again ? `Continue the move to ${host}?` : `Move ${BRAND.name} to ${host}?`,
      message: h('div', { class: 'stack-sm' },
        h('p', null, 'Root on this host will give this instance’s data to ', h('strong', null, values.target), `: every repository, every secret and key, and the settings of its service. ${BRAND.name} is started there, and this host keeps a frozen copy.`),
        h('ol', { class: 'update-confirm-steps' },
          h('li', null, `The new host is set up while ${BRAND.name} keeps working here.`),
          h('li', null, `${BRAND.name} is frozen for the copy: every task and terminal pauses and nothing answers here.${said ? ` The plan said: ${said}.` : ''}`),
          h('li', null, 'The copy is proven and the new instance started. Until that start a failure, or an abort, leaves everything running here.'),
          yours ? h('li', null, `${plural(yours, 'step')} of the plan ${yours === 1 ? 'is' : 'are'} yours (marked “yours”): a DNS record, the GitHub App, what is not carried.`) : null)),
      confirmLabel: again ? 'Continue the move' : 'Run migration',
      danger: true,
      typeToConfirm: host,
    });
    if (!ok) return;
    await ask({ action: 'move', ...values, confirm: host }, (error) => errorToast(error, 'Couldn’t start the move'));
  }

  async function abort(view) {
    const running = view.run && view.run.state === 'running';
    const ok = await confirmDialog({
      title: 'Abort the move?',
      message: h('div', { class: 'stack-sm' },
        h('ul', { class: 'update-confirm-steps' },
          running ? h('li', null, 'The run in flight is stopped first.') : null,
          h('li', null, `${BRAND.name} keeps running on this host; if the move had frozen it, it is thawed.`),
          h('li', null, 'The move’s one-time key is removed from the new host and deleted here, and the recorded move is forgotten.'),
          h('li', null, 'What was already installed on the new host (packages, the service’s unit, the proxy’s configuration, a data disk) stays there.'),
          h('li', null, 'If the new instance had been started, it is stopped and disabled there first. When the new host cannot be reached, the abort stops and names the command to run on this host: it never guesses that the new instance is stopped.'))),
      confirmLabel: 'Abort the move',
      danger: true,
    });
    if (!ok) return;
    await ask({ action: 'abort' }, (error) => errorToast(error, 'Couldn’t abort the move'));
  }

  load();
  return {
    el: card,
    destroy() {
      gone = true;
      clearTimeout(timer);
    },
  };
}
