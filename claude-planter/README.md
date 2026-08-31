# claude-planter

A row of potted plants that floats above your windows, one per coding
session. Each plant tells you one thing: whether that session is working, or
waiting for you.

Built for running several sessions at once in different terminals, where the
thing you keep losing track of is which one needs you.

| The plant | The session |
| --- | --- |
| **Blooming**, swaying gently | The coding agent is working |
| Blooming **with side buds** | Subagents are running — one bud each, up to two |
| **Wilted**: flower closed, petals on the soil | Its turn ended. Your move |
| Wilted, **flower dropped** | Waiting more than 2 minutes |
| **Collapsed** onto the soil | Waiting more than 10 minutes |
| Wilted **with a `!`** | Blocked on a prompt that needs your input |

Work done by a subagent counts as working, so a session that delegated and is
waiting on its agents keeps flowering rather than looking idle.

Wilting is progressive, because a row of identically wilted plants can't tell you
the thing you actually want to know: which session you have been ignoring longest.
The clock starts when a session becomes your move, and survives an idle
notification, so nothing resets it but you.

Only a dialog that is waiting for you raises a `!`, and it appears when that
dialog opens. Claude Code also notifies when a session has merely gone quiet;
treating that as blocking was a false alarm, since the wilt already says it.

Every session gets its own colour from a palette of twelve, defined once in
the repo root's `/palette.json` and shared with `wt`. Planter picks new
automatic colours at random from colours that are not tied for most used by the
visible plants. If all twelve are tied, all twelve are eligible. The choice is
remembered for that session's lifetime, including overlay restarts. A launcher
that already knows better can set
`PLANTER_COLOR`, `PLANTER_LABEL`, and
`PLANTER_TAB_INDEX` on the agent process to choose that session's colour,
name, and place in the row directly, and can rewrite that place later if its
tabs move — see "How it works" below. `wt` is one such launcher: it sets
these directly, and no longer sends Claude Code a `/color` command.

macOS only — it draws with AppKit.

Built against Claude Code 2.1.220. Most of it needs only long-standing hooks, but
four features depend on newer events: the `!` needs `PermissionRequest`, side buds
need `SubagentStart`, labels follow `CwdChanged`, and a failed call clears the `!`
via `PostToolUseFailure`. On an older build those hooks simply never fire, and the
rest still works — check `install.sh`'s output against your own
`claude --version` if a feature seems missing.

## Install

```sh
git clone git@github.com:josh-sola/jrepo.git
cd jrepo/claude-planter
./install.sh
planter                      # start the overlay
./login-item.sh install      # and/or start it at every login
```

Keep the clone: `install.sh` builds from it, and it is where you upgrade from.

`install.sh` builds `~/.local/bin/planter` and
`~/.local/bin/planter-codex-bridge`, installs a hook at
`~/.claude/hooks/planter-state`, and wires twelve hooks into
`~/.claude/settings.json` — printing exactly which, and backing the file up
first. It needs `swiftc` (the Xcode command line tools) and `jq`. Re-running
replaces its own hook entries rather than stacking them.

To upgrade, from `jrepo/claude-planter`:

```sh
git pull && ./install.sh && ./login-item.sh install
```

Claude Code re-reads its settings, so sessions that are already open get a plant
on their next turn. Restart any session whose plant never shows up.

To remove everything:

```sh
./install.sh --uninstall     # hooks, hook script, binary, login item
rm -rf ~/.claude/planter     # state too, if you want it gone
```

## Using it

- **Drag** a plant to move the row. The position is remembered, pinned by the
  row's **right edge**, so a new session widens it leftwards and an overlay parked
  at the right-hand end of a screen stays where you put it.
- **⌘-drag** a plant to reorder it. Plants swap as the cursor crosses each
  neighbour, and the order is remembered per session.
- **Right-click** for labels, **Reset order**, and **Quit**. The label choice is
  remembered, so hiding them once is enough.
- Clicks between plants pass straight through to the window underneath.
- The row hides itself when no sessions are live.

```
planter --list          print the live sessions as text
planter --demo          run with four fake plants
planter --preview FILE  render every frame and colour to a PNG
planter --scale N       pixel size, default 3
planter --no-labels     hide the labels, and pack the plants tighter
planter --pack NAME     draw with a pack from ~/.config/planter/NAME
```

Reordering never changes a plant's colour. Automatic choices are remembered, and
new plants are assigned in creation order before the saved order is applied.

## Labels

Each plant is labelled with its session's directory, capped at 12 characters so
one long name cannot push the row out of line. `--no-labels` drops them and packs
the plants closer together.

A directory basename is a poor label if your worktrees are named after a uuid or
a ticket id. If `~/.claude/planter/label-hook` exists and is executable, it is
handed the session's directory, and whatever it prints becomes the label. It runs
once per session, so it can afford to be slow. A `PLANTER_LABEL` set on the
agent process wins over both and skips running the hook. Two examples
ship in `examples/`:

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

Claude hooks and the Codex and Pi integrations write one small JSON file per
session into `~/.claude/planter/`, and the overlay watches that directory.

Claude's hooks map events as follows:

| Hook | Effect on the plant |
| --- | --- |
| `SessionStart` | appears, wilted |
| `CwdChanged` | takes the new directory's name |
| `UserPromptSubmit` | blooms |
| `PostToolUse` / `PostToolUseFailure` | keeps blooming — this is what clears a `!` after you answer |
| `PermissionRequest` | shows `!` — a dialog is on screen |
| `Notification` | shows `!` too, unless it is only an idle nudge |
| `Stop` / `StopFailure` | wilts, **unless** a subagent is still running |
| `SubagentStart` / `SubagentStop` | counts running agents up and down |
| `SessionEnd` | disappears |

Four details do most of the work:

**A blocking subagent needs no counting.** `Stop` only fires when the main turn
ends, and a synchronous `Agent` call holds that turn open. The counter exists for
*background* agents, where the turn ends while work continues — the plant keeps
flowering until the last one finishes.

**A stuck subagent tally cannot last.** An interrupted agent's `SubagentStop`
never arrives, and nothing on disk says which agents are alive, so the tally would
stick above zero — leaving a plant blooming forever while it waits for you. It is
bounded two ways: your next prompt clears it, and it expires after 30 minutes
without news. The cost is that a background agent outliving a prompt loses its bud,
which is worth much less than a plant that never wilts again.

**Crashed sessions clean themselves up.** Each file records the pid of its agent
process. The overlay drops any plant whose process is gone, so a session killed
without a final lifecycle event never leaves one behind.

**Labels come from `CwdChanged`, not from every event.** The `cwd` on an ordinary
hook payload follows the session's *shell*, which wanders into subdirectories and
temporary agent worktrees as work goes on, so a label reading it would rename
itself constantly. `CwdChanged` fires only on a real directory change, and a
subagent's own changes carry the subagent's session id, so they cannot rename the
session you are watching.

**Wilting advances on its own.** The overlay works the stage out from a timestamp
each time it re-reads the directory, so a plant keeps wilting without any hook
firing — which matters, because a session waiting for you fires nothing at all.

**A `!` needs `PermissionRequest`, not `Notification`.** Claude Code holds
notifications back for a few seconds, so a dialog you answer quickly never
produces one — which meant the `!` almost never appeared. `PermissionRequest`
fires the moment the dialog does. The hook writes nothing to stdout and always
exits 0, so it never influences the permission decision itself.

**`planter-state` runs on every tool call**, so the common case — a tool
finishing in a session that is already blooming — is answered by one `grep` and
nothing else, about 10ms. If you would rather not pay it, delete the `PostToolUse`
entry from `~/.claude/settings.json`; all you lose is that a `!` then stays up
until the turn ends, instead of clearing when you approve.

A launcher can set three environment variables on an agent process. Each
provider integration reads them from the environment rather than remembering
them, so nothing can drift:
`PLANTER_COLOR` (one of `red`, `orange`, `yellow`, `lime`, `green`, `teal`,
`cyan`, `blue`, `indigo`, `purple`, `magenta`, `pink` — the twelve names in
`/palette.json`) fixes its colour exactly, even if another plant already has
it. Automatic colours are chosen randomly from palette slots below the current
largest usage count. If all twelve slots are tied, any slot may be chosen. The
automatic choice stays with the session until it ends,
`PLANTER_LABEL` fixes its name instead of the label-hook or directory basename,
and `PLANTER_TAB_INDEX` (a 1-based position) sets its place in the row instead
of sorting by creation time. An unset colour gets an automatic choice. Unset
labels and positions use their normal defaults.

`PLANTER_TAB_INDEX` is the exception: it only sets the position when the plant
is created, because a tab index goes stale the moment tabs move. After
creation the `tab` field in the state file is the record. A launcher that moves
tabs can rewrite that field in another live session's file, and the hook
carries the new value forward instead of stamping the old one back.

Set `PLANTER_STATE_DIR` to move the state directory. It takes precedence over
`CLAUDE_PLANTER_DIR`, which remains supported for existing Claude setups. Both
the hook and overlay respect either setting, which is also how to try things out
without touching your real sessions.

## Codex app-server sessions

When Planter is enabled in `wt`, `wt go --codex` starts interactive Codex
sessions through `planter-codex-bridge`. The bridge starts
`codex app-server --listen stdio://`, gives the Codex TUI a private Unix-socket
endpoint, and writes the same small live-state record as the Claude hook. It
does not use Codex hooks and does not store prompts, replies, tool input, or
credentials.

Install this repository so the bridge is on `PATH`, then use `wt go --codex`.
Direct `wt llm codex` commands, explicit Codex `--remote` sessions, and
non-interactive or administrative Codex commands remain direct. If the bridge
cannot start, `wt` prints a warning and starts Codex directly.

The bridge inherits `PLANTER_STATE_DIR` (or the legacy
`CLAUDE_PLANTER_DIR`) and removes its state record when the TUI, owner process,
or app server exits. Codex initially uses `PLANTER_TAB_INDEX`, then adopts a
valid `tab` change made in its state file by launchers such as `wt`. Codex
currently marks the [app-server interface as experimental](https://learn.chatgpt.com/docs/app-server),
so this integration stays isolated behind the bridge and direct-launch
fallback.

## Pi sessions

Pi support ships through the standalone
[jpi-planter](https://github.com/josh-sola/jpi-planter) plugin. Install both
Planter and `jpi-planter`, then start Pi directly or through `wt`:

```sh
pi install git:github.com/josh-sola/jpi-planter
wt go                         # Pi is the default
wt llm pi "fix login"         # direct launch in a selected tree
```

The `jpi-planter` extension writes the same provider-neutral state records as
the Claude hook and Codex bridge. It tracks Pi's main agent, optional
`pi-subagents` agents, and `pi-background-tasks` work. The
`ask_user_question` integration raises `!` while its questionnaire is waiting
for you. Pi does not expose one general event for every possible permission
prompt, so prompts from integrations without a matching public event remain in
their normal working state.

A Pi process launched by `wt go` inherits `PLANTER_COLOR`, `PLANTER_LABEL`, and
`PLANTER_TAB_INDEX`, so its plant shares the same colour and tab ordering as the
other supported agents. Direct Pi sessions still appear when `jpi-planter` is
loaded;
they use the normal automatic colour, label, and ordering rules.

## Icon packs

The plants are the art that ships, not the only art. A pack is a directory that
replaces them wholesale, and nothing in it has to be a plant.

```sh
mkdir -p ~/.config/planter/candles
# write pack.conf and the frame files
planter --pack candles
```

`--pack` is for one run. To keep a pack, name it in
`~/.claude/planter/prefs.json`:

```json
{"pack": "candles"}
```

Packs live in `$XDG_CONFIG_HOME/planter/<name>/`, or `~/.config/planter/<name>/`
when that variable is unset. One pack draws the whole row.

There is a worked one in `examples/packs/lavalamp` — a row of lava lamps, at
four times the resolution of the plants. Copy it in and run it to see what the
format can do:

```sh
cp -r examples/packs/lavalamp ~/.config/planter/lavalamp
planter --pack lavalamp
```

### pack.conf

The grid size, and a colour for each character used in the art.

```
size = 20 16

# glyph   hue           saturation  brightness  [alpha]
f       = session       0.66        1.00
F       = session+0.5   0.84        0.80
s       = 0.08          0.52        0.34
w       = 0.00          0.00        1.00        0.8
```

The hue field is where a pack decides which pixels carry session identity.
`session` takes the plant's own hue, so that pixel is a different colour in every
session. `session+0.3` or `session-0.25` tracks that hue at a fixed distance,
for an accent that stays in relation to it. A plain number is a fixed hue, the
same in every plant.

Saturation, brightness and alpha run 0 to 1, and alpha defaults to 1. `#` starts
a comment, except when the line is the palette entry for the `#` glyph itself
(`# = …`). `.` is always transparent and cannot be given a colour.

An optional `scale` says how many points one of your pixels is drawn at, and it
is how a pack changes its resolution without changing its size on screen. A pack
drawn at twice the detail asks for half the pixel size:

```
size  = 40 32
scale = 2
```

Leave it out and the plants are drawn at the default 3, same as the built-in art.
`--scale` still overrides it for a run, so you can blow a pack up to look at it.
Whole numbers are safest: on a display that isn't Retina, a fractional scale
lands pixels on half-points and the art comes out visibly uneven.

By default every plant shows the same frame at the same moment, so motion across
the whole row reads as one thing. An optional `phase` breaks them apart:

```
phase = scatter
```

Each plant then starts its cycle at its own point in the frames, taken from its
session id, so it keeps that point for the session's whole life instead of
jumping when a neighbour appears. Scatter art that has no reason to agree — a row
of lava lamps in lockstep looks like one lamp copied — and leave it out for art
whose animation is the message, like the plants' sway.

Nothing here knows what your art depicts, which is the point. The built-in pack
puts `session` on the petals and the pot and leaves the leaves green, but a
candle pack can put `session` on the flame and fix the wax instead.

### Frame files

Nine poses, one `.txt` file each. The names are the states the hook writes:

```
working-0     working-1     working-2      0, 1, 2-or-more subagents
waiting-0     waiting-1     waiting-2      fresh, past 2 minutes, past 10
attention-0   attention-1   attention-2    blocked, at each of those ages
```

The three level-`0` poses are required. Levels `1` and `2` fall back to the `0`
of their own state, so a pack that doesn't count subagents, or doesn't want to
grade its wilt, can ship three poses instead of nine. Nothing stands in for a
missing level `0`: a pack has to say what blocked looks like.

Planter decides which pose to draw and when — a pack supplies the art and
nothing else, so it cannot add a fourth wilt stage or raise the two-bud cap.

A pose is one static frame at `waiting-0.txt`, or several at `waiting-0-a.txt`,
`waiting-0-b.txt`, and so on, cycled in filename order. Any pose can animate;
the built-in pack animates only `working`, so that motion anywhere in the row
means work is happening. Shipping both a bare and a suffixed file for the same
pose is an error rather than a guess.

Frames are one character per pixel, top row first. A row shorter than the
declared width pads with transparent pixels; a longer one is an error. The row
count must equal the declared height exactly.

Every plant's cell is as wide as the widest ink any of its poses draws, pooled
across every frame, so the row never shifts when a plant changes pose. The cost
is that art sticking out to one side in a single pose — an alert glyph beside the
plant, say — holds every plant in the row apart, including all the ones not
drawing it. Keep every pose inside the same columns.

### When a pack is wrong

A pack is used whole or not at all. If any of these fails, planter draws the
built-in art and prints one line saying which check it was:

- `pack.conf` missing, unparseable, or without a valid `size`
- a palette entry for `.`
- any level-`0` pose missing, or both a bare and a suffixed file for one
- a frame file no pose uses, which is how a misspelt name is caught
- a frame with the wrong number of rows, or a row past the declared width
- a glyph in the art with no palette entry

```
planter: pack "candles" is missing pose attention-2 — using built-in
```

A half-loaded pack never reaches the screen, so a typo costs you the pack for
that run, never a broken row.

### Iterating

`--preview` renders a contact sheet of whichever pack is active: the main poses
across all twelve hues, twice, on a dark ground and a light one, so you can see
whether the art holds up against either desktop.

```sh
planter --pack candles --preview /tmp/candles.png && open /tmp/candles.png
```

The sheet covers the poses that change most while you draw. `attention-1` and
`attention-2` are not on it, so check those in the overlay itself.

## Editing the art

To change what ships rather than layer your own on top: the sprites are
character grids in `Sprites.swift`, one character per pixel,
stacked as layers — a pot, a plant, sometimes a glyph — so the pot stays put
while the plant above it sways. `f`/`F` petals and `p` pot are drawn in each
plant's hue; `g`/`G` leaves and `s` soil stay green and brown, which is what keeps
it looking like a plant.

Layout measures the art rather than assuming it, so if you redraw the plant wider
or narrower the spacing follows. After an edit:

```sh
swiftc -O -o planter main.swift Planter.swift Palette.swift Sprites.swift
./planter --preview /tmp/preview.png && open /tmp/preview.png
```
