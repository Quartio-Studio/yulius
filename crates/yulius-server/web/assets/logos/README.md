# Logos of known services

Logos shown next to well-known MCP servers on the MCP page (`pages/mcp.js`). They identify the
service only; the marks belong to their owners. Served same-origin like every other asset (no
external images, see the web README).

| File | Source | Change |
| --- | --- | --- |
| `quartio.svg` | `https://quartio.ai/assets/quartio-symbol.svg` (Quartio's own site icon) | `fill="currentColor"` set to `#e8ecf2` so it shows as an `<img>`; marked `onDark` in `pages/mcp.js`, so it always sits on a dark tile (`--logo-tile-dark`), also in the light theme |
| `apify.svg` | `https://apify.com/icon0.svg` (Apify's site icon; brand guidelines: https://apify.com/resources/brand) | none |

Add a logo only for a service the team uses, from the service's own site or brand page, as a
script-free SVG. Unknown servers get an icon for their transport instead.
