import { closeSync, mkdirSync, openSync, writeSync } from 'node:fs';
import { readdir, rm } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import type { Database } from 'bun:sqlite';
import type { HoverLanguage } from '../../shared/hover.ts';
import type { HoverProfile, RepoConfig } from '../config.ts';
import type { Git } from '../git/git.ts';

export const HOVER_TREES_SCHEMA = `
CREATE TABLE IF NOT EXISTS hover_trees (
  owner TEXT NOT NULL,
  repo TEXT NOT NULL,
  number INTEGER NOT NULL,
  head_sha TEXT NOT NULL,
  path TEXT NOT NULL,
  state TEXT NOT NULL,
  error TEXT,
  created_at TEXT NOT NULL,
  last_used_at TEXT NOT NULL,
  PRIMARY KEY (owner, repo, number)
);
`;

export const PROVISION_LOG_FILE = '.panopticon-provision.log';

type StoredState = 'provisioning' | 'ready' | 'failed';

interface HoverTreeRow {
  owner: string;
  repo: string;
  number: number;
  head_sha: string;
  path: string;
  state: StoredState;
  error: string | null;
  created_at: string;
  last_used_at: string;
}

interface PrRef {
  owner: string;
  repo: string;
  number: number;
}

export type HoverTreeState =
  | { state: 'unsupported' }
  | { state: 'none' }
  | { state: 'provisioning'; headSha: string }
  | { state: 'ready'; headSha: string; path: string }
  | { state: 'failed'; headSha: string; error: string };

export interface EnsureTreeParams extends PrRef {
  headSha: string;
  languages: HoverLanguage[];
  // Every path the diff touches; used only to decide whether a moved head
  // needs a fresh worktree instead of an in-place checkout.
  changedPaths: string[];
}

export interface ProvisionStep {
  run: string;
  cwd: string;
  logPath: string;
}

export type StepRunner = (step: ProvisionStep) => Promise<number>;

export const runStepInShell: StepRunner = async ({ run, cwd, logPath }) => {
  const fd = openSync(logPath, 'a');
  try {
    writeSync(fd, `$ ${run}\n`);
    const proc = Bun.spawn(['sh', '-c', run], {
      cwd,
      stdout: fd,
      stderr: fd,
    });
    return await proc.exited;
  } finally {
    closeSync(fd);
  }
};

// A changed lockfile or `pyproject.toml` can move dependency resolution
// out from under an already-provisioned tree, so those paths force a
// fresh worktree instead of a cheap in-place checkout.
const REPROVISION_TRIGGER =
  /(^|\/)(pnpm-lock\.yaml|uv\.lock|bun\.lock|package-lock\.json|pyproject\.toml)$/;

function touchesLockfileOrPyproject(paths: string[]): boolean {
  return paths.some((path) => REPROVISION_TRIGGER.test(path));
}

function profilesForLanguages(languages: HoverLanguage[]): HoverProfile[] {
  const profiles = new Set<HoverProfile>();
  for (const language of languages) {
    profiles.add(language === 'typescript' ? 'node' : 'python');
  }
  return [...profiles];
}

function prKey({ owner, repo, number }: PrRef): string {
  return `${owner}/${repo}#${number}`;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export interface TreeManagerDeps {
  db: Database;
  dataDir: string;
  repos: Record<string, RepoConfig>;
  maxTrees: number;
  git: Git;
  // Resolves to the repo's bare clone, cloning it first if needed.
  bareClone(owner: string, repo: string): Promise<string>;
  // Like `bareClone`, but never clones: null when there is no clone yet.
  existingBareClone(owner: string, repo: string): string | null;
  stopServers(owner: string, repo: string, number: number): Promise<void>;
  runStep?: StepRunner;
  clock?: () => number;
}

// Owns the git worktrees hover needs, one per PR under review, under
// `<dataDir>/trees`. `hover_trees` is the source of truth for which trees
// exist and what state they are in.
export class TreeManager {
  private readonly db: Database;
  private readonly dataDir: string;
  private readonly repos: Record<string, RepoConfig>;
  private readonly maxTrees: number;
  private readonly git: Git;
  private readonly bareClone: TreeManagerDeps['bareClone'];
  private readonly existingBareClone: TreeManagerDeps['existingBareClone'];
  private readonly stopServers: TreeManagerDeps['stopServers'];
  private readonly runStep: StepRunner;
  private readonly clock: () => number;
  private readonly pending = new Map<string, Promise<void>>();
  // Provisions run one at a time: installs are heavy, and the machine is
  // shared with the language servers.
  private queue: Promise<void> = Promise.resolve();

  constructor(deps: TreeManagerDeps) {
    this.db = deps.db;
    this.dataDir = deps.dataDir;
    this.repos = deps.repos;
    this.maxTrees = deps.maxTrees;
    this.git = deps.git;
    this.bareClone = deps.bareClone;
    this.existingBareClone = deps.existingBareClone;
    this.stopServers = deps.stopServers;
    this.runStep = deps.runStep ?? runStepInShell;
    this.clock = deps.clock ?? Date.now;

    this.migrate();
  }

  // Run once at boot, before serving. Failed and interrupted trees are
  // removed so a new build gets another try, and directories with no row are
  // swept. Only `<dataDir>/trees` is touched, and symlinks are removed as
  // links, never followed.
  async recover(): Promise<{ removedBroken: number; removedOrphans: number }> {
    const affected = new Set<string>();
    const broken = this.db
      .query<HoverTreeRow, []>(
        "SELECT * FROM hover_trees WHERE state != 'ready'",
      )
      .all();
    for (const row of broken) {
      await rm(row.path, { recursive: true, force: true });
      this.db
        .query(
          'DELETE FROM hover_trees WHERE owner = ? AND repo = ? AND number = ?',
        )
        .run(row.owner, row.repo, row.number);
      affected.add(`${row.owner}/${row.repo}`);
    }

    const known = new Set(
      this.db
        .query<{ path: string }, []>('SELECT path FROM hover_trees')
        .all()
        .map((row) => row.path),
    );
    const removedOrphans = await this.sweepOrphans(known, affected);

    for (const key of affected) {
      const [owner, repo] = key.split('/') as [string, string];
      const bare = this.existingBareClone(owner, repo);
      if (bare !== null) {
        await this.git.runOrThrow(['worktree', 'prune'], { cwd: bare });
      }
    }
    return { removedBroken: broken.length, removedOrphans };
  }

  private async sweepOrphans(
    known: Set<string>,
    affected: Set<string>,
  ): Promise<number> {
    const root = join(this.dataDir, 'trees');
    let removed = 0;
    const children = async (dir: string) =>
      readdir(dir, { withFileTypes: true }).catch(() => []);
    for (const owner of await children(root)) {
      const ownerPath = join(root, owner.name);
      if (!owner.isDirectory()) {
        await rm(ownerPath, { recursive: true, force: true });
        continue;
      }
      for (const repo of await children(ownerPath)) {
        const repoPath = join(ownerPath, repo.name);
        if (!repo.isDirectory()) {
          await rm(repoPath, { recursive: true, force: true });
          continue;
        }
        for (const tree of await children(repoPath)) {
          const treePath = join(repoPath, tree.name);
          if (tree.isDirectory() && known.has(treePath)) continue;
          await rm(treePath, { recursive: true, force: true });
          affected.add(`${owner.name}/${repo.name}`);
          removed += 1;
        }
      }
    }
    return removed;
  }

  private migrate(): void {
    const columns = this.db
      .query<{ name: string }, []>('PRAGMA table_info(hover_trees)')
      .all();
    if (columns.some((column) => column.name === 'tree_name')) {
      this.db.exec('DROP TABLE hover_trees');
    }
    this.db.exec(HOVER_TREES_SCHEMA);
  }

  private now(): string {
    return new Date(this.clock()).toISOString();
  }

  private hoverConfig({ owner, repo }: PrRef): RepoConfig['hover'] {
    return this.repos[`${owner}/${repo}`]?.hover;
  }

  private row({ owner, repo, number }: PrRef): HoverTreeRow | null {
    return this.db
      .query<HoverTreeRow, [string, string, number]>(
        'SELECT * FROM hover_trees WHERE owner = ? AND repo = ? AND number = ?',
      )
      .get(owner, repo, number);
  }

  private setState(
    pr: PrRef,
    state: StoredState,
    error: string | null,
    headSha?: string,
  ): void {
    this.db
      .query(
        `UPDATE hover_trees SET state = ?, error = ?, head_sha = COALESCE(?, head_sha)
         WHERE owner = ? AND repo = ? AND number = ?`,
      )
      .run(state, error, headSha ?? null, pr.owner, pr.repo, pr.number);
  }

  private treePath({ owner, repo, number }: PrRef): string {
    return join(this.dataDir, 'trees', owner, repo, `pr-${number}`);
  }

  // A provision or checkout failure is recorded on the tree, not thrown.
  // Concurrent calls for the same PR share one attempt.
  ensureTree(params: EnsureTreeParams): Promise<void> {
    const key = prKey(params);
    const inFlight = this.pending.get(key);
    if (inFlight !== undefined) return inFlight;

    const attempt = this.doEnsureTree(params).finally(() => {
      this.pending.delete(key);
    });
    this.pending.set(key, attempt);
    return attempt;
  }

  private async doEnsureTree(params: EnsureTreeParams): Promise<void> {
    const hover = this.hoverConfig(params);
    if (hover === undefined) return;

    const row = this.row(params);
    if (row !== null) {
      if (row.head_sha === params.headSha) {
        this.touch(params.owner, params.repo, params.number);
        return;
      }
      if (row.state === 'provisioning') return;
      if (
        row.state === 'ready' &&
        !touchesLockfileOrPyproject(params.changedPaths)
      ) {
        await this.checkoutInPlace(row, params.headSha);
        return;
      }
      await this.removeTree(row);
    }

    await this.evictForRoom();

    const path = this.treePath(params);
    const now = this.now();
    this.db
      .query(
        `INSERT INTO hover_trees (owner, repo, number, head_sha, path, state, error, created_at, last_used_at)
         VALUES (?, ?, ?, ?, ?, 'provisioning', NULL, ?, ?)`,
      )
      .run(
        params.owner,
        params.repo,
        params.number,
        params.headSha,
        path,
        now,
        now,
      );

    const profiles = profilesForLanguages(params.languages);
    const steps = hover.steps.filter((step) => profiles.includes(step.profile));
    await this.enqueue(() => this.provision(params, path, steps));
  }

  private enqueue(task: () => Promise<void>): Promise<void> {
    const run = this.queue.then(task);
    this.queue = run.catch(() => {});
    return run;
  }

  private async provision(
    pr: EnsureTreeParams,
    path: string,
    steps: NonNullable<RepoConfig['hover']>['steps'],
  ): Promise<void> {
    try {
      const bare = await this.bareClone(pr.owner, pr.repo);
      await this.git.runOrThrow(['fetch', 'origin', pr.headSha], { cwd: bare });
      await rm(path, { recursive: true, force: true });
      await this.git.runOrThrow(['worktree', 'prune'], { cwd: bare });
      mkdirSync(dirname(path), { recursive: true });
      await this.git.runOrThrow(
        ['worktree', 'add', '--detach', path, pr.headSha],
        { cwd: bare },
      );

      const logPath = join(path, PROVISION_LOG_FILE);
      for (const step of steps) {
        const cwd = join(path, step.cwd ?? '.');
        const exitCode = await this.runStep({ run: step.run, cwd, logPath });
        if (exitCode !== 0) {
          throw new Error(
            `"${step.run}" in ${step.cwd ?? '.'} exited with code ${exitCode}; see ${PROVISION_LOG_FILE}`,
          );
        }
      }
      this.setState(pr, 'ready', null);
    } catch (error) {
      this.setState(pr, 'failed', errorMessage(error));
    }
  }

  private async checkoutInPlace(
    row: HoverTreeRow,
    headSha: string,
  ): Promise<void> {
    try {
      await this.git.runOrThrow(['fetch', 'origin', headSha], {
        cwd: row.path,
      });
      await this.git.runOrThrow(['checkout', '--detach', headSha], {
        cwd: row.path,
      });
      this.setState(row, 'ready', null, headSha);
      this.touch(row.owner, row.repo, row.number);
    } catch (error) {
      this.setState(row, 'failed', errorMessage(error), headSha);
    }
  }

  // Trees still provisioning are never evicted.
  private async evictForRoom(): Promise<void> {
    const rows = this.db
      .query<HoverTreeRow, []>(
        'SELECT * FROM hover_trees ORDER BY last_used_at',
      )
      .all();
    const excess = rows.length - (this.maxTrees - 1);
    const evictable = rows.filter((row) => row.state !== 'provisioning');
    for (const row of evictable.slice(0, Math.max(0, excess))) {
      await this.removeTree(row);
    }
  }

  status(owner: string, repo: string, number: number): HoverTreeState {
    const pr = { owner, repo, number };
    if (this.hoverConfig(pr) === undefined) return { state: 'unsupported' };

    const row = this.row(pr);
    if (row === null) return { state: 'none' };
    switch (row.state) {
      case 'provisioning':
        return { state: 'provisioning', headSha: row.head_sha };
      case 'ready':
        return { state: 'ready', headSha: row.head_sha, path: row.path };
      case 'failed':
        return {
          state: 'failed',
          headSha: row.head_sha,
          error: row.error ?? 'Provisioning failed',
        };
    }
  }

  touch(owner: string, repo: string, number: number): void {
    this.db
      .query(
        'UPDATE hover_trees SET last_used_at = ? WHERE owner = ? AND repo = ? AND number = ?',
      )
      .run(this.now(), owner, repo, number);
  }

  private async removeTree(row: HoverTreeRow): Promise<void> {
    await this.stopServers(row.owner, row.repo, row.number);
    await rm(row.path, { recursive: true, force: true });
    const bare = await this.bareClone(row.owner, row.repo);
    await this.git.runOrThrow(['worktree', 'prune'], { cwd: bare });
    this.db
      .query(
        'DELETE FROM hover_trees WHERE owner = ? AND repo = ? AND number = ?',
      )
      .run(row.owner, row.repo, row.number);
  }

  // Waits for any provision in flight for the PR before removing its tree.
  async teardown(owner: string, repo: string, number: number): Promise<void> {
    const pr = { owner, repo, number };
    await this.pending.get(prKey(pr))?.catch(() => {});
    const row = this.row(pr);
    if (row !== null) await this.removeTree(row);
  }

  async reapIdle(olderThanMs: number): Promise<PrRef[]> {
    const cutoff = new Date(this.clock() - olderThanMs).toISOString();
    const idle = this.db
      .query<HoverTreeRow, [string]>(
        "SELECT * FROM hover_trees WHERE last_used_at < ? AND state != 'provisioning'",
      )
      .all(cutoff);
    for (const row of idle) await this.removeTree(row);
    return idle.map(({ owner, repo, number }) => ({ owner, repo, number }));
  }
}
