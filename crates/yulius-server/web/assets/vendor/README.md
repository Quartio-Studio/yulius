# Vendored third-party code

xterm.js, the Monaco editor and rrweb are vendored. The files are unmodified builds from the npm
tarballs; nothing else from the packages is copied. All packages are MIT licensed; the license
texts (and Monaco's third-party notices) are kept next to the files.

| Package | Version | Files | License | Tarball SHA-256 |
| --- | --- | --- | --- | --- |
| `@xterm/xterm` | 6.0.0 | `xterm/xterm.mjs` (from `lib/xterm.mjs`), `xterm/xterm.css` (from `css/xterm.css`) | `xterm/LICENSE.xterm` | `908e66e04af6c8dc6b00dd3b54de088e2e81e5ed866284fd6c2fb3c2d1c7a3f6` |
| `@xterm/addon-fit` | 0.11.0 | `xterm/addon-fit.mjs` (from `lib/addon-fit.mjs`) | `xterm/LICENSE.addon-fit` | `26003b4517a132b64e4ff228fd88a5fda3fff5e606c76093f6dcff772e9ecec0` |
| `monaco-editor` | 0.57.0 | `monaco/vs/**` (from `min/vs`, without `min/vs/language` and `min/vs/nls/lang`, which the build does not load) | `monaco/LICENSE.monaco-editor`, `monaco/ThirdPartyNotices.monaco-editor.txt` | `3ea1712fbacd3290cf4751007e3a5b57cc279767607812d4c3e57925fb0b05c2` |
| `rrweb` | 2.1.7 | `dom-mirror/replayer.js` (from `dist/rrweb.js`, the self-contained ES module with `Replayer`; source-map comment removed), `dom-mirror/replayer.css` (from `dist/style.min.css`) | `dom-mirror/LICENSE.rrweb` (MIT per package.json; the npm package ships no license file) | `13680f81b1df0f7bae04a27e5b373de2b0deb39db1506049d53448c21428ba26` |
| `@xterm/addon-web-links` | 0.12.0 | `xterm/addon-web-links.mjs` (from `lib/addon-web-links.mjs`) | `xterm/LICENSE.addon-web-links` | `bf463cfff1af4bb1903509830c62ed06bcd0589039f15ea5fba71d7bae5a86da` |

The `.mjs` builds are self-contained (no bare imports), so the browser loads them directly
as ES modules without a bundler. Source map comments at the end of the files refer to maps
that are not vendored; browsers only request them when developer tools are open.

## Updating

In a temporary directory outside the repository (Node is needed only for this step):

```sh
npm pack @xterm/xterm@<v> @xterm/addon-fit@<v> @xterm/addon-web-links@<v>
for f in *.tgz; do mkdir -p "${f%.tgz}" && tar -xzf "$f" -C "${f%.tgz}"; done
shasum -a 256 *.tgz
```

Copy `lib/*.mjs`, `css/xterm.css` and each `LICENSE` over the files above, update this
table, and check the terminal in mock mode (see the web UI README).

Monaco (the built-in editor, SPEC-001 EDT-001): download `monaco-editor@<v>` from the npm
registry, verify the tarball's `dist.integrity`, copy `package/min/vs` to `monaco/vs`,
delete `monaco/vs/language` and `monaco/vs/nls/lang`, then check in headless Chrome that the
editor loads, colors JavaScript and suggests `console.` members (the TypeScript worker) with no
failed requests.

rrweb (the DOM-mirrored browser view, SPEC-001 BRW-005): the replayer is `rrweb@<v>`
`dist/rrweb.js`; the recorder injected into pages is `@rrweb/record@<v>` `dist/record.cjs`, kept in
`crates/yulius-browser/assets/rrweb-record.cjs` (tarball SHA-256
`d07955b789976c4dea6ae92debc46f3b0b80453e06b15d9a34ad802c38339409`). Verify both tarballs'
`dist.integrity`; keep the versions equal.
Keep the replayer's neutral path `dom-mirror/replayer.js`: content blockers (Brave Shields,
uBlock Origin with EasyPrivacy) block any `/rrweb.js` in the user's browser. `browser.js` imports
it lazily, so a blocked load only turns the browser panel to pixels.
