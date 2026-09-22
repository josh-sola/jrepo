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
Vite proxies `/api` requests to the Bun server. The dev server reads the same
config file as the installed service, but `PANOPTICON_PORT` and
`PANOPTICON_DATA_DIR` give it its own port and its own database under
`~/.local/share/panopticon-dev/`, so both can run at once.

## Run it as a service

```sh
./install.sh install
```

This builds the web app and loads a login LaunchAgent that keeps the server
running. Run it from the main checkout, not a worktree: the agent runs from
whatever directory you install it from. Set `"port": 80` in the config to
reach it at `http://panopticon.localhost` with no port. macOS lets a normal
user listen on port 80, and browsers send any `*.localhost` name to this Mac.

The plist keeps the `PATH` you ran the installer with, so re-run the installer
after moving `gh`, `difft`, or `wt`.

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
