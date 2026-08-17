# jrepo

Personal projects that share one checkout and one Git history.

| Project | What it does |
| --- | --- |
| [wt-cli](wt-cli/) | Git worktree tooling for creating and managing prepared worktrees. |
| [claude-planter](claude-planter/) | A macOS overlay that shows the state of coding sessions as plants. |

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
