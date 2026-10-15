// Automations → one automation (SPEC-001 UI-039): a builder with as little typing as possible. An
// automation starts a task on a schedule: a recipe fills the name, the prompt and a sensible time;
// the schedule is clicked, not written — once a day on the chosen days (or every hour) at a time
// picked from chips — and the page says in words when it runs next; cron stays under Advanced.
// The workflow is a chip. How each task starts is New task's start chip, in the action bar beside
// Run now and Save (SPEC-078, SPEC-093), with "When I decide" but no "At a time…" (SPEC-155); Run
// now uses the choice it shows. Run now, the switch and
// the run history live on the same page (Automation, docs/api.md).

import { enc } from '../api.js';
import { h, icon, setChildren, placeholder, loadError, confirmDialog, toast, invalid, badge } from '../dom.js';
import { href, navigate, linkTarget } from '../router.js';
import { allows } from '../status.js';
import { openRuleEditor, ruleFromText, ruleText } from '../ruleeditor.js';
import { openStartMenu, renderStartChip, startKind } from '../startchoice.js';
import { describeCron, cronValid, utcOffset, dateTime, relLong, startPhrase } from '../format.js';
import { dataTable, notFound } from './common.js';

const RECIPES = [
  { id: 'feedback', name: 'Daily feedback', icon: 'mail', days: [1, 2, 3, 4, 5], time: '09:00', text: 'Weekdays, 09:00', prompt: 'Implement yesterday’s customer feedback from the Quartio MCP. Open one pull request per change.' },
  { id: 'deps', name: 'Dependency updates', icon: 'refresh', days: [1], time: '08:00', text: 'Mondays, 08:00', prompt: 'Update outdated dependencies, run the tests and open a pull request with the changes.' },
  { id: 'ci', name: 'Fix failing checks', icon: 'check', days: [1, 2, 3, 4, 5], time: '07:00', text: 'Weekdays, 07:00', prompt: 'Look at the failing checks on the default branch and fix them.' },
  { id: 'triage', name: 'Triage issues', icon: 'search', days: [1, 2, 3, 4, 5], time: '10:00', text: 'Weekdays, 10:00', prompt: 'Go through the new GitHub issues: label them, reproduce the bugs and comment what you found.' },
  { id: 'summary', name: 'Weekly summary', icon: 'book', days: [5], time: '16:00', text: 'Fridays, 16:00', prompt: 'Summarize what changed in the repository this week and add it to docs/changelog.md.' },
  { id: 'blank', name: 'Blank', icon: 'plus', days: [1, 2, 3, 4, 5], time: '09:00', text: 'Start from nothing', prompt: '' },
];
const DAY_ORDER = [1, 2, 3, 4, 5, 6, 0]; // Monday first on screen; cron numbers Sunday 0
const DAY_SHORT = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const DAY_LONG = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
const TIMES = ['07:00', '08:00', '09:00', '12:00', '17:00', '20:00'];
const MINUTES = [0, 15, 30, 45];
const RUN_TONE = { scheduled: 'warn', task_created: 'ok', skipped_previous_active: '', skipped_disabled: '', failed: 'danger' };
const RUN_LABEL = { scheduled: 'scheduled', task_created: 'task created', skipped_previous_active: 'skipped: previous task still active', skipped_disabled: 'skipped: disabled', failed: 'failed' };

/** A cron expression as the builder's schedule, or null when it has another shape. */
export function readSchedule(expr) {
  const parts = String(expr || '').trim().split(/\s+/);
  if (parts.length !== 5) return null;
  const [min, hour, dom, mon, dow] = parts;
  if (!/^\d+$/.test(min) || Number(min) > 59 || dom !== '*' || mon !== '*') return null;
  if (hour === '*') return dow === '*' ? { hourly: true, minute: Number(min), days: [0, 1, 2, 3, 4, 5, 6] } : null;
  if (!/^\d+$/.test(hour) || Number(hour) > 23) return null;
  const days = new Set();
  if (dow === '*') DAY_ORDER.forEach((d) => days.add(d));
  else {
    for (const item of dow.split(',')) {
      const range = item.match(/^(\d)(?:-(\d))?$/);
      if (!range) return null;
      const from = Number(range[1]);
      const to = range[2] === undefined ? from : Number(range[2]);
      if (from > 7 || to > 7 || to < from) return null;
      for (let d = from; d <= to; d += 1) days.add(d % 7);
    }
  }
  return { hourly: false, time: `${hour.padStart(2, '0')}:${min.padStart(2, '0')}`, days: [...days].sort() };
}

/** The builder's schedule as a cron expression. */
export function writeSchedule({ hourly, minute = 0, time = '09:00', days }) {
  if (hourly) return `${minute} * * * *`;
  const [hh, mm] = time.split(':').map(Number);
  const set = [...new Set(days)].sort((a, b) => a - b);
  const dow = set.length === 7 ? '*' : set.join(',') === '1,2,3,4,5' ? '1-5' : set.join(',');
  return `${mm} ${hh} * * ${dow}`;
}

/** The schedule in words: "Weekdays at 09:00", "Mon, Wed and Fri at 08:30", "Every hour at :15". */
export function sayDays(days) {
  const set = [...new Set(days)].sort((a, b) => a - b).join(',');
  if (set === '0,1,2,3,4,5,6') return 'Every day';
  if (set === '1,2,3,4,5') return 'Weekdays';
  if (set === '0,6') return 'Weekends';
  const ordered = DAY_ORDER.filter((d) => days.includes(d));
  if (ordered.length === 1) return `Every ${DAY_LONG[ordered[0]]}`;
  const names = ordered.map((d) => DAY_SHORT[d]);
  return `${names.slice(0, -1).join(', ')} and ${names.at(-1)}`;
}

/** The next run after `from` (ms) for a builder schedule at a fixed UTC offset; a Date. */
export function nextRun(schedule, offsetMinutes, from = Date.now()) {
  const shift = offsetMinutes * 60000;
  let t = Math.floor(from / 60000) * 60000 + 60000;
  const [hh, mm] = (schedule.time || '00:00').split(':').map(Number);
  for (let i = 0; i < 8 * 24 * 60; i += 1, t += 60000) {
    const local = new Date(t + shift);
    const ok = schedule.hourly
      ? local.getUTCMinutes() === schedule.minute
      : local.getUTCHours() === hh && local.getUTCMinutes() === mm && schedule.days.includes(local.getUTCDay());
    if (ok) return new Date(t);
  }
  return null;
}

/** Quarter-hour offsets from UTC−12 to UTC+14, plus `current` when it is another valid value. */
function offsetOptions(current) {
  const out = [];
  for (let m = -12 * 60; m <= 14 * 60; m += 15) out.push(m);
  if (Number.isInteger(current) && !out.includes(current)) out.push(current);
  return out.sort((a, b) => a - b);
}

/** Mount the builder: `id` is an automation id, or 'new'. */
export function mountAutomationEditor(ctx, sandbox, id) {
  const base = `/s/${encodeURIComponent(sandbox.slug)}/automations`;
  const creating = id === 'new';
  const write = allows(sandbox.access, 'write');
  let automation = null;
  let workflows = [];
  let recipe = null;
  let alive = true;
  let saved = '';
  let showZone = false;
  let loaded = false; // the automation has been read (SPEC-093): the chip and Run now turn on
  // The start choice (SPEC-078, SPEC-155): at most one of supertask, after_task_id, start_rule and
  // prepared.
  const state = { name: '', prompt: '', workflow_id: null, enabled: true, cron: '0 9 * * 1-5', offset: -new Date().getTimezoneOffset(), custom: false, supertask: false, after_task_id: null, after_task_number: null, start_rule: null, prepared: false };
  const live = h('div', { class: 'sr-only', role: 'status' });

  const nameInput = h('input', { class: 'wf-name', placeholder: 'Name the automation', 'aria-label': 'Automation name', maxlength: 100, onInput: (e) => { state.name = e.target.value; changed(); } });
  const when = h('span', { class: 'wf-desc au-when' });
  const save = h('button', { type: 'button', class: 'btn btn-primary', disabled: !write, onClick: () => doSave() }, creating ? 'Create automation' : 'Save');
  // How each task starts (SPEC-093): New task's start chip, before Run now. Both stay off until
  // the automation is read, so Run now never sends a choice the page has not shown.
  const startChip = h('button', { type: 'button', class: 'chip-btn au-start', 'aria-haspopup': 'menu', disabled: true, onClick: () => openStart() });
  const runNow = creating ? null : h('button', { type: 'button', class: 'btn au-run', disabled: true, title: write ? 'Create a task from it now' : 'Requires write access', onClick: () => doRunNow() }, icon('send', { size: 14 }), 'Run now');
  const del = creating ? null : h('button', { type: 'button', class: 'btn btn-quiet-danger', disabled: !write, onClick: () => doDelete() }, icon('trash', { size: 14 }), 'Delete');
  const body = h('div', { class: 'au-body' }, placeholder('Loading the automation…', 'loading', 'form'));
  const head = h('header', { class: 'gt-head wf-head' },
    h('a', { class: 'gt-back', href: href(base), dataset: { route: base, restore: 'page' } }, icon('back', { size: 14 }), 'Automations'),
    h('div', { class: 'gt-title-row' }, h('div', { class: 'wf-titles' }, nameInput, when), del, del ? h('span', { class: 'au-break', 'aria-hidden': 'true' }) : null, startChip, runNow, save),
    !write ? h('p', { class: 'banner small' }, icon('lock', { size: 14 }), h('span', { class: 'banner-text' }, 'Read-only: changing automations needs write access to this sandbox.')) : null);
  const el = h('div', { class: 'page au-page' }, h('div', { class: 'page-inner mc-inner' }, h('h1', { class: 'sr-only' }, creating ? 'New automation' : 'Edit automation'), head, body, live));

  const snapshot = () => JSON.stringify(state);
  const dirty = () => snapshot() !== saved;
  const schedule = () => readSchedule(state.cron);
  function sentence() {
    const s = schedule();
    if (!cronValid(state.cron)) return 'The schedule needs five fields.';
    const words = s ? (s.hourly ? `Every hour at :${String(s.minute).padStart(2, '0')}` : `${sayDays(s.days)} at ${s.time}`) : describeCron(state.cron);
    const next = s && state.enabled ? nextRun(s, state.offset) : null;
    const start = startPhrase(state);
    return `${words} (${utcOffset(state.offset)})${start ? ` · ${start}` : ''}${!state.enabled ? ' · paused' : next ? ` · next run ${relLong(next.toISOString())} (${dateTime(next.toISOString())})` : ''}`;
  }
  function changed() {
    save.disabled = !write || (!creating && !dirty());
    when.textContent = sentence();
  }

  function section(title, hint, content) {
    return h('section', { class: 'wf-section' }, h('h2', { class: 'wf-section-title' }, title, hint ? h('span', { class: 'muted small' }, hint) : null), content);
  }
  const chip = (label, on, onClick, { extraClass, ...attrs } = {}) => h('button', { type: 'button', class: ['wf-chip', 'au-chip', extraClass, on && 'on'], 'aria-pressed': String(on), disabled: !write, onClick, ...attrs }, label);

  // ---- recipes (new automations)
  function useRecipe(r) {
    recipe = r.id;
    if (!state.name || RECIPES.some((x) => x.name === state.name)) { state.name = r.id === 'blank' ? '' : r.name; nameInput.value = state.name; }
    state.prompt = r.prompt;
    state.cron = writeSchedule({ days: r.days, time: r.time });
    state.custom = false;
    render();
    live.textContent = `Started from ${r.name}`;
  }

  // ---- when
  function whenSection() {
    const s = schedule();
    const set = (next) => { state.cron = writeSchedule({ ...(s || { days: [1, 2, 3, 4, 5], time: '09:00' }), ...next }); state.custom = false; render(); };
    if (state.custom || !s) {
      return section('When', 'custom schedule', h('div', { class: 'stack-sm' },
        h('p', { class: 'small' }, `${describeCron(state.cron)} — a schedule the buttons can’t show. Edit it under Advanced, or start over:`),
        h('div', null, h('button', { type: 'button', class: 'btn btn-sm', disabled: !write, onClick: () => { state.cron = '0 9 * * 1-5'; state.custom = false; render(); } }, 'Weekdays at 09:00')),
        advanced(true)));
    }
    const mode = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'How often' },
      h('label', null, h('input', { type: 'radio', name: 'au-mode', checked: !s.hourly, disabled: !write, onChange: () => set({ hourly: false, time: s.time || '09:00', days: s.hourly ? [1, 2, 3, 4, 5] : s.days }) }), 'Once a day'),
      h('label', null, h('input', { type: 'radio', name: 'au-mode', checked: s.hourly, disabled: !write, onChange: () => set({ hourly: true, minute: 0 }) }), 'Every hour'));
    const rows = [h('div', { class: 'au-row' }, mode)];
    if (s.hourly) {
      rows.push(h('div', { class: 'au-row' }, h('span', { class: 'wf-label' }, 'At minute'), h('div', { class: 'wf-chips' }, MINUTES.map((m) => chip(`:${String(m).padStart(2, '0')}`, s.minute === m, () => set({ minute: m }))))));
    } else {
      const toggleDay = (d) => {
        const days = s.days.includes(d) ? s.days.filter((x) => x !== d) : [...s.days, d];
        if (!days.length) { live.textContent = 'At least one day'; return; }
        set({ days });
      };
      rows.push(h('div', { class: 'au-row' }, h('span', { class: 'wf-label' }, 'On'),
        h('div', { class: 'au-days', role: 'group', 'aria-label': 'Days' }, DAY_ORDER.map((d) => chip(DAY_SHORT[d], s.days.includes(d), () => toggleDay(d), { extraClass: 'au-day', 'aria-label': DAY_LONG[d] }))),
        h('div', { class: 'au-quick' }, [['Every day', [0, 1, 2, 3, 4, 5, 6]], ['Weekdays', [1, 2, 3, 4, 5]], ['Weekends', [0, 6]]].map(([label, days]) =>
          h('button', { type: 'button', class: 'link-btn small', disabled: !write, onClick: () => set({ days }) }, label)))));
      const custom = h('input', { class: 'input au-time', type: 'time', value: s.time, disabled: !write, 'aria-label': 'Another time', onChange: (e) => { if (e.target.value) set({ time: e.target.value }); } });
      rows.push(h('div', { class: 'au-row' }, h('span', { class: 'wf-label' }, 'At'), h('div', { class: 'wf-chips' }, TIMES.map((t) => chip(t, s.time === t, () => set({ time: t }))), custom)));
    }
    const zone = showZone
      ? h('select', { class: 'select au-zone', 'aria-label': 'Time zone (UTC offset)', disabled: !write, onChange: (e) => { state.offset = Number(e.target.value); changed(); } }, offsetOptions(state.offset).map((m) => h('option', { value: String(m), selected: m === state.offset }, utcOffset(m))))
      : h('button', { type: 'button', class: 'link-btn small', disabled: !write, onClick: () => { showZone = true; render(); el.querySelector('.au-zone')?.focus(); } }, 'Change');
    rows.push(h('div', { class: 'au-row' }, h('span', { class: 'wf-label' }, 'Time zone'), h('span', { class: 'small' }, `${utcOffset(state.offset)}${state.offset === -new Date().getTimezoneOffset() ? ' (yours)' : ''}`), zone,
      h('span', { class: 'muted xs' }, 'A fixed offset: it does not follow daylight saving time.')));
    rows.push(advanced(false));
    return section('When', null, h('div', { class: 'au-when-rows' }, rows));
  }

  function advanced(open) {
    const cron = h('input', { class: 'input mono au-cron', value: state.cron, disabled: !write, 'aria-label': 'Cron schedule', spellcheck: 'false', onInput: (e) => {
      state.cron = e.target.value.trim().split(/\s+/).join(' ');
      e.target.setAttribute('aria-invalid', cronValid(state.cron) ? 'false' : 'true');
      changed();
    }, onChange: () => { state.custom = !readSchedule(state.cron); render(); } });
    return h('details', { class: 'mc-more', open: open || null }, h('summary', null, 'Advanced: cron'), h('div', { class: 'stack-sm' }, cron, h('span', { class: 'muted xs' }, 'Five fields: minute, hour, day of month, month, day of week (0 or 7 is Sunday).')));
  }

  // ---- what
  function whatSection() {
    const prompt = h('textarea', { class: 'textarea au-prompt', rows: 4, value: state.prompt, readOnly: !write, placeholder: 'What the task should do each time, e.g. “Implement yesterday’s customer feedback from the Quartio MCP.”', 'aria-label': 'What the task does', onInput: (e) => { state.prompt = e.target.value; changed(); } });
    const pick = (value, label) => chip(label, (state.workflow_id || null) === value, () => { state.workflow_id = value; render(); }, { role: 'radio', 'aria-checked': String((state.workflow_id || null) === value), 'aria-pressed': null });
    return section('The task', null, h('div', { class: 'stack-sm' }, prompt,
      h('div', { class: 'au-row' }, h('span', { class: 'wf-label' }, 'Workflow'),
        h('div', { class: 'wf-chips', role: 'radiogroup', 'aria-label': 'Workflow' }, pick(null, 'None'), workflows.map((w) => pick(w.id, w.name))))));
  }

  // ---- how each task starts (SPEC-078, SPEC-093): New task's start chip and menu
  function setStart(next) {
    Object.assign(state, { supertask: false, after_task_id: null, after_task_number: null, start_rule: null, prepared: false }, next);
    renderStart();
    changed();
  }
  const kindOf = () => startKind({ supertask: state.supertask, afterTaskId: state.after_task_id, rule: state.start_rule, prepare: state.prepared ? {} : null });
  /** The chip reads the choice; its tooltip says what it means. It follows the task list. */
  function renderStart() {
    // Until the automation is read the chip names no choice (its name is just "Start").
    if (!loaded) { setChildren(startChip, icon('branch', { size: 14 }), h('span', { class: 'sr-only' }, 'Start'), h('span', { class: 'chip-label', 'aria-hidden': 'true' }, '…')); return; }
    const tasks = ctx.store.tasksOf(sandbox.id) || [];
    const kind = kindOf();
    const afterTask = tasks.find((t) => t.id === state.after_task_id);
    const number = afterTask ? afterTask.number : state.after_task_number;
    const rule = state.start_rule || '';
    renderStartChip(startChip, { kind, number, rule, prefix: 'Start: ' });
    startChip.title = {
      now: 'Each task starts as soon as a session slot is free.',
      supertask: 'Each task waits for this project’s running tasks, then runs alone: the project’s other tasks wait for it.',
      after: `Each task starts when #${number || '?'} is done. Once #${number || '?'} has ended, each task starts at once if it was done, or else waits for you to start or cancel it.`,
      rule: `Each task starts by itself when ${rule}.`,
      prepared: 'Each task waits, prepared, until you press Start now.',
    }[kind];
  }
  function openStart() {
    openStartMenu(startChip, ctx, sandbox, {
      kind: kindOf(),
      afterTaskId: state.after_task_id,
      prepare: true,
      onPick: ({ kind, task }) => setStart(kind === 'supertask' ? { supertask: true } : kind === 'after' ? { after_task_id: task.id, after_task_number: task.number } : kind === 'prepared' ? { prepared: true } : {}),
      onRule: () => openRuleEditor({ tasks: ctx.store.tasksOf(sandbox.id), value: ruleFromText(state.start_rule), onUse: (rule) => setStart({ start_rule: ruleText(rule) }) }),
    });
  }

  function runningSection() {
    const input = h('input', { type: 'checkbox', role: 'switch', class: 'switch-input', checked: state.enabled, disabled: !write, onChange: (e) => { state.enabled = e.target.checked; gate.classList.toggle('on', state.enabled); changed(); } });
    const gate = h('label', { class: ['wf-gate', state.enabled && 'on'] }, h('span', { class: 'wf-gate-icon' }, icon('zap', { size: 16 })),
      h('span', { class: 'wf-gate-text' }, h('strong', null, 'Runs on schedule'), h('span', { class: 'muted small' }, 'Off pauses it; Run now still works. A run is skipped while the task from the previous run is still active or waiting to start.')),
      h('span', { class: 'switch' }, input, h('span', { class: 'switch-track', 'aria-hidden': 'true' })));
    return section('Running', null, gate);
  }

  // ---- history (existing automations)
  const runsHost = h('div', null, placeholder('Loading runs…', 'loading', ['table', 4]));
  const taskLink = (taskId) => {
    const task = taskId ? ctx.store.findTask(taskId) : null;
    if (!task) return taskId ? h('span', { class: 'muted' }, 'a task') : '—';
    const path = `/s/${encodeURIComponent(sandbox.slug)}/tasks/${task.number}`;
    return h('a', { href: href(path), dataset: { route: path } }, `#${task.number} ${task.title || ''}`);
  };
  async function loadRuns() {
    try {
      const runs = await ctx.api.get(`/api/automations/${enc(id)}/runs`);
      if (!alive) return;
      setChildren(runsHost, dataTable([
        { label: 'Scheduled for', render: (r) => h('span', { title: dateTime(r.scheduled_for) }, relLong(r.scheduled_for)) },
        { label: 'Result', render: (r) => badge(RUN_LABEL[r.state] || r.state || 'unknown', RUN_TONE[r.state] || '') },
        { label: 'Task', render: (r) => taskLink(r.task_id) },
        { label: 'Error', render: (r) => (r.error ? h('span', { class: 'field-error' }, r.error) : '—') },
      ], (Array.isArray(runs) ? runs : []).slice(0, 20), { empty: 'No runs yet.' }));
    } catch (error) {
      if (alive) setChildren(runsHost, placeholder(`Couldn’t load the runs: ${error.message}`, 'error'));
    }
  }

  function render() {
    setChildren(body, h('div', { class: 'wf-main' },
      creating ? section('Start from', 'pick one, change anything', h('div', { class: 'wf-templates', role: 'radiogroup', 'aria-label': 'Recipe' }, RECIPES.map((r) => h('button', {
        type: 'button', role: 'radio', 'aria-checked': String(recipe === r.id), class: ['wf-template', recipe === r.id && 'on'], disabled: !write, onClick: () => useRecipe(r),
      }, h('span', { class: 'wf-template-icon' }, icon(r.icon, { size: 16 })), h('span', { class: 'wf-template-name' }, r.name), h('span', { class: 'muted xs' }, r.text))))) : null,
      whatSection(),
      whenSection(),
      runningSection(),
      creating ? null : section('Recent runs', null, runsHost)));
    renderStart();
    changed();
  }

  // ---- save, run, delete, leave
  async function doSave() {
    if (!write || save.classList.contains('busy')) return;
    if (!state.name.trim()) { invalid(nameInput, 'Give the automation a name.'); return; }
    if (!state.prompt.trim()) { invalid(el.querySelector('.au-prompt'), 'Say what the task should do.'); return; }
    if (!cronValid(state.cron)) { const cron = el.querySelector('.au-cron'); cron?.closest('details')?.setAttribute('open', ''); if (cron) invalid(cron, 'A schedule has five fields, e.g. “0 9 * * 1-5”.'); return; }
    const payload = { name: state.name.trim(), cron: state.cron, utc_offset_minutes: state.offset, prompt: state.prompt.trim(), workflow_id: state.workflow_id || null, enabled: state.enabled, supertask: state.supertask, after_task_id: state.after_task_id, start_rule: state.start_rule, prepared: state.prepared };
    save.classList.add('busy');
    try {
      if (creating) await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/automations`, payload);
      else automation = await ctx.api.put(`/api/automations/${enc(id)}`, payload);
      saved = snapshot();
      toast(creating ? `Automation ${payload.name} created.` : `Automation ${payload.name} saved.`, { kind: 'ok' });
      if (creating) navigate(base); else changed();
    } catch (error) {
      toast(`Couldn’t save: ${error.message}`, { kind: 'error' });
    } finally {
      save.classList.remove('busy');
    }
  }

  async function doRunNow() {
    if (!runNow || runNow.classList.contains('busy')) return;
    runNow.classList.add('busy');
    try {
      // The start choice the chip shows, saved or not (SPEC-093 AUT-023), for this run only.
      const run = await ctx.api.post(`/api/automations/${enc(id)}/run`, { supertask: state.supertask, after_task_id: state.after_task_id, start_rule: state.start_rule, prepared: state.prepared });
      const task = run && run.task_id ? ctx.store.findTask(run.task_id) : null;
      if (run && run.state === 'failed') toast(`Run failed${run.error ? `: ${run.error}` : ''}`, { kind: 'error' });
      else toast(`${RUN_LABEL[run && run.state] || 'Run recorded'}${task ? ` (#${task.number})` : ''}`, { kind: run && run.state === 'task_created' ? 'ok' : 'info' });
      if (run && run.task_id) ctx.store.loadTasks(sandbox.id).catch(() => null);
      loadRuns();
    } catch (error) {
      toast(`Run refused: ${error.message}`, { kind: 'error' });
    } finally {
      runNow.classList.remove('busy');
    }
  }

  async function doDelete() {
    if (!(await confirmDialog({ title: `Delete automation “${state.name}”?`, message: 'Future runs are cancelled. Tasks it already created stay.', confirmLabel: 'Delete', danger: true }))) return;
    try {
      await ctx.api.del(`/api/automations/${enc(id)}`);
      saved = snapshot();
      toast('Automation deleted.', { kind: 'ok' });
      navigate(base);
    } catch (error) {
      toast(`Couldn’t delete: ${error.message}`, { kind: 'error' });
    }
  }

  const onBeforeUnload = (event) => { if (dirty()) { event.preventDefault(); event.returnValue = ''; } };
  const onLink = async (event) => {
    const link = event.target.closest && event.target.closest('a[data-route]');
    if (!link || !dirty() || event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    if (await confirmDialog({ title: 'Leave without saving?', message: 'Your changes to this automation are not saved.', confirmLabel: 'Leave', danger: true })) { saved = snapshot(); navigate(linkTarget(link)); }
  };
  renderStart();
  window.addEventListener('beforeunload', onBeforeUnload);
  document.addEventListener('click', onLink, true);
  el.addEventListener('keydown', (event) => {
    if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 's') { event.preventDefault(); doSave(); }
  });
  const offRuns = creating ? () => {} : ctx.store.on('automation-run', (event) => { if (!event || event.automation_id === id) loadRuns(); });
  const offTasks = ctx.store.on('tasks', (sid) => { if (sid === sandbox.id) renderStart(); });

  async function load() {
    try {
      const [list, wfs] = await Promise.all([
        creating ? Promise.resolve([]) : ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/automations`),
        ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/workflows`).catch(() => []),
      ]);
      if (!alive) return;
      workflows = Array.isArray(wfs) ? wfs : [];
      if (!creating) {
        automation = (Array.isArray(list) ? list : []).find((a) => a.id === id);
        if (!automation) { saved = snapshot(); setChildren(el, notFound({ title: 'Automation not found', text: `There is no automation “${id}” in ${sandbox.name}. It may have been deleted.`, back: ['Automations', base] })); return; }
        Object.assign(state, { name: automation.name, prompt: automation.prompt || '', workflow_id: automation.workflow_id || null, enabled: automation.enabled, cron: automation.cron, offset: automation.utc_offset_minutes, custom: !readSchedule(automation.cron),
          supertask: Boolean(automation.supertask), after_task_id: automation.after_task_id || null, after_task_number: automation.after_task_number ?? null, start_rule: automation.start_rule || null, prepared: Boolean(automation.prepared) });
        nameInput.value = automation.name;
        showZone = state.offset !== -new Date().getTimezoneOffset();
      }
      nameInput.readOnly = !write;
      loaded = true;
      startChip.disabled = !write;
      if (runNow) runNow.disabled = !write;
      if (creating) { useRecipe(RECIPES[0]); saved = JSON.stringify({ ...state, name: '' }); } else { saved = snapshot(); render(); loadRuns(); }
    } catch (error) {
      if (alive) setChildren(body, loadError(`Couldn’t load the automation: ${error.message}`, load));
    }
  }
  load();

  return {
    el,
    destroy() { alive = false; offRuns(); offTasks(); window.removeEventListener('beforeunload', onBeforeUnload); document.removeEventListener('click', onLink, true); },
  };
}
