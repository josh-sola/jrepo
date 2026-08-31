# wt

`wt` manages adopted Git repositories and ready-to-use disposable worktrees.
Each repository has one base checkout. Worktrees live beside it with configured
environment files, shared durable paths, and provisioning steps already set up.

## Install

```sh
./install.sh
```

The installer puts `wt` on your path, installs the Claude Code skill and
session hook, updates the marked `wt()` block in `~/.zshrc`, and writes—but
does not load—a LaunchAgent that runs `wt repo sync` every five minutes.

Open a new shell, or run `source ~/.zshrc`, after installation. The shell
function handles only `wt cd`, because a child process cannot change its
parent shell's directory. It forwards `wt cd --help` to the binary and uses
`wt tree path` for ordinary `cd` calls. `wt go` always runs in the binary.

## Commands

Use `wt help -r` for the complete public command tree, or `wt help <command>
...` for detailed help at any level.

```text
wt
├── repo
│   ├── adopt
│   ├── sync
│   ├── lift
│   └── spare
│       ├── refresh
│       └── drop
├── tree
│   ├── new
│   ├── ls
│   ├── path
│   ├── name
│   ├── rm
│   ├── status
│   ├── wait
│   └── env
├── gt
│   ├── stack
│   └── restack
├── upkeep
│   ├── gc
│   └── doctor
├── llm
│   ├── pi
│   ├── claude
│   └── codex
├── go
├── cd
└── help
```

The old flat command forms remain accepted as hidden compatibility routes for
existing scripts and installed integrations. New commands, documentation, and
automation should use the paths shown above.

## Common workflows

Adopt an existing clone as a base:

```sh
wt repo adopt monorepo ~/repos/monorepo
```

Create a worktree. The command prints its path immediately; provisioning can
continue in the background.

```sh
wt tree new monorepo --name "fix login" --codex -- --model gpt-5
wt tree wait "fix login"
```

Pass `--pi`, `--claude`, or `--codex` to `wt tree new` or `wt repo lift` to
open that agent after provisioning. With no agent flag, these commands only
create the tree. Arguments after `--` require one of the three flags.

Open an existing tree in Pi, or choose one from the picker:

```sh
wt go "fix login"
wt go
```

Pi is the default. Pass `--pi`, `--claude`, or `--codex` to select an agent
explicitly. Use `wt go <tree> --repo <repo>` to create a named tree when it
does not already exist. A name beginning with `@` opens a scratch agent
session in a base checkout without creating a tree.

Change the current shell's directory:

```sh
wt cd "fix login"
```

Recover edits made in a base checkout:

```sh
wt repo lift monorepo --name "move base edits"
```

Inspect and maintain repositories:

```sh
wt tree ls --repo monorepo
wt repo spare
wt repo sync monorepo --stack
wt upkeep gc --dry-run
wt upkeep doctor --fix
```

Work with a Graphite stack or run an agent in a selected tree or base:

```sh
wt gt stack
wt gt restack "fix login" --dry-run
wt llm pi "fix login" -- --model custom
wt llm claude "fix login" -- --model opus
wt llm codex monorepo -- --model gpt-5
```

The `wt llm` commands only resolve the working directory and pass arguments
through unchanged. `wt go` adds `-n <label>` before Pi arguments so the
session has the tree or scratch label; a later `-n` from the caller takes
precedence.

`<TREE>` consistently accepts a tree name, unique name substring, UUID or
UUID prefix, or branch name. `<TREE_OR_BRANCH>` accepts either a tree or a
branch. Ambiguous references fail with the candidates instead of guessing.

## Layout and configuration

```text
~/repos/wt/
├─ state.json                 machine state for bases and trees; one atomic file
└─ <repo>/
   ├─ base/                   canonical clone; shared paths are symlinks into
   │                          shared/, as they are in every tree
   ├─ trees/<uuidv7>/         working copies, including hot spares
   ├─ shared/                 durable paths symlinked into base and every tree
   ├─ backup/                 original base directories, moved aside once when
   │                          first symlinked; delete manually when safe
   └─ cache/cargo-target/     shared CARGO_TARGET_DIR
```

Configuration lives at `~/.config/wt/config.kdl` (or `$WT_CONFIG`).
`wt repo adopt` adds a repo block without rewriting a block you have edited.
Use `--redetect` to replace only detected provisioning steps.

The base stays on trunk and is not a work area. Use `wt tree new` for work.
Shared, gitignored directories such as `plans/`, `local/`, and
`user-memories/` are linked into the base and every tree. They are not copied,
so changes survive tree removal and remain visible across the repository.
The base paths moved aside during the first adoption stay in `backup/`; wt
does not remove them automatically.

Provisioning steps, trunk, branch prefix, spare count, and per-repo environment
settings are hand-editable in `config.kdl`. The shared and copied paths come
from `.worktreeinclude` each time a tree is created, so a later edit takes
effect on the next `wt tree new`.

## Hot spares

Provisioning a large tree can require a checkout, submodule setup, dependency
installs, and builds. A hot spare has already completed those steps on a
detached `origin/<trunk>` checkout. `wt tree new` claims a ready spare when
one is available. It returns immediately when the spare is at the requested
start point; otherwise it reuses the warm tree and reprovisions it.

Hot spares have no branch, so they stay out of Graphite's graph. `wt repo
sync` refreshes them against trunk and replaces a missing spare in the
background. They use one extra checkout and installed dependency set per repo,
and a background install can run when trunk changes. Set `spares 0` in
`config.kdl`, or run `wt repo spare drop`, to disable them.

## Features

Optional terminal and Planter hooks live under `features` in `config.kdl`.
Without that block, wt does not enable either integration.

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

- `planter` connects sessions launched through `wt go` to the optional
  Planter overlay. Pi, Claude, and interactive Codex sessions share tab
  position. Codex uses `planter-codex-bridge` when available. A missing or
  failed bridge falls back to direct Codex. Explicit `--remote` endpoints,
  administrative Codex commands, and direct `wt llm codex` remain direct.
  Eligible sessions receive `PLANTER_COLOR`, `PLANTER_LABEL`, and
  `PLANTER_TAB_INDEX`. The bridge preserves `PLANTER_STATE_DIR` or
  `CLAUDE_PLANTER_DIR`.
- `terminal` sets a session's terminal background to its tint color: an
  OSC 11 escape sequence outside tmux, and tmux's window-scoped
  `window-style` option inside it, so tmux itself keeps each window's tint
  right across splits, clients, and reattach. It applies to Pi, Claude, and
  Codex sessions launched through `wt go`.

Each tree's color comes from the 12-color palette in `/palette.json` at the
repo root, keyed by a hash of the repo and tree name so the same tree always
gets the same color. Each hook is either a wt `builtin` or a `cmd`. Commands
receive `WT_TREE_PATH`, `WT_REPO`, `WT_LABEL`, `WT_COLOR_HEX` (the near-black
tint used as the terminal background), `WT_COLOR_PRIMARY_HEX` (the color's
identity hex), and `WT_COLOR_TEXT_HEX` (a lighter hex for labels on a dark
ground), and wt stops them after two seconds. A hook error, timeout, or
invalid result is treated as absent; it never prevents the agent session from
starting. The builtins are `ghostty-tab`, `planter-state`, and `osc11`.

## Integrations

- **Statusline.** New integrations should call `wt tree name --path "$PWD"`
  and fall back to the directory basename when it prints nothing. Existing
  flat statusline calls keep working through hidden compatibility.
- **Session hook.** `hooks/session-context.sh` backs Claude Code's
  `SessionStart` and `CwdChanged` hooks. In a tree it reports the name,
  branch, shared `plans/` path, and Graphite position. In a base it explains
  that `wt tree new` is the place to start work. `CwdChanged` delivers the
  same text as a system message because Claude Code does not expose
  `additionalContext` for that hook.
- **Skill.** `plugin/` is a Claude Code skill installed at
  `~/.claude/skills/wt`.
- **Base commit block.** `wt repo adopt` sets a worktree-scoped
  `core.hooksPath` on the base. Its generated hooks point accidental commits
  toward `wt tree new`. It does not overwrite an existing worktree-scoped
  hook path. New trees clear the copied base hook path after `git worktree
  add`, so their repository hooks keep working.
- The LaunchAgent is written but not loaded. It runs `wt repo sync` every
  five minutes and logs to `~/repos/wt/wt-sync.log` and
  `~/repos/wt/wt-sync.err.log`.
