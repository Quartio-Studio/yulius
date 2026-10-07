# Vendored third-party code

Only xterm.js is vendored. The files are the unmodified ES module builds from the npm
tarballs; nothing else from the packages is copied. All three packages are MIT licensed; the
license texts are kept next to the files.

| Package | Version | Files | License | Tarball SHA-256 |
| --- | --- | --- | --- | --- |
| `@xterm/xterm` | 6.0.0 | `xterm/xterm.mjs` (from `lib/xterm.mjs`), `xterm/xterm.css` (from `css/xterm.css`) | `xterm/LICENSE.xterm` | `908e66e04af6c8dc6b00dd3b54de088e2e81e5ed866284fd6c2fb3c2d1c7a3f6` |
| `@xterm/addon-fit` | 0.11.0 | `xterm/addon-fit.mjs` (from `lib/addon-fit.mjs`) | `xterm/LICENSE.addon-fit` | `26003b4517a132b64e4ff228fd88a5fda3fff5e606c76093f6dcff772e9ecec0` |
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
