# Vendored xterm.js assets

Embedded-terminal frontend assets, served offline by `@dshl/control` from
`/dshl-control/assets/xterm/*` (no CDN — the plugin must work in restricted
networks).

| File | Source package | Version |
| --- | --- | --- |
| `xterm.mjs` | `@xterm/xterm` | 6.0.0 |
| `xterm.css` | `@xterm/xterm` | 6.0.0 |
| `addon-fit.mjs` | `@xterm/addon-fit` | 0.11.0 |

Upstream license: MIT (see `LICENSE.md`, from `@xterm/xterm`; `@xterm/addon-fit`
ships under the same repository license). To upgrade, re-download the three
files from jsdelivr/npm at the matching versions and update this table.
