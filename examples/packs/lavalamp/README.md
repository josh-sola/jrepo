# lavalamp

A worked example of the pack format: a row of lava lamps instead of plants.

```sh
cp -r examples/packs/lavalamp ~/.config/planter/lavalamp
planter --pack lavalamp
```

To keep it, put `{"pack": "lavalamp"}` in `~/.claude/planter/prefs.json`.

## What it shows

The art is 40x32 with `scale = 2`, so it carries four times the detail of the
built-in plants while drawing about the same size on screen. Whole-number scales
only, unless you know the row lands on a Retina display.

`phase = scatter` starts each lamp at its own frame, so a row of them never
circulates as one. Nothing about a lamp says it should agree with the lamp beside
it, and in lockstep they read as one lamp drawn several times.

Two blobs circulate at opposite phases. Their rise and fall is eased so they
crawl at the wide bottom of the glass and run at the narrow top, which is also
what drops the point where they pass each other down into the lower half. Lava
collects in a pool under the cap and in a deeper one above the collar.

Waiting drains both pools and drops the pale core, so a still frame of waiting
never looks like a still frame of working. That matters because the three wilt
stages are the same art at three depths, and motion is the only other cue.

Blocked pours all the lava into one glowing `!` and leaves the glass otherwise
empty. Hanging the `!` outside the lamp would have been easier to draw, but every
lamp in the row reserves the width of the widest thing any of its poses draws, so
one `!` sticking out to the side held all of them apart even when none was
alerting. The three ages cool it instead of draining it: hot core, then dull,
then barely lit. Same direction the pools drain in, so ignoring a lamp always
makes it look colder.

## Why there is no working-1 or working-2

This pack spends the whole glass on one signal — working or not — rather than
showing how many subagents are running. Those two poses fall back to
`working-0`, so leaving them out draws the same lamp whatever is running.
