// How a task starts: New task's start chip and its menu, shared with the automation editor's
// action bar (SPEC-093). Start now, Supertask (SPEC-071), after a task is done (TSK-012; the
// filterable list of SPEC-038) or when a rule is true (SPEC-017, picked in the rule editor of
// SPEC-022); New task also prepares a task, started when you decide or at a time (SPEC-105), and
// an automation prepares its tasks, started when you decide (SPEC-155).
// Each page keeps its own state and opens the rule editor with its own value. A task that waits
// to start changes its choice with the same menu, from its view (SPEC-126).
// Every API path is a literal at its call site (tests/ui_contract.rs checks them).

import { enc } from './api.js';
import { h, icon, setChildren, openMenu, toast, errorToast } from './dom.js';
import { statusInfo } from './status.js';
import { whenLabel } from './format.js';
import { chainCandidates } from './intake.js';
import { ruleChoices, openRuleEditor, ruleText, ruleFromText } from './ruleeditor.js';
import { openScheduleDialog } from './actions.js';

export const SUPERTASK_HELP = 'Runs alone once this project’s running tasks end; its other tasks wait. For broad work: security hardening, one-shotting the product.';

/** What "When I decide" does (SPEC-105). */
export const WHEN_I_DECIDE_HELP = 'The task waits, prepared, until you press Start now';

/**
 * The kind of a start choice: 'prepared' (when I decide), 'scheduled' (at a time), 'supertask',
 * 'rule', 'after' or 'now'. `prepare` is null or `{ at }` (SPEC-105).
 */
export function startKind({ supertask, afterTaskId, rule, prepare }) {
  if (prepare) return prepare.at ? 'scheduled' : 'prepared';
  if (supertask) return 'supertask';
  if (rule) return 'rule';
  if (afterTaskId) return 'after';
  return 'now';
}

/**
 * Fills a start chip: an icon, the hidden `prefix` of its accessible name (WCAG 2.5.3: the
 * visible choice stays in the name), the choice ("Start now", "Supertask", "After #12",
 * "When #12 done", "When I decide", "At tomorrow 09:00") and a down arrow. `number` is the task
 * to run after; `rule` the rule's text; `at` a scheduled time (ISO).
 */
export function renderStartChip(chip, { kind, number, rule, at, prefix }) {
  const ruleLabel = rule && rule.length > 28 ? `${rule.slice(0, 27)}…` : rule;
  const label = { supertask: 'Supertask', rule: `When ${ruleLabel}`, after: `After #${number || '?'}`, now: 'Start now', prepared: 'When I decide', scheduled: `At ${whenLabel(at)}` }[kind];
  const glyph = { supertask: 'shield', prepared: 'st-prepared', scheduled: 'clock' }[kind] || 'branch';
  setChildren(chip, icon(glyph, { size: 14 }), h('span', { class: 'sr-only' }, prefix), h('span', { class: 'chip-label' }, label), icon('down', { size: 12 }));
}

/**
 * Opens the start menu under `anchor`: Start now, Supertask, every live task of the sandbox under
 * "After a task is done" (the menu's filter finds one) and, when the sandbox has a task a rule
 * can name, "When a rule is true…". `kind` and `afterTaskId` mark the current choice; `onPick`
 * gets `{ kind: 'now' }`, `{ kind: 'supertask' }`, `{ kind: 'prepared' }` or
 * `{ kind: 'after', task }`, and `onRule` opens the caller's rule editor. With `onTime` (New task
 * only, SPEC-105) it also offers "When I decide" and "At a time…", which calls `onTime`; `at` is
 * the time picked before. With `prepare` (the automation editor, SPEC-155) it offers "When I
 * decide" alone. `exclude` is a task the menu leaves out: one cannot wait for itself.
 */
export function openStartMenu(anchor, ctx, sandbox, { kind, afterTaskId, at, onPick, onRule, onTime, prepare, exclude }) {
  const candidates = chainCandidates(ctx, sandbox).filter((t) => t.id !== exclude);
  openMenu(anchor, [
    { label: 'Start now', icon: 'up', description: 'The task starts as soon as a session slot is free', checked: kind === 'now', onSelect: () => onPick({ kind: 'now' }) },
    { label: 'Supertask', icon: 'shield', description: SUPERTASK_HELP, checked: kind === 'supertask', onSelect: () => onPick({ kind: 'supertask' }) },
    // Written now, started later (SPEC-105).
    ...(onTime || prepare ? [
      { label: 'When I decide', icon: 'st-prepared', description: WHEN_I_DECIDE_HELP, checked: kind === 'prepared', onSelect: () => onPick({ kind: 'prepared' }) },
    ] : []),
    ...(onTime ? [
      { label: 'At a time…', icon: 'clock', description: kind === 'scheduled' && at ? `Starts by itself ${whenLabel(at)}` : 'Starts by itself at the date and time you pick', checked: kind === 'scheduled', onSelect: onTime },
    ] : []),
    // One line per task under one heading, every live one: the menu's filter finds it (SPEC-038);
    // five show and the rest scroll in the group's box (SPEC-102).
    ...(candidates.length ? [{ separator: true }, {
      header: 'After a task is done',
      items: candidates.map((t) => ({ label: `#${t.number} ${t.title}`, hint: statusInfo(t).label, title: `Starts by itself when #${t.number} is done`, checked: kind === 'after' && t.id === afterTaskId, onSelect: () => onPick({ kind: 'after', task: t }) })),
    }] : []),
    // A rule names tasks of this sandbox, so a sandbox without any offers none (SPEC-022).
    ...(ruleChoices((ctx.store.tasksOf(sandbox.id) || []).filter((t) => t.id !== exclude)).length ? [
      { separator: true },
      { label: 'When a rule is true…', icon: 'branch', description: 'Pick tasks and whether they are done, merged or ended', checked: kind === 'rule', onSelect: onRule },
    ] : []),
  ]);
}

// ---------------------------------------------------------------- a waiting task (SPEC-126)

/** The start choice of a task that waits to start: 'prepared', 'scheduled', 'rule' or 'after'. */
export function waitingKind(task) {
  if (task.status === 'prepared') return task.start_at ? 'scheduled' : 'prepared';
  return task.after && task.after.rule ? 'rule' : 'after';
}

/** How a task starts now, after "#12 " in a toast. */
function startLine(task) {
  if (task.status === 'prepared') return task.start_at ? `starts ${whenLabel(task.start_at)}.` : 'waits until you start it.';
  if (task.status === 'chained' && task.after) return task.after.rule ? `starts when ${task.after.rule}.` : `runs after #${task.after.number}.`;
  return 'is starting.';
}

/**
 * Changes how a waiting task starts (`body` as New task's start choice; `{}`: Start now).
 * Resolves to the updated task; throws the server's refusal.
 */
export async function changeStart(ctx, task, body) {
  const updated = await ctx.api.put(`/api/tasks/${enc(task.id)}/start`, body);
  if (updated && updated.id) ctx.store.upsertTask(updated);
  toast(`#${task.number} ${startLine(updated || task)}`, { kind: 'ok' });
  return updated;
}

/**
 * The chip of a waiting task's view ("When I decide", "After #12", "When #12 done", "At …"): it
 * opens the start menu, without the task itself, and a pick changes how the task starts.
 */
export function waitingStartChip(ctx, sandbox, task) {
  const button = h('button', { type: 'button', class: 'chip-btn start-change', 'aria-haspopup': 'menu', 'aria-expanded': 'false', title: 'Change how this task starts' });
  const kind = waitingKind(task);
  renderStartChip(button, { kind, number: task.after && task.after.number, rule: task.after && task.after.rule, at: task.start_at, prefix: 'Starts: ' });
  const change = (body) => changeStart(ctx, task, body).catch((error) => errorToast(error, `Couldn’t change how #${task.number} starts`));
  const afterTaskId = kind === 'after' && task.after ? task.after.task_id : '';
  button.addEventListener('click', () => openStartMenu(button, ctx, sandbox, {
    kind,
    afterTaskId,
    at: task.start_at,
    exclude: task.id,
    onPick: ({ kind: picked, task: before }) => {
      // The choice it has already changes nothing.
      if (picked === kind && (picked !== 'after' || (before && before.id === afterTaskId))) return;
      change(picked === 'after' ? { after_task_id: before.id } : picked === 'supertask' ? { supertask: true } : picked === 'prepared' ? { prepared: true } : {});
    },
    onRule: () => openRuleEditor({
      tasks: (ctx.store.tasksOf(sandbox.id) || []).filter((t) => t.id !== task.id),
      value: kind === 'rule' ? ruleFromText(task.after.rule) : null,
      onUse: (rule) => change({ start_rule: ruleText(rule) }),
    }),
    // The dialog stays open with the server's reason when it refuses the time.
    onTime: () => openScheduleDialog({ value: task.start_at, onUse: (at) => changeStart(ctx, task, { start_at: at }) }),
  }));
  return button;
}
