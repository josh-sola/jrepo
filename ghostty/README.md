# Ghostty

This directory tracks these custom Ghostty shaders:

- `shaders/balatro_bg.glsl`
- `shaders/cursor_smear.glsl`
- `shaders/cursor_smear_bg.glsl` (inactive)

The active Ghostty configuration uses:

```ini
custom-shader = shaders/balatro_bg.glsl
custom-shader = shaders/cursor_smear.glsl
```

After merging the shaders, sync the primary checkout at
`/Users/joshbassin/repos/jrepo` to `main`. The installer refuses linked `wt`
worktrees and uncommitted shader sources.

Run the repository installer to install symlinks into Ghostty's macOS shader
directory:

```sh
cd /Users/joshbassin/repos/jrepo
just install
```

Ghostty's shader links use absolute paths, and `wt` trees are disposable, so
rerun the installer from this long-lived checkout after merging shader changes.
