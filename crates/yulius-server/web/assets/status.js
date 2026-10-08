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
 * How a task's status reads: an SVG `icon` (one distinct shape per state, dom.js), a colour
 * `tone`, a plain `label`, and the sidebar `group` — `needs` (waiting for the user), `working`
 * (running or on its way) or `ended` (stopped or failed). `glyph` is the text fallback.
 */
export function statusInfo(task) {
  const status = task.status;
  const activity = task.activity;
  if (status === 'running') {
    if (activity === 'needs_attention') return { glyph: '!', icon: 'st-attention', tone: 'attention', label: 'Needs attention', group: 'needs' };
    if (activity === 'waiting') return { glyph: '○', icon: 'st-input', tone: 'waiting', label: 'Waiting for your input', group: 'needs' };
    if (activity === 'working') return { glyph: '●', icon: 'st-working', tone: 'working', label: 'Working', group: 'working' };
    return { glyph: '●', icon: 'st-working', tone: 'running', label: 'Running', group: 'working' };
  }
  if (status === 'queued') return { glyph: '⧗', icon: 'clock', tone: 'queued', label: 'Queued', group: 'working' };
  if (status === 'starting') return { glyph: '⧗', icon: 'clock', tone: 'queued', label: 'Starting', group: 'working' };
  if (status === 'switching') return { glyph: '↻', icon: 'refresh', tone: 'switching', label: 'Switching account', group: 'working' };
  // Nothing for the user to do: Yulius starts it when a session slot frees up.
  if (status === 'waiting_capacity') return { glyph: '⧗', icon: 'clock', tone: 'queued', label: 'Waiting for a free slot', group: 'working' };
  if (status === 'stopping') return { glyph: '■', icon: 'st-stopped', tone: 'stopping', label: 'Stopping', group: 'working' };
  if (status === 'closed') return { glyph: '✓', icon: 'st-closed', tone: 'closed', label: 'Closed', group: 'ended' };
  if (status === 'stopped_cancelled') return { glyph: '■', icon: 'st-stopped', tone: 'stopped', label: STOP_LABELS[status], group: 'ended' };
  if (status in STOP_LABELS) return { glyph: '▲', icon: 'st-crashed', tone: 'stopped-abnormal', label: STOP_LABELS[status], group: 'ended' };
  if (status in FAIL_LABELS) return { glyph: '✕', icon: 'st-failed', tone: 'failed', label: FAIL_LABELS[status], group: 'ended' };
  return { glyph: '?', icon: 'st-stopped', tone: 'stopped', label: `Unknown (${status})`, group: 'ended' };
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
  if (harness === 'claude') return { glyph: 'C', name: 'Claude Code' };
  return { glyph: '?', name: String(harness || 'unknown') };
}
