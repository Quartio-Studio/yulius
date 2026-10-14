// How a task starts: New task's start chip and its menu, shared with the automation editor's
// action bar (SPEC-093). Start now, Supertask (SPEC-071), after a task is done (TSK-012; the
// filterable list of SPEC-038) or when a rule is true (SPEC-017, picked in the rule editor of
// SPEC-022); New task also prepares a task, started when you decide or at a time (SPEC-105).
// Each page keeps its own state and opens the rule editor with its own value.

import { h, icon, setChildren, openMenu } from './dom.js';
import { statusInfo } from './status.js';
import { whenLabel } from './format.js';
import { chainCandidates } from './intake.js';
import { ruleChoices } from './ruleeditor.js';

export const SUPERTASK_HELP = 'Waits for this project’s running tasks, then runs alone while the project’s other tasks wait. For broad work such as security hardening.';

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
 * the time picked before.
 */
export function openStartMenu(anchor, ctx, sandbox, { kind, afterTaskId, at, onPick, onRule, onTime }) {
  const candidates = chainCandidates(ctx, sandbox);
  openMenu(anchor, [
    { label: 'Start now', icon: 'up', description: 'The task starts as soon as a session slot is free', checked: kind === 'now', onSelect: () => onPick({ kind: 'now' }) },
    { label: 'Supertask', icon: 'shield', description: SUPERTASK_HELP, checked: kind === 'supertask', onSelect: () => onPick({ kind: 'supertask' }) },
    // Written now, started later (SPEC-105).
    ...(onTime ? [
      { label: 'When I decide', icon: 'st-prepared', description: WHEN_I_DECIDE_HELP, checked: kind === 'prepared', onSelect: () => onPick({ kind: 'prepared' }) },
      { label: 'At a time…', icon: 'clock', description: kind === 'scheduled' && at ? `Starts by itself ${whenLabel(at)}` : 'Starts by itself at the date and time you pick', checked: kind === 'scheduled', onSelect: onTime },
    ] : []),
    // One line per task under one heading, every live one: the menu's filter finds it (SPEC-038);
    // five show and the rest scroll in the group's box (SPEC-102).
    ...(candidates.length ? [{ separator: true }, {
      header: 'After a task is done',
      items: candidates.map((t) => ({ label: `#${t.number} ${t.title}`, hint: statusInfo(t).label, title: `Starts by itself when #${t.number} is done`, checked: kind === 'after' && t.id === afterTaskId, onSelect: () => onPick({ kind: 'after', task: t }) })),
    }] : []),
    // A rule names tasks of this sandbox, so a sandbox without any offers none (SPEC-022).
    ...(ruleChoices(ctx.store.tasksOf(sandbox.id)).length ? [
      { separator: true },
      { label: 'When a rule is true…', icon: 'branch', description: 'Pick tasks and whether they are done, merged or ended', checked: kind === 'rule', onSelect: onRule },
    ] : []),
  ]);
}
