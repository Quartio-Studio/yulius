// Task status presentation and permitted actions (SPEC-001 §6.1, §6.3, §7.1; SPEC-002 §6.1).

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

/** True while a task has (or is about to have) a running session. */
export function isLive(status) {
  return LIVE_STATUSES.has(status);
}

/**
 * Icon, tone and label for a task's status/activity.
 * Glyphs follow SPEC-001 §6.1: ● working, ○ waiting, ! needs attention, ↻ switching,
 * ■ stopped/closed, ✕ failed, ⧗ queued.
 */
export function statusInfo(task) {
  const status = task.status;
  const activity = task.activity;
  if (status === 'running') {
    if (activity === 'needs_attention') return { glyph: '!', tone: 'attention', label: 'Needs attention' };
    if (activity === 'waiting') return { glyph: '○', tone: 'waiting', label: 'Waiting for input' };
    if (activity === 'working') return { glyph: '●', tone: 'working', label: 'Working' };
    return { glyph: '●', tone: 'running', label: 'Running' };
  }
  if (status === 'queued') return { glyph: '⧗', tone: 'queued', label: 'Queued' };
  if (status === 'starting') return { glyph: '⧗', tone: 'queued', label: 'Starting' };
  if (status === 'switching') return { glyph: '↻', tone: 'switching', label: 'Switching' };
  if (status === 'waiting_capacity') return { glyph: '!', tone: 'attention', label: 'Waiting for capacity' };
  if (status === 'stopping') return { glyph: '■', tone: 'stopping', label: 'Stopping' };
  if (status === 'closed') return { glyph: '■', tone: 'closed', label: 'Closed' };
  if (status in STOP_LABELS) return { glyph: '■', tone: status === 'stopped_cancelled' ? 'stopped' : 'stopped-abnormal', label: STOP_LABELS[status] };
  if (status in FAIL_LABELS) return { glyph: '✕', tone: 'failed', label: FAIL_LABELS[status] };
  return { glyph: '?', tone: 'stopped', label: `Unknown (${status})` };
}

/** Coarse group used by filters: active | attention | closed | stopped | failed. */
export function statusGroup(task) {
  const info = statusInfo(task);
  if (info.tone === 'attention') return 'attention';
  if (isLive(task.status)) return 'active';
  if (task.status === 'closed') return 'closed';
  if (task.status.startsWith('failed_')) return 'failed';
  return 'stopped';
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

/** "C" or "X" harness glyph and display name. */
export function harnessInfo(harness) {
  if (harness === 'codex') return { glyph: 'X', name: 'Codex' };
  if (harness === 'claude') return { glyph: 'C', name: 'Claude Code' };
  return { glyph: '?', name: String(harness || 'unknown') };
}
