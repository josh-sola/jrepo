---
name: wt
description: Use when starting or resuming work in a large repo managed by wt (monorepo, helm, toy-apps) — creating a disposable worktree, stacking a pull request on an existing branch, submitting or restacking a Graphite stack, finding an existing tree, checking background provisioning, or opening a Pi, Claude, or Codex session. Read before creating a worktree by hand with `git worktree add` or running `gt` directly.
---

# wt

`wt` manages adopted repositories and disposable worktrees arranged into
Graphite stacks. Use canonical commands only. Run `wt help -r` to inspect
the full public hierarchy and `wt help <command> ...` for detailed argument
help.

## Commands

| Command | Use |
|---|---|
| `wt repo adopt <REPO> <PATH> [--branch-prefix <PREFIX>] [--redetect]` | Register an existing clone as a repository base. |
| `wt repo sync [REPO] [--stack]` | Fetch and fast-forward bases. With no repo, sync every registered base. |
| `wt repo lift [REPO] --name "<SUMMARY>" [--branch <BRANCH>] [--profile <PROFILE,...>] [--pi\|--claude\|--codex] [-- <AGENT_ARGS>...]` | Move tracked and untracked base edits into a fresh tree. An agent flag opens that agent after provisioning; no flag only creates the tree. |
| `wt repo spare [--repo <REPO>] [--json]` | Show hot-spare status. Use `wt repo spare refresh` or `wt repo spare drop` for actions. |
| `wt new <REPO> --name "<SUMMARY>" [--branch <BRANCH>] [--profile <PROFILE,...>] [--pi\|--claude\|--codex] [-- <AGENT_ARGS>...]` | Create a tree and root a new stack on trunk. An agent flag opens that agent after provisioning; no flag only creates the tree. |
| `wt pr new --name "<SUMMARY>" [--onto <TREE_OR_BRANCH>] [--branch <BRANCH>] [--profile <PROFILE,...>] [--pi\|--claude\|--codex] [-- <AGENT_ARGS>...]` | Create a tree stacked on another branch. With no `--onto`, stacks on the branch of the tree containing the current directory. |
| `wt sync [TREE_OR_BRANCH]` | Restack one tree's branch if it's pending, then mark its children pending in turn. |
| `wt submit [TREE_OR_BRANCH] [--stack] [--draft] [--publish]` | Submit a branch and its downstack ancestors as pull requests. `--stack` also submits what's stacked on top. Refuses if anything in scope still needs a restack. |
| `wt ls [--repo <REPO>] [--all] [--json]` | List registered trees, grouped by stack. |
| `wt stack [TREE_OR_BRANCH] [--json] [--all] [--all-branches]` | Show the stack containing a tree or branch. |
| `wt restack [TREE_OR_BRANCH] [--dry-run]` | Restack a whole Graphite stack across its trees, bottom-up. |
| `wt tree ls [--repo <REPO>] [--all] [--json]` | List registered trees, flat. |
| `wt tree path <TREE>` | Print a tree's absolute path. |
| `wt tree name [--path <PATH>]` | Print a tree name for a path, or nothing outside a registered tree. |
| `wt tree rm <TREE> [--force] [--delete-branch] [--reparent-children]` | Remove a tree after safety checks. |
| `wt tree status [TREE] [--all] [--json]` | Show provisioning state. |
| `wt tree wait [TREE] [--timeout <SECONDS>]` | Wait for provisioning to succeed or fail. |
| `wt tree env <TREE>` | Copy configured environment files from base into a tree. |
| `wt upkeep gc [--repo <REPO>] [--dry-run]` | Reap safe unused trees, including one whose pull request has merged or closed. |
| `wt upkeep doctor [--fix]` | Report registry drift — including a branch Graphite tracks that no tree holds — and repair what `--fix` can. |
| `wt adopt-branch <BRANCH> [--repo <REPO>] [--name <SUMMARY>] [--profile <PROFILE,...>]` | Materialize a tree for a branch that already exists, such as one `wt upkeep doctor` reports as homeless. |
| `wt llm pi [TREE_OR_REPO] [-- <PI_ARGS>...]` | Run Pi with its working directory set and pass arguments through unchanged. |
| `wt llm claude [TREE_OR_REPO] [-- <CLAUDE_ARGS>...]` | Run Claude with its working directory set and pass arguments through unchanged. |
| `wt llm codex [TREE_OR_REPO] [-- <CODEX_ARGS>...]` | Run Codex with its working directory set and pass arguments through unchanged. |
| `wt go [TREE] [--repo <REPO>] [--branch <BRANCH>] [--onto <TREE_OR_REF>] [--profile <PROFILE,...>] [--pi\|--claude\|--codex] [-- <AGENT_ARGS>...]` | Open a tree, create a named tree with `--repo` (rooting a new stack on trunk unless `--onto` stacks it on an existing branch), or open the picker with no tree. Pi is the default. |
| `wt cd <TREE>` | Change the interactive shell directory through the installed shell function. |

`<TREE>` means a tree name, unique name substring, UUID or UUID prefix, or
branch name. `<TREE_OR_BRANCH>` can also be a branch. Ambiguity is an error;
never guess.

## Stack workflow

The usual life of a stack, end to end:

```sh
wt new myrepo --name "feature base"    # root a new stack on trunk
wt tree wait "feature base"            # let provisioning finish
# edit and commit inside the tree
wt pr new --name "follow-up" --onto "feature base"  # stack the next PR
# edit and commit inside the new tree
wt submit "follow-up"                  # submit it and its downstack ancestors
wt ls                                  # see stacks and pending restacks
wt restack "feature base"              # after trunk or a parent moves
```

`wt submit` refuses while anything in scope is pending restack; run `wt sync`
on the one tree or `wt restack` on the stack first.

## Working rules

- Do not create worktrees directly with `git worktree add`. Use `wt new` for
  a new stack, `wt pr new` to stack a pull request on an existing one.
- Never run `gt create` inside a tree. It moves the tree onto a new branch
  without telling `wt`. Use `wt pr new` instead.
- Do not work in a base checkout. It stays on trunk and a background sync can
  update it. Use `wt repo lift` if edits already exist there.
- `wt new` and `wt pr new` may return before provisioning completes. Run
  `wt tree wait` before assuming dependencies are ready.
- A branch stacked on another can need a restack before it's safe to submit.
  `wt ls` and `wt stack` show which branches are pending; `wt sync` drains
  one tree's own debt, `wt restack` walks a whole stack.
- `wt tree ls` hides hot spares by default. Use `--all` or `wt repo spare`
  when inspecting them.
- `wt cd` needs the installed zsh integration. `wt cd --help` is served by
  the binary. `wt go` is an ordinary binary command and works without that
  shell function. It adds `-n <label>` before Pi arguments; a later `-n`
  from the caller takes precedence.
- Arguments after `--` on `wt new`, `wt pr new`, or `wt repo lift` require
  one of `--pi`, `--claude`, or `--codex`.
