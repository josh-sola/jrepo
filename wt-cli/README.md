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

Use `wt help -r` for the complete public command tree and `wt help <command>
...` for detailed help.

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
├── upkeep
│   ├── gc
│   └── doctor
├── adopt-branch
├── llm
│   ├── pi
│   ├── claude
│   └── codex
├── go
├── cd
└── help
```

`wt init` and `wt launch` remain hidden compatibility routes for existing
scripts. New commands, documentation, and automation should use the paths
shown above.

## Common workflows

Adopt an existing clone as a base:

```sh
wt repo adopt monorepo ~/repos/monorepo
```

Create a worktree from the repository trunk. The command prints its path
immediately; provisioning continues in the background.

```sh
wt tree new monorepo --name "fix login" --codex -- --model gpt-5
wt tree wait "fix login"
```

Use `--onto` to create from a tree branch, local branch, or commit instead of
trunk:

```sh
wt tree new monorepo --name "follow-up" --onto "fix login"
```

Pass `--pi`, `--claude`, or `--codex` to `wt tree new` or `wt repo lift` to
open an agent after provisioning. Without an agent flag, the command only
creates the tree. Arguments after `--` require an agent flag.

Open an existing tree in Pi, or pick one in the launch screen:

```sh
wt go "fix login"
wt go
```

Pi is the default. Pass `--pi`, `--claude`, or `--codex` to select an agent
explicitly. `wt go <name> --repo <repo>` creates a named tree when no existing
tree matches. `--onto` works here too. A name beginning with `@` opens a
scratch agent session in a base checkout without creating a tree.

### The launch screen

A bare `wt go`, or one narrowed only by `--pi`/`--claude`/`--codex`,
`--profile`, `--repo`, `--branch`, `--onto`, or trailing agent arguments,
opens a full-screen picker instead of failing for want of a tree name. Any
of those flags preselect the matching field. It needs a real terminal on
both stdin and stdout; without one, `wt go` fails instead of hanging.

Type to fuzzy-filter the tree list by repo, name, or branch. The right pane
previews the highlighted tree — the same detail `wt tree status` shows.

| Key | Effect |
| --- | --- |
| type | filter, or edit the focused field |
| ↑ / ↓ | move the tree selection |
| Tab | cycle focus forward: filter → profile → args |
| Shift-Tab | toggle the agent between Pi and Claude |
| Ctrl-P / Ctrl-L / Ctrl-X | select Pi / Claude / Codex |
| Enter | launch the highlighted tree |
| Esc | cancel |

Typing `@label` hides the list and Enter opens a scratch session, same as
typing it as `wt go`'s argument. Typing a name that matches no tree and
pressing Enter opens a small form — name, repo (required, defaulting to the
current repo), branch, and onto — that creates the tree on submit. Esc from
that form returns to the list without canceling the picker.

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
wt repo sync monorepo
wt upkeep gc --dry-run
wt upkeep doctor --fix
```

Run an agent in a selected tree or base directly, bypassing `wt go`:

```sh
wt llm pi "fix login" -- --model custom
wt llm claude "fix login" -- --model opus
wt llm codex monorepo -- --model gpt-5
```

The `wt llm` commands only resolve the working directory and pass arguments
through unchanged. `wt go` adds `-n <label>` before Pi arguments so the
session has the tree or scratch label; a later `-n` from the caller takes
precedence.

`<TREE>` consistently accepts a tree name, unique name substring, UUID or UUID
prefix, or branch name. Ambiguous references fail with the candidates instead
of guessing.

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
effect on the next tree.

## Hot spares

Provisioning a large tree can require a checkout, submodule setup, dependency
installs, and builds. A hot spare has already completed those steps on a
detached `origin/<trunk>` checkout. `wt tree new` claims a ready spare when
one is available. It returns immediately when the spare is at the requested
start point; otherwise it reuses the warm tree and reprovisions it.

`wt repo sync` refreshes spares against trunk and replaces missing ones in the
background. They use one extra checkout and installed dependency set per repo,
and a background install can run when trunk changes. Set `spares 0` in
`config.kdl`, or run `wt repo spare drop`, to disable them.

## Features

`wt go`/`wt llm` always asks `planter --resolve-color` for a color once it has
a ready target. `planter` must be on PATH; the command must exit successfully
and print exactly one supported palette token followed by a newline. wt uses
that token for the terminal background and the eligible Planter session
handoff. A missing binary, a timeout, a failure, or malformed output stops the
launch before terminal hooks or an agent start — there is no fallback.

Optional Planter hooks, terminal hooks, and herdr placement live under
`features` in `config.kdl`. Without a given block, wt does not enable that
integration, but the color preflight above still runs.

```kdl
features {
    planter {
        get-position { builtin "tmux-window" }
        renumber-peers { builtin "planter-state" }
    }
    terminal {
        set-background { builtin "osc11" }
    }
    herdr {
    }
}
```

- `planter` connects sessions launched through `wt go` to the optional
  Planter overlay. Pi, Claude, and interactive Codex sessions share a
  position. In tmux, position follows supported agent windows in the current
  session: unrelated windows are skipped and splits share one position. Outside
  tmux, or when tmux cannot be queried, `PLANTER_TAB_INDEX` is left unset and
  Planter falls back to its normal ordering. Codex uses `planter-codex-bridge`
  when available. A missing or failed bridge falls back to direct Codex.
  Explicit `--remote` endpoints, administrative Codex commands, and direct
  `wt llm codex` remain direct. Eligible sessions receive `PLANTER_COLOR` —
  the result of the required color preflight, which lets Planter bind the real
  session to the color wt already used — and `PLANTER_LABEL`, plus
  `PLANTER_TAB_INDEX` when the position hook succeeds. The bridge preserves
  `PLANTER_STATE_DIR` or `CLAUDE_PLANTER_DIR`.
- `terminal` sets a session's terminal background to its tint color: an
  OSC 11 escape sequence outside tmux, and tmux's window-scoped
  `window-style` option inside it, so tmux itself keeps each window's tint
  right across splits, clients, and reattach. Inside tmux it also colors
  the window's status-bar tab, via window-scoped `window-status-format` /
  `window-status-current-format` overrides — the tab background takes the
  tint with the tree's light text color on it, and the current tab swaps
  in the primary color for its text, bold. It applies to Pi, Claude, and
  Codex sessions launched through `wt go`.

Each tree's color is whatever `planter --resolve-color` returns, one of the
12-color palette in `/palette.json` at the repo root. Each hook is either a wt
`builtin` or a `cmd`. Commands receive `WT_TREE_PATH`, `WT_REPO`, `WT_LABEL`,
`WT_COLOR_HEX` (the near-black tint used as the terminal background),
`WT_COLOR_PRIMARY_HEX` (the color's identity hex), and `WT_COLOR_TEXT_HEX` (a
lighter hex for labels on a dark ground), and wt stops them after two seconds.
A hook error, timeout, or invalid result is treated as absent; it never prevents
the agent session from starting. The builtins are `tmux-window`,
`planter-state`, and `osc11`.

### herdr

The `herdr` block above has no hooks; its presence alone turns on placement.
With it set, and `HERDR_ENV` in the environment (true inside any herdr-managed
pane), `wt go` does not exec the agent in the calling pane. Instead it finds
or creates a herdr workspace named after the tree — a new one via `herdr
worktree open`, or a new tab in the existing one via `herdr tab create` — and
types `wt go <tree> --here ...` into that workspace's pane, which is the run
that actually waits for provisioning and execs the agent. A scratch launch
(`@label`) is placed the same way, keyed by workspace label instead of tree
path, since it has no tree of its own.

Pass `--here` to run the agent in the current pane instead, the same as
without the `herdr` feature block. The placed command always includes it, so
a placed run never tries to place again.

`wt-cli/herdr-plugin/` is a herdr plugin manifest that opens the picker in a
popup. `./install.sh` links it with `herdr plugin link` when `herdr` is on
PATH. Bind the action to open it:

```toml
[[keys.command]]
key = "prefix+alt+w"
type = "plugin_action"
command = "dev.wt.pick"
description = "wt: pick a worktree"
```

Leave herdr's own `remove_worktree` key action unbound. It calls herdr's
`worktree.remove`, which deletes the git worktree directly and bypasses wt's
own store; use `wt tree rm` instead.

## Integrations

- **Statusline.** Integrations should call `wt tree name --path "$PWD"` and
  fall back to the directory basename when it prints nothing.
- **Session hook.** `hooks/session-context.sh` backs Claude Code's
  `SessionStart` and `CwdChanged` hooks. In a tree it reports the name,
  branch, and shared `plans/` path. In a base it explains that `wt tree new`
  is the place to start work. `CwdChanged` delivers the same text as a system
  message because Claude Code does not expose `additionalContext` for that
  hook.
- **Skill.** `plugin/` is a Claude Code skill installed at
  `~/.claude/skills/wt`.
- **Base commit block.** `wt repo adopt` sets a worktree-scoped
  `core.hooksPath` on the base. Its generated hooks point accidental commits
  toward `wt tree new`. It does not overwrite an existing worktree-scoped hook
  path. New trees clear the copied base hook path after `git worktree add`, so
  their repository hooks keep working.
- The LaunchAgent is written but not loaded. It runs `wt repo sync` every five
  minutes and logs to `~/repos/wt/wt-sync.log` and `~/repos/wt/wt-sync.err.log`.
