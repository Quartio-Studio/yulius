// Formatting helpers shared by pages. Pure functions; no DOM.

/** Milliseconds since an RFC 3339 timestamp, or null. */
function ageMs(iso, now = Date.now()) {
  if (!iso) return null;
  const t = Date.parse(iso);
  return Number.isFinite(t) ? now - t : null;
}

/** Compact relative time for dense lists: "now", "4m", "3h", "2d", "5w". */
export function relShort(iso, now = Date.now()) {
  const ms = ageMs(iso, now);
  if (ms === null) return '';
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 45) return 'now';
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m`;
  const hours = Math.round(m / 60);
  if (hours < 24) return `${hours}h`;
  const d = Math.round(hours / 24);
  if (d < 14) return `${d}d`;
  return `${Math.round(d / 7)}w`;
}

/** Relative time in words, past or future: "5 min ago", "in 2 h". */
export function relLong(iso, now = Date.now()) {
  const ms = ageMs(iso, now);
  if (ms === null) return '—';
  const future = ms < 0;
  const abs = Math.abs(ms) / 1000;
  let text;
  if (abs < 45) return future ? 'in a moment' : 'just now';
  if (abs < 3600) text = `${Math.round(abs / 60)} min`;
  else if (abs < 86400) text = `${Math.round(abs / 3600)} h`;
  else text = `${Math.round(abs / 86400)} d`;
  return future ? `in ${text}` : `${text} ago`;
}

/** Local date-time for tooltips and tables. */
export function dateTime(iso) {
  if (!iso) return '—';
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return String(iso);
  return t.toLocaleString(undefined, { year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

/** Duration from milliseconds: "45 s", "12 min", "1 h 05 min", "2 d 3 h". */
/**
 * When a usage limit resets, the way Claude says it: "Resets at 5:50 PM" within a day,
 * "Resets Friday 10:00 AM" within a week, otherwise "Resets Oct 12".
 */
export function resetLabel(iso, now = Date.now()) {
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return '';
  const ahead = t - now;
  const time = new Date(t).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  if (ahead <= 0) return 'Resetting now';
  if (ahead < 20 * 3600e3 && new Date(t).getDate() === new Date(now).getDate()) return `Resets at ${time}`;
  if (ahead < 6.5 * 86400e3) return `Resets ${new Date(t).toLocaleDateString(undefined, { weekday: 'long' })} ${time}`;
  return `Resets ${new Date(t).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}`;
}

export function duration(ms) {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return '—';
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min`;
  const hours = Math.floor(m / 60);
  if (hours < 48) return `${hours} h ${String(m % 60).padStart(2, '0')} min`;
  return `${Math.floor(hours / 24)} d ${hours % 24} h`;
}

/** Thousands-separated integer, or "—" for null (unknown is never shown as zero). */
export function int(n) {
  if (n === null || n === undefined || !Number.isFinite(Number(n))) return '—';
  return Math.round(Number(n)).toLocaleString('en-US');
}

/** Compact number: 1,284 / 12.9K / 4.2M / 1.1B. */
export function compact(n) {
  if (n === null || n === undefined || !Number.isFinite(Number(n))) return '—';
  const v = Number(n);
  const abs = Math.abs(v);
  if (abs < 10000) return Math.round(v).toLocaleString('en-US');
  if (abs < 1e6) return `${(v / 1e3).toFixed(abs < 1e5 ? 1 : 0)}K`;
  if (abs < 1e9) return `${(v / 1e6).toFixed(abs < 1e7 ? 1 : 0)}M`;
  return `${(v / 1e9).toFixed(1)}B`;
}

/** Bytes in binary units. */
export function bytes(n) {
  if (n === null || n === undefined || !Number.isFinite(Number(n))) return '—';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let v = Number(n);
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i += 1; }
  return `${v.toFixed(v < 10 && i > 0 ? 1 : 0)} ${units[i]}`;
}

/** "UTC+02:00" from an offset in minutes. */
export function utcOffset(minutes) {
  const m = Number(minutes) || 0;
  const sign = m < 0 ? '−' : '+';
  const abs = Math.abs(m);
  return `UTC${sign}${String(Math.floor(abs / 60)).padStart(2, '0')}:${String(abs % 60).padStart(2, '0')}`;
}

const DOW = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];

/** Validate a 5-field cron expression syntactically (the server is authoritative). */
export function cronValid(expr) {
  const parts = String(expr || '').trim().split(/\s+/);
  if (parts.length !== 5) return false;
  const item = '(\\*|[0-9A-Za-z]+(-[0-9A-Za-z]+)?)(\\/[0-9]+)?';
  const re = new RegExp(`^${item}(,${item})*$`);
  return parts.every((p) => re.test(p));
}

/** Plain-language description for common cron shapes; falls back to the expression. */
export function describeCron(expr) {
  const parts = String(expr || '').trim().split(/\s+/);
  if (parts.length !== 5) return expr || '';
  const [min, hour, dom, mon, dow] = parts;
  const hhmm = () => `${hour.padStart(2, '0')}:${min.padStart(2, '0')}`;
  const isNum = (v) => /^\d+$/.test(v);
  if (/^\*\/\d+$/.test(min) && hour === '*' && dom === '*' && mon === '*' && dow === '*') return `Every ${min.slice(2)} minutes`;
  if (isNum(min) && hour === '*' && dom === '*' && mon === '*' && dow === '*') return `Every hour at :${min.padStart(2, '0')}`;
  if (isNum(min) && isNum(hour) && dom === '*' && mon === '*') {
    if (dow === '*') return `Every day at ${hhmm()}`;
    if (dow === '1-5') return `Weekdays at ${hhmm()}`;
    if (isNum(dow) && Number(dow) <= 7) return `Every ${DOW[Number(dow) % 7]} at ${hhmm()}`;
  }
  if (isNum(min) && isNum(hour) && isNum(dom) && mon === '*' && dow === '*') return `Monthly on day ${dom} at ${hhmm()}`;
  return expr;
}

/** The weekday names, Sunday first (cron numbering). */
export const WEEKDAYS = DOW;

/**
 * Reads a cron expression as one of the schedule presets (daily, weekdays, weekly, hourly) or
 * 'custom' when it is any other shape. Times are "HH:MM".
 */
export function parseSchedule(expr) {
  const parts = String(expr || '').trim().split(/\s+/);
  if (parts.length !== 5) return { kind: 'custom' };
  const [min, hour, dom, mon, dow] = parts;
  const num = (v, max) => /^\d+$/.test(v) && Number(v) <= max;
  if (!num(min, 59) || dom !== '*' || mon !== '*') return { kind: 'custom' };
  if (hour === '*' && dow === '*') return { kind: 'hourly', minute: Number(min) };
  if (!num(hour, 23)) return { kind: 'custom' };
  const time = `${hour.padStart(2, '0')}:${min.padStart(2, '0')}`;
  if (dow === '*') return { kind: 'daily', time };
  if (dow === '1-5') return { kind: 'weekdays', time };
  if (num(dow, 7)) return { kind: 'weekly', time, day: Number(dow) % 7 };
  return { kind: 'custom' };
}

/** The cron expression for a schedule preset; null for 'custom'. */
export function scheduleCron({ kind, time = '09:00', day = 1, minute = 0 }) {
  const [hh, mm] = String(time || '09:00').split(':').map((v) => Number(v) || 0);
  if (kind === 'hourly') return `${Math.min(59, Math.max(0, Math.round(Number(minute) || 0)))} * * * *`;
  if (kind === 'daily') return `${mm} ${hh} * * *`;
  if (kind === 'weekdays') return `${mm} ${hh} * * 1-5`;
  if (kind === 'weekly') return `${mm} ${hh} * * ${day}`;
  return null;
}

/** Plural helper: plural(3, 'task') → "3 tasks". */
export function plural(n, word, many) {
  return `${n} ${n === 1 ? word : many || `${word}s`}`;
}
