// Small charts beside a figure (SPEC-110): the trend behind a number in a Dashboard figure or a
// Capacity tile. One series, the app's series colour, no axes; the number stays in the tile.

import { svgEl } from './dom.js';

/**
 * A trend line of `values` (oldest first; null is a period without a value).
 * With `label` ("Tokens, last 7 days") it is an image whose text alternative names its first,
 * highest and last values formatted with `format` (TUS-010), also shown on hover; without one it
 * is decorative (a chart nearby has the values). Null when fewer than two values are known.
 */
export function sparkline(values, { label = null, format = String } = {}) {
  const known = values.filter((v) => Number.isFinite(v));
  if (values.length < 2 || known.length < 2) return null;
  const w = 120;
  const hgt = 28;
  const max = Math.max(...known, 1e-9);
  // A period without a value (no tasks that day, too few readings) is skipped: the line joins the
  // periods around it, so a lone value between two gaps still shows.
  const line = values.map((v, i) => (Number.isFinite(v) ? [(i / (values.length - 1)) * w, hgt - 2 - (Math.max(0, v) / max) * (hgt - 4)] : null))
    .filter(Boolean).map(([x, y], i) => `${i ? 'L' : 'M'}${x.toFixed(1)},${y.toFixed(1)}`).join('');
  const first = values.findIndex((v) => Number.isFinite(v));
  const last = values.length - 1 - [...values].reverse().findIndex((v) => Number.isFinite(v));
  const x = (i) => ((i / (values.length - 1)) * w).toFixed(1);
  const said = label ? `${label}: first ${format(known[0])}, highest ${format(Math.max(...known))}, last ${format(known[known.length - 1])}` : null;
  return svgEl('svg', { class: 'spark', viewBox: `0 0 ${w} ${hgt}`, preserveAspectRatio: 'none', ...(said ? { role: 'img', 'aria-label': said } : { 'aria-hidden': 'true' }) },
    said ? svgEl('title', null, said) : null,
    svgEl('path', { class: 'spark-area', d: `${line}L${x(last)},${hgt}L${x(first)},${hgt}Z` }),
    svgEl('path', { class: 'spark-line', d: line }));
}

/** US dollars for an estimate: "$0.42", "$18.60", "$1,234". */
export function usd(n) {
  if (!Number.isFinite(n)) return '—';
  return n < 100 ? `$${n.toFixed(2)}` : `$${Math.round(n).toLocaleString('en-US')}`;
}
