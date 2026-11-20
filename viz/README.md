# viz

`viz` is a terminal-native visualization engine. Agents author declarative KDL v2
(or JSON) documents combining graphs, charts, custom SVGs, code, markdown, stats,
tables, and live log streams, then open them in a Rex floating layer or split and
stream updates live.

`viz guide` prints this file, and `viz --help` points agents to it.

## Install & Upgrade

```sh
curl -fsSL https://raw.githubusercontent.com/josh-sola/jrepo/main/viz/install.sh | sh
```

This installs the newest release on macOS or Linux into `~/.local/bin/viz`. Put
`VIZ_VERSION=0.1.0` in front of `sh` to pick a version.

```sh
viz upgrade          # install the newest release
viz upgrade --check  # only report the installed and latest versions
```

Once a day, `viz` checks GitHub for a newer release in the background. If it
finds one, the next command prints one line on stderr, at most once a day. If
you are an agent, tell the user about it; do not run `viz upgrade` yourself.
Set `VIZ_NO_UPDATE_CHECK=1` to disable the check (also off when `CI` is set).

## Commands

Create a session and open it in Rex (prints the session ID on stdout):

```sh
ID=$(viz open examples/walkthrough.viz.kdl)
ID=$(viz open --split right --title "Deploy" < layout.viz.kdl)
ID=$(viz open --bounds 0.18,0.08,0.74,0.80 --theme dark --focus layout.viz.kdl)
ID=$(viz open --no-rex layout.viz.kdl)
```

View a static document or follow an existing session in the current terminal:

```sh
viz view examples/walkthrough.viz.kdl
viz view --follow "$ID" --theme auto
```

Patch a live session with convenience flags or KDL event ops on stdin:

```sh
viz patch "$ID" --node-status arch:auth=warn --stat kpis:rps=2.4k --set-state selected=auth
viz patch "$ID" --step 1
viz patch "$ID" <<'EOF'
set-edge graph="arch" from="gw" to="auth" active=#true label="gRPC"
chart-append target="latency" series="p99" x-label="12:05" max-points=60 {
    point 41.2
}
EOF
```

Stream log lines into a log pane:

```sh
viz log "$ID" events --level warn --source auth "retry budget low"
tail -f app.log | viz log "$ID" events --level info --source app
```

Inspect, export, import, self-render, validate, and close sessions:

```sh
viz ls
viz ls --json
viz export "$ID" snapshot.viz.kdl
viz export "$ID" --json
viz chart-from bench.csv --type hbar --id bench --unit seconds --scale build-sec --value-labels
viz render examples/walkthrough.viz.kdl --png /tmp/step1.png --step 1 --theme light
viz check examples/walkthrough.viz.kdl
viz close "$ID"
```

## Where data lives

`$VIZ_DIR` overrides the session directory. Without it, `viz` stores sessions in
`~/.local/share/viz/<slug>/` with `meta.json` and framed append-only `events.kdl`.

## Keyboard and mouse controls

- `q` / `Ctrl-C`: quit
- `←` / `→` or `p` / `n`: step through walkthrough steps
- `Tab` / `Shift-Tab` (or click a pane): focus pane
- `z` (and `Esc` to exit): zoom / unzoom focused pane
- `w`: toggle soft-wrap on code panes (on by default; when off, use `h`/`l` or `H`/`L` to scroll horizontally)
- `j` / `k`, arrows, `PgUp` / `PgDn`, `J` / `K` (half-page), `g` / `G` (top/end): scroll focused pane or `scroll` page (`G` also resumes log tail)
- `Enter`: select highlighted table row
- `[` / `]`: switch tabs
- `t`: toggle Light / Dark theme live
- `/`: filter log panes by substring (`Enter` apply, `Esc` clear)
- `0`–`5`: minimum log level (`0` all, `1` trace, `2` debug, `3` info, `4` warn, `5` error)
- Mouse wheel scrolls the pane or `scroll` page under the cursor; hovering over graphs, charts, and custom SVGs shows an interactive tooltip card; clicking selects the element and updates `by-selected` panes.

## KDL v2 document schema

Every document starts with a single `viz v=1 title="..."` node (with optional
`subtitle`, preferred floating window fractions `width=0.15..0.98` and
`height=0.15..0.98`, optional `state`, optional `steps`, and one root layout or
primitive node). Horizontal `split` nodes automatically stack vertically when
the narrowest child column would drop below 42 columns.

```kdl
viz v=1 title="Request Flow" subtitle="Production edge" {
    state selected="gw" highlight="gw,auth" code-focus="12..24"
    steps {
        step "Edge Ingress" selected="gw" highlight="gw" code-focus="1..8" {
            markdown "TLS terminates at **Gateway**."
        }
    }
    split direction="horizontal" weights="3,2" {
        // Any of the primitives below (or use `scroll id="report" { ... }` for report pages)
    }
}
```

### Primitives

1. `split` — `direction="horizontal"|"vertical"`, `ratio=0.6` or `weights="2,1"`
2. `scroll` — `id="..."` vertical-flow report container; children render at natural height (or explicit `height=N`) with partial-viewport clipping and scroll indicator
3. `tabs` — `id="..."` containing `tab "Label" { <node> }`
4. `markdown` — `id`, `title`, `tone="ok"|"warn"|"error"|"active"|"idle"|"muted"` (colored callout box), `height`, inline body or `{ content """..."""; by-selected { case "id" """...""" } }` (borderless prose inside `scroll` when `title` and `tone` are omitted)
5. `code` — `id`, `title`, `lang` (inferred from `path` extension when omitted), `path="src/file.rs"`, `lines="1..30"`, `height`, `start-line`, `highlight-lines="1..4,9..12"`, `focus-key`, `{ source #"""..."""#; by-selected { case "id" path="src/other.rs" lines="10..25" } }` (`viz export` inlines external files so snapshots stay self-contained)
6. `stat` — `id`, `title`, `height`, `{ item "id" label="RPS" value="1.4k" delta="+8%" status="ok" sparkline="10,14,22" }`
7. `table` — `id`, `title`, `height`, `{ columns "Col1" "Col2"; row id="r1" status="ok" "a" "b" }` (uses content-aware water-fill column widths and falls back to stacked cards when narrow)
8. `log` — `id`, `title`, `height`, `{ entry ts="12:00:01" level="info" source="api" "Ready" }`
9. `graph` — `id`, `title`, `caption`, `height`, `direction="lr"|"tb"`, `highlight-key`, `select-key`, `{ node "id" label="Name" sublabel="sub" status="ok" group="edge" shape="rounded"|"box"|"diamond"|"pill" tooltip="..."; edge "from" "to" label="gRPC" style="solid"|"dashed" active=#true tooltip="..." }` (when `highlight` is non-empty, non-lit nodes/edges/groups dim automatically; `highlight` tokens match node IDs, group names, or `"from->to"` edge specs)
10. `chart` — `id`, `title`, `caption`, `height`, `type="line"|"area"|"bar"|"hbar"|"scatter"`, `x-labels="t0,t1"`, `x-unit="seconds"`, `y-unit="ms"`, `value-labels=#true`, `scale="shared-group"`, `{ series "p99" color="#60A5FA" { point 12.5; point x=1.0 y=18.0 } }` (or `data="12.5,18.0"`)
11. `svg` — `id`, `title`, `caption`, `height`, `select-key`, `highlight-key`, `{ source #"""<svg ...>...</svg>"""#; tooltip "elem-id" "Hover text" }`

Status enum: `ok`, `warn`, `error`, `active`, `idle`, `muted`.
Booleans in KDL v2 use `#true` / `#false` (KDL v1 `true` / `false` is also accepted).

### Semantic live event ops (`viz patch`)

- `set-state selected="auth" highlight="auth,db" code-focus="10..20"`
- `set-node graph="arch" node="auth" label="Auth" sublabel="v2" status="warn" tooltip="..."`
- `set-edge graph="arch" from="gw" to="auth" label="gRPC" active=#true tooltip="..."`
- `set-stat stat="kpis" item="rps" value="2.1k" delta="+12%" status="ok" sparkline-push=44.0`
- `log-append target="events" { entry ts="12:01:00" level="error" source="auth" "timeout" }`
- `chart-append target="lat" series="p99" x-label="t4" max-points=60 { point 32.1 }`
- `replace target="notes" { markdown id="notes" "Updated body" }`
- `close`

### Theme-aware SVG tokens

Custom `svg` blocks can reference CSS variables resolved against the active Light
or Dark palette before rasterization:

- `var(--bg)`, `var(--surface)`, `var(--surface-alt)`, `var(--border)`
- `var(--fg)`, `var(--muted)`, `var(--accent)`, `var(--selection)`, `var(--highlight)`
- `var(--ok)`, `var(--warn)`, `var(--error)`, `var(--active)`, `var(--idle)`

Any SVG element with an `id` attribute becomes hoverable and selectable; `<title>`
children supply default tooltips, which KDL `tooltip "id" "..."` overrides.
