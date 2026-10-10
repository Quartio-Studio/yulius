# Vendored third-party code

xterm.js, the Monaco editor and Shaders are vendored. The files are builds from the npm
tarballs (unmodified, except the two Shaders changes listed below); nothing else from the packages
is copied. All packages are MIT licensed; the license
texts (and Monaco's third-party notices) are kept next to the files.

| Package | Version | Files | License | Tarball SHA-256 |
| --- | --- | --- | --- | --- |
| `@xterm/xterm` | 6.0.0 | `xterm/xterm.mjs` (from `lib/xterm.mjs`), `xterm/xterm.css` (from `css/xterm.css`) | `xterm/LICENSE.xterm` | `908e66e04af6c8dc6b00dd3b54de088e2e81e5ed866284fd6c2fb3c2d1c7a3f6` |
| `@xterm/addon-fit` | 0.11.0 | `xterm/addon-fit.mjs` (from `lib/addon-fit.mjs`) | `xterm/LICENSE.addon-fit` | `26003b4517a132b64e4ff228fd88a5fda3fff5e606c76093f6dcff772e9ecec0` |
| `monaco-editor` | 0.57.0 | `monaco/vs/**` (from `min/vs`, without `min/vs/language` and `min/vs/nls/lang`, which the build does not load) | `monaco/LICENSE.monaco-editor`, `monaco/ThirdPartyNotices.monaco-editor.txt` | `3ea1712fbacd3290cf4751007e3a5b57cc279767607812d4c3e57925fb0b05c2` |
| `@xterm/addon-web-links` | 0.12.0 | `xterm/addon-web-links.mjs` (from `lib/addon-web-links.mjs`) | `xterm/LICENSE.addon-web-links` | `bf463cfff1af4bb1903509830c62ed06bcd0589039f15ea5fba71d7bae5a86da` |
| `shaders` | 4.0.2 | `shaders/js/**`, `shaders/core/**` (from `dist/`: exactly the module graph of `dist/js/createShader.js`, 208 files, source-map comments removed; TypeGPU 0.12.3 is bundled inside) | `shaders/LICENSE.shaders`, `shaders/LICENSE.typegpu` | `87fed38ade6f4d2d7204fee5f36bef59e048bac19dd10f271a0057a370b1aa4a` (TypeGPU's license from `typegpu-0.12.3.tgz`, `e7e654876f6f08b11283b4f05fc00d35e7bfd2949bcdd6ef95dea1b96fcd2605`) |

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

Shaders (the brand's WebGPU effects, brandfx.js, SPEC-001 UI-041): `npm pack shaders@<v>`, then copy
the module graph of `dist/js/createShader.js` (follow its relative imports) to `shaders/`, keeping
the paths under `dist/`. Two changes, as a bundler would make and as privacy requires:
`shaders/core/telemetry/index.js` is replaced by a no-op with the same exports (the original posts
the page's domain, browser and frame timings to shaders.com for a share of sessions), and
`process.env.NODE_ENV` in `core/index.js` and `core/std-*.js` becomes `"production"`. The other
network code in the graph (image, SDF and font loaders) only runs for effects given a URL, which
Yulius does not use. Check that `grep -r shaders.com/api shaders/` finds nothing and that the
composer's niche shows its smoke with `?mock=1&lab=roma` in a browser with WebGPU.

