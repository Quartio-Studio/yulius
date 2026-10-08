// Brand lab: a development aid for choosing the brand on the real screens. It loads only with
// ?lab in the URL (?lab=1, ?lab=<proposal>, ?lab=<direction> or a copied ?lab=<settings> link) and
// overrides tokens.css live. Proposals (lab-proposals.js, or pasted as JSON) set any tokens — colours
// of both themes, typefaces, type scale, radii, motion; directions are generated from a few sliders.
// It measures the repository's contrast pairs in both themes as you go and copies the result, which
// is then written into tokens.css and brand.js for good. Fonts come from Google Fonts here only;
// shipped fonts are self-hosted (web/README.md, "Brand typeface").

import { h, icon, setChildren, toast, keepFocus, openModal, invalid } from './dom.js';
import { applyTheme, activeTheme, setThemeChoice, onThemeChange } from './theme.js';
import { readJson, writeJson } from './store.js';
import { PROPOSALS } from './lab-proposals.js';

// ------------------------------------------------------------------ colour

/** OKLCH (perceptual lightness, chroma, hue) to sRGB 0–1, or null when out of gamut. */
function oklchToRgb(L, C, hue) {
  const a = C * Math.cos((hue * Math.PI) / 180);
  const b = C * Math.sin((hue * Math.PI) / 180);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const linear = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  if (linear.some((v) => v < -0.0005 || v > 1.0005)) return null;
  return linear.map((v) => {
    const x = Math.min(1, Math.max(0, v));
    return x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055;
  });
}

/** An OKLCH colour as #rrggbb; chroma is reduced until the colour fits sRGB. */
function oklch(L, C, hue) {
  let low = 0;
  let high = Math.max(0, C);
  let rgb = oklchToRgb(L, high, hue);
  if (!rgb) {
    for (let i = 0; i < 20; i += 1) {
      const mid = (low + high) / 2;
      if (oklchToRgb(L, mid, hue)) low = mid; else high = mid;
    }
    rgb = oklchToRgb(L, low, hue) || [L, L, L];
  }
  return `#${rgb.map((v) => Math.round(v * 255).toString(16).padStart(2, '0')).join('')}`;
}

function luminance(hex) {
  const channels = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
    .map((v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
}

function contrast(a, b) {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

/**
 * APCA lightness contrast (0.0.98G-4g, the WCAG 3 draft's perceptual measure) of text on a
 * background, as an absolute Lc. Unlike the WCAG 2 ratio it tracks how dark themes are seen.
 */
function apca(text, background) {
  const y = (hex) => {
    const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
    const v = 0.2126729 * r ** 2.4 + 0.7151522 * g ** 2.4 + 0.072175 * b ** 2.4;
    return v < 0.022 ? v + (0.022 - v) ** 1.414 : v;
  };
  const yt = y(text);
  const yb = y(background);
  if (Math.abs(yb - yt) < 0.0005) return 0;
  if (yb > yt) {
    const s = (yb ** 0.56 - yt ** 0.57) * 1.14;
    return s < 0.1 ? 0 : (s - 0.027) * 100;
  }
  const s = (yb ** 0.65 - yt ** 0.62) * 1.14;
  return s > -0.1 ? 0 : -(s + 0.027) * 100;
}

const clamp = (v, low, high) => Math.min(high, Math.max(low, v));

// ------------------------------------------------------------------ typefaces

const SANS = 'system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif';
const SERIF = 'Georgia, "Times New Roman", serif';
const MONO = 'ui-monospace, "SF Mono", SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace';
const WEIGHTS = ':wght@400;500;600';

/** name → [Google Fonts family spec or null for system, fallback stack] */
const TEXT_FONTS = {
  System: [null, SANS],
  Inter: ['Inter' + WEIGHTS, SANS],
  Geist: ['Geist' + WEIGHTS, SANS],
  'IBM Plex Sans': ['IBM+Plex+Sans' + WEIGHTS, SANS],
  Manrope: ['Manrope' + WEIGHTS, SANS],
  'DM Sans': ['DM+Sans' + WEIGHTS, SANS],
  Figtree: ['Figtree' + WEIGHTS, SANS],
  'Instrument Sans': ['Instrument+Sans' + WEIGHTS, SANS],
  'Plus Jakarta Sans': ['Plus+Jakarta+Sans' + WEIGHTS, SANS],
  Onest: ['Onest' + WEIGHTS, SANS],
  'Public Sans': ['Public+Sans' + WEIGHTS, SANS],
  'Space Grotesk': ['Space+Grotesk' + WEIGHTS, SANS],
};
const DISPLAY_FONTS = {
  'Same as text': null,
  ...TEXT_FONTS,
  Sora: ['Sora' + WEIGHTS, SANS],
  'Bricolage Grotesque': ['Bricolage+Grotesque' + WEIGHTS, SANS],
  Fraunces: ['Fraunces' + WEIGHTS, SERIF],
  'Source Serif 4': ['Source+Serif+4' + WEIGHTS, SERIF],
  Newsreader: ['Newsreader' + WEIGHTS, SERIF],
  'Instrument Serif': ['Instrument+Serif', SERIF],
  'Tenor Sans': ['Tenor+Sans', SANS],
};
const CODE_FONTS = {
  System: [null, MONO],
  'JetBrains Mono': ['JetBrains+Mono' + WEIGHTS, MONO],
  'IBM Plex Mono': ['IBM+Plex+Mono' + WEIGHTS, MONO],
  'Geist Mono': ['Geist+Mono' + WEIGHTS, MONO],
  'Fira Code': ['Fira+Code' + WEIGHTS, MONO],
  'Source Code Pro': ['Source+Code+Pro' + WEIGHTS, MONO],
};

const loadedFonts = new Set();
/**
 * Loads a Google Fonts family (lab only) and resolves when its glyphs can be measured. A family the
 * lists above do not know is asked for in the UI's weights, then as it comes if that fails.
 */
function loadFont(name, entry) {
  if (!name || name === 'System' || name === 'Same as text') return Promise.resolve();
  const spec = entry ? entry[0] : `${name.trim().replace(/\s+/g, '+')}${WEIGHTS}`;
  if (!spec) return Promise.resolve();
  if (!loadedFonts.has(spec)) {
    loadedFonts.add(spec);
    const plain = spec.split(':')[0];
    document.head.append(h('link', { rel: 'stylesheet', href: `https://fonts.googleapis.com/css2?family=${spec}&display=swap`, dataset: { labFont: name },
      onError: () => { if (plain !== spec) document.head.append(h('link', { rel: 'stylesheet', href: `https://fonts.googleapis.com/css2?family=${plain}&display=swap` })); } }));
  }
  return document.fonts.load(`500 16px "${name}"`).catch(() => {});
}

const stack = (name, entry) => (entry && entry[0] ? `"${name}", ${entry[1]}` : entry ? entry[1] : null);

// ------------------------------------------------------------------ directions

/**
 * Starting points. accent: hue (0–360) and chroma (0–0.25); lift brightens fills (and darkens
 * the light theme's); tint: the neutrals' hue and chroma; depth shifts the dark surfaces.
 */
const DIRECTIONS = [
  { id: 'graphite', name: 'Graphite', note: 'Today: cool grey, electric blue', s: { hue: 255, chroma: 0.17, lift: 0, tintHue: 255, tint: 0.012, depth: 0, font: 'System', display: 'Same as text', mono: 'System', radius: 6, caps: true, tracking: 0.22 } },
  { id: 'ink-lime', name: 'Ink & Lime', note: 'Near-black, acid lime, tight corners', s: { hue: 115, chroma: 0.2, lift: 0.12, tintHue: 115, tint: 0.004, depth: -0.03, font: 'Geist', display: 'Geist', mono: 'Geist Mono', radius: 4, caps: true, tracking: 0.3 } },
  { id: 'terracotta', name: 'Terracotta', note: 'Warm sand, clay accent, serif titles', s: { hue: 48, chroma: 0.13, lift: -0.02, tintHue: 65, tint: 0.014, depth: 0.01, font: 'DM Sans', display: 'Fraunces', mono: 'IBM Plex Mono', radius: 10, caps: false, tracking: 0.02 } },
  { id: 'forest', name: 'Forest', note: 'Green-grey calm, engineering type', s: { hue: 172, chroma: 0.12, lift: 0, tintHue: 172, tint: 0.012, depth: 0, font: 'IBM Plex Sans', display: 'IBM Plex Sans', mono: 'IBM Plex Mono', radius: 6, caps: true, tracking: 0.18 } },
  { id: 'mono', name: 'Monochrome', note: 'No colour: black, white, grey', s: { hue: 0, chroma: 0, lift: 0.22, tintHue: 0, tint: 0, depth: -0.04, font: 'Inter', display: 'Inter', mono: 'JetBrains Mono', radius: 2, caps: true, tracking: 0.35 } },
  { id: 'violet', name: 'Violet', note: 'Indigo-violet, soft round shapes', s: { hue: 290, chroma: 0.17, lift: 0, tintHue: 285, tint: 0.016, depth: 0, font: 'Manrope', display: 'Manrope', mono: 'JetBrains Mono', radius: 12, caps: false, tracking: 0 } },
  { id: 'ocean', name: 'Ocean', note: 'Deep blue-grey, teal accent', s: { hue: 205, chroma: 0.12, lift: 0.02, tintHue: 225, tint: 0.02, depth: 0.02, font: 'Figtree', display: 'Sora', mono: 'Fira Code', radius: 8, caps: true, tracking: 0.2 } },
  { id: 'plum', name: 'Plum', note: 'Warm neutrals, berry accent, editorial', s: { hue: 335, chroma: 0.15, lift: 0, tintHue: 20, tint: 0.01, depth: 0, font: 'Instrument Sans', display: 'Instrument Serif', mono: 'Geist Mono', radius: 8, caps: false, tracking: 0.01 } },
];

// ------------------------------------------------------------------ tokens from settings

function brandRamp(s) {
  const steps = {
    200: [0.89, 0.45], 300: [0.82, 0.65], 400: [0.75 + s.lift, 0.85], 500: [0.68 + s.lift, 1],
    600: [0.59 + s.lift, 1], 700: [Math.min(0.5, 0.5 - s.lift), 0.95], 800: [Math.min(0.43, 0.43 - s.lift), 0.85],
  };
  const ramp = {};
  for (const [step, [L, share]] of Object.entries(steps)) ramp[step] = oklch(clamp(L, 0.18, 0.97), s.chroma * share, s.hue);
  const dark = oklch(0.17, s.chroma * 0.3, s.hue);
  const white = oklch(1, 0, 0);
  ramp.ink = contrast(dark, ramp[500]) >= contrast(white, ramp[500]) ? dark : white; // text on dark-theme fills
  ramp.lightInk = contrast(white, ramp[700]) >= contrast(dark, ramp[700]) ? white : dark; // text on light-theme fills
  return ramp;
}

// Lightness of each neutral role (OKLCH L); surfaces take the tint, ink half of it.
const DARK_SURFACES = { '--term-bg': 0.145, '--bg': 0.155, '--surface': 0.185, '--surface-2': 0.215, '--surface-3': 0.25, '--surface-hover': 0.27, '--canvas': 0.235, '--border': 0.27, '--border-strong': 0.33, '--grid': 0.265, '--axis': 0.36 };
const DARK_INK = { '--border-control': 0.52, '--text': 0.93, '--text-2': 0.81, '--muted': 0.69, '--text-faint': 0.63, '--term-fg': 0.9, '--term-cursor': 0.94 };
const LIGHT_SURFACES = { '--term-bg': 0.99, '--bg': 0.975, '--surface': 1, '--surface-2': 1, '--surface-3': 0.95, '--surface-hover': 0.935, '--canvas': 0.94, '--border': 0.915, '--border-strong': 0.845, '--grid': 0.92, '--axis': 0.845 };
const LIGHT_INK = { '--border-control': 0.64, '--text': 0.21, '--text-2': 0.38, '--muted': 0.49, '--text-faint': 0.53, '--term-fg': 0.25, '--term-cursor': 0.25 };

function neutrals(s) {
  const make = (surfaces, ink, depth) => {
    const out = {};
    for (const [name, L] of Object.entries(surfaces)) out[name] = oklch(clamp(L + depth, 0.05, 1), s.tint, s.tintHue);
    for (const [name, L] of Object.entries(ink)) out[name] = oklch(L, s.tint / 2, s.tintHue);
    return out;
  };
  return { dark: make(DARK_SURFACES, DARK_INK, s.depth), light: make(LIGHT_SURFACES, LIGHT_INK, 0) };
}

function typefaces(s) {
  const text = stack(s.font, TEXT_FONTS[s.font]) || SANS;
  const display = s.display === 'Same as text' ? 'var(--font)' : stack(s.display, DISPLAY_FONTS[s.display]) || 'var(--font)';
  return { '--font': text, '--font-display': display, '--font-mono': stack(s.mono, CODE_FONTS[s.mono]) || MONO };
}

/** Every token the lab sets: shared, dark theme and light theme. */
function tokensOf(s) {
  const ramp = brandRamp(s);
  const theme = neutrals(s);
  const r = Math.max(0, s.radius);
  const shared = {
    '--brand-200': ramp[200], '--brand-300': ramp[300], '--brand-400': ramp[400], '--brand-500': ramp[500],
    '--brand-600': ramp[600], '--brand-700': ramp[700], '--brand-800': ramp[800], '--brand-ink': ramp.ink,
    ...typefaces(s),
    '--radius-sm': `${Math.round(r * 0.67)}px`, '--radius': `${r}px`, '--radius-lg': `${Math.round(r * 1.67)}px`, '--radius-xl': `${Math.round(r * 2.67)}px`,
    '--wordmark-case': s.caps ? 'uppercase' : 'none', '--wordmark-tracking': `${s.tracking}em`,
  };
  return { shared, dark: theme.dark, light: { ...theme.light, '--accent-ink': ramp.lightInk }, ramp };
}

const block = (selector, vars) => `${selector} {\n${Object.entries(vars).map(([k, v]) => `  ${k}: ${v};`).join('\n')}\n}`;

// ------------------------------------------------------------------ checks

const HUES = [[25, 'the error red'], [75, 'the warning amber'], [145, 'the success green'], [255, 'the “your turn” blue']];

/** A direction's accent next to a state colour reads as that state (or the state as brand). */
function hueNotes(s) {
  if (!s || s.chroma < 0.06) return [];
  return HUES.filter(([hue]) => Math.abs(((s.hue - hue + 540) % 360) - 180) < 20)
    .map(([, what]) => `The accent is close to ${what}; states may read as brand (or the reverse).`);
}

// The main pairs of scripts/check_repository.py CONTRAST_PAIRS: [text or edge, background (layers
// over the last, left to right), WCAG 2 minimum (a must), APCA Lc target (perceptual, advisory):
// body text 90, secondary 75, meta 60, faint and control edges 45, links 70, semibold labels 60].
const PAIRS = [
  ['--text', '--surface', 4.5, 90], ['--text', '--bg', 4.5, 90], ['--text-2', '--surface', 4.5, 75],
  ['--muted', '--surface', 4.5, 60], ['--muted', '--bg', 4.5, 60], ['--muted', '--surface-hover', 4.5, 60], ['--muted', '--surface-3', 4.5, 60],
  ['--text-faint', '--surface', 4.5, 45], ['--accent-fg', '--surface', 4.5, 70], ['--accent-fg', '--bg', 4.5, 70],
  ['--accent-fg', '--accent-soft/--surface', 4.5, 60], ['--accent-ink', '--accent', 4.5, 60], ['--accent-ink', '--accent-hover', 4.5, 60],
  ['--ok-fg', '--surface', 4.5, 60], ['--warn-fg', '--surface', 4.5, 60], ['--danger-fg', '--surface', 4.5, 60], ['--info-fg', '--surface', 4.5, 60],
  ['--warn-fg', '--warn-soft/--surface', 4.5, null], ['--danger-fg', '--danger-soft/--surface', 4.5, null], ['--info-fg', '--info-soft/--surface', 4.5, null],
  ['--term-fg', '--term-bg', 4.5, 90], ['--ansi-bright-black', '--term-bg', 4.5, 45],
  ['--border-control', '--surface', 3, 45], ['--accent', '--bg', 3, null],
];

/** A computed colour (rgb(), rgba() or color(srgb …)) as [r, g, b] 0–255 and alpha 0–1. */
function rgba(value) {
  const n = (value.match(/[\d.]+/g) || ['0', '0', '0']).map(Number);
  const scale = value.startsWith('color(') ? 255 : 1;
  return [n[0] * scale, n[1] * scale, n[2] * scale, n.length > 3 ? n[3] : 1];
}
const over = (top, bottom) => [0, 1, 2].map((i) => top[i] * top[3] + bottom[i] * (1 - top[3])).concat(1);
const toHex = (c) => `#${c.slice(0, 3).map((v) => Math.round(clamp(v, 0, 255)).toString(16).padStart(2, '0')).join('')}`;

/** Every pair, measured as the browser resolves the tokens now, in a dark and a light island. */
function measure() {
  const failures = [];
  const perceptual = [];
  for (const theme of ['dark', 'light']) {
    const island = h('div', { 'data-theme': theme, hidden: true });
    document.body.append(island);
    const read = (name) => {
      const probe = h('span');
      probe.style.color = `var(${name})`;
      island.append(probe);
      const colour = rgba(getComputedStyle(probe).color);
      probe.remove();
      return colour;
    };
    for (const [fg, bg, min, target] of PAIRS) {
      const layers = bg.split('/');
      let back = read(layers[layers.length - 1]);
      for (const layer of layers.slice(0, -1).reverse()) back = over(read(layer), back);
      const text = toHex(over(read(fg), back));
      const ratio = contrast(text, toHex(back));
      const where = `${theme === 'dark' ? 'Dark' : 'Light'}: ${fg} on ${bg.replace('/', ' over ')}`;
      if (ratio < min - 0.005) failures.push(`${where} is ${ratio.toFixed(2)}:1 (needs ${min}:1)`);
      const lc = apca(text, toHex(back));
      if (target && lc < target - 0.5) perceptual.push(`${where}: Lc ${lc.toFixed(0)} (target ${target})`);
    }
    island.remove();
  }
  return { failures, perceptual };
}

/** A token's value in a theme, as declared (var() resolved), read from an island. */
function tokenIn(theme, name) {
  const island = h('div', { 'data-theme': theme, hidden: true });
  document.body.append(island);
  const value = getComputedStyle(island).getPropertyValue(name).trim();
  island.remove();
  return value;
}

// ------------------------------------------------------------------ proposals

const PASTED_KEY = 'yulius.lab.proposals'; // proposals pasted in this browser
const TOKEN_NAME = /^--[a-z0-9-]+$/;
const SAFE_VALUE = /^[^{};<>]*$/;

function pastedProposals() {
  const list = readJson(PASTED_KEY, []);
  return Array.isArray(list) ? list.filter((p) => p && typeof p.id === 'string') : [];
}

const allProposals = () => [...PROPOSALS, ...pastedProposals()];

/** A pasted proposal, checked: token names only, no value that could leave its declaration. */
function cleanProposal(raw) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('Paste one JSON object, as “Copy template” gives it.');
  const slug = String(raw.id || raw.name || 'proposal').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'proposal';
  const out = { id: PROPOSALS.some((p) => p.id === slug) ? `pasted-${slug}` : slug, name: String(raw.name || slug), note: String(raw.note || ''), fonts: {}, shared: {}, light: {}, dark: {}, pasted: true };
  for (const key of ['text', 'display', 'mono']) if (raw.fonts && typeof raw.fonts[key] === 'string') out.fonts[key] = raw.fonts[key].slice(0, 60);
  for (const [part, theme] of Object.entries(raw.islands || {})) if (ISLANDS[part] && (theme === 'dark' || theme === 'light')) (out.islands ||= {})[part] = theme; // sidebar, well
  for (const part of ['shared', 'light', 'dark']) {
    for (const [name, value] of Object.entries(raw[part] || {})) {
      if (!TOKEN_NAME.test(name)) throw new Error(`${part}: “${name}” is not a token name (--like-this).`);
      if (typeof value !== 'string' || !SAFE_VALUE.test(value)) throw new Error(`${part}: ${name} needs a CSS value without { } ; < >.`);
      out[part][name] = value;
    }
  }
  if (!Object.keys(out.shared).length && !Object.keys(out.light).length && !Object.keys(out.dark).length) throw new Error('No tokens: put them under shared, light and dark.');
  return out;
}

// Parts of the app a proposal may turn into a theme island: an ink sidebar on a light page, or a
// dark work area (terminals and the editor) — they read their colours where they sit.
const ISLANDS = { sidebar: '.sidebar', well: '.term, .ed-main' };

const proposalCss = (p) => [block(':root, [data-theme]', p.shared || {}), block(':root, [data-theme="dark"]', p.dark || {}), block('[data-theme="light"]', p.light || {})].join('\n');

// ------------------------------------------------------------------ state in the URL

// Settings are plain ASCII (numbers, font names), so base64url of the JSON is enough.
const encode = (s) => btoa(JSON.stringify(s)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
function decode(value) {
  try {
    const parsed = JSON.parse(atob(value.replace(/-/g, '+').replace(/_/g, '/')));
    return parsed && typeof parsed === 'object' ? { ...DIRECTIONS[0].s, ...parsed } : null;
  } catch {
    return null;
  }
}

function writeUrl(value) {
  const url = new URL(window.location.href);
  url.searchParams.set('lab', value);
  history.replaceState(history.state, '', url);
}

// ------------------------------------------------------------------ panel

/** Opens the lab: `value` is the ?lab parameter. */
export function startLab(value) {
  const style = h('style', { id: 'brand-lab' });
  document.head.append(style);
  // What is shown: tokens.css as it is, a proposal, or a direction from the sliders.
  let mode = { kind: 'current' };
  if (allProposals().some((p) => p.id === value)) mode = { kind: 'proposal', id: value };
  else if (DIRECTIONS.some((d) => d.id === value)) mode = { kind: 'direction', id: value, settings: { ...DIRECTIONS.find((d) => d.id === value).s } };
  else if (value && value !== '1' && decode(value)) mode = { kind: 'direction', id: null, settings: decode(value) };
  let open = true;
  let side = 'right';
  let slidersOpen = mode.kind === 'direction';
  let writeTimer = null;
  let checkFrame = 0;

  const panel = h('aside', { class: 'lab', 'aria-label': 'Brand lab' });
  document.body.append(panel);
  const checksHost = h('div', { class: 'lab-checks', 'aria-live': 'polite' });
  const choiceButtons = new Map(); // 'current' | proposal id | 'direction:<id>' → button

  const proposal = () => (mode.kind === 'proposal' ? allProposals().find((p) => p.id === mode.id) || null : null);
  const sameAs = (id) => mode.kind === 'direction' && JSON.stringify(DIRECTIONS.find((d) => d.id === id).s) === JSON.stringify(mode.settings);
  const urlValue = () => (mode.kind === 'proposal' ? mode.id : mode.kind === 'direction' ? (mode.id && sameAs(mode.id) ? mode.id : encode(mode.settings)) : '1');

  /** Marks the parts the proposal turns into islands (for the theme in view); true if any was new. */
  function ensureIslands() {
    const p = proposal();
    if (!p || !p.islands) return false;
    const theme = activeTheme();
    let added = false;
    for (const [part, islandTheme] of Object.entries(p.islands)) {
      if (!ISLANDS[part] || islandTheme === theme) continue;
      for (const el of document.querySelectorAll(ISLANDS[part])) {
        if (el.hasAttribute('data-lab-island')) continue;
        el.setAttribute('data-theme', islandTheme);
        el.setAttribute('data-lab-island', '');
        added = true;
      }
    }
    return added;
  }
  /** Clears every island and sets the proposal's again (after a change of proposal or theme). */
  function applyIslands() {
    for (const el of document.querySelectorAll('[data-lab-island]')) { el.removeAttribute('data-theme'); el.removeAttribute('data-lab-island'); }
    ensureIslands();
  }
  onThemeChange(() => applyIslands());
  // Views come and go (a task's terminal, the editor, the shell after sign-in): new parts become
  // islands within a frame, and terminals and the editor then read their colours again.
  let islandFrame = 0;
  const appRoot = document.getElementById('app');
  if (appRoot) {
    new MutationObserver(() => {
      if (!proposal()?.islands || islandFrame) return;
      islandFrame = requestAnimationFrame(() => { islandFrame = 0; if (ensureIslands()) applyTheme(); });
    }).observe(appRoot, { childList: true, subtree: true });
  }

  async function apply() {
    const p = proposal();
    applyIslands();
    if (p) style.textContent = proposalCss(p);
    else if (mode.kind === 'direction') {
      const t = tokensOf(mode.settings);
      style.textContent = [block(':root, [data-theme]', t.shared), block(':root, [data-theme="dark"]', t.dark), block('[data-theme="light"]', t.light)].join('\n');
    } else style.textContent = '';
    applyTheme(); // terminals, the editor, the favicon and theme-color follow
    scheduleChecks();
    clearTimeout(writeTimer);
    writeTimer = setTimeout(() => writeUrl(urlValue()), 300);
    const fonts = p ? [[p.fonts.text, TEXT_FONTS[p.fonts.text]], [p.fonts.display, DISPLAY_FONTS[p.fonts.display]], [p.fonts.mono, CODE_FONTS[p.fonts.mono]]]
      : mode.kind === 'direction' ? [[mode.settings.font, TEXT_FONTS[mode.settings.font]], [mode.settings.display, DISPLAY_FONTS[mode.settings.display]], [mode.settings.mono, CODE_FONTS[mode.settings.mono]]] : [];
    await Promise.all(fonts.map(([name, entry]) => loadFont(name, entry)));
    if (fonts.length) { applyTheme(); scheduleChecks(); } // measured again with the real glyphs
  }

  function scheduleChecks() {
    cancelAnimationFrame(checkFrame);
    checkFrame = requestAnimationFrame(() => {
      const { failures, perceptual } = measure();
      const notes = mode.kind === 'direction' ? hueNotes(mode.settings) : [];
      setChildren(checksHost,
        failures.length
          ? h('ul', { class: 'lab-fails' }, failures.map((f) => h('li', null, icon('warn', { size: 12 }), f)))
          : h('p', { class: 'lab-ok' }, icon('check', { size: 12 }), 'All text and control edges meet WCAG AA in both themes.'),
        perceptual.length
          ? h('details', { class: 'lab-perceptual' }, h('summary', null, `Perceptual contrast (APCA): ${perceptual.length} below target`), h('ul', { class: 'lab-misses' }, perceptual.map((f) => h('li', null, f))))
          : h('p', { class: 'lab-ok' }, icon('check', { size: 12 }), 'Perceptual contrast (APCA) meets every target.'),
        notes.map((note) => h('p', { class: 'lab-note' }, note)));
    });
  }

  function markChoices() {
    for (const [key, button] of choiceButtons) {
      const on = key === 'current' ? mode.kind === 'current'
        : key.startsWith('direction:') ? sameAs(key.slice('direction:'.length))
          : mode.kind === 'proposal' && mode.id === key;
      button.classList.toggle('on', on);
      button.setAttribute('aria-checked', String(on));
    }
  }

  function choose(next) {
    mode = next;
    if (mode.kind === 'direction') slidersOpen = true;
    apply();
    keepFocus(panel, render);
  }

  // A slider or select changes one setting of the direction in view and refreshes only the checks
  // and the marks, so a slider keeps its drag and a select its focus.
  function set(key, value) {
    mode = { ...mode, settings: { ...mode.settings, [key]: value } };
    apply();
    markChoices();
  }

  function slider(label, key, min, max, step, show) {
    const value = mode.settings[key];
    const output = h('output', { class: 'lab-value' }, show(value));
    const input = h('input', { type: 'range', min, max, step, value, 'aria-label': label, onInput: (event) => { output.textContent = show(Number(event.target.value)); set(key, Number(event.target.value)); } });
    return h('label', { class: 'lab-field' }, h('span', { class: 'lab-label' }, label, output), input);
  }

  function select(label, key, options) {
    const input = h('select', { class: 'select', 'aria-label': label, onChange: (event) => set(key, event.target.value) }, Object.keys(options).map((name) => h('option', { value: name, selected: mode.settings[key] === name }, name)));
    return h('label', { class: 'lab-field' }, h('span', { class: 'lab-label' }, label), input);
  }

  function dots(colours) {
    return h('span', { class: 'lab-swatches', 'aria-hidden': 'true' }, colours.filter(Boolean).map((colour) => {
      const dot = h('span', { class: 'lab-swatch' });
      dot.style.setProperty('--lab-swatch', colour);
      return dot;
    }));
  }

  const proposalDots = (p) => dots([p.dark['--bg'], p.dark['--accent-fg'], p.light['--bg'], p.light['--accent'], p.light['--accent-fg']]);
  const directionDots = (s) => { const t = tokensOf(s); return dots([t.dark['--bg'], t.dark['--surface-3'], t.ramp[500], t.light['--bg'], t.ramp[700]]); };

  function choiceButton(key, name, note, extra, run, remove) {
    const button = h('button', { type: 'button', role: 'radio', class: 'lab-direction', onClick: run }, h('span', { class: 'lab-direction-name' }, name, extra), note ? h('span', { class: 'lab-direction-note' }, note) : null);
    choiceButtons.set(key, button);
    return remove ? h('div', { class: 'lab-choice' }, button, h('button', { type: 'button', class: 'btn-icon lab-remove', 'aria-label': `Remove ${name}`, title: `Remove ${name}`, onClick: remove }, icon('x', { size: 12 }))) : button;
  }

  async function copy(text, what) {
    try {
      await navigator.clipboard.writeText(text);
      toast(`${what} copied.`, { kind: 'ok' });
    } catch {
      window.prompt(`Copy the ${what.toLowerCase()}:`, text);
    }
  }

  /** The proposal in view, or a skeleton of today's values in the proposal format. */
  function template() {
    const p = proposal();
    if (p) {
      const { pasted, ...rest } = p;
      return { ...rest, id: `${p.id}-2`, name: `${p.name} (copy)` };
    }
    if (mode.kind === 'direction') {
      const t = tokensOf(mode.settings);
      return { id: 'my-proposal', name: 'My proposal', note: '', fonts: { text: mode.settings.font, display: mode.settings.display, mono: mode.settings.mono }, shared: t.shared, light: t.light, dark: t.dark };
    }
    const keys = PROPOSALS[0];
    const out = { id: 'my-proposal', name: 'My proposal', note: '', fonts: { text: 'System', display: 'System', mono: 'System' }, shared: {}, light: {}, dark: {} };
    for (const part of ['shared', 'light', 'dark']) for (const name of Object.keys(keys[part])) out[part][name] = tokenIn(part === 'shared' ? activeTheme() : part, name);
    return out;
  }

  function exported() {
    const p = proposal();
    if (p) { const { pasted, ...rest } = p; return JSON.stringify(rest, null, 2); }
    if (mode.kind === 'direction') { const t = tokensOf(mode.settings); return JSON.stringify({ direction: mode.id, settings: mode.settings, tokens: { shared: t.shared, dark: t.dark, light: t.light } }, null, 2); }
    return JSON.stringify(template(), null, 2);
  }

  function pasteDialog() {
    const area = h('textarea', { class: 'textarea mono', rows: 14, spellcheck: 'false', 'aria-label': 'Proposal JSON', placeholder: '{ "name": "…", "fonts": { … }, "shared": { … }, "light": { … }, "dark": { … } }' });
    openModal({
      title: 'Paste a brand proposal',
      wide: true,
      body: h('div', { class: 'stack-sm' },
        h('p', { class: 'text-2' }, 'JSON in the shape “Copy template” gives: any tokens of tokens.css under shared (both themes), light and dark, and Google Fonts names under fonts. It is kept in this browser.'),
        h('label', { class: 'field' }, area)),
      actions: [
        { label: 'Cancel', value: false },
        { label: 'Preview', kind: 'primary', run: () => {
          let parsed;
          try { parsed = JSON.parse(area.value); } catch (error) { return invalid(area, `Not valid JSON: ${error.message}`); }
          let clean;
          try { clean = cleanProposal(parsed); } catch (error) { return invalid(area, error.message); }
          writeJson(PASTED_KEY, [...pastedProposals().filter((p) => p.id !== clean.id), clean]);
          choose({ kind: 'proposal', id: clean.id });
          return true;
        } },
      ],
    });
  }

  function removePasted(id) {
    writeJson(PASTED_KEY, pastedProposals().filter((p) => p.id !== id));
    if (mode.kind === 'proposal' && mode.id === id) { choose({ kind: 'current' }); return; }
    keepFocus(panel, render);
  }

  function render() {
    panel.classList.toggle('lab-left', side === 'left');
    choiceButtons.clear();
    const head = h('div', { class: 'lab-head' },
      h('button', { type: 'button', class: 'lab-title', 'aria-expanded': String(open), onClick: () => { open = !open; render(); } }, icon(open ? 'down' : 'right', { size: 14 }), 'Brand lab'),
      h('span', { class: 'spacer' }),
      h('button', { type: 'button', class: 'btn-icon', 'aria-label': side === 'right' ? 'Move to the left' : 'Move to the right', title: side === 'right' ? 'Move to the left' : 'Move to the right', onClick: () => { side = side === 'right' ? 'left' : 'right'; render(); } }, icon(side === 'right' ? 'left' : 'right', { size: 14 })));
    if (!open) { setChildren(panel, head); return; }

    const theme = activeTheme();
    const themeSeg = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Theme' }, ['dark', 'light'].map((name) => h('label', null,
      h('input', { type: 'radio', name: 'lab-theme', checked: theme === name, onChange: () => { setThemeChoice(name); render(); } }), name === 'dark' ? 'Dark' : 'Light')));

    const proposals = h('div', { class: 'lab-list', role: 'radiogroup', 'aria-label': 'Proposal' },
      choiceButton('current', 'Current', 'tokens.css as it is', null, () => choose({ kind: 'current' })),
      allProposals().map((p) => choiceButton(p.id, p.name, p.note, proposalDots(p), () => choose({ kind: 'proposal', id: p.id }), p.pasted ? () => removePasted(p.id) : null)));

    const sliders = h('details', { class: 'lab-sliders', open: slidersOpen, onToggle: (event) => { slidersOpen = event.target.open; } },
      h('summary', { class: 'lab-section' }, 'Directions and sliders'),
      h('div', { class: 'lab-directions', role: 'radiogroup', 'aria-label': 'Direction' },
        DIRECTIONS.map((d) => choiceButton(`direction:${d.id}`, d.name, d.note, directionDots(d.s), () => choose({ kind: 'direction', id: d.id, settings: { ...d.s } })))),
      mode.kind === 'direction' ? [
        h('h3', { class: 'lab-section' }, 'Colour'),
        slider('Accent hue', 'hue', 0, 359, 1, (v) => `${v}°`),
        slider('Accent strength', 'chroma', 0, 0.25, 0.005, (v) => v.toFixed(3)),
        slider('Accent brightness', 'lift', -0.08, 0.24, 0.01, (v) => (v > 0 ? `+${v.toFixed(2)}` : v.toFixed(2))),
        slider('Grey tint hue', 'tintHue', 0, 359, 1, (v) => `${v}°`),
        slider('Grey tint amount', 'tint', 0, 0.035, 0.001, (v) => v.toFixed(3)),
        slider('Dark background depth', 'depth', -0.06, 0.08, 0.005, (v) => (v > 0 ? `+${v.toFixed(3)}` : v.toFixed(3))),
        h('h3', { class: 'lab-section' }, 'Type'),
        select('Text', 'font', TEXT_FONTS),
        select('Titles and wordmark', 'display', DISPLAY_FONTS),
        select('Code and terminal', 'mono', CODE_FONTS),
        h('h3', { class: 'lab-section' }, 'Shape'),
        slider('Corner radius', 'radius', 0, 14, 1, (v) => `${v} px`),
        h('label', { class: 'lab-row' }, h('span', { class: 'lab-label' }, 'Wordmark in capitals'), h('input', { type: 'checkbox', checked: mode.settings.caps, onChange: (event) => set('caps', event.target.checked) })),
        slider('Wordmark tracking', 'tracking', 0, 0.4, 0.01, (v) => `${v.toFixed(2)} em`),
      ] : h('p', { class: 'lab-note' }, 'Pick a direction to tune it with sliders.'));

    const body = [
      h('div', { class: 'lab-row' }, h('span', { class: 'lab-label' }, 'Theme'), themeSeg),
      h('h3', { class: 'lab-section' }, 'Proposals'),
      proposals,
      h('div', { class: 'lab-actions' },
        h('button', { type: 'button', class: 'btn btn-sm', onClick: pasteDialog }, 'Paste proposal…'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => copy(JSON.stringify(template(), null, 2), 'Template') }, 'Copy template')),
      h('h3', { class: 'lab-section' }, 'Checks'),
      checksHost,
      mode.kind === 'current' ? null : h('div', { class: 'lab-actions' },
        h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: () => copy(exported(), 'Settings') }, 'Copy settings'),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => copy(window.location.href, 'Link') }, 'Copy link')),
      sliders,
      h('p', { class: 'lab-note' }, 'Fonts load from Google Fonts in the lab only; the chosen ones get self-hosted. The editor takes a new code font when it opens next.'),
    ];
    const scroll = panel.querySelector('.lab-body')?.scrollTop || 0;
    setChildren(panel, head, h('div', { class: 'lab-body' }, body));
    panel.querySelector('.lab-body').scrollTop = scroll;
    markChoices();
    scheduleChecks();
  }

  apply();
  render();
}
