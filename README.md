# wt

Enriched git worktree tooling. One canonical base clone per repo, kept
symlinked at `~/repos/wt/<repo>/base`; disposable worktrees under
`~/repos/wt/<repo>/trees/<uuid>` that arrive with dependencies installed,
`.env` files copied, and durable state like `plans/` shared with base and
every other tree instead of duplicated per tree.

## Install

```sh
./install.sh
```

This runs `cargo install --path .` (so `wt` lands on `~/.cargo/bin/wt`) and
adds a `wt()` shell function to `~/.zshrc`, marker-guarded so re-running is
safe. Open a new shell, or `source ~/.zshrc`, afterward.

The shell function exists only because a child process can't change its
parent shell's directory: `wt go`/`wt cd` resolve a selector to a path and
`cd` there; every other subcommand forwards straight to the `wt` binary.

## Layout

```
~/repos/wt/
├─ data.json                  registry (single file, atomic writes)
└─ <repo>/
   ├─ base/                   canonical clone; shared paths inside it are
   │                          symlinks into shared/, same as in a tree
   ├─ trees/<uuidv7>/         working copies
   ├─ shared/                 durable state symlinked into base and every tree
   ├─ backup/                 base's original directories, moved aside once
   │                          when first symlinked — delete by hand
   └─ cache/cargo-target/     shared CARGO_TARGET_DIR
```

## Commands

```sh
wt init <name> --adopt <path> [--branch-prefix <prefix>]
    # register an existing clone as a repo's base

wt new <repo> --name "<short summary>" [--branch <b>] [--profile node,python]
    # worktree add + registry + shared state + env copy, then return the
    # path in ~2s; provisioning steps run detached in the background

wt ls [--repo <r>] [--json]
    # list registered worktrees: name, repo, branch, state, uuid, dirty flag

wt path <selector>
    # print a worktree's absolute path

wt name [--path <p>]
    # print the worktree name for a path (default $PWD); silent if not a tree

wt status [selector] [--json]
    # provisioning state, current step (index/total), elapsed time, log
    # path; every non-ready tree if no selector is given

wt wait [selector] [--timeout <secs>]
    # block until a tree is ready or failed (default timeout 600s); exits
    # non-zero on failure; with no selector, requires exactly one tree
    # currently provisioning

wt rm <selector> [--force] [--delete-branch]
    # remove a worktree; refuses if dirty or has unpushed commits, or if
    # git's own removal genuinely fails (the registry entry is kept either
    # way); --delete-branch skips branches with commits not on the remote

wt gc [--repo <r>] [--dry-run]
    # reap every tree that is clean, not provisioning, and has no commits
    # beyond origin/<trunk> — deletes its branch too; --dry-run touches
    # nothing

wt doctor [--fix]
    # compare the registry against `git worktree list`: stale entries,
    # worktrees git knows about that wt doesn't, and branch mismatches;
    # --fix drops stale entries and runs `git worktree prune`. Never
    # touches a worktree wt didn't create.

wt go <selector>
wt cd <selector>
    # shell-function only: cd into a worktree
```

A selector is resolved in one order everywhere: exact uuid, uuid prefix,
exact name, unique case-insensitive name substring, then branch name.
Ambiguity is an error listing the candidates — never a guess.

Provisioning steps and the shared/copy path lists are per-repo data in
`data.json`, seeded from a repo's `.worktreeinclude` if it has one.
