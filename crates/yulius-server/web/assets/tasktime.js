// How long a task has been in the state a person sees (SPEC-087 TIM-003). Each sidebar group
// answers its own question, in words: "working 1h 20m" (since it last started working without
// needing anyone), "waiting 12m" (for you), "queued 2h", "paused 5m", "in review 3m" (its checks
// run), "done 3h ago", "stopped 2d ago". The tooltip gives the exact times. The sidebar, the Tasks
// page and a task's overview all use this, so they always agree.

import { dateTime, duration } from './format.js';

const MIN = 60_000;

/** Milliseconds of an RFC 3339 time, or null. */
function at(iso) {
  const ms = iso ? Date.parse(iso) : NaN;
  return Number.isFinite(ms) ? ms : null;
}

/**
 * A compact length for dense lists, rounded down so it never says more than it is: "<1m", "12m",
 * "1h 20m", "5h", "2d 3h", "3w".
 */
export function span(ms) {
  if (!Number.isFinite(ms)) return '';
  const minutes = Math.floor(Math.max(0, ms) / MIN);
  if (minutes < 1) return '<1m';
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 ? `${hours}h ${minutes % 60}m` : `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 14) return hours % 24 ? `${days}d ${hours % 24}h` : `${days}d`;
  return `${Math.floor(days / 7)}w`;
}

/** How long ago, in one unit: "just now", "12m ago", "3h ago", "2d ago", "3w ago". */
export function ago(ms) {
  if (!Number.isFinite(ms)) return '';
  const minutes = Math.floor(Math.max(0, ms) / MIN);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return days < 14 ? `${days}d ago` : `${Math.floor(days / 7)}w ago`;
}

const ENDED_WORDS = { closed: 'closed', stopping: 'stopping' };

/**
 * What a task's row says about time: `{ text, title, since }` — `text` names the measure
 * ("working 1h 20m"), `title` the exact times, `since` the start of the measure (ms) or null.
 * `info` is the task's `taskInfo` (status.js): its group decides the question.
 *
 * Measures, from the server's times (TaskSummary): `state_since` is the last change of the task's
 * status or activity (not of its title or anything else); `working_since` the start of its
 * working stretch, which goes on through the turns its own background work wakes and through
 * subscription switches. An older server without them falls back to `updated_at`.
 */
export function taskTime(task, info, now = Date.now()) {
  const state = at(task.state_since) ?? at(task.updated_at);
  const created = at(task.created_at);
  const tail = created !== null ? ` · created ${dateTime(new Date(created).toISOString())}` : '';
  const exact = (ms) => dateTime(new Date(ms).toISOString());
  // A length ("working 1h 20m") or a time ago ("done 3h ago"); `lead` starts the tooltip.
  const lasting = (word, since, lead) => (since === null
    ? { text: word, title: `${word[0].toUpperCase()}${word.slice(1)}${tail}`, since: null }
    : { text: `${word} ${span(now - since)}`, title: `${lead} ${exact(since)} (${duration(now - since)})${tail}`, since });
  const past = (word, since, lead) => (since === null
    ? { text: word, title: `${word[0].toUpperCase()}${word.slice(1)}${tail}`, since: null }
    : { text: `${word} ${ago(now - since)}`, title: `${lead} ${exact(since)}${tail}`, since });
  const status = task.status;
  const idle = status === 'running' && (task.activity === 'waiting' || task.activity === 'done');
  // Never ran (SPEC-153): it was not closed; it waits for Start onboarding, or was only created.
  if (info.notStarted) {
    return task.awaits_start_onboarding
      ? lasting('waiting', state, 'Waiting for Start onboarding since')
      : past('created', created, 'Created');
  }
  switch (info.group) {
    case 'needs':
      return lasting('waiting', state, 'Waiting for you since');
    case 'done':
      return past('done', state, 'Done since');
    case 'ended': {
      const word = ENDED_WORDS[status] || (String(status).startsWith('failed') ? 'failed' : 'stopped');
      if (status === 'stopping') return lasting(word, state, 'Stopping since');
      return past(word, state, `${word[0].toUpperCase()}${word.slice(1)}`);
    }
    default:
      if (status === 'queued' || status === 'chained') return lasting('queued', state, 'In line since');
      // Written and waiting to be started (SPEC-105).
      if (status === 'prepared') return lasting('prepared', state, task.start_at ? 'Scheduled since' : 'Prepared since');
      if (status === 'waiting_capacity') return lasting('paused', state, task.waits_for_account ? 'Waiting for a Claude account since' : 'Paused for subscription capacity since');
      // The agent finished its turn and its pull request's checks run: since the turn ended.
      if (idle && info.pr && !info.shells) return lasting('in review', state, 'In review since the agent finished its turn at');
      return lasting('working', at(task.working_since) ?? state, 'Working without needing you since');
  }
}
