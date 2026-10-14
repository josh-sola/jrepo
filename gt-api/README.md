# gt-api

A local REST service that knows Graphite stacks. It reads Graphite's files
from a repo checkout and asks GitHub for the rest. Other services call it
instead of each learning how Graphite works.

All responses are JSON. The request and response shapes are zod schemas in
`src/api.ts`. The routes, the types, and the OpenAPI spec all come from them.

## API reference

- `GET /openapi.json` serves the OpenAPI 3.1 spec.
- `GET /docs` serves a browsable reference built from that spec.
- `openapi.json` in this directory is a committed copy of the spec.

Run `bun run openapi` after you change a route or a schema. It rewrites
`openapi.json` without starting a server. `bun run openapi:check` fails when
the file is stale, and `bun run check` runs it.

## Write a client

Generate types from `openapi.json` instead of copying them by hand. For a
TypeScript client, use
[openapi-typescript](https://openapi-ts.dev/):

```sh
bunx openapi-typescript ../gt-api/openapi.json -o src/gtapi/schema.d.ts
```

Then import `components['schemas']['StackResponse']` and its siblings. See
`panopticon/src/server/gtapi/client.ts` for an example. Commit the generated
file, and add a check that fails when it is stale.

## Endpoints

A repo that is not in the config gets `404 {"error":"unknown-repo"}`. Owner
and repo names match without regard to case.

- `GET /health` returns `{"ok":true}`.
- `GET /repos/:owner/:repo/snapshot` returns Graphite's local view of the
  repo: `{ trunk, branches, prs }`. Each branch has a `name`, a `parent`, and
  its `children`. Each PR has a `number`, `headRef`, `baseRef`, `state`
  (`OPEN`, `MERGED`, or `CLOSED`), `title`, and `draft`. It returns
  `404 {"error":"no-graphite-state"}` when the repo has no checkout or the
  Graphite files are missing.
- `GET /repos/:owner/:repo/prs/:number/stack` returns
  `{ entries, truncatedBelow }`: the PR's whole stack, from the bottom up.
  Results are cached for 60 seconds. A missing PR gives 404. Any other
  failure, including a GitHub error, gives 502 with `{ "error": "..." }`.
- `GET /repos/:owner/:repo/prs/:number/landed?createdAt=<ISO>` returns
  `{ "mergeCommitSha": string | null }`. It looks for the squash commit that
  landed the PR on trunk. `createdAt` is when the PR was opened. It gives 400
  when `createdAt` is missing or not a timestamp.

## Run it

```sh
bun install
bun run dev
```

This starts the server on port 7434 with its own data under
`~/.local/share/gt-api-dev/`. Use `bun run start` to run it as the service
would.

## Run the checks

```sh
bun run check
```

This runs lint, format check, typecheck, tests, and the spec check, in that
order.

## Config

The server reads `~/.config/gt-api/config.json`. Point it at another file
with `GT_API_CONFIG`. See `config.example.json`.

```json
{
  "repos": [
    {
      "owner": "Sola-Solutions",
      "repo": "monorepo",
      "trunk": "master",
      "checkout": "~/Library/wrk/repos/monorepo"
    }
  ]
}
```

Each repo takes:

- `owner`, `repo`: the GitHub repo.
- `trunk`: the trunk branch.
- `checkout`: optional. A local checkout of the repo. gt-api reads only its
  Graphite files, and never writes to it, so it can be mounted read-only.
  Without it, the snapshot is a 404 and stacks come from GitHub alone.

Environment variables:

- `GT_API_PORT`: the port. Defaults to 7434.
- `GT_API_DATA_DIR`: where gt-api keeps its clones. Defaults to
  `~/.local/share/gt-api`.
- `GITHUB_TOKEN`: the GitHub token. Without it, gt-api runs `gh auth token`.
  It uses the token for the API and for git over HTTPS. The token reaches git
  through environment variables, never through a file.

## Landed PRs

Graphite's merge queue closes a PR and squashes it onto trunk, so GitHub
shows the PR as closed, not merged. gt-api finds the squash commit by
searching trunk for a subject ending in `(#<number>)`.

For that it keeps a bare clone at `<dataDir>/repos/<owner>/<repo>.git`. The
clone is made with `--filter=tree:0`, so it holds commits and no file trees,
and it fetches only the trunk branch. Concurrent requests share one fetch.

## Run it on the local cluster

gt-api runs on OrbStack's Kubernetes cluster, behind Traefik, at
`http://gt-api.k8s.orb.local`. From the repo root:

```sh
just gt-api-secret   # once, and again after `gh auth refresh`
just k8s-deploy
```

`k8s-deploy` builds an image tagged with the current commit and applies
`helmfile.yaml.gotmpl`. It refuses to build while `gt-api/` has uncommitted
changes. The in-cluster config lives under `config` in
`deploy/chart/values.yaml`.

The pod mounts the monorepo checkout read-only at `/graphite/monorepo`. The
clones live on the 5Gi `gt-api-data` volume.
