// What a subscription's stored usage means at a moment (SPEC-473 RFR-008). The server stores a
// subscription's `status` at each reading; between readings a window resets, or reaches its end on
// a subscription whose status was written before. So the page works out from the windows and their
// reset times whether the default model can run on a subscription now, which windows are used up,
// and how old the reading is. The card, the Status panel and "Run #N on…" all ask here.

import { relLong, resetLabel } from './format.js';
import { PROVIDERS } from './models.js';

/**
 * A reading older than this may be out of date: the background round reads an idle subscription
 * every 5 minutes, so an older one means the round is backing off or cannot read.
 */
export const STALE_AFTER_MS = 10 * 60 * 1000;

/** The model a task runs when it names none: the first one offered, as on the server. */
const DEFAULT_MODEL = PROVIDERS[0].models[0].id;

/** The windows a card shows, by their key in the API's usage: the name people read. */
const WINDOWS = [
  ['primary', '5-hour window'],
  ['secondary', 'This week'],
  ['weekly_opus', 'Opus this week'],
  ['weekly_sonnet', 'Sonnet this week'],
  ['weekly_fable', 'Fable this week'],
];

/** Milliseconds of an RFC 3339 time, or null. */
function at(iso) {
  const ms = iso ? Date.parse(iso) : NaN;
  return Number.isFinite(ms) ? ms : null;
}

/** The weekly window of a model's own on a subscription's usage (its cap, or its allowance of usage credits), or null. */
export function modelWindow(usage, model) {
  const name = String(model || '').toLowerCase();
  if (name.includes('opus')) return usage.weekly_opus || null;
  if (name.includes('sonnet')) return usage.weekly_sonnet || null;
  if (name.includes('fable')) return usage.weekly_fable || null;
  return null;
}

/** Whether a usage window is past its reset at `now`: a new window began, which nobody has read yet. */
function hasReset(w, now) {
  const reset = w ? at(w.resets_at) : null;
  return reset !== null && reset <= now;
}

/** A window's percent as it is known at `now`: null (not read) without a reading, and after its reset. */
export function percentNow(w, now = Date.now()) {
  return w && Number.isFinite(w.used_percent) && !hasReset(w, now) ? w.used_percent : null;
}

/** Whether a usage window is used up at `now`: at 100% and not past its reset. */
export function usedUp(w, now = Date.now()) {
  return Boolean(w) && Number(w.used_percent) >= 100 && !hasReset(w, now);
}

/** Whether a limit Claude reported for the whole subscription is in effect at `now`. */
export function limitInEffect(usage, now = Date.now()) {
  const ends = at(usage.limit_resets_at);
  return Boolean(usage.limit_reached) && !(ends !== null && ends <= now);
}

/** "resets at 5:50 PM", or that Claude named no reset. */
function when(iso, now) {
  const label = iso ? resetLabel(iso, now) : '';
  return label ? `${label[0].toLowerCase()}${label.slice(1)}` : 'Claude did not say when it resets';
}

/**
 * What a subscription's stored usage means at `now`:
 * - `limited`: the default model cannot run on it: a limit of the whole subscription is in
 *   effect, or the 5-hour, the weekly or the default model's own window is used up;
 * - `state`: `limited` or `ok` by that, or the stored status of a subscription that needs a
 *   sign-in (`relogin`, `pending_login`, `error`), which no window changes;
 * - `lines`: one `{ key, text }` per used-up window with its reset ("5-hour window used up ·
 *   resets at 5:50 PM"), also another model's, and one for a limit of the subscription that no
 *   window shows;
 * - `age`: how old the reading is, in words; `stale`: older than [`STALE_AFTER_MS`].
 */
export function limitsNow(account, now = Date.now()) {
  const usage = account.usage || {};
  const own = modelWindow(usage, DEFAULT_MODEL);
  const full = WINDOWS.map(([key, name]) => ({ key, name, window: usage[key] })).filter((w) => usedUp(w.window, now));
  const ofSubscription = full.some((w) => w.key === 'primary' || w.key === 'secondary');
  const reported = limitInEffect(usage, now);
  const limited = reported || ofSubscription || full.some((w) => w.window === own);
  const lines = full.map((w) => ({ key: w.key, text: `${w.name} used up · ${when(w.window.resets_at, now)}` }));
  if (reported && !ofSubscription) {
    lines.push({ key: 'reported', text: usage.limit_resets_at ? `Claude reported a usage limit · ${when(usage.limit_resets_at, now)}` : 'Claude reported a usage limit without saying when it resets' });
  }
  const read = at(usage.observed_at);
  const settled = account.status === 'ok' || account.status === 'limited' || !account.status;
  return {
    state: settled ? (limited ? 'limited' : 'ok') : account.status,
    limited,
    lines,
    age: read === null ? 'Usage not read yet' : `Usage read ${relLong(usage.observed_at, now)}`,
    stale: read !== null && now - read > STALE_AFTER_MS,
  };
}
