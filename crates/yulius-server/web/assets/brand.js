// Branding (web/README.md "Theming and branding"): the product's name, tagline and mark, in one
// place. The sidebar wordmark, the sign-in and setup screens, the browser tab title and the
// favicon all read from here; brand colours are the --brand-* tokens in tokens.css. No imports,
// so any module can use it.

export const BRAND = {
  name: 'Yulius',
  tagline: 'by Quartio team',
  // The mark: SVG path data on a 24-unit grid, drawn as a 1.8-unit stroke in the accent colour.
  mark: 'M6.75 6L12 12.75 17.25 6M12 12.75v6',
};

/** The favicon as an SVG data URL: the mark in `accent` on a rounded `background` tile. */
export function faviconUrl(background, accent) {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" rx="5.5" fill="${background}"/>`
    + `<path d="${BRAND.mark}" stroke="${accent}" stroke-width="2.4" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}
