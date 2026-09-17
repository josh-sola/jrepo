# panopticon

A personal server for reviewing GitHub pull requests. It runs on Josh's Mac
and serves a review page at `/pr/<owner>/<repo>/<number>`.

This package is a scaffold. It serves a health check, a preferences store,
and placeholder pages. The GitHub layer, diff engine, web diff viewer, and
stack discovery come later.

## Run it in development

```sh
bun install
bun run dev
```

This starts the Bun server on port 7433 and the Vite dev server together.
Vite proxies `/api` requests to the Bun server.

## Run the checks

```sh
bun run check
```

This runs lint, format check, typecheck, and tests, in that order.

## Config

The server reads `~/.config/panopticon/config.json` on start. Point it at a
different file with the `PANOPTICON_CONFIG` environment variable. See
`config.example.json` for the shape. The only required field is
`githubLogin`; everything else has a default.

## Data

The server keeps its SQLite database, cloned git repos, and logs under
`~/.local/share/panopticon/`. Change this with the config's `dataDir` field.
