---
description: Use when starting or resuming work in a large repo managed by wt (monorepo, helm, toy-apps) — creating a disposable worktree, finding an existing one, checking whether background provisioning has finished, or opening a session in one. Triggers on "start a new worktree", "give me a tree for X", "wt new", "is my worktree ready yet", "where do plans live for this tree", "run claude in this worktree". Read before creating a worktree by hand with `git worktree add`.
---

# wt

`wt` manages disposable git worktrees for large adopted repos. Each tree
arrives with dependencies installed and `.env` files copied in; `plans/`,
`local/`, and `user-memories/` are shared with base and every other tree at
the same path they already expect.

## Commands

| Command | Does |
|---|---|
| `wt new <repo> --name "<summary>" [--branch <b>] [--onto <sel>] [--profile node,python]` | Create a tree and start provisioning it in the background; prints the path immediately. `--onto <sel>` branches from a tree, branch, or commit instead of trunk, joining its Graphite stack. |
| `wt ls [--repo <r>] [--json]` | List registered trees: name, repo, branch, state, uuid, dirty flag. |
| `wt path <selector>` | Print a tree's absolute path. |
| `wt name [--path <p>]` | Print the tree name for a path; silent if not a tree. |
| `wt status [selector] [--json]` | Provisioning state, current step, elapsed time, log path. |
| `wt wait [selector] [--timeout <secs>]` | Block until a tree is ready or failed. |
| `wt claude [selector\|repo]` | Open a Claude session with cwd set to a tree, a repo's base, or the current tree. |
| `wt launch [worktree] [repo] [--onto <sel>]` | Find or create `<worktree>` (in `<repo>` if given, with `--onto` if given), wait for provisioning, and open a colored Claude session in it; `@name` opens a scratch session in the repo's base instead, creating nothing. With no `<worktree>`, opens an fzf picker over the registered trees. |
| `wt rm <selector> [--force] [--delete-branch] [--reparent-children]` | Remove a tree; refuses if dirty or unpushed. `--delete-branch` also refuses a branch with Graphite children unless `--reparent-children` moves them onto its parent first, or `--force` skips every check. |
| `wt gc [--repo <r>] [--dry-run]` | Reap trees with no commits ahead of trunk, no dirty files, and no Graphite children on their branch. |
| `wt sync [<repo>] [--stack]` | Fetch and fast-forward base's trunk; refuses if base is dirty. `--stack` also restacks every Graphite stack with branches in more than one tree. |
| `wt restack [selector] [--dry-run]` | Restack a Graphite stack across every worktree that holds one of its branches, bottom-up, stopping at the first conflict. `--dry-run` prints the plan and runs nothing. |
| `wt stack [selector] [--json] [--all] [--all-branches]` | Show the Graphite stack containing a branch, with tree names instead of worktree paths, PR state, and needs-restack flags. `--all` shows every stack in the repo. |
| `wt doctor [--fix]` | Reconcile the registry against git's actual worktree list. |

A selector is resolved in one order everywhere: exact uuid, uuid prefix,
exact name, unique name substring, then branch name. Ambiguity is an error
listing the candidates — never a guess.

## Traps

- **A tree cannot be moved after provisioning.** `.venv/bin/*` and
  `node_modules/.bin/*` bake in absolute paths at install time. Tear down
  and recreate instead of relocating.
- **Never work in base.** It stays on trunk and is fetched by a background
  timer; committing there is blocked outright. Run `wt new` and work in the
  tree it creates.
- **`wt new` returns before provisioning finishes.** The path it prints
  exists, but dependencies may still be installing. Run `wt wait <selector>`
  before relying on a build succeeding.
