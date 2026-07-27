# `wt` — enriched worktree tooling

**Goal.** Make a fully working checkout of a large repo cheap enough to create that
starting fresh work is never a reason to hesitate. One base clone per repo, kept at
`origin/master`; all work in disposable trees that arrive with dependencies installed,
`.env` files in place, and plans already wired up.

Last updated: 2026-07-27 (Phases 1 through 3 built, reviewed, and verified live)
Base monorepo `master`: `f0b4214c56`; all timings measured at `1e7ef3083f`
Measured against: pnpm 10.33.0, node 24.17.0, uv 0.11.30, rustc 1.96.0

Every timing in this plan was measured on this machine on 2026-07-27. If master has
moved a long way, re-measure before trusting the numbers.

---

## Locked decisions

Each decision names the fact that forces it.

| # | Decision | Forced by |
|---|---|---|
| 1 | **No warm pool.** Provision on demand; return the path immediately and finish in the background. | Full provisioning is ~44s. A pool must be invalidated whenever the lockfile moves, which is often. The cost of the state is larger than the 44s it saves. |
| 2 | **Existing worktrees stay where they are.** `wt` is for new work only. | Moving a tree breaks it: `node_modules/.bin/*` shims bake an absolute `NODE_PATH`, and every `.venv/bin/*` console script hardcodes its own absolute path. Both were verified. |
| 3 | **`wt` supersedes Claude Code's worktree isolation.** | Isolation copies `.worktreeinclude` paths but does not install dependencies — verified: in one `agent-*` tree `node_modules` was born four days after the worktree. Agents currently get a tree that cannot build. |
| 4 | **`plans/` is a plain directory, not a git repo.** | User decision. |
| 5 | **Shared gitignored state is symlinked, not copied** — `plans/`, `local/`, `user-memories/`, in base as well as in every tree. | User decision. Symlinks give one truth and survive teardown, which is the point of moving plans out of trees; base needs the same symlink so a plan written from a base session is visible everywhere else. |
| 6 | **Base stays fresh two ways:** a LaunchAgent every few minutes, plus a fetch on `wt new`. | User decision. The `wt new` fetch is cheap precisely because the timer keeps base nearly current. |
| 7 | **Always fresh `pnpm install`; never clone `node_modules`.** | Measured: install is 23s, `cp -Rc` of `node_modules` is 42–69s and still needs a repair pass because the `.bin` shims carry absolute paths. Cloning is both slower and wrong. |
| 8 | **Shared `CARGO_TARGET_DIR` per repo.** | Measured: cuts `dspy-worker`'s `uv sync` from ~19s to 8.4s, and saves ~400 MB of real disk per tree. Cargo output is the only artifact here that is neither hardlinked nor reflinked from a shared store. |
| 9 | **Rust, single binary.** | The statusline re-renders every 10s and hooks fire on every session start. A Python CLI costs ~100–200ms per invocation; Rust costs ~2ms, so the statusline can call `wt` directly instead of parsing `data.json` with `jq`. |
| 10 | **Base is protected by reversibility, not locks.** | Read-only permissions or `chflags uchg` would break the fetch-and-fast-forward that keeps base current. Only commits can be hard-blocked, via hooks. |
| 11 | **Provisioning steps and the carry-over list are per-repo data in `data.json`,** seeded from `.worktreeinclude` when the repo has one. | helm and toy-apps have no `.worktreeinclude`, no pnpm lockfile, and no submodules, but do have their own provisioning needs (`helm dependency update`, per-app `uv sync`). Hardcoding monorepo steps would make them unsupportable. |
| 12 | **Shared symlink names go into base's `.git/info/exclude`, for every repo.** | A directory-only `.gitignore` pattern like `local/` does not match a symlink named `local` — git still reports `?? local`, even in the monorepo, which already ignores `local/` as a directory. `.worktreeinclude` parsing strips the trailing slash, so `info/exclude` gets `local`, which does match. `info/exclude` lives in the common git dir, so it is shared by every worktree and never committed. |
| 13 | **Base's original directories move to `<repo>/backup/`, not somewhere inside base.** | A backup left inside base (e.g. `local.pre-wt`) would not match the `.gitignore` pattern that covers the real name and would sit as untracked noise forever. Outside base it is inert and left for manual deletion once the shared copy is confirmed good. |
| 14 | **`wt rm`/`wt gc` tear a tree down with `fs::remove_dir_all` plus `git worktree prune` — never `git worktree remove`, never `git submodule deinit`.** | `git submodule deinit` writes `submodule.<name>.url`/`.active` into the *common* `.git/config`, shared by base and every worktree — confirmed live: one `wt rm` on a monorepo tree deregistered `helm` and `n8n` for base and all 24 pre-existing worktrees at once, repaired by hand with `git submodule init`. `git worktree remove` is what forces deinit in the first place (it refuses outright on a populated submodule), but `wt`'s own dirty/unpushed guards already run before teardown, so git's refusal was never buying safety — only risk. Removing the directory directly and pruning afterward reaches the same end state without ever touching shared config. |
| 15 | **The base commit block sets `core.hooksPath` at `--worktree` scope, and `wt new` immediately clears that same key in every new tree.** | `extensions.worktreeConfig` makes `core.hooksPath` overridable per-worktree, which is what lets base block commits without touching `.git/hooks` (dead under the monorepo's repo-wide `core.hooksPath=.husky`). But `git worktree add` copies the *main* worktree's `config.worktree` into every new linked worktree's admin dir — verified empirically — so without the clear step, every fresh tree would inherit base's block instead of the repo's real hooks path (Husky, for the monorepo) and could never commit either. |
| 16 | **`wt sync` requires a genuinely clean base — no allowance for a dirty submodule pointer.** | Base is supposed to stay clean by construction (decision 10). A special case for benign submodule drift only existed to tolerate a stale ` M helm` pointer that has since been reset by hand; with base actually clean, a plain `git status --porcelain` emptiness check is the whole rule, and a simple rule with one clear failure message beats a special case carried for a state that shouldn't recur. |

### `wt init` always adopts; it never clones

**Settled: every repo is adopted in place, and `~/repos/wt/<repo>/base` is a symlink to
the real checkout.** This holds for monorepo, helm, and toy-apps alike — helm has five
worktrees already attached and its own Graphite stacks, and toy-apps has `planhub/.venv`
whose console scripts hardcode their absolute path.

The layout you sketched puts base at `~/repos/wt/<repo>/base`. Getting there by
cloning fresh, or by moving the existing checkout, both break things:

- **A fresh clone splits Graphite.** Graphite keeps its stack database in the clone's
  git dir (`.git/.graphite_metadata.db`, `.graphite_repo_config`). A second clone means
  trees created by `wt` cannot see or stack on branches in your existing checkout.
- **Moving the checkout breaks absolute paths.** `git worktree repair` would fix the 24
  linked worktrees' pointers, but every `.venv` console script and every
  `node_modules/.bin` shim in the moved tree hardcodes the old path.

So: register `~/repos/monorepo` as monorepo's base where it sits, and put
`~/repos/wt/monorepo/base` there as a symlink so the documented layout still holds.
The consequence you need to accept is that **`~/repos/monorepo` stops being a working
checkout** — it goes back to `master` and stays there.

Migration is a one-liner, because nothing there needs rescuing: `josh/zod-stable-compat`
is pushed and level with `origin` (both at `472d456a6e`), so base just checks out
`master`. The only other local state is the drifted `helm` submodule pointer, which is
noise rather than work. `wt adopt` stays in the command set for accidental edits in base
later, but it is not part of migration.

---

## Layout

```
~/repos/wt/
├─ data.json                        registry (single file, atomic writes)
└─ <repo>/
   ├─ base/                         canonical clone, tracks origin/master; plans/, local/,
   │                                 user-memories/ inside it are symlinks into shared/,
   │                                 same as in a tree
   ├─ trees/<uuidv7>/               working copies
   ├─ shared/                       durable state symlinked into base and every tree
   │  ├─ plans/
   │  ├─ local/
   │  └─ user-memories/
   ├─ backup/                       base's original plans/, local/, user-memories/,
   │                                 moved here once when wt init first symlinks them —
   │                                 deleted by hand, never automatically
   └─ cache/
      └─ cargo-target/              shared CARGO_TARGET_DIR
```

`plans/` sits under `shared/` with the other two, because all three are the same thing:
durable gitignored state that every tree sees at the path the repo already expects.
Keeping `plans/` top-level would only make it visually prominent, and prominence is not
what makes an agent find it — the SessionStart hook injecting the path is.

Base gets the same symlinks as a tree, so a plan written from a session in base is not
stranded there. `wt init` moves base's real directories into `shared/` (copying content
in first, if there is any) and replaces them with symlinks. The originals land in
`backup/`, deliberately outside base: a backup left inside base would not match the
`.gitignore` pattern that covers the real name and would sit as untracked noise forever.
`wt init` prints the backup path; nothing deletes it automatically.

`cache/` stays separate. It is not repo state and is not symlinked into trees; it is
reached through `CARGO_TARGET_DIR`.

All three shared paths are gitignored in the monorepo (`plans/` at `.gitignore:102`,
`local/` at `:100`) and named in its tracked `.worktreeinclude`. **They are not
gitignored in helm or toy-apps** — see "Other repos" below.

---

## `data.json`

```json
{
  "version": 1,
  "repos": {
    "monorepo": {
      "base": "/Users/joshbassin/repos/monorepo",
      "trunk": "master",
      "lastFetch": "2026-07-27T18:04:11Z",
      "profiles": ["node", "python"]
    }
  },
  "trees": [
    {
      "id": "01991f2c-8a11-7c3e-b0d2-4f1a6c7e9d31",
      "repo": "monorepo",
      "name": "wt cli bootstrap",
      "branch": "josh/wt-cli",
      "path": "/Users/joshbassin/repos/wt/monorepo/trees/01991f2c-…",
      "created": "2026-07-27T18:04:11Z",
      "state": "ready",
      "provisioned": { "node": "ready", "python": "skipped" }
    }
  ]
}
```

`state` is one of `provisioning`, `ready`, `failed`. A tree is usable for reading as
soon as it exists; `state` tells you whether dependencies have landed.

**Writes are atomic and locked.** Parallel agents will call `wt new` at the same time,
so every mutation takes an advisory lock on a sibling lockfile, re-reads, mutates, then
writes to a temp file and renames. `recipes` writes its store with a plain
`write_text()` and no lock; that is fine for one human and not fine here.

**`data.json` is authoritative for names, git is authoritative for existence.** `wt
doctor` reconciles the two: registry entries whose path is gone, and worktrees git knows
about that the registry does not.

---

## Commands

| Command | Does |
|---|---|
| `wt init <repo> --adopt <path>` | Register an existing clone as base; create the layout and symlink `base`. |
| `wt new <repo> --name "short summary" [--branch <b>] [--profile node,python]` | Fetch if stale, create the tree, wire symlinks, copy env files, start provisioning, print the path. |
| `wt ls [--repo <r>] [--json]` | Registry with name, branch, state, dirty flag, ahead/behind. |
| `wt path <selector>` | Resolve a name, uuid prefix, or branch to a path. |
| `wt go <selector>` | Shell-function-backed `cd`. |
| `wt name [--path <p>]` | The statusline's lookup. Prints the nice name for a path, or nothing. |
| `wt status [selector]` | Provisioning state and per-step results. |
| `wt wait [selector]` | Block until `ready` or `failed`. For scripts and agents. |
| `wt claude [selector\|<repo>]` | Exec `claude` with cwd set; bare repo name means base. |
| `wt rm <selector>` | Tear down, guarding dirty and unpushed work. `--force` to override. |
| `wt gc [--repo <r>]` | Reap trees with no commits ahead of trunk and no dirty files. Replaces the auto-cleanup lost with decision 3. |
| `wt sync [<repo>]` | Fetch and fast-forward base; refuse if base is dirty. |
| `wt adopt [<repo>]` | Move uncommitted work out of base into a fresh tree. |
| `wt env refresh <selector>` | Re-copy the repo's `copy` globs from base into a tree, overwriting what is there. |
| `wt doctor` | Reconcile registry against `git worktree list`; report and offer to fix. |

`wt env refresh` deliberately does **not** run `internal-cli config generate-env`, as an
earlier draft of this plan said it would. That command is monorepo-specific and needs AWS
auth and network, so it cannot be a repo-agnostic step. Regenerating env files in base
stays a manual step; refresh pushes base's current copies out to a tree.

Selectors resolve in one order everywhere: exact uuid, uuid prefix, exact name, unique
name substring, branch name. Ambiguity is an error listing the candidates, never a
guess.

---

## Provisioning pipeline

Measured cost per step, monorepo, warm caches (a second tree onward — the
first tree against a repo pays a cold shared `CARGO_TARGET_DIR` and measured
57s end to end, almost entirely in `dspy-worker`'s `uv sync` building
`fast-vnc-core` from nothing instead of the ~8.4s warm figure below):

| Step | Cost | Notes |
|---|---|---|
| `git worktree add trees/<uuid> -b <branch> origin/master` | 1.9s | |
| Symlink `plans/`, `local/`, `user-memories/` | instant | |
| Copy `**/.env*` from base | 0.13s | ~12 files. Copy, do not regenerate: `internal-cli config generate-env` needs AWS auth and network. The file list comes from `git ls-files --others --ignored --exclude-standard --directory`, never a filesystem walk — walking the base tree would cross `.turbo`, five `.venv` dirs, and the cargo target dir to find twelve files. Dropping `--directory` costs 3.36s and 298,630 entries to gain one stray `.env` vendored inside a dependency. |
| **— path printed here —** | | Everything above is under 2s. The agent can start reading. |
| `git submodule update --init --recursive` | 9.5s | `helm` and `n8n`. |
| `pnpm install --frozen-lockfile` | 23.0s | Warm store; adds ~14 MB of real disk because packages hardlink from the store. |
| `pnpm build:packages` | ~1s | Effectively free off the Vercel remote cache; 6.7s genuinely cold. |
| `uv sync --all-packages` at `python/` | 0.7s | **Must** be `--all-packages`; a plain `uv sync` produces a useless 68 KB venv because `python/pyproject.toml` declares no dependencies. |
| `uv sync` in datahub, data-processing-worker, scripts | 2.0s total | |
| `uv sync` in dspy-worker | 8.4s | With shared `CARGO_TARGET_DIR`; ~19s without. It builds `fast-vnc`'s Rust extension, an editable path dependency. |

**~44s total warm, ~57s cold on the first tree, ~2s to a usable path either way.**
Steps after the path is printed run in a detached child so the parent can exit;
progress (state, current step, index/total, log path) lands in `data.json` and a
per-tree log.

Profiles keep the long pole optional: `--profile node` skips Python entirely,
`--profile node,python` is the default, and `dspy-worker` is the one project worth a
separate opt-out if 8.4s ever matters.

Failure policy: a failed step marks `state: failed`, records which step and where the
log is, and leaves the tree in place. `wt new` never half-deletes a tree on failure.

---

## Other repos: helm and toy-apps

**Not out of the box.** The git half works anywhere; the provisioning and shared-state
halves are monorepo-shaped and need generalising. I checked both repos:

| | helm | toy-apps |
|---|---|---|
| `package.json` / pnpm lockfile | none | none |
| Submodules | none | none |
| `.worktreeinclude` | none | none |
| `plans/`, `local/`, `user-memories/` gitignored | **no** | **no** |
| Gitignored state that provisioning must produce | chart dependencies: `charts/*/charts/`, `Chart.lock` | per-app Python venvs: `planhub/.venv`, plus mypy/ruff/pytest caches |

Three gaps, each with a fix that also makes the monorepo path cleaner:

**1. Provisioning must be per-repo configuration, not monorepo logic.** Neither repo
wants `pnpm install`. helm wants `helm dependency update`; toy-apps wants `uv sync` per
app. `wt init` detects what a repo needs and records an ordered step list in
`data.json`; `wt new` runs that list. The monorepo's steps become data, not code, which
also makes them easy to change without a rebuild.

**2. The carry-over manifest cannot depend on `.worktreeinclude`.** Only the monorepo has
one. So the per-repo config holds the carry-over list, seeded *from* `.worktreeinclude`
when the file exists so the monorepo keeps tracking the team's shared contract, and set
explicitly otherwise. For helm and toy-apps the list starts empty.

**3. `info/exclude` is required for every repo, not only ones without a matching
`.gitignore` entry.** In helm or toy-apps a `plans` symlink shows up as untracked because
nothing ignores it. But even in the monorepo, which already lists `plans/` and `local/`
as directories, a symlink of the same name is not covered — a directory-only pattern does
not match a symlink, so git still reports `?? local`. The fix needs no repo change either
way: `wt init` appends the shared names (trailing slash stripped, which is what makes the
match work) to the base clone's `.git/info/exclude`. That file lives in the common git
dir, so — verified — it is shared by every worktree of the clone automatically, and it is
never committed or pushed.

With those three, helm and toy-apps are nearly instant to provision, since neither has a
dependency tree worth installing. They are a good first test of the general path precisely
because they are so much cheaper than the monorepo.

## Integrations

**Statusline.** `statusline.sh` gains one call: `wt name --path "$PWD"`, falling back to
the current basename when it prints nothing. Worktree detection already keys off
`git rev-parse --absolute-git-dir` matching `*/worktrees/*`, so `~/repos/wt/<repo>/trees/<uuid>`
is detected with no change. The per-tree colour keeps hashing `basename(toplevel)`,
which is now the uuid — stable per tree, which is what it needs to be.

**Hooks. Built.** One `SessionStart`/`CwdChanged` hook, registered by patching
`~/.claude/settings.json` the way `recipes/install.sh` does. In a tree it surfaces the
tree's name, its branch, and the plans path, so an agent never has to be told where
plans live. In base it surfaces that base stays on trunk and `wt new` is how to start
work. `hooks/session-context.sh` delegates the resolution to a hidden `wt
__session-context --path <p>` subcommand and only formats the result as hook JSON.

Discovered live: `CwdChanged` cannot inject model context — `hookSpecificOutput
.additionalContext` is `SessionStart`-only; `CwdChanged` only supports a user-facing
`systemMessage`. The hook branches on the incoming `hook_event_name` and sends the same
text either way, but a mid-session directory change only ever reaches the user, not the
model, until Claude Code adds context injection for `CwdChanged`.

This replaces `~/.claude/hooks/worktree-setup.sh`, which was orphaned — the script
existed but nothing in `settings.json` referenced it.

**Base commit block. Built.** `wt init` sets a `--worktree`-scoped `core.hooksPath` on
base pointing at generated `pre-commit`/`pre-push` hooks that fail with a message
pointing at `wt new`. Editing in base is recoverable; committing from base is what
actually costs you. See decision 15 for the leak this needed a second fix for.

**Agent-facing skill. Built.** A `plugin/` directory symlinked into `~/.claude/skills/wt`,
documenting `new`, `ls`, `path`, `name`, `status`, `wait`, `claude`, `rm`, `gc`, `sync`,
`doctor`, plus selector rules and the three traps that actually bite. Short, because the
whole point is that an agent can create a working tree in one command.

**Shell function. Built.** `wt go` and `wt cd` need a `wt()` wrapper in `.zshrc`, since no
child process can change its parent's directory. Every other command is a plain binary
call, including `wt claude`, which execs with cwd set via
`std::os::unix::process::CommandExt::exec`.

**LaunchAgent. Built.** Every 5 minutes: `git -C base fetch --prune`, then fast-forward
trunk only if base is clean and actually on trunk. Never `reset --hard`. Logs to
`~/repos/wt/wt-sync.log` / `wt-sync.err.log`. `install.sh` writes the plist and prints
the `launchctl bootstrap` command; it never loads it.

---

## Build order

**Phase 1 — core. Built, reviewed, and adopted against the real monorepo.** `data.json`
store with locking and atomic writes, `init --adopt`, `new`, `ls`, `path`, `name`, `rm`.
A full provisioning run against the real monorepo base worked end to end: 12 gitignored
env files copied, shared symlinks invisible to `git status` in base and every tree.

**Phase 2 — speed and lifecycle. Done.** `new` returns the tree path in ~2s and finishes
steps in a detached, re-exec'd `wt __provision` child; `status` and `wait` read progress
back from `data.json` (state, current step index/total, elapsed, log path), no IPC. `gc`
reaps clean trees with no commits beyond `origin/<trunk>` and deletes their branch;
`doctor` reconciles the registry against `git worktree list` and reports (never
auto-adopts) worktrees `wt` didn't create — the monorepo's 24 pre-existing
`.claude/worktrees/` entries show up this way, correctly, not as errors. Fixed along the
way: a failed removal now always keeps the registry entry instead of orphaning the
tree; `rm --delete-branch` and `gc` both refuse to delete a branch with commits not on
the remote. The original submodule teardown (deinit-then-force) shipped in this phase
was found live to corrupt shared state and was replaced in Phase 3 — see decision 14.

**Phase 3 — integrations. Done, verified live.** `wt sync`, `wt claude`, the
`SessionStart`/`CwdChanged` hook, the agent-facing skill, the base commit block, and the
LaunchAgent are all built and tested — see Verification below. Fixed along the way: the
Phase 2 submodule teardown corrupted base's shared submodule config on every removal
(decision 14, now `fs::remove_dir_all` + `git worktree prune`); the base commit block's
`core.hooksPath` leaked into every new tree via `git worktree add`'s config copy
(decision 15, now cleared in `wt new` immediately after creation); `wt sync`'s dirty
check was simplified to a plain `git status --porcelain` emptiness check once base was
reset to genuinely clean (decision 16).

**Phase 4 — the other repos and upkeep. Built.** Step detection generalised, `wt adopt`,
and `wt env refresh`. (`sync` moved to Phase 3 — the LaunchAgent needed it.) Migration of
the monorepo base was a `git checkout master`, so it needed no phase of its own. Adopting
helm and toy-apps is a manual step, left until wanted.

Step detection, in order: `.gitmodules` gives the submodule step; a root
`pnpm-lock.yaml` gives `pnpm install --frozen-lockfile` then `pnpm build:packages`;
`python/pyproject.toml` gives the monorepo's `uv sync --all-packages` plus a sync per
known project; otherwise every direct subdirectory holding both a `pyproject.toml` and a
`uv.lock` gets its own `uv sync`. Checked read-only against the real repos: helm produces
no steps at all and toy-apps produces one sync for `planhub`.

**helm producing nothing is correct, not a gap.** Its only generated state is per-chart
`charts/*/charts/` and `Chart.lock` from `helm dependency update`, which needs network and
only matters when working on one specific chart. Trees there are near-instant.

When a repo has no `.worktreeinclude`, `shared` defaults to `["plans"]` rather than being
empty. Otherwise helm and toy-apps trees would get no plans directory, quietly undoing the
reason `shared/` exists. Neither repo gitignores `plans/`, so this leans on the
`info/exclude` mechanism from decision 12.

---

## Risks

- **Trees are not movable.** Absolute paths in `.venv/bin/*` and `node_modules/.bin/*`
  mean a tree cannot be relocated after provisioning. `wt` must never offer a move;
  teardown and recreate instead. Worth a line in the skill so no agent tries.
- **Lost cwd rooting.** Superseding harness isolation means agents are told a path
  rather than being placed in one. An agent that wanders back to base can edit the
  wrong tree. The base commit hooks catch the expensive version of this mistake; a
  hook that complains on writes to base would catch the rest.
- **Registry drift.** Anything that creates worktrees outside `wt` — including the
  harness, if it is ever used again — is invisible to `data.json`. `wt doctor` is the
  answer, and it should be cheap enough to run from a hook.
- **`.worktreeinclude` is a shared contract.** It is tracked and maintained by the team
  (last changed in #10405). `wt` reads it rather than keeping a private list, so a
  teammate adding a path gets picked up. If it grows something huge, provisioning slows
  down silently.
- **8.4s floor on `dspy-worker`.** Cargo fingerprints include the source path, so
  `fast-vnc-core` recompiles for every new tree path no matter what is cached. Only the
  dependency graph is reused.

---

## Out of scope

- No warm pool.
- No migration of the 29 existing worktrees; they keep working untouched.
- No version control or cross-machine sync for `plans/`.
- `helm` and `toy-apps` land in Phase 4, not Phase 1. Their provisioning is cheap, but
  they force the per-repo config work, so they come after the monorepo path is proven.
- No changes to the monorepo itself. `wt` reads `.worktreeinclude` and
  `.cursor/setup-worktree-unix.sh` as they are.

---

## Verification

Phase 1 is done when, from a cold shell:

```
wt init monorepo --adopt ~/repos/monorepo
wt new monorepo --name "scratch test"        # prints a path in under 2s
cd "$(wt path 'scratch test')"
pnpm -F types-shared run typecheck           # passes
wt ls                                        # shows the tree by name
wt rm 'scratch test'                         # gone from disk and registry
```

Phase 2 is done — verified live against the real monorepo:

```
wt new monorepo --name "..."                 # returns the path in ~2s (plus a fetch if stale)
wt status                                    # shows state, current step index/total, elapsed, log
wt wait '...'                                # blocks, then exits 0 once ready
wt gc --dry-run                              # lists what it would reap, touches nothing
wt gc                                        # reaps it and deletes its branch
wt doctor                                    # reports the 24 pre-existing .claude/worktrees/
                                              # entries as unregistered, not broken; touches none
```

Phase 3 is done — verified live against the real monorepo and a suite of throwaway
fixture repos:

```
wt sync monorepo                             # fetches, fast-forwards trunk, prints
                                              # what moved; refuses cleanly if base
                                              # is ever dirty
echo '{"hook_event_name":"SessionStart","cwd":"<tree path>"}' \
  | hooks/session-context.sh                 # prints the tree's name/branch/plans path
                                              # as SessionStart additionalContext JSON
echo '{"hook_event_name":"SessionStart","cwd":"<base path>"}' \
  | hooks/session-context.sh                 # prints the base notice instead
git -C <fixture base> commit                 # blocked, points at `wt new`
git -C <fixture tree> commit                 # still succeeds — the block does not leak
plutil -lint ~/Library/LaunchAgents/com.joshbassin.wt.sync.plist   # OK
./install.sh                                 # run twice against scratch paths: second
                                              # run is a no-op on every step; the real
                                              # ~/.zshrc, ~/.claude/settings.json, and
                                              # ~/.claude/statusline.sh are byte-identical
                                              # before and after (verified via shasum)
```

A fresh Claude session in a tree shows the tree's name in the statusline instead of a
uuid, and its first context includes the plans path.
