// Branding (web/README.md "Theming and branding"): the product's name, tagline and mark, in one
// place. The sidebar wordmark, the sign-in and setup screens, the browser tab title, the favicon
// and every sentence that names the product read from here; brand colours are the --brand-*
// tokens and the mark's colour is --brand-mark in tokens.css. No imports, so any module can use it.

export const BRAND = {
  name: 'Yulius',
  tagline: 'by Quartio team',
  // The mark (from assets/brand/logo-lockup.svg): SVG path data in its own viewBox. markStyle is
  // 'fill' for a solid shape, or 'stroke' for a line drawing (drawn with a 2.4-unit stroke).
  mark: 'M 28,96 C 63,60 123,70 166,125 C 189,153 207,192 220,226 C 241,193 248,145 269,102 C 285,68 314,42 359,25 C 331,66 319,116 299,162 C 282,203 252,241 222,269 C 229,324 215,399 195,448 L 137,460 C 164,402 176,337 176,280 C 169,214 129,150 81,122 C 63,111 45,104 28,96 Z',
  markViewBox: '-41.5 7.5 470 470',
  markStyle: 'fill',
};

/** The mark as SVG markup in `color`, scaled into a box of `size` at (x, y). */
export function markSvg(color, { x = 0, y = 0, size = 24 } = {}) {
  const paint = BRAND.markStyle === 'fill'
    ? `fill="${color}"`
    : `stroke="${color}" stroke-width="2.4" fill="none" stroke-linecap="round" stroke-linejoin="round"`;
  return `<svg x="${x}" y="${y}" width="${size}" height="${size}" viewBox="${BRAND.markViewBox || '0 0 24 24'}"><path d="${BRAND.mark}" ${paint}/></svg>`;
}

/** The favicon as an SVG data URL: the mark in `color` on a rounded `background` tile. */
export function faviconUrl(background, color) {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" rx="5.5" fill="${background}"/>${markSvg(color, { x: 3, y: 3, size: 18 })}</svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}
