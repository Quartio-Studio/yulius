// Task status presentation and permitted actions (SPEC-001 §6.1, §6.3, §7.1; SPEC-002 §6.1).

import { BRAND } from './brand.js';
import { resetLabel } from './format.js';

/** Statuses with a live process or pending start. */
export const LIVE_STATUSES = new Set(['queued', 'starting', 'running', 'switching', 'waiting_capacity', 'stopping']);

const STOP_LABELS = {
  stopped_cancelled: 'Stopped',
  stopped_crashed: 'Stopped (crashed)',
  stopped_killed: 'Stopped (process ended)',
  stopped_lost: 'Stopped (process lost)',
  stopped_switch_failed: 'Stopped (switch failed)',
  stopped_switch_timeout: 'Stopped (switch timed out)',
  stopped_server_lost: 'Stopped (server restarted)',
};

const FAIL_LABELS = {
  failed_spawn_error: 'Failed to start',
  failed_start_timeout: 'Failed (start timeout)',
  failed_precondition: 'Failed (precondition)',
};

const SAFE = 'Its branch and changes are safe.';
const ENDED_MESSAGES = {
  closed: { tone: 'info', text: 'This task is closed. Its branch and changes are kept.' },
  stopped_cancelled: { tone: 'info', text: `You stopped this task. ${SAFE}` },
  stopped_crashed: { tone: 'warn', text: `The agent stopped unexpectedly. ${SAFE}` },
  stopped_killed: { tone: 'warn', text: `The agent's process was ended outside ${BRAND.name} (for example killed). ${SAFE}` },
  stopped_lost: { tone: 'warn', text: `The agent's process was lost. ${SAFE}` },
  stopped_switch_failed: { tone: 'warn', text: `The session couldn’t continue on another subscription, so the agent stopped. ${SAFE}`, accounts: true },
  stopped_switch_timeout: { tone: 'warn', text: `Continuing on another subscription took too long, so the agent stopped. ${SAFE}`, accounts: true },
  stopped_server_lost: { tone: 'warn', text: `${BRAND.name} restarted while the agent was working. ${SAFE}` },
  failed_spawn_error: { tone: 'error', text: 'Claude Code couldn’t start — usually a subscription must sign in again, or no slot is free.', accounts: true, retry: true },
  failed_start_timeout: { tone: 'error', text: 'Claude Code took too long to start — usually a subscription must sign in again, or the machine is busy.', accounts: true, retry: true },
  failed_precondition: { tone: 'error', text: 'The task couldn’t start because the sandbox is not ready yet.', retry: true },
};

/**
 * What to tell the user when a task's session has ended (UX-02): a plain sentence with a `tone`
 * ('info' | 'warn' | 'error'), whether checking the subscriptions may help (`accounts`), and
 * whether the next step is to try again rather than continue (`retry`).
 */
export function endedMessage(status) {
  return ENDED_MESSAGES[status] || { tone: 'info', text: `The agent's session ended. ${SAFE}` };
}

/** True while a task has (or is about to have) a running session. */
export function isLive(status) {
  return LIVE_STATUSES.has(status);
}

/**
 * Background work the agent left running while idle (TSK-014): shells, and the background agents
 * Claude Code waits for (SPEC-032); 0 when none or not known. The field keeps its API name.
 */
export function backgroundShells(task) {
  return Number.isInteger(task.background_shells) && task.background_shells > 0 ? task.background_shells : 0;
}

/** "1 background task still running" (TSK-014). */
export function shellsLine(count, since) {
  // How long they have run (complaint 1: a wait that never ends shows its age).
  const minutes = since ? Math.floor((Date.now() - Date.parse(since)) / 60000) : NaN;
  const age = minutes >= 1 ? ` · ${minutes < 120 ? `${minutes} min` : `${Math.floor(minutes / 60)} h`}` : '';
  return `${count} background task${count === 1 ? '' : 's'} still running${age}`;
}

/** A finished task that still has background shells reads as Working, with them said (TSK-014). */
function stillWorking(count, since) {
  return { glyph: '●', icon: 'st-working', tone: 'working', label: 'Working', group: 'working', shells: count, shellsSince: since, detail: shellsLine(count, since) };
}

const DONE = { glyph: '✓', icon: 'check', tone: 'done', label: 'Done', group: 'done' };

/** Whether the task's plan has steps not done or skipped yet (STP-001, STP-005). */
export function planOpen(task) {
  const steps = task.steps || {};
  return Boolean(steps.total) && (steps.done || 0) < steps.total;
}

/**
 * How a task's status reads: an SVG `icon` (one distinct shape per state, dom.js), a colour
 * `tone`, a plain `label`, and the sidebar `group` — `needs` (waiting for the user), `working`
 * (running or on its way), `done` (finished, TSK-014) or `ended` (stopped or failed). `glyph` is
 * the text fallback.
 */
export function statusInfo(task) {
  const status = task.status;
  const activity = task.activity;
  if (status === 'running') {
    // needs_attention: blocked mid-turn (a permission prompt, a browser request, failing gates);
    // waiting: the agent finished its turn and the next move is the user's — unless its
    // background shells still run (it wakes by itself when they report, TSK-014);
    // done: the goal is achieved and the turn ended — Done once no background shell is left.
    if (activity === 'needs_attention') return { glyph: '!', icon: 'st-attention', tone: 'attention', label: 'Needs your action', group: 'needs' };
    if (activity === 'done') {
      const shells = backgroundShells(task);
      return shells ? stillWorking(shells, task.background_shells_since) : { ...DONE };
    }
    if (activity === 'waiting') {
      // Background shells keep the agent going: it wakes by itself when they report, so the next
      // move is not the user's yet.
      const shells = backgroundShells(task);
      if (shells) return stillWorking(shells, task.background_shells_since);
      return { glyph: '○', icon: 'st-input', tone: 'waiting', label: 'Your turn', group: 'needs' };
    }
    if (activity === 'working') return { glyph: '●', icon: 'st-working', tone: 'working', label: 'Working', group: 'working' };
    return { glyph: '●', icon: 'st-working', tone: 'running', label: 'Running', group: 'working' };
  }
  // Each waiting state has its own shape (#147): an hourglass in line for a slot, a ring being
  // drawn while the session starts, pause bars, and a chain link after another task.
  if (status === 'queued') {
    // Held by a supertask (SPEC-071): a supertask waits for the project's running tasks, any
    // other task for the supertask.
    const held = heldBy(task);
    if (held.length && task.supertask) return { glyph: '⧗', icon: 'st-queued', tone: 'queued', label: `Supertask · waits for ${numbers(held)}`, group: 'working' };
    if (held.length) return { glyph: '⧗', icon: 'st-queued', tone: 'queued', label: `Queued after supertask #${held[0]}`, group: 'working' };
    return { glyph: '⧗', icon: 'st-queued', tone: 'queued', label: 'Queued', group: 'working' };
  }
  if (status === 'starting') return { glyph: '◌', icon: 'st-starting', tone: 'queued', label: 'Starting', group: 'working' };
  // The pool moves the session to another subscription at a turn end (ROT-003): under the hood,
  // the task goes on (F 2026-10-10: tasks are subscription-agnostic).
  if (status === 'switching') return { glyph: '↻', icon: 'refresh', tone: 'switching', label: 'Continuing', group: 'working' };
  // No subscription has capacity left (every one at 98 % or limited, ROT-010): Yulius paused the
  // task at its turn boundary without telling the agent. Nothing is for the user to do: it
  // continues by itself. A task never pauses while another subscription has room.
  if (status === 'waiting_capacity') return { glyph: '⏸', icon: 'st-paused', tone: 'queued', label: 'Paused', group: 'working' };
  // A stop was asked for: the task is on its way out, so it sits with the ended ones rather than
  // flashing under Working until the process is gone.
  if (status === 'stopping') return { glyph: '■', icon: 'st-stopped', tone: 'stopping', label: 'Stopping', group: 'ended' };
  // Waiting for another task to be Done (TSK-012); when that task ended otherwise, a person
  // decides: Start now or Cancel. It never starts by itself then.
  if (status === 'chained') {
    const after = task.after;
    if (after && after.blocked) return { glyph: '!', icon: 'st-attention', tone: 'attention', label: after.rule ? 'Needs you · start rule' : `Needs you · after #${after.number}`, group: 'needs', detail: after.blocked };
    if (after && after.rule) return { glyph: '⛓', icon: 'st-chained', tone: 'queued', label: 'Queued by a rule', group: 'working' };
    return { glyph: '⛓', icon: 'st-chained', tone: 'queued', label: after ? (after.rule ? `Queued: when ${after.rule}` : `Queued after #${after.number}`) : 'Queued after another task', group: 'working' };
  }
  if (status === 'closed') return { glyph: '✓', icon: 'st-closed', tone: 'closed', label: 'Closed', group: 'ended' };
  if (status === 'stopped_cancelled') return { glyph: '■', icon: 'st-stopped', tone: 'stopped', label: STOP_LABELS[status], group: 'ended' };
  if (status in STOP_LABELS) return { glyph: '▲', icon: 'st-crashed', tone: 'stopped-abnormal', label: STOP_LABELS[status], group: 'ended' };
  if (status in FAIL_LABELS) return { glyph: '✕', icon: 'st-failed', tone: 'failed', label: FAIL_LABELS[status], group: 'ended' };
  return { glyph: '?', icon: 'st-stopped', tone: 'stopped', label: 'Unknown state', group: 'ended', detail: status };
}

/** What a queued task waits for because of a supertask (SPEC-071): task numbers, or []. */
export function heldBy(task) {
  return Array.isArray(task.held_by) ? task.held_by.filter(Number.isInteger) : [];
}

/** "#3", "#3 and #5", "#3, #5 and #8"; past four, "#3, #5, #8 and 2 more". */
export function numbers(list) {
  const named = list.slice(0, list.length > 4 ? 3 : 4).map((n) => `#${n}`);
  const rest = list.length - named.length;
  if (rest) return `${named.join(', ')} and ${rest} more`;
  return named.length > 1 ? `${named.slice(0, -1).join(', ')} and ${named[named.length - 1]}` : named.join('');
}

/** "resets ~14:20" (or "~Tuesday 14:20", "~Oct 12") from a reset time; '' when unknown. */
export function resetAround(iso) {
  const label = iso ? resetLabel(iso) : '';
  if (!label) return '';
  return label === 'Resetting now' ? 'resets now' : label.replace(/^Resets (at )?/, 'resets ~');
}

/**
 * "Paused: no subscription has capacity left (resets ~14:20)" for a paused task (ROT-010): it
 * pauses only when every subscription is used up, and continues by itself.
 */
export function pausedLine(task) {
  const reset = resetAround(task.resumes_at);
  return `Paused: no subscription has capacity left${reset ? ` (${reset})` : ''}`;
}

/**
 * The second line of a task row (sidebar, Tasks page): what the agent is doing, or what the user
 * should do, in words — the reason of a browser request, the current step, or the state.
 */
export function activityLine(task, info = statusInfo(task)) {
  if (info.request) return info.request.reason || 'Do something in the sandbox browser';
  if (info.held) return 'A prompt was held · choose what to do';
  if (info.shells) return shellsLine(info.shells, info.shellsSince);
  if (info.pr) return info.label;
  const steps = task.steps || {};
  if (task.status === 'running') {
    if (task.activity === 'done') return steps.total ? `Goal achieved · ${steps.done || 0}/${steps.total} steps` : 'Goal achieved';
    if (task.activity === 'needs_attention') return 'Approve or answer in the terminal';
    if (task.activity === 'waiting') return steps.total && steps.done >= steps.total ? 'All steps done — review it' : 'Agent replied — your turn';
    if (steps.total) return `Step ${Math.min((steps.done || 0) + 1, steps.total)}/${steps.total}${steps.current ? ` · ${steps.current}` : ''}`;
    return 'Working';
  }
  if (task.status === 'chained') {
    const after = task.after;
    if (after && after.blocked) return after.blocked;
    if (after && after.rule) return `Starts by itself when ${after.rule}`;
    return after ? `Starts by itself when #${after.number} is done` : info.label;
  }
  if (task.status === 'waiting_capacity') return pausedLine(task);
  if (task.status === 'queued' && heldBy(task).length) return supertaskLine(task);
  if (info.group === 'ended' && steps.total) return `${info.label} · ${steps.done || 0}/${steps.total} steps`;
  return info.label;
}

/** Why a queued task waits for a supertask, in a sentence (SPEC-071). */
export function supertaskLine(task) {
  const held = heldBy(task);
  if (task.supertask) return `It starts once ${numbers(held)} ${held.length === 1 ? 'finishes' : 'finish'}, then runs alone; the project's other tasks wait for it.`;
  return `It starts once ${numbers(held)} ${held.length === 1 ? 'is' : 'are'} done: a supertask runs alone in this project.`;
}

/**
 * statusInfo plus what only the live store knows: a browser action request from the agent makes
 * a live task "Needs you in the browser", and the pull request of a finished turn makes it
 * "Done" (tested: checks green or not required, or merged), "In review" (checks running) or
 * "Needs you" (checks failing) (#35). `actionRequests` is store.state.actionRequests and `pr` the
 * task's pull request from store.pullsOf(). The sidebar, the Tasks page, the task header and the
 * Git page all use this, so they always agree.
 *
 * The one rule for Done (SPEC-001 TSK-014): the last turn ended cleanly, nothing needs the user,
 * no background shell is left, and the work is evidently complete — Claude Code recorded the goal
 * as achieved (activity `done`), or the plan is complete and the pull request is tested. Failing
 * checks need the user; a background shell keeps it Working; checks still running (without an
 * achieved goal) are In review. The group `needs` is what Home and the tabs count.
 */
export function taskInfo(task, actionRequests, pr) {
  const info = statusInfo(task);
  // A prompt typed into the session was held before it reached the agent (TSK-013).
  if (task.held_prompt && info.group !== 'ended') return { ...info, icon: 'st-attention', tone: 'attention', label: 'Prompt held · your choice', group: 'needs', held: task.held_prompt };
  const map = actionRequests && actionRequests.get(task.sandbox_id);
  if (map && info.group !== 'ended') {
    for (const [tabId, request] of map) {
      if (request.task_id === task.id) return { ...info, icon: 'globe', tone: 'attention', label: 'Needs you in the browser', group: 'needs', detail: request.reason, request: { ...request, tab_id: tabId } };
    }
  }
  // The agent finished and its pull request exists (#35): done once it is tested — checks green
  // or not required — or merged; in review while checks run; needs you when they fail.
  // Only after a finished turn: a task waiting for approval (needs_attention) or working is not done (#68).
  // A plan with open steps is not finished work, whatever its pull request says (#139): the task's
  // own state shows ("Your turn"); failing checks still need you. The server counts the same way
  // (`AttentionFacts` in yulius-domain).
  const finished = task.status === 'running' && (task.activity === 'waiting' || task.activity === 'done');
  if (pr && finished && (pr.state === 'open' || pr.state === 'merged')) {
    if (pr.state === 'open' && pr.checks === 'failure') return { ...info, icon: 'branch', tone: 'attention', label: `PR #${pr.number} · checks failing`, group: 'needs', pr };
    // CI could not run (disk, a timeout, the host): not the code, and nothing for a person yet; the
    // agent is told to run it again (S5).
    if (pr.state === 'open' && pr.checks === 'error') return { ...info, icon: 'branch', tone: 'waiting', label: `In review · PR #${pr.number} · CI could not run (not the code)`, group: 'working', pr };
    if (planOpen(task)) return info;
    const shells = backgroundShells(task);
    if (shells) return { ...info, ...stillWorking(shells, task.background_shells_since), pr };
    const done = { ...info, ...DONE, label: pr.state === 'merged' ? `Done · PR #${pr.number} merged` : `Done · PR #${pr.number}`, pr };
    // An achieved goal is done whatever the checks are still doing (TSK-014).
    if (task.activity === 'done') return done;
    // Tested means known green (or no required checks); running or unreadable checks are not.
    if (pr.state === 'open' && !['success', 'none'].includes(pr.checks)) return { ...info, icon: 'branch', tone: 'waiting', label: `In review · PR #${pr.number} · ${pr.checks === 'pending' ? 'checks running' : 'checks not known yet'}`, group: 'working', pr };
    return done;
  }
  return info;
}

/**
 * The counter a task adds to on Home's cards and the sandbox tabs (#170), from its taskInfo:
 * `needs`, `queued` (queued or chained: the Working group's tasks in line) or `working` (the rest
 * of the Working group), or null for Done and ended tasks. So a card's working + queued is the
 * sidebar's Working count. The server counts the same way (`AttentionFacts::group`).
 */
export function counter(task, info) {
  if (info.group === 'needs') return 'needs';
  if (info.group !== 'working') return null;
  return task.status === 'queued' || task.status === 'chained' ? 'queued' : 'working';
}

const RANK = { read: 1, write: 2, admin: 3 };

/** Whether an access level satisfies a minimum ('read' | 'write' | 'admin'). */
export function allows(access, minimum) {
  return (RANK[access] || 0) >= (RANK[minimum] || 99);
}

/**
 * Actions available for a task given sandbox access (UI-013, §6.3). `pr` is the task's pull
 * request (store.pullsOf()), so a task the sidebar shows as Done can be archived or deleted.
 */
export function taskActions(task, access, pr) {
  const write = allows(access, 'write');
  const live = isLive(task.status);
  const chained = task.status === 'chained';
  const ended = !live && !chained;
  const finished = live && taskInfo(task, null, pr).group === 'done';
  return {
    // A stopping task can be stopped again: a stop whose session could not be ended is retried
    // (SPEC-065).
    stop: write && (live || chained),
    startNow: write && chained,
    continue: write && ended && !task.archived,
    deliver: write && !task.archived,
    feedback: allows(access, 'read'),
    // A finished task (Done by the one rule of taskInfo: goal achieved, or plan complete with its
    // pull request merged or tested, and no shell left) is closed as it is archived (TSK-014) or
    // deleted (SPEC-039).
    archive: write && (ended || finished) && !task.archived,
    restore: write && task.archived,
    delete: write && (ended || chained || finished) && task.status !== 'stopping',
  };
}
