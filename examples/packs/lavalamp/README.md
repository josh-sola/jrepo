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

Two blobs circulate at opposite phases. Their rise and fall is eased so they
crawl at the wide bottom of the glass and run at the narrow top, which is also
what drops the point where they pass each other down into the lower half. Lava
collects in a pool under the cap and in a deeper one above the collar.

Waiting drains both pools and drops the pale core, so a still frame of waiting
never looks like a still frame of working. That matters because the three wilt
stages are the same art at three depths, and motion is the only other cue.

## Why so many files

The three `working-*` levels are byte-identical. This pack spends the whole
glass on one signal — working or not — rather than showing how many subagents
are running, but a pack has to supply all nine poses, so the same twenty frames
are stored three times.
