# `wt` — enriched worktree tooling

**Goal.** Make a fully working checkout of a large repo cheap enough to create that
starting fresh work is never a reason to hesitate. One base clone per repo, kept at
`origin/master`; all work in disposable trees that arrive with dependencies installed,
`.env` files in place, and plans already wired up.

Last updated: 2026-07-27 (Phase 1 built and reviewed)
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
| 5 | **Shared gitignored state is symlinked, not copied** — `plans/`, `local/`, `user-memories/`. | User decision. Symlinks give one truth and survive teardown, which is the point of moving plans out of trees. |
| 6 | **Base stays fresh two ways:** a LaunchAgent every few minutes, plus a fetch on `wt new`. | User decision. The `wt new` fetch is cheap precisely because the timer keeps base nearly current. |
| 7 | **Always fresh `pnpm install`; never clone `node_modules`.** | Measured: install is 23s, `cp -Rc` of `node_modules` is 42–69s and still needs a repair pass because the `.bin` shims carry absolute paths. Cloning is both slower and wrong. |
| 8 | **Shared `CARGO_TARGET_DIR` per repo.** | Measured: cuts `dspy-worker`'s `uv sync` from ~19s to 8.4s, and saves ~400 MB of real disk per tree. Cargo output is the only artifact here that is neither hardlinked nor reflinked from a shared store. |
| 9 | **Rust, single binary.** | The statusline re-renders every 10s and hooks fire on every session start. A Python CLI costs ~100–200ms per invocation; Rust costs ~2ms, so the statusline can call `wt` directly instead of parsing `data.json` with `jq`. |
| 10 | **Base is protected by reversibility, not locks.** | Read-only permissions or `chflags uchg` would break the fetch-and-fast-forward that keeps base current. Only commits can be hard-blocked, via hooks. |
| 11 | **Provisioning steps and the carry-over list are per-repo data in `data.json`,** seeded from `.worktreeinclude` when the repo has one. | helm and toy-apps have no `.worktreeinclude`, no pnpm lockfile, and no submodules, but do have their own provisioning needs (`helm dependency update`, per-app `uv sync`). Hardcoding monorepo steps would make them unsupportable. |
| 12 | **Shared symlink names go into base's `.git/info/exclude`.** | `plans/`, `local/`, and `user-memories/` are gitignored in the monorepo but not in helm or toy-apps, where the symlinks would otherwise show as untracked. `info/exclude` lives in the common git dir, so it is shared by every worktree and never committed. |

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
   ├─ base/                         canonical clone, tracks origin/master, read-only by convention
   ├─ trees/<uuidv7>/               working copies
   ├─ shared/                       durable state symlinked into every tree
   │  ├─ plans/
   │  ├─ local/
   │  └─ user-memories/
   └─ cache/
      └─ cargo-target/              shared CARGO_TARGET_DIR
```

`plans/` sits under `shared/` with the other two, because all three are the same thing:
durable gitignored state that every tree sees at the path the repo already expects.
Keeping `plans/` top-level would only make it visually prominent, and prominence is not
what makes an agent find it — the SessionStart hook injecting the path is.

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
| `wt env refresh <selector>` | Re-run `internal-cli config generate-env` when copied env files go stale. |
| `wt doctor` | Reconcile registry against `git worktree list`; report and offer to fix. |

Selectors resolve in one order everywhere: exact uuid, uuid prefix, exact name, unique
name substring, branch name. Ambiguity is an error listing the candidates, never a
guess.

---

## Provisioning pipeline

Measured cost per step, monorepo, warm caches:

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

**~44s total, ~2s to a usable path.** Steps after the path is printed run in a detached
child so the parent can exit; progress lands in `data.json` and a per-tree log.

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

**3. Symlinking shared dirs into a repo that does not ignore them would create dirty-tree
noise.** In helm or toy-apps, a `plans` symlink shows up as an untracked file and can get
committed by accident. The fix needs no repo change: `wt init` appends the shared names to
the base clone's `.git/info/exclude`. That file lives in the common git dir, so — verified
— it is shared by every worktree of the clone automatically, and it is never committed or
pushed.

With those three, helm and toy-apps are nearly instant to provision, since neither has a
dependency tree worth installing. They are a good first test of the general path precisely
because they are so much cheaper than the monorepo.

## Integrations

**Statusline.** `statusline.sh` gains one call: `wt name --path "$PWD"`, falling back to
the current basename when it prints nothing. Worktree detection already keys off
`git rev-parse --absolute-git-dir` matching `*/worktrees/*`, so `~/repos/wt/<repo>/trees/<uuid>`
is detected with no change. The per-tree colour keeps hashing `basename(toplevel)`,
which is now the uuid — stable per tree, which is what it needs to be.

**Hooks.** One `SessionStart` and `CwdChanged` hook, registered by patching
`~/.claude/settings.json` the way `recipes/install.sh` does. In a tree it injects the
tree's name, its branch, and the plans path, so an agent never has to be told where
plans live. In base it injects that base is read-only and `wt new` is how to start work.

This replaces `~/.claude/hooks/worktree-setup.sh`, which is currently orphaned — the
script exists but nothing in `settings.json` references it.

**Base commit block.** `wt init` installs `pre-commit` and `pre-push` hooks in base that
fail with a message pointing at `wt new`. Editing in base is recoverable; committing
from base is what actually costs you.

**Agent-facing skill.** A `plugin/` directory symlinked into `~/.claude/skills/wt`,
documenting `new`, `ls`, `path`, `status`, `wait`, `rm`. Short, because the whole point
is that an agent can create a working tree in one command.

**Shell function.** `wt go` and `wt cd` need a `wt()` wrapper in `.zshrc`, since no child
process can change its parent's directory. Every other command is a plain binary call,
including `wt claude`, which just execs with cwd set.

**LaunchAgent.** Every 5 minutes: `git -C base fetch --prune`, then fast-forward trunk
only if base is clean. Never `reset --hard`. Logs to a file under `~/repos/wt/`.

---

## Build order

**Phase 1 — core. Built and reviewed.** `data.json` store with locking and atomic writes,
`init --adopt`, `new` (synchronous provisioning), `ls`, `path`, `name`, `rm`. 1,512 lines
of Rust, 17 tests, clean under `clippy -D warnings`. Not yet run against a real repo —
adopting the monorepo is the next step and needs doing deliberately, since it appends to
base's `info/exclude` and seeds `shared/` from base's `local/` and `user-memories/`.

**Phase 2 — speed and lifecycle.** Background provisioning with `status` and `wait`;
`gc`; `doctor`.

**Phase 3 — integrations.** Statusline call, hooks, skill, `claude`, shell function,
LaunchAgent, base commit hooks.

**Phase 4 — the other repos and upkeep.** Generalise provisioning to per-repo step lists,
onboard helm and toy-apps, then `sync`, `adopt`, and `env refresh`. Migration of the
monorepo base is a `git checkout master`, so it needs no phase of its own.

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

Phase 3 is done when a fresh Claude session in a tree shows the tree's name in the
statusline instead of a uuid, and its first context includes the plans path.
