import type { Database } from 'bun:sqlite';
import type { HoverLanguage } from '../../shared/hover.ts';
import type { RepoConfig } from '../config.ts';
import { ensureSchema } from '../db.ts';
import type { WtRunner } from './wt.ts';
import { treeNew, treePath, treeRm, treeStatus } from './wt.ts';

export const HOVER_TREES_SCHEMA = `
CREATE TABLE IF NOT EXISTS hover_trees (
  owner TEXT NOT NULL,
  repo TEXT NOT NULL,
  number INTEGER NOT NULL,
  tree_name TEXT NOT NULL,
  head_sha TEXT NOT NULL,
  profiles TEXT NOT NULL,
  created_at TEXT NOT NULL,
  last_used_at TEXT NOT NULL,
  PRIMARY KEY (owner, repo, number)
);
`;

interface HoverTreeRow {
  owner: string;
  repo: string;
  number: number;
  tree_name: string;
  head_sha: string;
  profiles: string;
  created_at: string;
  last_used_at: string;
}

export type HoverTreeState =
  | { state: 'unsupported' }
  | { state: 'none' }
  | { state: 'provisioning' }
  | { state: 'ready'; path: string }
  | { state: 'failed'; error: string };

export interface EnsureTreeParams {
  owner: string;
  repo: string;
  number: number;
  headSha: string;
  languages: HoverLanguage[];
  // Every path the diff touches; used only to decide whether a moved head
  // needs a fresh worktree instead of an in-place checkout.
  changedPaths: string[];
}

export interface GitOps {
  fetch(cwd: string, ref: string): Promise<void>;
  checkoutDetached(cwd: string, sha: string): Promise<void>;
}

async function runGit(
  args: string[],
  cwd: string,
): Promise<{ stdout: string; stderr: string; code: number }> {
  const proc = Bun.spawn(['git', ...args], {
    cwd,
    stdout: 'pipe',
    stderr: 'pipe',
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  return { stdout, stderr, code };
}

async function runGitOrThrow(args: string[], cwd: string): Promise<void> {
  const { stderr, code } = await runGit(args, cwd);
  if (code !== 0) {
    throw new Error(`git ${args.join(' ')} failed: ${stderr.trim()}`);
  }
}

export const bunGitOps: GitOps = {
  fetch: (cwd, ref) => runGitOrThrow(['fetch', 'origin', ref], cwd),
  checkoutDetached: (cwd, sha) =>
    runGitOrThrow(['checkout', '--detach', sha], cwd),
};

// A changed lockfile or `pyproject.toml` can move dependency resolution
// out from under an already-provisioned tree, so those paths force a
// fresh worktree instead of a cheap in-place checkout.
const REPROVISION_TRIGGER =
  /(^|\/)(pnpm-lock\.yaml|uv\.lock|bun\.lock|package-lock\.json|pyproject\.toml)$/;

function touchesLockfileOrPyproject(paths: string[]): boolean {
  return paths.some((path) => REPROVISION_TRIGGER.test(path));
}

function profilesForLanguages(languages: HoverLanguage[]): string[] {
  const profiles = new Set<string>();
  for (const language of languages) {
    profiles.add(language === 'typescript' ? 'node' : 'python');
  }
  return [...profiles].sort();
}

function prKey(owner: string, repo: string, number: number): string {
  return `${owner}/${repo}#${number}`;
}

export interface TreeManagerDeps {
  db: Database;
  runner: WtRunner;
  repos: Record<string, RepoConfig>;
  stopServers(owner: string, repo: string, number: number): Promise<void>;
  git?: GitOps;
  clock?: () => number;
}

// Owns the lifecycle of the wt worktrees hover needs, one per PR under
// review. `wt` itself is the source of truth for whether a tree exists;
// `hover_trees` only remembers which wt tree name and head SHA belong to
// which PR.
export class TreeManager {
  private readonly db: Database;
  private readonly runner: WtRunner;
  private readonly repos: Record<string, RepoConfig>;
  private readonly stopServers: (
    owner: string,
    repo: string,
    number: number,
  ) => Promise<void>;
  private readonly git: GitOps;
  private readonly clock: () => number;
  private readonly pending = new Map<string, Promise<void>>();

  private readonly selectStmt;
  private readonly upsertStmt;
  private readonly updateHeadStmt;
  private readonly touchStmt;
  private readonly deleteStmt;
  private readonly selectIdleStmt;

  constructor(deps: TreeManagerDeps) {
    ensureSchema(deps.db, HOVER_TREES_SCHEMA);
    this.db = deps.db;
    this.runner = deps.runner;
    this.repos = deps.repos;
    this.stopServers = deps.stopServers;
    this.git = deps.git ?? bunGitOps;
    this.clock = deps.clock ?? Date.now;

    this.selectStmt = this.db.query<
      HoverTreeRow,
      { $owner: string; $repo: string; $number: number }
    >(
      'SELECT * FROM hover_trees WHERE owner = $owner AND repo = $repo AND number = $number',
    );
    this.upsertStmt = this.db.query<
      unknown,
      {
        $owner: string;
        $repo: string;
        $number: number;
        $treeName: string;
        $headSha: string;
        $profiles: string;
        $createdAt: string;
        $lastUsedAt: string;
      }
    >(
      `INSERT INTO hover_trees (owner, repo, number, tree_name, head_sha, profiles, created_at, last_used_at)
       VALUES ($owner, $repo, $number, $treeName, $headSha, $profiles, $createdAt, $lastUsedAt)
       ON CONFLICT(owner, repo, number) DO UPDATE SET
         tree_name = $treeName, head_sha = $headSha, profiles = $profiles, last_used_at = $lastUsedAt`,
    );
    this.updateHeadStmt = this.db.query<
      unknown,
      {
        $owner: string;
        $repo: string;
        $number: number;
        $headSha: string;
        $lastUsedAt: string;
      }
    >(
      `UPDATE hover_trees SET head_sha = $headSha, last_used_at = $lastUsedAt
       WHERE owner = $owner AND repo = $repo AND number = $number`,
    );
    this.touchStmt = this.db.query<
      unknown,
      { $owner: string; $repo: string; $number: number; $lastUsedAt: string }
    >(
      `UPDATE hover_trees SET last_used_at = $lastUsedAt
       WHERE owner = $owner AND repo = $repo AND number = $number`,
    );
    this.deleteStmt = this.db.query<
      unknown,
      { $owner: string; $repo: string; $number: number }
    >(
      'DELETE FROM hover_trees WHERE owner = $owner AND repo = $repo AND number = $number',
    );
    this.selectIdleStmt = this.db.query<
      { owner: string; repo: string; number: number },
      { $cutoff: string }
    >(
      'SELECT owner, repo, number FROM hover_trees WHERE last_used_at < $cutoff',
    );
  }

  private repoConfig(owner: string, repo: string): RepoConfig | null {
    return this.repos[`${owner}/${repo}`] ?? null;
  }

  private now(): string {
    return new Date(this.clock()).toISOString();
  }

  // Kicks off whatever the tree needs (nothing, a fresh provision, or an
  // in-place checkout) and returns once that step has happened. Concurrent
  // calls for the same PR share one in-flight attempt instead of racing
  // `wt tree new`.
  async ensureTree(params: EnsureTreeParams): Promise<void> {
    const key = prKey(params.owner, params.repo, params.number);
    const inFlight = this.pending.get(key);
    if (inFlight !== undefined) return inFlight;

    const attempt = this.doEnsureTree(params).finally(() => {
      this.pending.delete(key);
    });
    this.pending.set(key, attempt);
    return attempt;
  }

  private async doEnsureTree(params: EnsureTreeParams): Promise<void> {
    const repoConfig = this.repoConfig(params.owner, params.repo);
    if (repoConfig === null || repoConfig.wtRepo === undefined) return;

    const row = this.selectStmt.get({
      $owner: params.owner,
      $repo: params.repo,
      $number: params.number,
    });

    if (row !== null) {
      if (row.head_sha === params.headSha) {
        this.touch(params.owner, params.repo, params.number);
        return;
      }
      if (!touchesLockfileOrPyproject(params.changedPaths)) {
        const existingPath = await treePath(this.runner, row.tree_name);
        if (existingPath !== null) {
          await this.git.fetch(existingPath, params.headSha);
          await this.git.checkoutDetached(existingPath, params.headSha);
          this.updateHeadStmt.run({
            $owner: params.owner,
            $repo: params.repo,
            $number: params.number,
            $headSha: params.headSha,
            $lastUsedAt: this.now(),
          });
          return;
        }
        // wt has forgotten a tree we still had a row for; fall through and
        // reprovision instead of failing the hover request.
      } else {
        await this.teardown(params.owner, params.repo, params.number);
      }
    }

    await this.provision(params, repoConfig.wtRepo, repoConfig.referenceClone);
  }

  private async provision(
    params: EnsureTreeParams,
    wtRepo: string,
    referenceClone: string | undefined,
  ): Promise<void> {
    if (referenceClone !== undefined) {
      await this.git.fetch(referenceClone, params.headSha);
    }
    const profiles = profilesForLanguages(params.languages);
    const name = `panopticon-pr-${params.number}`;
    const branch = `panopticon/pr-${params.number}`;

    await treeNew(this.runner, {
      repo: wtRepo,
      name,
      branch,
      onto: params.headSha,
      profiles,
    });

    const now = this.now();
    this.upsertStmt.run({
      $owner: params.owner,
      $repo: params.repo,
      $number: params.number,
      $treeName: name,
      $headSha: params.headSha,
      $profiles: profiles.join(','),
      $createdAt: now,
      $lastUsedAt: now,
    });
  }

  async status(
    owner: string,
    repo: string,
    number: number,
  ): Promise<HoverTreeState> {
    const repoConfig = this.repoConfig(owner, repo);
    if (repoConfig === null || repoConfig.wtRepo === undefined) {
      return { state: 'unsupported' };
    }

    const row = this.selectStmt.get({
      $owner: owner,
      $repo: repo,
      $number: number,
    });
    if (row === null) {
      const stillProvisioning = this.pending.has(prKey(owner, repo, number));
      return stillProvisioning ? { state: 'provisioning' } : { state: 'none' };
    }

    const wtStatus = await treeStatus(this.runner, row.tree_name);
    if (wtStatus === null) {
      this.deleteStmt.run({ $owner: owner, $repo: repo, $number: number });
      return { state: 'none' };
    }

    switch (wtStatus.state) {
      case 'provisioning':
        return { state: 'provisioning' };
      case 'ready':
        return { state: 'ready', path: wtStatus.path };
      case 'failed':
        return {
          state: 'failed',
          error: `wt tree provisioning failed; see ${wtStatus.logPath}`,
        };
    }
  }

  async path(
    owner: string,
    repo: string,
    number: number,
  ): Promise<string | null> {
    const row = this.selectStmt.get({
      $owner: owner,
      $repo: repo,
      $number: number,
    });
    if (row === null) return null;
    return treePath(this.runner, row.tree_name);
  }

  touch(owner: string, repo: string, number: number): void {
    this.touchStmt.run({
      $owner: owner,
      $repo: repo,
      $number: number,
      $lastUsedAt: this.now(),
    });
  }

  async teardown(owner: string, repo: string, number: number): Promise<void> {
    const row = this.selectStmt.get({
      $owner: owner,
      $repo: repo,
      $number: number,
    });
    if (row !== null) {
      const wtStatus = await treeStatus(this.runner, row.tree_name);
      if (wtStatus !== null) {
        await treeRm(this.runner, row.tree_name, {
          force: true,
          deleteBranch: true,
        });
      }
    }
    await this.stopServers(owner, repo, number);
    this.deleteStmt.run({ $owner: owner, $repo: repo, $number: number });
  }

  // For a later poller: tears down every tree whose hover has been idle
  // longer than `olderThanMs` and reports which PRs it reaped.
  async reapIdle(
    olderThanMs: number,
  ): Promise<{ owner: string; repo: string; number: number }[]> {
    const cutoff = new Date(this.clock() - olderThanMs).toISOString();
    const idle = this.selectIdleStmt.all({ $cutoff: cutoff });
    const reaped: { owner: string; repo: string; number: number }[] = [];
    for (const row of idle) {
      await this.teardown(row.owner, row.repo, row.number);
      reaped.push({ owner: row.owner, repo: row.repo, number: row.number });
    }
    return reaped;
  }
}
