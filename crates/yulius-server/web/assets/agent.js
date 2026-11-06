// The Superagent (SPEC-074 SUP-007; the Agent of SPEC-035, formerly the Cockpit of SPEC-021
// CKP-008): the user's own plain Claude Code session in this sandbox, in the right tool panel beside
// the page, for quick work that should not become a task. One per user and sandbox, opened (or started, resuming its last conversation) by
// POST /api/sandboxes/{sid}/cockpit; its terminal is the shell terminal protocol. There is no way
// to delete it: /clear starts over inside it. The panel shows it and the person's Global
// Superagent (SPEC-427) as two tabs, This sandbox and All sandboxes (SPEC-428).

import { enc } from './api.js';
import { BRAND } from './brand.js';
import { h, icon, setChildren, emptyState, loadError, skeleton, toast, errorToast } from './dom.js';
import { createTerminalView } from './terminal.js';
import { allows } from './status.js';
import { href } from './router.js';
import { attachTerminalDrop, shellAttachments } from './filedrop.js';
import { isPhone } from './theme.js';
import { readJson, writeJson } from './store.js';

const TAB_KEY = 'yulius.superagentTab';
/** SPEC-428 SAT-001: the panel's tabs, local first. */
const TABS = [['sandbox', 'This sandbox'], ['global', 'All sandboxes']];
/** The tab the person picked last, as this browser stored it (SAT-002): This sandbox until one is picked. */
export const chosenSuperagentTab = () => (readJson(TAB_KEY, 'sandbox') === 'global' ? 'global' : 'sandbox');
let panelSeq = 0;

/**
 * The Superagent panel of `sandbox` (null outside a sandbox): This sandbox and All sandboxes as tabs,
 * as Notes shows its notepads (SPEC-428). `global()` is the page's one Global Superagent view
 * (SAT-004): the panel showing its tab holds it, so its session is attached once and its box keeps
 * what was typed from one sandbox to the next. A session starts when its tab first shows. `tab()` is
 * the page's tab in front in every sandbox, which `onTab` updates (review round 2).
 */
export function createSuperagentPanel(ctx, sandbox, { global, tab, onTab }) {
  const seq = ++panelSeq;
  const id = (part, key) => `superagent-${seq}-${part}-${key}`;
  const tablist = h('div', { class: 'term-tabs superagent-tabs', role: 'tablist', 'aria-label': 'Superagents' });
  const panes = {};
  for (const [key] of TABS) panes[key] = h('div', { class: 'superagent-pane', id: id('pane', key), role: 'tabpanel', 'aria-labelledby': id('tab', key), hidden: true });
  const el = h('div', { class: 'superagent' }, tablist, panes.sandbox, panes.global);
  // This sandbox's tab says when its Superagent asks to start a task while it is not in front
  // (review round 3).
  const dot = h('span', { class: 'superagent-tab-dot', 'aria-hidden': 'true', hidden: true });
  const asking = h('span', { id: id('asking', 'sandbox'), hidden: true }, 'Your Superagent asks to start a task');
  function mark(count) {
    dot.hidden = count === 0;
    if (count) tabs[0].setAttribute('aria-describedby', asking.id);
    else tabs[0].removeAttribute('aria-describedby');
  }
  // This sandbox's own Superagent, made with the panel so its requests show on the tab; its session
  // starts only when its tab first shows. Its note's All sandboxes selects that tab.
  let own = null;
  const viewOf = (key) => (key === 'global' ? global() : own);
  let key = null;
  let shown = false;
  // Outside a sandbox only All sandboxes is there (SAT-003).
  const enabled = (k) => Boolean(sandbox) || k === 'global';
  const tabs = TABS.map(([k, label]) => h('button', {
    type: 'button', class: 'term-tab', role: 'tab', id: id('tab', k), 'aria-controls': id('pane', k), dataset: { key: k },
    'aria-disabled': enabled(k) ? null : 'true', title: enabled(k) ? null : 'Open a sandbox first',
    onClick: () => { if (enabled(k)) select(k, { focusTab: true, picked: true }); },
  }, label));
  setChildren(tablist, tabs);
  tabs[0].append(dot, asking);
  if (sandbox) {
    own = createAgentView(ctx, sandbox, { openGlobal: () => select('global', { picked: true }), onRequests: mark });
    panes.sandbox.append(own.el);
  }
  tablist.addEventListener('keydown', (event) => {
    // With a modifier the keys are someone else's (Notes reorders its tabs with Alt+←/→).
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const keys = TABS.map(([k]) => k).filter(enabled);
    const at = keys.indexOf(key);
    const next = { ArrowLeft: at - 1, ArrowRight: at + 1, Home: 0, End: keys.length - 1 }[event.key];
    if (next === undefined) return;
    event.preventDefault();
    select(keys[(next + keys.length) % keys.length], { focusTab: true, picked: true });
  });

  function render(focusTab) {
    for (const tab of tabs) {
      const selected = tab.dataset.key === key;
      tab.classList.toggle('active', selected);
      tab.setAttribute('aria-selected', String(selected));
      tab.tabIndex = selected ? 0 : -1;
      if (selected && focusTab) tab.focus();
    }
  }

  /**
   * Shows tab `k`, in a sandbox the page's tab from now on. Only a tab the person `picked` is stored
   * for the next page load (review round 1);
   * `focusTab` keeps the keyboard on the strip (as Notes), `context` (a new page, not the person)
   * leaves it where it is.
   */
  function select(k, { focusTab = false, context = false, picked = false } = {}) {
    const next = enabled(k) ? k : 'global';
    if (sandbox) {
      onTab(next);
      if (picked) writeJson(TAB_KEY, next);
    }
    const before = key && viewOf(key);
    const view = viewOf(next);
    if (before && before !== view && shown && before.hide) before.hide();
    key = next;
    if (next === 'global' && view.el.parentNode !== panes.global) panes.global.append(view.el);
    for (const [k2] of TABS) panes[k2].hidden = k2 !== next;
    render(focusTab);
    if (shown) view.show({ context: context || focusTab });
  }

  return {
    el,
    title: 'Superagent',
    /**
     * Shows `opts.tab`, else the page's tab in front (whatever another browser tab stored meanwhile;
     * review rounds 1 and 2).
     */
    show(opts = {}) {
      shown = true;
      select(opts.tab || (sandbox ? tab() : 'global'), { context: Boolean(opts.context) });
    },
    hide() {
      shown = false;
      if (!key) return;
      // The Global view may already show in another sandbox's panel, or not exist yet.
      if (key === 'global' && !panes.global.firstChild) return;
      const view = viewOf(key);
      if (view.hide) view.hide();
    },
    destroy() {
      if (own) own.destroy();
      // The Global view outlives this panel: it is only taken out.
      if (panes.global.firstChild) panes.global.firstChild.remove();
    },
  };
}

/**
 * The Superagent's panel view: its quick-work notice above the session. Starts on the first show.
 * Without a sandbox it is the person's Global Superagent (SPEC-427 GSA-006): one session that sees
 * every sandbox of theirs, opened by POST /api/superagent.
 */
export function createAgentView(ctx, sandbox, { openGlobal, onRequests } = {}) {
  const global = !sandbox;
  const write = global || allows(sandbox.access, 'write');
  const name = global ? 'Global Superagent' : 'Superagent';
  // Opens (or starts) the session: the person's Global one, or this sandbox's.
  const openShell = () => (global ? ctx.api.post('/api/superagent') : ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/cockpit`));
  // On phones the panel covers the page: it closes so the composer shows.
  const closeOnPhone = () => { if (isPhone() && ctx.closePanel) ctx.closePanel(); };
  const newTask = !global && write ? h('a', { href: href(`/s/${encodeURIComponent(sandbox.slug)}`), dataset: { route: `/s/${encodeURIComponent(sandbox.slug)}` }, onClick: closeOnPhone }, 'New task') : null;
  // SPEC-428 SAT-006: the other tab of the same panel.
  const globalTab = h('button', { type: 'button', class: 'link-btn agent-note-tab', onClick: () => openGlobal && openGlobal() }, 'All sandboxes');
  // AGT-002: what the Superagent is for, and where bigger work goes.
  const notice = global
    ? h('div', { class: 'banner banner-info agent-note small', role: 'note', 'aria-label': 'About the Global Superagent' },
      icon('info', { size: 16 }),
      h('span', { class: 'banner-text' },
        h('strong', null, 'Sees all your sandboxes'), ' and keeps them in sync: what runs where, the same change across repositories. Code work still belongs in a task in its sandbox. For example: check what my sandboxes need from each other and solve it.'))
    : h('div', { class: 'banner banner-info agent-note small', role: 'note', 'aria-label': 'About the Superagent' },
      icon('info', { size: 16 }),
      h('span', { class: 'banner-text' },
        h('strong', null, 'For quick work'), ': setting up automations, asking a quick question. Anything more complex, such as writing code, belongs in a task, not here', newTask ? [': ', newTask, '.'] : '.',
        ' Work across sandboxes: the Global Superagent, in ', globalTab, '.'));
  const body = h('div', { class: 'agent-term' });
  const requests = global ? null : createRequests(ctx, sandbox, onRequests);
  const link = global ? null : createLink(ctx, sandbox, () => restart());
  const el = h('div', { class: 'agent' }, notice, link && link.el, requests && requests.el, body);
  let terminal = null;
  let alive = true;
  let opening = false;
  let started = false;
  let shown = false;
  let current = null;
  let recovering = false;
  /** A restart the person asked for is under way: the old session's `switched` exit is expected. */
  let restarting = false;
  /** Whether the last mount was to take the keyboard, and from where (for a recovery after it). */
  let asked = { focus: false, from: null };

  function drop() {
    if (terminal) terminal.destroy();
    terminal = null;
  }

  /** Opens the session; `focus` only when the person asked for the Superagent (not a new page or sandbox). */
  async function open({ focus: wantFocus = true } = {}) {
    if (opening || !alive) return;
    // Where the keyboard was: a slow start does not take it once the person has moved on.
    const from = document.activeElement;
    opening = true;
    drop();
    setChildren(body, skeleton(`Starting your ${name}…`, 3, { block: true }));
    let shell;
    try {
      shell = await openShell();
    } catch (err) {
      opening = false;
      if (alive) setChildren(body, h('div', { class: 'agent-msg' }, loadError(`Couldn’t start the ${name}: ${err.message}`, () => open())));
      return;
    }
    opening = false;
    if (alive) mount(shell, { focus: wantFocus && stayed(from), from });
  }

  /**
   * The terminal lost its session (refused as not found, or the connection dropped): ask for the
   * Superagent again, which starts it when its process is gone (a reboot), and follow it to its new id.
   * The same id means it still runs; the terminal reconnects by itself.
   */
  async function recover() {
    if (recovering || opening || !alive || !current) return;
    recovering = true;
    try {
      const shell = await openShell();
      // The keyboard stays where it is: in the Superagent if it was there (a refused session's terminal
      // takes it when shown, before it is refused), or was going there when the connection dropped
      // before the first screen; else with the page beside it.
      const keep = el.contains(document.activeElement) || (asked.focus && stayed(asked.from));
      if (alive && current && shell.id !== current.id) mount(shell, { focus: keep, from: asked.from });
    } catch {
      // The terminal shows its own connection state and Retry.
    } finally {
      recovering = false;
    }
  }

  /**
   * SPEC-355 SLR-006: Restart Superagent. The server ends the session and starts it again in place,
   * resuming its conversation; the panel follows it to the new one.
   */
  async function restart() {
    if (restarting || !alive) return;
    restarting = true;
    const keep = el.contains(document.activeElement);
    try {
      const shell = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/cockpit/restart`);
      if (alive) mount(shell, { focus: keep });
    } catch (err) {
      // The old session may have ended: ask for the Superagent again, which shows why it cannot start.
      restarting = false;
      if (alive) open({ focus: keep });
      throw err;
    } finally {
      restarting = false;
    }
  }

  /** The keyboard is still where it was when the start began (or nowhere, or in the Superagent). */
  const stayed = (from) => {
    const now = document.activeElement;
    return now === from || !now || now === document.body || el.contains(now);
  };

  function focus() {
    // On phones a focused terminal raises the soft keyboard over the page (as in the task view).
    if (!shown || isPhone()) return;
    if (terminal) terminal.focus();
  }

  function mount(shell, { focus: wantFocus = true, from = null } = {}) {
    drop();
    asked = { focus: wantFocus, from };
    current = shell;
    terminal = createTerminalView({
      transport: ctx.transport,
      path: `/api/shells/${enc(shell.id)}/terminal`,
      onLink: (uri, event) => ctx.openLink(uri, event),
      // A usage limit moved it to another subscription (SPEC-074 SUP-006), or an update restarts
      // it to get new tools (SPEC-186 SAV-006): it goes on, so the panel follows it instead of
      // offering Start again.
      exitMessage: (exit) => (exit.status === 'switched'
        ? { tone: 'info', text: `Restarting the ${name}; its conversation continues…` }
        : { tone: 'info', text: `The ${name} stopped. Its conversation is kept.` }),
      exitActions: (exit) => (exit.status === 'switched' ? [] : [{ label: 'Start again', run: () => open() }]),
      // Opened again rather than recovered: the server answers once the new one runs (it holds the
      // start), and a failure shows why with Try again instead of leaving "Moving…" on screen.
      onExit: (exit) => { if (exit.status === 'switched' && !restarting) open({ focus: el.contains(document.activeElement) }); },
      onFatal: (message) => { if (message.code === 'not_found') recover(); },
      // Once its first screen shows, the wish to focus from opening it is spent: a later recovery
      // leaves the keyboard where it is.
      onReady: () => { asked = { focus: false, from: null }; },
      onConnection: (status) => { if (status === 'reconnecting' || status === 'failed') recover(); },
      // Paths in its output open in the Editor, as in a terminal of the sandbox repository. The
      // Global Superagent has no files: it works through Yulius only.
      fileLinks: global ? undefined : {
        list: (dir) => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/files?root=sandbox&path=${encodeURIComponent(dir)}`).then((listing) => listing.entries || []),
        roots: () => (shell.folder ? [shell.folder] : []),
        open: (file) => ctx.openFile && ctx.openFile(sandbox, { root: 'sandbox', ...file }),
      },
    });
    // Files dropped here (or a pasted image) reach Claude by path, as in a task's terminal.
    if (!global) attachTerminalDrop(ctx, terminal, { label: 'Drop to send to Claude', upload: shellAttachments(ctx, sandbox.id, shell.id) });
    setChildren(body, terminal.el);
    terminal.start();
    if (wantFocus) focus();
    if (link) link.load();
  }

  if (!write) {
    setChildren(body, h('div', { class: 'agent-msg' }, emptyState({
      iconName: 'spark',
      title: 'The Superagent needs write access to this sandbox.',
      text: 'It runs Claude Code in the sandbox as you; GitHub gives you read access to this repository.',
    })));
  }

  return {
    el,
    title: name,
    show(opts = {}) {
      shown = true;
      // The session starts the first time the panel shows it, not when a sandbox opens.
      if (write && !started) { started = true; open({ focus: !opts.context }); return; }
      // Shown again for a new page (`context`), it only refits: the keyboard stays with the page.
      if (terminal) setTimeout(() => { if (terminal) { terminal.refit(); if (!opts.context) focus(); } }, 0);
    },
    hide() { shown = false; },
    destroy() {
      alive = false;
      if (requests) requests.destroy();
      if (link) link.destroy();
      drop();
    },
  };
}

/**
 * SPEC-355 SLR-006: when the Superagent's link to Yulius is lost (tested from its sandbox), above
 * its session: that it cannot reach Yulius, or that it lost it and its Yulius tools may stay
 * disconnected, with Restart Superagent, which keeps the conversation. Nothing while the link works.
 */
function createLink(ctx, sandbox, restart) {
  const el = h('div', { class: 'agent-link', hidden: true });
  const path = `/api/sandboxes/${enc(sandbox.id)}/cockpit/link`;
  let alive = true;
  let reading = null;
  let again = false;
  let busy = false;

  async function load() {
    if (!allows(sandbox.access, 'write')) return;
    if (reading) { again = true; return; }
    reading = ctx.api.get(path).then(render, () => { /* the next event or resync reads again */ });
    try { await reading; } finally {
      reading = null;
      if (again && alive) { again = false; load(); }
    }
  }

  function render(link) {
    if (!alive) return;
    const state = link && link.state;
    if (state !== 'lost' && state !== 'recovered') {
      el.hidden = true;
      setChildren(el);
      return;
    }
    const at = link.lost_at ? new Date(link.lost_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : null;
    const text = state === 'lost'
      ? `Your Superagent can’t reach ${BRAND.name} from this sandbox, so its ${BRAND.name} tools fail. Leftover processes may hold the sandbox’s connections.`
      : `Your Superagent lost its connection to ${BRAND.name}${at ? ` at ${at}` : ''}. Its ${BRAND.name} tools may stay disconnected until it restarts.`;
    const button = h('button', { type: 'button', class: 'btn btn-sm', disabled: busy, onClick: () => run() }, 'Restart Superagent');
    async function run() {
      if (busy) return;
      busy = true;
      button.disabled = true;
      try {
        await restart();
        load();
      } catch (err) {
        errorToast(err, 'Couldn’t restart the Superagent');
      } finally {
        busy = false;
        button.disabled = false;
      }
    }
    setChildren(el, h('div', { class: `banner ${state === 'lost' ? 'banner-warn' : 'banner-info'} small agent-link-banner`, role: 'status', 'aria-label': 'Superagent connection' },
      icon('warn', { size: 16 }),
      h('span', { class: 'banner-text' }, text, ' ', h('span', { class: 'muted' }, 'Restarting keeps its conversation; a task request it is waiting on ends.')),
      button));
    el.hidden = false;
  }

  const offs = [
    ctx.store.on('superagent-link', (event) => { if (event && event.sandbox_id === sandbox.id) load(); }),
    ctx.store.on('resync', () => load()),
  ];

  return {
    el,
    load,
    destroy() {
      alive = false;
      for (const off of offs) if (typeof off === 'function') off();
    },
  };
}

/** Start options a request shows, in the composer's words. */
const OPTION_LABELS = {
  workflow: (v) => `Workflow: ${v}`,
  mode: (v) => `Mode: ${v}`,
  after_task: (v) => `After #${v}`,
  start_rule: (v) => `When ${v}`,
  supertask: (v) => (v ? 'Supertask: runs alone' : null),
  prepared: (v) => (v ? 'Prepared: waits for Start now' : null),
  start_at: (v) => `Starts at ${v}`,
  model: (v) => `Model: ${v}`,
  effort: (v) => `Effort: ${v}`,
};

/** How long a new request's buttons wait before they take a click (one aimed at the terminal it pushed down). */
const ARM_MS = 600;
/** Characters a person cannot see that could hide or reorder a prompt's words: shown by their code. */
const INVISIBLE = /(?![\n\t])[\p{Cc}\p{Cf}\p{Co}\p{Cn}\u{034F}\u{115F}\u{1160}\u{17B4}\u{17B5}\u{180B}-\u{180F}\u{3164}\u{FFA0}\u{FE00}-\u{FE0F}\u{E0100}-\u{E01EF}]/gu;

/** A request's text as a person can check it: invisible characters named by their code (the server refuses them too). */
const shownText = (text) => String(text).replace(/\r\n?|[\u{2028}\u{2029}]/gu, '\n').replace(INVISIBLE, (c) => `⟨U+${c.codePointAt(0).toString(16).toUpperCase().padStart(4, '0')}⟩`);

/** The prompt as a person can check it: invisible characters named, long runs of empty lines counted. */
export function shownPrompt(text) {
  const named = shownText(text);
  // A line of only spaces of any kind (no-break, ideographic, braille blank…) is empty too.
  return named.replace(/\n(?:(?:[^\S\n]|\u{2800})*\n){2,}/gu, (run) => `\n[${run.split('\n').length - 2} empty lines]\n`);
}

/**
 * SPEC-335 XSR-008: what the person's Superagent asks to start in another sandbox, above its
 * session. Nothing reaches that sandbox until the person presses Create task; each request is
 * answered once, and goes when the Superagent's call ends. Create task takes a click only once the
 * whole prompt was in view (scrolled to its end) and the card has been there a moment.
 */
function createRequests(ctx, sandbox, onCount) {
  const el = h('div', { class: 'agent-requests', hidden: true });
  const path = `/api/sandboxes/${enc(sandbox.id)}/superagent/requests`;
  let alive = true;
  let reading = null;
  let again = false;
  const cards = new Map(); // request id → { el, destroy }

  async function load() {
    if (!allows(sandbox.access, 'write')) return;
    // One read at a time; a change announced during it brings one more.
    if (reading) { again = true; return; }
    reading = ctx.api.get(path).then(render, () => { /* the next event or resync reads again */ });
    try { await reading; } finally {
      reading = null;
      if (again && alive) { again = false; load(); }
    }
  }

  /**
   * Cards come and go; one already shown keeps its scroll. When one goes, the others move up under
   * the pointer, so their buttons arm again first: a double-click answers one request, not the next.
   */
  function render(list) {
    if (!alive) return;
    const items = Array.isArray(list) ? list : [];
    const ids = new Set(items.map((r) => r.id));
    const gone = [...cards.keys()].filter((id) => !ids.has(id));
    if (gone.length) for (const [id, shown] of cards) if (ids.has(id)) shown.rearm();
    for (const id of gone) { cards.get(id).destroy(); cards.delete(id); }
    for (const request of items) {
      if (cards.has(request.id)) continue;
      const shown = card(request);
      cards.set(request.id, shown);
      el.append(shown.el);
    }
    el.hidden = cards.size === 0;
    if (onCount) onCount(cards.size);
  }

  function card(request) {
    const target = request.sandbox || {};
    const slug = target.slug;
    const options = Object.entries(request.options || {})
      .map(([key, value]) => (OPTION_LABELS[key] ? OPTION_LABELS[key](typeof value === 'string' ? shownText(value) : value) : null))
      .filter(Boolean);
    const heading = `Your Superagent asks to start a task in ${slug}`;
    // The prompt's last line: once it was on screen (not clipped by the prompt box, the list or the window), it was all read.
    const end = h('span', { class: 'agent-request-end', 'aria-hidden': 'true' });
    const prompt = h('pre', { class: 'agent-request-prompt', tabindex: '0', 'aria-label': 'Prompt' }, shownPrompt(request.prompt || ''), end);
    const hint = h('div', { class: 'agent-request-hint', hidden: true }, 'Scroll to the end of the prompt to create the task.');
    const create = h('button', { type: 'button', class: 'btn btn-sm btn-primary', disabled: true, onClick: () => respond(true) }, 'Create task');
    const decline = h('button', { type: 'button', class: 'btn btn-sm', disabled: true, onClick: () => respond(false) }, 'Decline');
    let armed = false;
    let read = false;
    let sending = false;
    // Answered: the card only waits to go; it takes no second click.
    let done = false;
    const update = () => {
      create.disabled = !armed || !read || sending || done;
      decline.disabled = !armed || sending || done;
      hint.hidden = read;
    };
    // The whole prompt was in view: its end was on screen. A hidden panel shows none of it.
    const seen = typeof IntersectionObserver === 'function'
      ? new IntersectionObserver((entries) => {
        if (!read && entries.some((entry) => entry.isIntersecting)) { read = true; update(); }
      })
      : null;
    if (seen) seen.observe(end);
    let timer = null;
    const arm = () => {
      clearTimeout(timer);
      armed = false;
      update();
      timer = setTimeout(() => { armed = true; if (!seen) read = true; update(); }, ARM_MS);
    };
    arm();

    async function respond(approve) {
      if (sending) return;
      sending = true;
      update();
      try {
        const answer = await ctx.api.post(`${path}/${enc(request.id)}`, { approve });
        done = true;
        if (approve && answer && answer.task) {
          const to = `/s/${encodeURIComponent(answer.sandbox || slug)}/t/${answer.task.number}`;
          toast(`Created #${answer.task.number} in ${answer.sandbox || slug}.`, { kind: 'ok', action: { label: 'Open', run: () => ctx.navigate(to) } });
        } else if (!approve) {
          toast('Declined: nothing was created.', { kind: 'info' });
        }
      } catch (err) {
        errorToast(err, approve ? `Couldn’t create the task in ${slug}` : 'Couldn’t decline');
      } finally {
        sending = false;
        update();
        load();
      }
    }

    return {
      el: h('section', { class: 'agent-request banner banner-warn small', role: 'group', 'aria-label': heading },
        h('div', { class: 'agent-request-head' }, icon('spark', { size: 16 }),
          h('strong', null, 'Your Superagent asks to start a task in ', h('span', { class: 'agent-request-target' }, slug)),
          target.repository ? h('span', { class: 'muted' }, ` (${target.repository})`) : null),
        request.title ? h('div', { class: 'agent-request-title' }, shownText(request.title)) : null,
        prompt,
        options.length ? h('ul', { class: 'agent-request-options', 'aria-label': 'Start options' }, options.map((o) => h('li', null, o))) : null,
        hint,
        h('div', { class: 'agent-request-actions' }, create, decline)),
      rearm: arm,
      destroy() {
        clearTimeout(timer);
        if (seen) seen.disconnect();
        this.el.remove();
      },
    };
  }

  const offs = [
    ctx.store.on('superagent-requests', (event) => { if (event && event.sandbox_id === sandbox.id) load(); }),
    ctx.store.on('resync', () => load()),
  ];
  load();

  return {
    el,
    destroy() {
      alive = false;
      for (const off of offs) if (typeof off === 'function') off();
      for (const shown of cards.values()) shown.destroy();
      cards.clear();
    },
  };
}
