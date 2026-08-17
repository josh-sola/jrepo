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
├─ state.json                 machine state: repo base paths, every tree
│                             (single file, atomic writes)
└─ <repo>/
   ├─ base/                   canonical clone; shared paths inside it are
   │                          symlinks into shared/, same as in a tree
   ├─ trees/<uuidv7>/         working copies, including each repo's hot spare
   ├─ shared/                 durable state symlinked into base and every tree
   ├─ backup/                 base's original directories, moved aside once
   │                          when first symlinked — delete by hand
   └─ cache/cargo-target/     shared CARGO_TARGET_DIR
```

Portable, hand-editable config — trunk, branch prefix, spare count,
per-repo env, provisioning steps — lives separately at
`~/.config/wt/config.kdl` (override with `$WT_CONFIG`). `wt init` appends a
`repo` block there once per repo and never rewrites one you've edited by
hand; re-running it is safe to do as often as you like.

## Commands

```sh
wt init <name> --adopt <path> [--branch-prefix <prefix>] [--redetect]
    # register an existing clone as a repo's base, appending a config block
    # for it. A repo that already has a block is left alone (a passed
    # --branch-prefix is ignored, with a warning) — edit config.kdl by hand
    # to change trunk/branch-prefix/spares/env once it exists.
    # --redetect re-runs step detection against an existing block, replacing
    # just its steps; everything else in the block is untouched.

wt new <repo> --name "<short summary>" [--branch <b>] [--onto <sel>]
                                       [--profile node,python]
                                       [--codex|--claude [-- <agent args>]]
    # worktree add + registry + shared state + env copy, then return the
    # path in ~2s; provisioning steps run detached in the background.
    # Claims a repo's hot spare when one is ready, returning an
    # already-provisioned tree instantly instead of the above.
    # --onto branches from <sel> instead of origin/<trunk>, joining
    # whatever Graphite stack it belongs to: a wt tree selector (its live
    # branch), a branch name, or a commit-ish.
    # --codex or --claude waits for provisioning, then opens that agent in
    # the tree. Arguments are passed through unchanged:
    #   wt new monorepo --name "fix the thing" --codex -- --model gpt-5
    #   wt new monorepo --name "fix the thing" --claude -- --model opus
    # Progress prints while it waits. A failed install refuses to open a
    # session and tells you to check `wt status`.

wt launch [worktree] [repo] [--branch <b>] [--onto <sel>] [--profile node,python]
                             [--claude] [-- <agent args>]
    # find <worktree>, creating it in <repo> (like `wt new`, including
    # --onto) if it doesn't exist there, wait for provisioning, then open a
    # Codex session in it. Codex is the default; --claude keeps Claude's
    # existing session setup. <repo> can be omitted if exactly one tree
    # matches <worktree>, or the current directory's repo breaks the tie.
    # The session gets a color derived from repo+name — both as Claude's
    # prompt-bar color and the terminal background — so a tab is
    # identifiable at a glance:
    #   wt launch "fix login" monorepo -- --model gpt-5
    #   wt launch "fix login" monorepo --claude -- --model opus
    # Claude receives a generated label and trailing /color command. If
    # <claude args> already contains a bare prompt, claude receives two
    # positional prompts and the trailing /color one may be ignored.
    #
    # With no <worktree>, an fzf picker lists every registered tree (a
    # preview pane shows its state, age, and commits) and opens whichever
    # one you pick:
    #   wt launch
    #   wt launch -- --model opus
    #
    # A worktree starting with `@` is a scratch session: it opens in
    # <repo>'s base with the same naming and coloring, but creates no
    # worktree and touches nothing in the registry. --branch, --onto, and
    # --profile don't apply and are an error alongside it:
    #   wt launch @poking-around monorepo

wt ls [--repo <r>] [--json] [--all]
    # list registered worktrees: name, repo, branch, state, uuid, dirty flag.
    # Hides each repo's hot spare; --all shows it too.

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

wt rm <selector> [--force] [--delete-branch] [--reparent-children]
    # remove a worktree; refuses if dirty or has unpushed commits, or if
    # git's own removal genuinely fails (the registry entry is kept either
    # way); --delete-branch skips branches with commits not on the remote,
    # and refuses a branch with Graphite children instead of orphaning
    # them — --reparent-children re-parents each one onto the deleted
    # branch's own parent (trunk, if it has none) first; --force skips
    # every check above

wt gc [--repo <r>] [--dry-run]
    # reap every tree that is clean, not provisioning, and has no commits
    # beyond origin/<trunk> — deletes its branch too; skips a tree whose
    # branch has Graphite children rather than orphaning them; never reaps
    # a repo's hot spare; --dry-run touches nothing

wt doctor [--fix]
    # compare the registry against `git worktree list`: stale entries,
    # worktrees git knows about that wt doesn't, and branch mismatches;
    # --fix drops stale entries and runs `git worktree prune`. Never
    # touches a worktree wt didn't create.

wt sync [<repo>] [--stack]
    # fetch and fast-forward base's trunk; refuses if base is dirty
    # (never `reset --hard`, never force). No argument syncs every
    # registered repo. Run by the LaunchAgent every 5 minutes.
    # --stack also restacks every Graphite stack in the repo that has
    # branches in more than one worktree, walking bottom-up from wherever
    # each branch lives; never deletes a branch, that's `gt sync`'s job.
    # Also refreshes each repo's hot spare and tops up a missing one, both
    # in the background.

wt spare [--repo <r>] [--json]
    # show each repo's hot spare: state, short HEAD, age, commits behind
    # origin/<trunk>, and its provisioning log path

wt spare refresh [--repo <r>]
    # force a refresh now, instead of waiting for the next `wt sync` tick

wt spare drop [--repo <r>]
    # remove a repo's spare and set its `spares` to 0 in config.kdl —
    # otherwise the next background top-up just rebuilds it

wt restack [selector] [--dry-run]
    # restack a Graphite stack across every worktree that holds one of its
    # branches, bottom-up, running each step in the tree that holds it.
    # A selector is a worktree name/uuid first, then a branch name; with
    # none, the stack containing the current tree's (or base's)
    # checked-out branch. Stops at the first conflict and names where to
    # continue; --dry-run prints the plan and runs nothing.

wt stack [selector] [--json] [--all] [--all-branches]
    # show the Graphite stack containing a branch, with wt identity — tree
    # names instead of raw worktree paths — plus PR state and needs-restack
    # flags. Selector resolves like `wt restack`'s. --all shows every stack
    # in the repo; --all-branches also shows merged/closed branches, hidden
    # by default; --json for agents.

wt adopt [<repo>] --name "<short summary>" [--codex|--claude [-- <agent args>]]
    # move uncommitted work out of base into a fresh tree, for when you
    # started editing in base by mistake. Refuses on a clean base. If the
    # stash cannot be applied, it stays intact and the command exits
    # non-zero rather than dropping it.
    # --codex and --claude wait for provisioning, then pass their arguments
    # through unchanged.

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

wt codex [<selector>|<repo>] [-- <codex args>]
    # the direct Codex analogue of `wt claude`: exec `codex` with cwd set to
    # a selector, bare repo base, or the current tree/base. It warns when
    # targeting base and forwards everything after `--` unchanged:
    #   wt codex monorepo -- --model gpt-5

wt go <selector>
wt cd <selector>
    # shell-function only: cd into a worktree
```

A selector is resolved in one order everywhere: exact uuid, uuid prefix,
exact name, unique case-insensitive name substring, then branch name.
Ambiguity is an error listing the candidates — never a guess.

Provisioning steps are per-repo data in `config.kdl`, detected by `wt init`
and refreshed on demand with `--redetect`. The shared/copy path lists aren't
stored anywhere — they're read from a repo's `.worktreeinclude` fresh each
time a tree is created, so an edit to that file takes effect on the next
`wt new` with no re-init needed.

## Hot spares

Provisioning a tree from cold on a big monorepo means a full checkout,
submodule init, `pnpm install`, `pnpm build:packages`, and `uv sync` — none of
which depends on the branch name you asked for. So each repo keeps one **hot
spare**: a worktree that has already been through every step, sitting on a
detached HEAD at `origin/<trunk>`.

`wt new` claims a repo's spare when one is ready, instead of provisioning a
tree from cold. If the spare's HEAD is already the commit the new branch
starts from, no working-tree file changes and the installed dependencies are
correct by construction — `wt new` creates the branch, marks the tree ready,
and returns instantly. If the commits differ, the provisioning steps still
have to run, but in a tree that's already warm instead of empty.

A spare has no branch of its own. Detached HEAD keeps it out of your branch
namespace and off Graphite's graph. `wt sync` keeps each spare pinned to
`origin/<trunk>` and tops up a missing one, both in the background, so the
common case is a spare that's already current by the time you ask for a
tree.

The cost is one extra checkout and one extra set of installed dependencies
per repo, idling on disk, plus a background install that can fire whenever
trunk moves. Set `spares 0` on a repo in `config.kdl`, or run
`wt spare drop`, to opt out.

## Features

`wt launch` can reach outside wt to set a terminal background color. Claude
launches can also find their tab position and coordinate with claude-planter.
These opt-in hooks are declared under a `features` block in `config.kdl`.
Absent block means off:

```kdl
features {
    planter {
        get-position { builtin "ghostty-tab" }
        renumber-peers { builtin "planter-state" }
    }
    terminal {
        set-background { builtin "osc11" }
    }
}
```

- **`planter`** is Claude-only. It integrates with claude-planter, a
  tab-labeling overlay:
  `get-position` finds this session's rank among its terminal's other tabs,
  and `renumber-peers` corrects the other claude-planter sessions whose rank
  a new tab just pushed down a slot. Declaring `planter` also makes `wt
  launch` set `PLANTER_COLOR`, `PLANTER_LABEL`, and `PLANTER_TAB_INDEX` in
  the Claude session's environment. Codex never runs planter hooks or
  receives `PLANTER_*` environment variables.
- **`terminal`** sets the terminal background: `set-background` writes an
  OSC 11 escape sequence in the session's color. It applies to both Codex
  and Claude launches.

Each hook takes exactly one of a `builtin` (one of wt's own
implementations) or a `cmd` (your own command, run with `WT_TREE_PATH`,
`WT_REPO`, `WT_LABEL`, and `WT_COLOR_HEX` in its environment, killed after
2 seconds). A hook that fails — wrong builtin, non-zero exit, unparseable
output, timeout — is treated as absent; it never fails the launch. The
builtins are `ghostty-tab` and `planter-state` for `planter`, and `osc11`
for `terminal`. A user on a different terminal swaps in their own:

```kdl
    get-position { cmd "~/bin/iterm-tab-index" }
```

## Integrations

- **Statusline.** `statusline.sh` calls `wt name --path "$PWD"` and falls
  back to the directory basename when that prints nothing.
- **Session hook.** `hooks/session-context.sh` backs the `SessionStart` and
  `CwdChanged` Claude Code hooks: in a tree it surfaces the tree's name,
  branch, `plans/` path, and — when Graphite tracks the branch — its stack
  position: the parent branch and which tree holds it, the children and
  their trees, and a note when the tree is mid-stack that a restack of the
  branch below belongs in that tree, not this one. In base it surfaces that
  base stays on trunk and `wt new` is how to start work. `CwdChanged` can't
  inject model context (no `additionalContext` support there), so it
  delivers the same text as a `systemMessage` instead — a real Claude Code
  limitation, not a half-finished feature.
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
