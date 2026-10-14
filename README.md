# jrepo

Personal projects that share one checkout and one Git history.

| Project | What it does |
| --- | --- |
| [ghostty](ghostty/) | Custom shaders for the Ghostty terminal. |
| [gt-api](gt-api/) | A local REST service that knows Graphite stacks. |
| [panopticon](panopticon/) | A personal server for reviewing GitHub pull requests. |

## Validate

Run this from the repository root:

```sh
just check
```

## Install

```sh
git clone git@github.com:josh-sola/jrepo.git
cd jrepo
just install
```

`just install` installs the Ghostty shaders. Run it from the primary checkout
with `main` checked out and synced. The shader installer refuses linked git
worktrees, any other branch, and uncommitted shader sources because it creates
absolute symlinks, so moving or deleting the checkout later breaks them.
