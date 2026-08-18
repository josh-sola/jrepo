# jrepo

Personal projects that share one checkout and one Git history.

| Project | What it does |
| --- | --- |
| [wt-cli](wt-cli/) | Git worktree tooling for creating and managing prepared worktrees. |
| [claude-planter](claude-planter/) | A macOS overlay that shows the state of coding sessions as plants. |
| [ghostty](ghostty/) | Custom shaders for the Ghostty terminal. |
| [claudex](claudex/) | Experimental local ChatGPT gateway for Claude Code. |

## Validate

Run this from the repository root:

```sh
just check
```

## Install

Install `claudex` directly with `uv`; no checkout is needed:

```sh
uv tool install "git+https://github.com/josh-sola/jrepo.git#subdirectory=claudex"
```

See [claudex](claudex/) for login and use. This does not run the repository's
other installers.

## Other project installers

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
