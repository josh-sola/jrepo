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
Vite proxies `/api` requests to the Bun server. The dev server reads
`~/.config/panopticon/config.json`, and `PANOPTICON_PORT` and
`PANOPTICON_DATA_DIR` give it its own port and its own database under
`~/.local/share/panopticon-dev/`.

## Run it on the local cluster

panopticon runs on OrbStack's Kubernetes cluster, behind Traefik, at
`http://panopticon.k8s.orb.local`. From the repo root:

```sh
just panopticon-secret   # once, and again after `gh auth refresh`
just k8s-deploy
```

`k8s-deploy` builds an image tagged with the current commit and applies
`helmfile.yaml.gotmpl`. It refuses to build while `panopticon/` has
uncommitted changes. The in-cluster config lives under `config` in
`deploy/chart/values.yaml`.

panopticon gets Graphite stacks and landed-PR checks from gt-api, a separate
service in the `gt-api` namespace. The pod reaches it at
`http://gt-api.gt-api.svc.cluster.local`. The database, clones, hover trees,
and package caches live on the `panopticon-data` volume.

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

`gtApiUrl` is the base URL of gt-api, the service that reads Graphite's
stack state. It defaults to `http://gt-api.k8s.orb.local`. If gt-api is down,
the stack view shows an error and the inbox falls back to each PR's GitHub
base branch.

Each entry under `repos` takes:

- `trunk`: the repo's trunk branch. Required.
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
commit tries again, and so does a restart: at boot, panopticon removes
failed and interrupted trees and any tree directory with no record. Trees idle for 24 hours are removed, as are the trees of
closed and merged PRs.

The TypeScript server is `node_modules/.bin/tsc --lsp` from the tree. The
Python server is `basedpyright-langserver`, which must be on `PATH`.

## Data

The server keeps its SQLite database, cloned git repos, hover trees, and logs under
`~/.local/share/panopticon/`. Change this with the config's `dataDir` field.
