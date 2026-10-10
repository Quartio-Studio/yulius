// Task status presentation and permitted actions (SPEC-001 §6.1, §6.3, §7.1; SPEC-002 §6.1).

import { BRAND } from './brand.js';
import { resetLabel } from './format.js';

/** Statuses with a live process or pending start. */
export const LIVE_STATUSES = new Set(['queued', 'starting', 'running', 'switching', 'waiting_capacity', 'stopping']);

const STOP_LABELS = {
  stopped_cancelled: 'Stopped',
  stopped_crashed: 'Stopped (crashed)',
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
  stopped_lost: { tone: 'warn', text: `The agent's process was lost. ${SAFE}` },
  stopped_switch_failed: { tone: 'warn', text: `Moving to another AI account failed, so the agent stopped. ${SAFE}`, accounts: true },
  stopped_switch_timeout: { tone: 'warn', text: `Moving to another AI account took too long, so the agent stopped. ${SAFE}`, accounts: true },
  stopped_server_lost: { tone: 'warn', text: `${BRAND.name} restarted while the agent was working. ${SAFE}` },
  failed_spawn_error: { tone: 'error', text: 'Claude Code could not start — usually an AI account must sign in again, or no slot is free.', accounts: true, retry: true },
  failed_start_timeout: { tone: 'error', text: 'Claude Code took too long to start — usually an AI account must sign in again, or the machine is busy.', accounts: true, retry: true },
  failed_precondition: { tone: 'error', text: 'The task could not start because the sandbox is not ready yet.', retry: true },
};

/**
 * What to tell the user when a task's session has ended (UX-02): a plain sentence with a `tone`
 * ('info' | 'warn' | 'error'), whether checking the AI accounts may help (`accounts`), and
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
 * How a task's status reads: an SVG `icon` (one distinct shape per state, dom.js), a colour
 * `tone`, a plain `label`, and the sidebar `group` — `needs` (waiting for the user), `working`
 * (running or on its way) or `ended` (stopped or failed). `glyph` is the text fallback.
 */
export function statusInfo(task) {
  const status = task.status;
  const activity = task.activity;
  if (status === 'running') {
    // needs_attention: blocked mid-turn (a permission prompt, a browser request, failing gates);
    // waiting: the agent finished its turn and the next move is the user's.
    if (activity === 'needs_attention') return { glyph: '!', icon: 'st-attention', tone: 'attention', label: 'Needs your action', group: 'needs' };
    if (activity === 'waiting') return { glyph: '○', icon: 'st-input', tone: 'waiting', label: 'Your turn', group: 'needs' };
    if (activity === 'working') return { glyph: '●', icon: 'st-working', tone: 'working', label: 'Working', group: 'working' };
    return { glyph: '●', icon: 'st-working', tone: 'running', label: 'Running', group: 'working' };
  }
  if (status === 'queued') return { glyph: '⧗', icon: 'clock', tone: 'queued', label: 'Queued', group: 'working' };
  if (status === 'starting') return { glyph: '⧗', icon: 'clock', tone: 'queued', label: 'Starting', group: 'working' };
  if (status === 'switching') return { glyph: '↻', icon: 'refresh', tone: 'switching', label: 'Switching account', group: 'working' };
  // Every subscription is at 98 % or limited (ROT-010): Yulius paused the task at its turn
  // boundary without telling the agent. Nothing is for the user to do: it continues by itself.
  if (status === 'waiting_capacity') return { glyph: '⏸', icon: 'clock', tone: 'queued', label: 'Paused', group: 'working' };
  if (status === 'stopping') return { glyph: '■', icon: 'st-stopped', tone: 'stopping', label: 'Stopping', group: 'working' };
  if (status === 'closed') return { glyph: '✓', icon: 'st-closed', tone: 'closed', label: 'Closed', group: 'ended' };
  if (status === 'stopped_cancelled') return { glyph: '■', icon: 'st-stopped', tone: 'stopped', label: STOP_LABELS[status], group: 'ended' };
  if (status in STOP_LABELS) return { glyph: '▲', icon: 'st-crashed', tone: 'stopped-abnormal', label: STOP_LABELS[status], group: 'ended' };
  if (status in FAIL_LABELS) return { glyph: '✕', icon: 'st-failed', tone: 'failed', label: FAIL_LABELS[status], group: 'ended' };
  return { glyph: '?', icon: 'st-stopped', tone: 'stopped', label: `Unknown (${status})`, group: 'ended' };
}

/** "resets ~14:20" (or "~Tuesday 14:20", "~Oct 12") from a reset time; '' when unknown. */
export function resetAround(iso) {
  const label = iso ? resetLabel(iso) : '';
  if (!label) return '';
  return label === 'Resetting now' ? 'resets now' : label.replace(/^Resets (at )?/, 'resets ~');
}

/** "Paused: waiting for subscription capacity (resets ~14:20)" for a paused task (ROT-010). */
export function pausedLine(task) {
  const reset = resetAround(task.resumes_at);
  return `Paused: waiting for subscription capacity${reset ? ` (${reset})` : ''}`;
}

/**
 * The second line of a task row (sidebar, Tasks page): what the agent is doing, or what the user
 * should do, in words — the reason of a browser request, the current step, or the state.
 */
export function activityLine(task, info = statusInfo(task)) {
  if (info.request) return info.request.reason || 'Do something in the sandbox browser';
  if (info.pr) return info.label;
  const steps = task.steps || {};
  if (task.status === 'running') {
    if (task.activity === 'needs_attention') return 'Approve or answer in the terminal';
    if (task.activity === 'waiting') return steps.total && steps.done >= steps.total ? 'All steps done — review it' : 'Agent replied — your turn';
    if (steps.total) return `Step ${Math.min((steps.done || 0) + 1, steps.total)}/${steps.total}${steps.current ? ` · ${steps.current}` : ''}`;
    return 'Working';
  }
  if (task.status === 'waiting_capacity') return pausedLine(task);
  if (info.group === 'ended' && steps.total) return `${info.label} · ${steps.done || 0}/${steps.total} steps`;
  return info.label;
}

/**
 * statusInfo plus what only the live store knows: a browser action request from the agent makes
 * a live task "Needs you in the browser", and the pull request of a finished turn makes it
 * "Done" (tested: checks green or not required, or merged), "In review" (checks running) or
 * "Needs you" (checks failing) (#35). `actionRequests` is store.state.actionRequests and `pr` the
 * task's pull request from store.pullsOf(). The sidebar and the Tasks page both use this, so they
 * always agree.
 */
export function taskInfo(task, actionRequests, pr) {
  const info = statusInfo(task);
  const map = actionRequests && actionRequests.get(task.sandbox_id);
  if (map && info.group !== 'ended') {
    for (const [tabId, request] of map) {
      if (request.task_id === task.id) return { ...info, icon: 'globe', tone: 'attention', label: 'Needs you in the browser', group: 'needs', detail: request.reason, request: { ...request, tab_id: tabId } };
    }
  }
  // The agent finished and its pull request exists (#35): done once it is tested — checks green
  // or not required — or merged; in review while checks run; needs you when they fail.
  // Only after a finished turn: a task waiting for approval (needs_attention) or working is not done (#68).
  if (pr && task.status === 'running' && task.activity === 'waiting' && (pr.state === 'open' || pr.state === 'merged')) {
    if (pr.state === 'open' && pr.checks === 'failure') return { ...info, icon: 'branch', tone: 'attention', label: `PR #${pr.number} · checks failing`, group: 'needs', pr };
    // Tested means known green (or no required checks); running or unreadable checks are not.
    if (pr.state === 'open' && !['success', 'none'].includes(pr.checks)) return { ...info, icon: 'branch', tone: 'waiting', label: `In review · PR #${pr.number} · ${pr.checks === 'pending' ? 'checks running' : 'checks not known yet'}`, group: 'working', pr };
    return { ...info, glyph: '✓', icon: 'check', tone: 'done', label: pr.state === 'merged' ? `Done · PR #${pr.number} merged` : `Done · PR #${pr.number}`, group: 'done', pr };
  }
  return info;
}

const RANK = { read: 1, write: 2, admin: 3 };

/** Whether an access level satisfies a minimum ('read' | 'write' | 'admin'). */
export function allows(access, minimum) {
  return (RANK[access] || 0) >= (RANK[minimum] || 99);
}

/** Actions available for a task given sandbox access (UI-013, §6.3). */
export function taskActions(task, access) {
  const write = allows(access, 'write');
  const live = isLive(task.status);
  const ended = !live;
  return {
    stop: write && live && task.status !== 'stopping',
    continue: write && ended && !task.archived,
    switch: write && !task.archived && (task.status === 'running' || task.status === 'waiting_capacity' || ended),
    deliver: write && !task.archived,
    feedback: allows(access, 'read'),
    archive: write && ended && !task.archived,
    restore: write && task.archived,
    delete: write && ended && task.status !== 'stopping',
  };
}
