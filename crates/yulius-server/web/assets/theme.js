// Themes (web/README.md "Theming and branding"): dark (the default), light, or system, kept per
// browser. The choice sets `data-theme` on <html>, which switches the role tokens in tokens.css;
// terminals and editors cannot read CSS, so they take their colours from the same tokens here and
// re-colour on the `yulius:theme` event.

import { readJson, writeJson } from './store.js';
import { faviconUrl } from './brand.js';

const KEY = 'yulius.theme';

/** The phone layout's media query: the one breakpoint scripts share with styles.css (767 px). */
export const PHONE = '(max-width: 767px)';
/** True while the phone layout applies. */
export const isPhone = () => window.matchMedia(PHONE).matches;
/** A size token in pixels, e.g. px('--fs') → 13. */
export function px(name) {
  return parseFloat(token(name)) || 0;
}
export const THEME_CHOICES = [
  { id: 'dark', name: 'Dark' },
  { id: 'light', name: 'Light' },
  { id: 'system', name: 'System' },
];
const prefersLight = window.matchMedia('(prefers-color-scheme: light)');

/** The stored choice: 'dark' | 'light' | 'system'. */
export function themeChoice() {
  const value = readJson(KEY, 'dark');
  return THEME_CHOICES.some((t) => t.id === value) ? value : 'dark';
}

/** The theme in effect: 'dark' | 'light'. */
export function activeTheme() {
  const choice = themeChoice();
  if (choice === 'system') return prefersLight.matches ? 'light' : 'dark';
  return choice;
}

/** A token's current value, e.g. token('--surface') → '#11141a'. */
export function token(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** Applies the active theme to the page and tells terminals and editors. */
export function applyTheme() {
  const theme = activeTheme();
  const root = document.documentElement;
  if (root.dataset.theme !== theme) root.dataset.theme = theme;
  const scheme = document.querySelector('meta[name="color-scheme"]');
  if (scheme) scheme.content = theme;
  const color = document.querySelector('meta[name="theme-color"]');
  if (color) color.content = token('--bg');
  const favicon = document.querySelector('link[rel="icon"]');
  if (favicon) favicon.href = faviconUrl(token('--surface'), token('--brand-mark'));
  window.dispatchEvent(new CustomEvent('yulius:theme', { detail: { theme } }));
}

/** Stores a choice and applies it. */
export function setThemeChoice(choice) {
  writeJson(KEY, choice);
  applyTheme();
}

/** Subscribes to theme changes; returns an unsubscribe function. */
export function onThemeChange(fn) {
  const listener = (event) => fn(event.detail.theme);
  window.addEventListener('yulius:theme', listener);
  return () => window.removeEventListener('yulius:theme', listener);
}

/**
 * A colour token as #rrggbb or #rrggbbaa (editors want hex; tokens may be var() or color-mix()):
 * the browser resolves it on a probe element.
 */
export function hexToken(name) {
  const probe = document.createElement('span');
  probe.style.color = `var(${name})`;
  probe.hidden = true;
  document.body.append(probe);
  const rgb = getComputedStyle(probe).color;
  probe.remove();
  // rgb()/rgba() give 0–255 channels; color(srgb …) (from color-mix) gives 0–1.
  const parts = (rgb.match(/[\d.]+/g) || ['0', '0', '0']).map(Number);
  const scale = rgb.startsWith('color(') ? 255 : 1;
  const hex = (n) => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, '0');
  const alpha = parts.length > 3 ? hex(parts[3] * 255) : 'ff';
  return `#${hex(parts[0] * scale)}${hex(parts[1] * scale)}${hex(parts[2] * scale)}${alpha === 'ff' ? '' : alpha}`;
}

/** xterm.js colours from the tokens. */
export function terminalColors() {
  return {
    background: token('--term-bg'),
    foreground: token('--term-fg'),
    cursor: token('--term-cursor'),
    cursorAccent: token('--term-bg'),
    selectionBackground: token('--term-selection'),
    black: token('--ansi-black'), red: token('--ansi-red'), green: token('--ansi-green'), yellow: token('--ansi-yellow'),
    blue: token('--ansi-blue'), magenta: token('--ansi-magenta'), cyan: token('--ansi-cyan'), white: token('--ansi-white'),
    brightBlack: token('--ansi-bright-black'), brightRed: token('--ansi-bright-red'), brightGreen: token('--ansi-bright-green'),
    brightYellow: token('--ansi-bright-yellow'), brightBlue: token('--ansi-bright-blue'), brightMagenta: token('--ansi-bright-magenta'),
    brightCyan: token('--ansi-bright-cyan'), brightWhite: token('--ansi-bright-white'),
  };
}

// Follow the operating system while the choice is "system".
prefersLight.addEventListener('change', () => { if (themeChoice() === 'system') applyTheme(); });
