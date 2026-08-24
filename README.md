# jrepo

Personal projects that share one checkout and one Git history.

| Project | What it does |
| --- | --- |
| [wt-cli](wt-cli/) | Git worktree tooling for creating and managing prepared worktrees. |
| [claude-planter](claude-planter/) | A macOS overlay that shows the state of coding sessions as plants. |
| [ghostty](ghostty/) | Custom shaders for the Ghostty terminal. |

Pi extensions that used to live here as the `jpi` package moved to standalone
plugin repos under [github.com/josh-sola](https://github.com/josh-sola)
(`jpi-base`, `jpi-guardian`, `jpi-planter`, `jpi-status`, `jpi-title`,
`jpi-web`). This repository still exposes the `wt` skill as a Pi package.

## Validate

Run this from the repository root:

```sh
just check
```

## Install

Install `wt-cli` from a long-lived checkout. Its installer records absolute
paths for its Claude hook and plugin symlink, so moving or deleting that
checkout later breaks those integrations.

```sh
git clone git@github.com:josh-sola/jrepo.git
cd jrepo
just install
```

`just install` also installs the Ghostty shaders. Run it from the primary
checkout after syncing `main`. The shader installer refuses disposable `wt`
trees and uncommitted shader sources because it creates absolute symlinks.
