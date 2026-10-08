// Brand lab: a development aid for choosing the brand on the real screens. It loads only with
// ?lab in the URL (?lab=1, ?lab=<direction> or a copied ?lab=<settings> link) and overrides the
// brand tokens of tokens.css live: the accent ramp, the neutrals of both themes, the typefaces,
// the radii and the wordmark. It checks text contrast in both themes and copies the result, which
// is then written into tokens.css and brand.js for good. Fonts come from Google Fonts here only;
// shipped fonts are self-hosted (web/README.md, "Brand typeface").

import { h, icon, setChildren, toast, keepFocus } from './dom.js';
import { applyTheme, activeTheme, setThemeChoice, token } from './theme.js';

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
/** Loads a Google Fonts family (lab only) and resolves when its glyphs can be measured. */
function loadFont(name, entry) {
  if (!entry || !entry[0]) return Promise.resolve();
  if (!loadedFonts.has(entry[0])) {
    loadedFonts.add(entry[0]);
    document.head.append(h('link', { rel: 'stylesheet', href: `https://fonts.googleapis.com/css2?family=${entry[0]}&display=swap`, dataset: { labFont: name } }));
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

function checks(s) {
  const t = tokensOf(s);
  const out = [];
  const pair = (theme, label, fg, bg, min) => { const ratio = contrast(fg, bg); if (ratio < min) out.push(`${theme}: ${label} ${ratio.toFixed(2)}:1 (needs ${min}:1)`); };
  const d = t.dark;
  const l = t.light;
  for (const [name, label] of [['--text', 'text'], ['--text-2', 'secondary text'], ['--muted', 'muted text'], ['--text-faint', 'faint text']]) {
    pair('Dark', label, d[name], d['--surface'], 4.5);
    pair('Light', label, l[name], l['--bg'], 4.5);
  }
  pair('Dark', 'links and accent text', t.ramp[300], d['--surface'], 4.5);
  pair('Dark', 'text on accent buttons', t.ramp.ink, t.ramp[500], 4.5);
  pair('Dark', 'accent against the page (focus, buttons)', t.ramp[500], d['--surface'], 3);
  pair('Dark', 'input edges', d['--border-control'], d['--surface'], 3);
  pair('Light', 'links and accent text', t.ramp[700], l['--bg'], 4.5);
  pair('Light', 'text on accent buttons', l['--accent-ink'], t.ramp[700], 4.5);
  pair('Light', 'accent against the page (focus, buttons)', t.ramp[700], l['--bg'], 3);
  pair('Light', 'input edges', l['--border-control'], l['--bg'], 3);
  // State colours stay fixed; the new surfaces must still carry them.
  for (const [role, dark, light] of [['success', '--green-300', '--green-700'], ['warning', '--amber-300', '--amber-700'], ['error', '--red-300', '--red-700'], ['your turn', '--info-300', '--info-700']]) {
    const dk = token(dark);
    const lt = token(light);
    if (/^#[0-9a-f]{6}$/i.test(dk)) pair('Dark', `${role} text`, dk, d['--surface'], 4.5);
    if (/^#[0-9a-f]{6}$/i.test(lt)) pair('Light', `${role} text`, lt, l['--bg'], 4.5);
  }
  const notes = [];
  if (s.chroma >= 0.06) {
    for (const [hue, what] of HUES) {
      const distance = Math.abs(((s.hue - hue + 540) % 360) - 180);
      if (distance < 20) notes.push(`The accent is close to ${what}; states may read as brand (or the reverse).`);
    }
  }
  return { failures: out, notes };
}

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
  const preset = DIRECTIONS.find((d) => d.id === value);
  let settings = preset ? { ...preset.s } : (value && value !== '1' ? decode(value) : null);
  let direction = preset ? preset.id : null;
  let open = true;
  let side = 'right';
  let writeTimer = null;

  const panel = h('aside', { class: 'lab', 'aria-label': 'Brand lab' });
  document.body.append(panel);

  async function apply() {
    if (!settings) {
      style.textContent = '';
    } else {
      const t = tokensOf(settings);
      style.textContent = [block(':root, [data-theme]', t.shared), block(':root, [data-theme="dark"]', t.dark), block('[data-theme="light"]', t.light)].join('\n');
      await Promise.all([[settings.font, TEXT_FONTS[settings.font]], [settings.display, DISPLAY_FONTS[settings.display]], [settings.mono, CODE_FONTS[settings.mono]]].map(([name, entry]) => loadFont(name, entry)));
    }
    applyTheme(); // terminals, the editor, the favicon and theme-color follow
    clearTimeout(writeTimer);
    writeTimer = setTimeout(() => writeUrl(direction && settings && sameAs(direction) ? direction : settings ? encode(settings) : '1'), 300);
  }

  const sameAs = (id) => JSON.stringify(DIRECTIONS.find((d) => d.id === id).s) === JSON.stringify(settings);

  // A control changes one setting and refreshes only the checks and the direction marks, so a
  // slider keeps its drag and a select its focus.
  function set(key, value) {
    settings = { ...settings, [key]: value };
    apply();
    refresh();
  }

  function slider(label, key, min, max, step, show) {
    const value = settings[key];
    const output = h('output', { class: 'lab-value' }, show(value));
    const input = h('input', { type: 'range', min, max, step, value, 'aria-label': label, onInput: (event) => { output.textContent = show(Number(event.target.value)); set(key, Number(event.target.value)); } });
    return h('label', { class: 'lab-field' }, h('span', { class: 'lab-label' }, label, output), input);
  }

  const checksHost = h('div', { class: 'lab-checks', 'aria-live': 'polite' });
  const directionButtons = new Map(); // direction id (or 'current') → button

  function refresh() {
    for (const [id, button] of directionButtons) {
      const on = id === 'current' ? !settings : Boolean(settings) && direction === id && sameAs(id);
      button.classList.toggle('on', on);
      button.setAttribute('aria-checked', String(on));
    }
    if (!settings) { setChildren(checksHost); return; }
    const result = checks(settings);
    setChildren(checksHost,
      result.failures.length
        ? h('ul', { class: 'lab-fails' }, result.failures.map((f) => h('li', null, icon('warn', { size: 12 }), f)))
        : h('p', { class: 'lab-ok' }, icon('check', { size: 12 }), 'All text meets WCAG AA in both themes.'),
      result.notes.map((note) => h('p', { class: 'lab-note' }, note)));
  }

  function choose(id) {
    const d = DIRECTIONS.find((x) => x.id === id);
    settings = d ? { ...d.s } : null;
    direction = d ? d.id : null;
    apply();
    keepFocus(panel, render);
  }

  function select(label, key, options) {
    const input = h('select', { class: 'select', 'aria-label': label, onChange: (event) => set(key, event.target.value) }, Object.keys(options).map((name) => h('option', { value: name, selected: settings[key] === name }, name)));
    return h('label', { class: 'lab-field' }, h('span', { class: 'lab-label' }, label), input);
  }

  function swatches(s) {
    const t = tokensOf(s);
    return h('span', { class: 'lab-swatches', 'aria-hidden': 'true' }, [t.dark['--bg'], t.dark['--surface-3'], t.ramp[500], t.light['--bg'], t.ramp[700]].map((colour) => {
      const dot = h('span', { class: 'lab-swatch' });
      dot.style.setProperty('--lab-swatch', colour);
      return dot;
    }));
  }

  async function copy(text, what) {
    try {
      await navigator.clipboard.writeText(text);
      toast(`${what} copied.`, { kind: 'ok' });
    } catch {
      window.prompt(`Copy the ${what.toLowerCase()}:`, text);
    }
  }

  function exported() {
    const t = tokensOf(settings);
    return JSON.stringify({ direction, settings, tokens: { shared: t.shared, dark: t.dark, light: t.light } }, null, 2);
  }

  function render() {
    panel.classList.toggle('lab-left', side === 'left');
    const head = h('div', { class: 'lab-head' },
      h('button', { type: 'button', class: 'lab-title', 'aria-expanded': String(open), onClick: () => { open = !open; render(); } }, icon(open ? 'down' : 'right', { size: 14 }), 'Brand lab'),
      h('span', { class: 'spacer' }),
      h('button', { type: 'button', class: 'btn-icon', 'aria-label': side === 'right' ? 'Move to the left' : 'Move to the right', title: side === 'right' ? 'Move to the left' : 'Move to the right', onClick: () => { side = side === 'right' ? 'left' : 'right'; render(); } }, icon(side === 'right' ? 'left' : 'right', { size: 14 })));
    if (!open) { setChildren(panel, head); return; }

    directionButtons.clear();
    const directionButton = (id, name, note, extra) => {
      const button = h('button', { type: 'button', role: 'radio', class: 'lab-direction', onClick: () => choose(id) }, h('span', { class: 'lab-direction-name' }, name, extra), h('span', { class: 'lab-direction-note' }, note));
      directionButtons.set(id, button);
      return button;
    };
    const directions = h('div', { class: 'lab-directions', role: 'radiogroup', 'aria-label': 'Direction' },
      directionButton('current', 'Current', 'tokens.css as it is', null),
      DIRECTIONS.map((d) => directionButton(d.id, d.name, d.note, swatches(d.s))));

    const theme = activeTheme();
    const themeSeg = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Theme' }, ['dark', 'light'].map((name) => h('label', null,
      h('input', { type: 'radio', name: 'lab-theme', checked: theme === name, onChange: () => { setThemeChoice(name); render(); } }), name === 'dark' ? 'Dark' : 'Light')));

    const body = [h('div', { class: 'lab-row' }, h('span', { class: 'lab-label' }, 'Theme'), themeSeg), h('h3', { class: 'lab-section' }, 'Direction'), directions];
    if (settings) {
      body.push(
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
        h('label', { class: 'lab-row' }, h('span', { class: 'lab-label' }, 'Wordmark in capitals'), h('input', { type: 'checkbox', checked: settings.caps, onChange: (event) => set('caps', event.target.checked) })),
        slider('Wordmark tracking', 'tracking', 0, 0.4, 0.01, (v) => `${v.toFixed(2)} em`),
        h('h3', { class: 'lab-section' }, 'Checks'),
        checksHost,
        h('div', { class: 'lab-actions' },
          h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: () => copy(exported(), 'Settings') }, 'Copy settings'),
          h('button', { type: 'button', class: 'btn btn-sm', onClick: () => copy(window.location.href, 'Link') }, 'Copy link')),
        h('p', { class: 'lab-note' }, 'Fonts load from Google Fonts in the lab only; the chosen ones get self-hosted. The editor takes a new code font when it opens next.'));
    }
    const scroll = panel.querySelector('.lab-body')?.scrollTop || 0;
    setChildren(panel, head, h('div', { class: 'lab-body' }, body));
    panel.querySelector('.lab-body').scrollTop = scroll;
    refresh();
  }

  apply();
  render();
}
