---
name: wt
description: Use when starting or resuming work in a large repo managed by wt (monorepo, helm, toy-apps) — creating a disposable worktree, finding an existing one, checking background provisioning, or opening a Codex or Claude session. Read before creating a worktree by hand with `git worktree add`.
---

# wt

`wt` manages adopted repositories and disposable worktrees. Use canonical
commands only. Run `wt help -r` to inspect the full public hierarchy and
`wt help <command> ...` for detailed argument help.

## Commands

| Command | Use |
|---|---|
| `wt repo adopt <REPO> <PATH> [--branch-prefix <PREFIX>] [--redetect]` | Register an existing clone as a repository base. |
| `wt repo sync [REPO] [--stack]` | Fetch and fast-forward bases. With no repo, sync every registered base. |
| `wt repo lift [REPO] --name "<SUMMARY>" [--branch <BRANCH>] [--profile <PROFILE,...>] [--codex\|--claude] [-- <AGENT_ARGS>...]` | Move tracked and untracked base edits into a fresh tree. |
| `wt repo spare [--repo <REPO>] [--json]` | Show hot-spare status. Use `wt repo spare refresh` or `wt repo spare drop` for actions. |
| `wt tree new <REPO> --name "<SUMMARY>" [--branch <BRANCH>] [--onto <TREE_OR_REF>] [--profile <PROFILE,...>] [--codex\|--claude] [-- <AGENT_ARGS>...]` | Create a tree and start provisioning. A ready hot spare may make this immediate. |
| `wt tree ls [--repo <REPO>] [--all] [--json]` | List registered trees. |
| `wt tree path <TREE>` | Print a tree's absolute path. |
| `wt tree name [--path <PATH>]` | Print a tree name for a path, or nothing outside a registered tree. |
| `wt tree rm <TREE> [--force] [--delete-branch] [--reparent-children]` | Remove a tree after safety checks. |
| `wt tree status [TREE] [--all] [--json]` | Show provisioning state. |
| `wt tree wait [TREE] [--timeout <SECONDS>]` | Wait for provisioning to succeed or fail. |
| `wt tree env <TREE>` | Copy configured environment files from base into a tree. |
| `wt gt stack [TREE_OR_BRANCH] [--json] [--all] [--all-branches]` | Show a Graphite stack. |
| `wt gt restack [TREE_OR_BRANCH] [--dry-run]` | Restack a Graphite stack across its trees. |
| `wt upkeep gc [--repo <REPO>] [--dry-run]` | Reap safe unused trees. |
| `wt upkeep doctor [--fix]` | Reconcile registry entries with Git worktree metadata. |
| `wt llm claude [TREE_OR_REPO] [-- <CLAUDE_ARGS>...]` | Run Claude with its working directory set. |
| `wt llm codex [TREE_OR_REPO] [-- <CODEX_ARGS>...]` | Run Codex with its working directory set. |
| `wt go [TREE] [--repo <REPO>] [--branch <BRANCH>] [--onto <TREE_OR_REF>] [--profile <PROFILE,...>] [--claude] [-- <AGENT_ARGS>...]` | Open a tree, create a named tree with `--repo`, or open the picker with no tree. |
| `wt cd <TREE>` | Change the interactive shell directory through the installed shell function. |

`<TREE>` means a tree name, unique name substring, UUID or UUID prefix, or
branch name. `<TREE_OR_BRANCH>` can also be a branch. Ambiguity is an error;
never guess.

## Working rules

- Do not create worktrees directly with `git worktree add`. Use `wt tree new`.
- Do not work in a base checkout. It stays on trunk and a background sync can
  update it. Use `wt repo lift` if edits already exist there.
- `wt tree new` may return before provisioning completes. Run `wt tree wait`
  before assuming dependencies are ready.
- `wt tree ls` hides hot spares by default. Use `--all` or `wt repo spare`
  when inspecting them.
- `wt cd` needs the installed zsh integration. `wt cd --help` is served by
  the binary. `wt go` is an ordinary binary command and works without that
  shell function.
- Legacy flat routes remain hidden for compatibility. Do not use them in new
  instructions, scripts, or examples.
