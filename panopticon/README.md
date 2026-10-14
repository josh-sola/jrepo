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
after moving `gh` or `difft`.

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

Each entry under `repos` takes:

- `trunk`: the repo's trunk branch. Required.
- `referenceClone`: a local checkout of the repo. Panopticon reads only its
  Graphite files, to show your stacks. It never fetches into it or clones
  from it, so it can be mounted read-only.
- `hover`: turns on hover types for the repo. `hover.steps` is a list of
  `{ "profile": "node" | "python", "run": "<shell command>", "cwd": "<dir>" }`.
  `cwd` is relative to the tree's root and defaults to the root.

The top-level `hover.maxTrees` (default 3) caps how many hover trees stay on
disk. Panopticon removes the least recently used one to make room.

### GitHub token

Panopticon uses the `GITHUB_TOKEN` environment variable when it is set, and
otherwise runs `gh auth token`. It uses the same token for the API and for
git over HTTPS, so clones need no SSH key. The token reaches git through
environment variables on each git process. It is never written to a
gitconfig file or to a clone's remote URL.

### Hover trees

Hover needs a checked-out tree of the PR with its dependencies installed.
Panopticon makes one per PR under `<dataDir>/trees/<owner>/<repo>/pr-<N>`
with `git worktree add`, then runs the repo's `hover.steps` for the languages
in the diff, one at a time and in the background. Output goes to
`.panopticon-provision.log` in the tree. If a step fails, the tree stays
failed for that head commit and the hover badge shows the error. A new head
commit tries again. Trees idle for 24 hours are removed, as are the trees of
closed and merged PRs.

The TypeScript server is `node_modules/.bin/tsc --lsp` from the tree. The
Python server is `basedpyright-langserver`, which must be on `PATH`.

## Data

The server keeps its SQLite database, cloned git repos, hover trees, and logs under
`~/.local/share/panopticon/`. Change this with the config's `dataDir` field.
