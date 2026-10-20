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

// Formatters are built once: toLocaleString with options builds a new one on every call, which
// made date tooltips most of the cost of re-rendering long task lists (PERF-06).
const DATE_TIME = new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
const TIME = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' });
const WEEKDAY = new Intl.DateTimeFormat(undefined, { weekday: 'long' });
const MONTH_DAY = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' });

/** Local date-time for tooltips and tables. */
export function dateTime(iso) {
  if (!iso) return '—';
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return String(iso);
  return DATE_TIME.format(t);
}

/**
 * When a usage limit resets, the way Claude says it: "Resets at 5:50 PM" within a day,
 * "Resets Friday 10:00 AM" within a week, otherwise "Resets Oct 12".
 */
export function resetLabel(iso, now = Date.now()) {
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return '';
  const ahead = t - now;
  const time = TIME.format(t);
  if (ahead <= 0) return 'Resetting now';
  if (ahead < 20 * 3600e3 && new Date(t).getDate() === new Date(now).getDate()) return `Resets at ${time}`;
  if (ahead < 6.5 * 86400e3) return `Resets ${WEEKDAY.format(t)} ${time}`;
  return `Resets ${MONTH_DAY.format(t)}`;
}

const SHORT_DAY = new Intl.DateTimeFormat(undefined, { weekday: 'short' });

/**
 * When a prepared task starts (SPEC-105), in the person's time: "today 14:30", "tomorrow 09:00",
 * "Fri 09:00" within the week, otherwise "Oct 21 09:00".
 */
export function whenLabel(iso, now = Date.now()) {
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return '';
  const day = (ms) => { const d = new Date(ms); d.setHours(0, 0, 0, 0); return d.getTime(); };
  const days = Math.round((day(t) - day(now)) / 86400e3);
  const time = TIME.format(t);
  if (days === 0) return `today ${time}`;
  if (days === 1) return `tomorrow ${time}`;
  if (days > 1 && days < 7) return `${SHORT_DAY.format(t)} ${time}`;
  return `${MONTH_DAY.format(t)} ${time}`;
}

/** An ISO time as a `datetime-local` value in the person's time zone ("2026-10-15T09:00"). */
export function localInputValue(iso) {
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return '';
  const pad = (n) => String(n).padStart(2, '0');
  return `${t.getFullYear()}-${pad(t.getMonth() + 1)}-${pad(t.getDate())}T${pad(t.getHours())}:${pad(t.getMinutes())}`;
}

/** A `datetime-local` value (the person's time zone) as an ISO time in UTC, or null. */
export function fromLocalInput(value) {
  const t = value ? new Date(value) : null;
  return t && !Number.isNaN(t.getTime()) ? t.toISOString().replace(/\.\d{3}Z$/, 'Z') : null;
}

/** Duration from milliseconds: "45 s", "12 min", "1 h 05 min", "2 d 3 h". */
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
    // A list of days ("1,3,5", "0,6", "1-3,5"): their names, Monday first.
    if (/^\d(-\d)?(,\d(-\d)?)+$/.test(dow)) {
      const days = new Set();
      for (const item of dow.split(',')) { const [a, b = a] = item.split('-').map(Number); for (let d = a; d <= b && d <= 7; d += 1) days.add(d % 7); }
      const key = [...days].sort().join(',');
      if (key === '0,6') return `Weekends at ${hhmm()}`;
      if (key === '0,1,2,3,4,5,6') return `Every day at ${hhmm()}`;
      const names = [1, 2, 3, 4, 5, 6, 0].filter((d) => days.has(d)).map((d) => DOW[d].slice(0, 3));
      return `${names.slice(0, -1).join(', ')} and ${names.at(-1)} at ${hhmm()}`;
    }
  }
  if (isNum(min) && isNum(hour) && isNum(dom) && mon === '*' && dow === '*') return `Monthly on day ${dom} at ${hhmm()}`;
  return expr;
}

/** Plural helper: plural(3, 'task') → "3 tasks". */
/** A subscription's email, or "email unknown" until Claude gave one (SPEC-244 SUB-005). */
export function accountEmail(email) {
  return email || 'email unknown';
}

/**
 * A subscription as one line: its name and its Claude account's email (SPEC-244 SUB-005). The
 * server sends emails to administrators only (UI-022); `unknown` says whether a missing one reads
 * "email unknown" (an administrator's view) or is left out.
 */
export function accountRef(label, email, { unknown = true } = {}) {
  if (!label) return null;
  return email || unknown ? `${label} · ${accountEmail(email)}` : label;
}

export function plural(n, word, many) {
  return `${n} ${n === 1 ? word : many || `${word}s`}`;
}

/**
 * How an automation's tasks start, in a few words for its sentence and list row (SPEC-078):
 * "supertask", "after #12", "when #12 done", or '' for start now.
 */
export function startPhrase(a) {
  if (a.supertask) return 'supertask';
  if (a.after_task_id) return a.after_task_number ? `after #${a.after_task_number}` : 'after a task';
  if (a.start_rule) return `when ${a.start_rule}`;
  if (a.prepared) return 'when I decide';
  return '';
}
