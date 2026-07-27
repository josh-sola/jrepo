# claude-planter

A row of potted plants that floats above your windows, one per Claude Code
session. Each plant tells you one thing: whether that session is working, or
waiting for you.

Built for running several sessions at once in different terminals, where the
thing you keep losing track of is which one needs you.

| The plant | The session |
| --- | --- |
| **Blooming**, swaying gently | Claude is working |
| Blooming **with side buds** | Subagents are running — one bud each, up to two |
| **Wilted**: half height, flower closed, petals dropped on the soil | Its turn ended. Your move |
| Wilted **with a `!`** | Blocked on a permission prompt, or gone quiet waiting for input |

Work done by a subagent counts as working, so a session that delegated and is
waiting on its agents keeps flowering rather than looking idle.

Every session gets its own colour from a palette of eight, picked by hashing the
session id, so a plant keeps its variety for its whole life.

macOS only — it draws with AppKit.

## Install

```sh
./install.sh
planter                      # start the overlay
./login-item.sh install      # and/or start it at every login
```

`install.sh` builds `~/.local/bin/planter`, installs a hook at
`~/.claude/hooks/planter-state`, and wires eight hooks into
`~/.claude/settings.json`, backing it up first. It needs `swiftc` (the Xcode
command line tools) and `jq`. Re-running replaces its own hook entries rather
than stacking them.

Claude Code re-reads its settings, so sessions that are already open get a plant
on their next turn. Restart any session whose plant never shows up.

To remove everything:

```sh
./install.sh --uninstall     # hooks, hook script, binary, login item
rm -rf ~/.claude/planter     # state too, if you want it gone
```

## Using it

- **Drag** a plant to move the row. The position is remembered.
- **⌘-drag** a plant to reorder it. Plants swap as the cursor crosses each
  neighbour, and the order is remembered per session.
- **Right-click** for labels, **Reset order**, and **Quit**.
- Clicks between plants pass straight through to the window underneath.
- The row hides itself when no sessions are live.

```
planter --list          print the live sessions as text
planter --demo          run with four fake plants
planter --preview FILE  render every frame and colour to a PNG
planter --scale N       pixel size, default 3
planter --no-labels     hide the labels, and pack the plants tighter
```

Reordering never changes a plant's colour: hues are assigned in creation order,
before the saved order is applied.

## Labels

Each plant is labelled with its session's directory, capped at 12 characters so
one long name cannot push the row out of line. `--no-labels` drops them and packs
the plants closer together.

A directory basename is a poor label if your worktrees are named after a uuid or
a ticket id. If `~/.claude/planter/label-hook` exists and is executable, it is
handed the session's directory, and whatever it prints becomes the label. It runs
once per session, so it can afford to be slow. Two examples ship in `examples/`:

```sh
cp examples/label-hook.git-branch ~/.claude/planter/label-hook
chmod +x ~/.claude/planter/label-hook
```

- `label-hook.git-branch` — the git branch when the session is in a worktree, the
  repository name otherwise.
- `label-hook.wt` — resolves `wt` worktrees, whose directories are uuids, to their
  registry name.

## Starting it at login

```sh
./login-item.sh install      # write the LaunchAgent and start it now
./login-item.sh status       # what launchd thinks
./login-item.sh uninstall    # stop it and remove the plist
```

The agent restarts the overlay if it crashes but leaves a clean exit alone, so
**Quit** from the right-click menu sticks until your next login. A `kill` is not a
clean exit and will be restarted — uninstall to stop it for good. Logs go to
`~/Library/Logs/claude-planter.log`.

## How it works

Hooks write one small JSON file per session into `~/.claude/planter/`, and the
overlay watches that directory.

| Hook | Effect on the plant |
| --- | --- |
| `SessionStart` | appears, wilted |
| `UserPromptSubmit` | blooms |
| `PostToolUse` | keeps blooming — this is what clears a `!` after you approve |
| `Notification` | shows `!` |
| `Stop` | wilts, **unless** a subagent is still running |
| `SubagentStart` / `SubagentStop` | counts running agents up and down |
| `SessionEnd` | disappears |

Four details do most of the work:

**A blocking subagent needs no counting.** `Stop` only fires when the main turn
ends, and a synchronous `Agent` call holds that turn open. The counter exists for
*background* agents, where the turn ends while work continues — the plant keeps
flowering until the last one finishes.

**Crashed sessions clean themselves up.** Each file records the pid of its claude
process. The overlay drops any plant whose process is gone, so a session killed
without a `SessionEnd` never leaves one behind.

**The label is fixed when the plant is created.** A session's reported working
directory follows its *shell*, which wanders into subdirectories and temporary
agent worktrees as work goes on, so a label that tracked it would rename itself
mid-session.

**`planter-state` runs on every tool call**, so the common case — a tool
finishing in a session that is already blooming — is answered by one `grep` and
nothing else, about 10ms. If you would rather not pay it, delete the `PostToolUse`
entry from `~/.claude/settings.json`; all you lose is that a `!` then stays up
until the turn ends, instead of clearing when you approve.

Set `CLAUDE_PLANTER_DIR` to move the state directory. Both the hook and the
overlay respect it, which is also how to try things out without touching your
real sessions.

## Editing the art

The sprites are character grids in `Sprites.swift`, one character per pixel,
stacked as layers — a pot, a plant, sometimes a glyph — so the pot stays put
while the plant above it sways. `f`/`F` petals and `p` pot are drawn in each
plant's hue; `g`/`G` leaves and `s` soil stay green and brown, which is what keeps
it looking like a plant.

Layout measures the art rather than assuming it, so if you redraw the plant wider
or narrower the spacing follows. After an edit:

```sh
swiftc -O -o planter main.swift Planter.swift Sprites.swift
./planter --preview /tmp/preview.png && open /tmp/preview.png
```
