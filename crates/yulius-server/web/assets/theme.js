// Themes (web/README.md "Theming and branding"): dark (the default), light, or system, kept per
// browser. The choice sets `data-theme` on <html>, which switches the role tokens in tokens.css;
// terminals and editors cannot read CSS, so they take their colours from the same tokens here and
// re-colour on the `yulius:theme` event.

import { readJson, writeJson } from './store.js';
import { faviconUrl } from './brand.js';

const KEY = 'yulius.theme';
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
  if (favicon) favicon.href = faviconUrl(token('--surface'), token('--accent'));
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
