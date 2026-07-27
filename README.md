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

This runs `cargo install --path .` (so `wt` lands on `~/.cargo/bin/wt`),
adds a `wt()` shell function to `~/.zshrc`, symlinks `~/.claude/skills/wt`
to `plugin/`, registers the `SessionStart`/`CwdChanged` hook in
`~/.claude/settings.json`, and writes (but does not load) a LaunchAgent that
runs `wt sync` every 5 minutes. Every step is marker-guarded so re-running is
safe. Open a new shell, or `source ~/.zshrc`, afterward; run the
`launchctl bootstrap` command the installer prints when you want the
background sync active.

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
                                       [--claude [-- <claude args>]]
    # worktree add + registry + shared state + env copy, then return the
    # path in ~2s; provisioning steps run detached in the background.
    # --claude waits for provisioning, then opens a session in the tree:
    #   wt new monorepo --name "fix the thing" --claude -- --model opus
    # Progress prints while it waits. A failed install refuses to open a
    # session and tells you to check `wt status`.

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

wt sync [<repo>]
    # fetch and fast-forward base's trunk; refuses if base is dirty
    # (never `reset --hard`, never force). No argument syncs every
    # registered repo. Run by the LaunchAgent every 5 minutes.

wt adopt [<repo>] --name "<short summary>"
    # move uncommitted work out of base into a fresh tree, for when you
    # started editing in base by mistake. Refuses on a clean base. If the
    # stash cannot be applied, it stays intact and the command exits
    # non-zero rather than dropping it.

wt env refresh <selector>
    # re-copy the repo's env files from base into a tree, overwriting what
    # is there. Regenerating them in base (from Parameter Store, needing
    # AWS auth) is a separate manual step; this only pushes base's current
    # copies outward.

wt claude [<selector>|<repo>] [-- <claude args>]
    # exec `claude` (process replacement) with cwd set: a selector
    # resolves to that tree, a bare registered repo name means its base,
    # no argument means the current directory's tree or base if there is
    # one. Warns to stderr and proceeds anyway when the target is base.
    # Anything after `--` goes straight to claude, e.g.
    #   wt claude monorepo -- --model opus

wt go <selector>
wt cd <selector>
    # shell-function only: cd into a worktree
```

A selector is resolved in one order everywhere: exact uuid, uuid prefix,
exact name, unique case-insensitive name substring, then branch name.
Ambiguity is an error listing the candidates — never a guess.

Provisioning steps and the shared/copy path lists are per-repo data in
`data.json`, seeded from a repo's `.worktreeinclude` if it has one.

## Integrations

- **Statusline.** `statusline.sh` calls `wt name --path "$PWD"` and falls
  back to the directory basename when that prints nothing.
- **Session hook.** `hooks/session-context.sh` backs the `SessionStart` and
  `CwdChanged` Claude Code hooks: in a tree it surfaces the tree's name,
  branch, and `plans/` path; in base it surfaces that base stays on trunk
  and `wt new` is how to start work. `CwdChanged` can't inject model
  context (no `additionalContext` support there), so it delivers the same
  text as a `systemMessage` instead — a real Claude Code limitation, not a
  half-finished feature.
- **Skill.** `plugin/` is a Claude Code skill documenting `wt` for agents,
  installed at `~/.claude/skills/wt`.
- **Base commit block.** `wt init` sets a `--worktree`-scoped
  `core.hooksPath` on base pointing at generated `pre-commit`/`pre-push`
  hooks that fail with a message pointing at `wt new`. This needs
  `extensions.worktreeConfig` (`wt init` enables it if unset) and never
  overwrites an existing worktree-scoped `core.hooksPath`. `git worktree
  add` copies base's `config.worktree` into every new tree, so `wt new`
  clears the copied `core.hooksPath` from each tree right after creating
  it — otherwise every tree would inherit base's block instead of whatever
  hooks path the repo normally uses (e.g. Husky's `.husky`).
- **LaunchAgent.** `com.joshbassin.wt.sync`, written (not loaded) by
  `install.sh`, runs `wt sync` every 5 minutes and logs to
  `~/repos/wt/wt-sync.log` / `wt-sync.err.log`.
