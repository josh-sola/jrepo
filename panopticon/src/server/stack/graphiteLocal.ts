import { Database } from 'bun:sqlite';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

export interface GraphiteBranch {
  name: string;
  parent: string | null;
  children: string[];
}

export type GraphitePrState = 'OPEN' | 'MERGED' | 'CLOSED';

export interface GraphitePr {
  number: number;
  headRef: string;
  baseRef: string;
  state: GraphitePrState;
  title: string;
  draft: boolean;
}

export interface GraphiteSnapshot {
  trunk: string;
  branches: Map<string, GraphiteBranch>;
  prsByHead: Map<string, GraphitePr>;
  prsByNumber: Map<number, GraphitePr>;
}

const REPO_CONFIG_FILE = '.graphite_repo_config';
const METADATA_DB_FILE = '.graphite_metadata.db';
const PR_INFO_FILE = '.graphite_pr_info';

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isPrState(value: unknown): value is GraphitePrState {
  return value === 'OPEN' || value === 'MERGED' || value === 'CLOSED';
}

function readJson(path: string): unknown {
  try {
    return JSON.parse(readFileSync(path, 'utf-8'));
  } catch {
    return null;
  }
}

function readTrunk(path: string): string | null {
  const parsed = readJson(path);
  if (typeof parsed !== 'object' || parsed === null) return null;
  const trunk = (parsed as Record<string, unknown>).trunk;
  return isString(trunk) ? trunk : null;
}

function parseChildren(raw: string | null): string[] {
  if (raw === null) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed) || !parsed.every(isString)) return [];
  return parsed;
}

interface BranchRow {
  branch_name: string;
  parent_branch_name: string | null;
  children: string | null;
}

function readBranches(path: string): Map<string, GraphiteBranch> {
  const branches = new Map<string, GraphiteBranch>();
  const db = new Database(path, { readonly: true });
  try {
    const rows = db
      .query<BranchRow, []>(
        'SELECT branch_name, parent_branch_name, children FROM branch_metadata',
      )
      .all();
    for (const row of rows) {
      branches.set(row.branch_name, {
        name: row.branch_name,
        parent: row.parent_branch_name,
        children: parseChildren(row.children),
      });
    }
  } finally {
    db.close();
  }
  return branches;
}

function toGraphitePr(value: unknown): GraphitePr | null {
  if (typeof value !== 'object' || value === null) return null;
  const entry = value as Record<string, unknown>;
  const number = entry.prNumber;
  const headRef = entry.headRefName;
  const baseRef = entry.baseRefName;
  const state = entry.state;
  const title = entry.title;
  const draft = entry.isDraft;
  if (
    typeof number !== 'number' ||
    !isString(headRef) ||
    !isString(baseRef) ||
    !isPrState(state) ||
    !isString(title) ||
    typeof draft !== 'boolean'
  ) {
    return null;
  }
  return { number, headRef, baseRef, state, title, draft };
}

function readPrs(path: string): {
  byHead: Map<string, GraphitePr>;
  byNumber: Map<number, GraphitePr>;
} {
  const byHead = new Map<string, GraphitePr>();
  const byNumber = new Map<number, GraphitePr>();
  const parsed = readJson(path);
  if (typeof parsed !== 'object' || parsed === null) {
    return { byHead, byNumber };
  }
  const prInfos = (parsed as Record<string, unknown>).prInfos;
  if (!Array.isArray(prInfos)) return { byHead, byNumber };
  for (const entry of prInfos) {
    const pr = toGraphitePr(entry);
    if (pr === null) continue;
    byHead.set(pr.headRef, pr);
    byNumber.set(pr.number, pr);
  }
  return { byHead, byNumber };
}

// All three files live together in the git common dir, so missing any one
// of them counts as no local Graphite data.
export function readGraphiteSnapshot(gitDir: string): GraphiteSnapshot | null {
  const repoConfigPath = join(gitDir, REPO_CONFIG_FILE);
  const metadataDbPath = join(gitDir, METADATA_DB_FILE);
  const prInfoPath = join(gitDir, PR_INFO_FILE);
  if (
    !existsSync(repoConfigPath) ||
    !existsSync(metadataDbPath) ||
    !existsSync(prInfoPath)
  ) {
    return null;
  }

  const trunk = readTrunk(repoConfigPath);
  if (trunk === null) return null;

  const branches = readBranches(metadataDbPath);
  const { byHead, byNumber } = readPrs(prInfoPath);

  return { trunk, branches, prsByHead: byHead, prsByNumber: byNumber };
}

interface FileMtimes {
  repoConfig: number;
  metadataDb: number;
  prInfo: number;
}

function statMtimes(gitDir: string): FileMtimes | null {
  try {
    return {
      repoConfig: statSync(join(gitDir, REPO_CONFIG_FILE)).mtimeMs,
      metadataDb: statSync(join(gitDir, METADATA_DB_FILE)).mtimeMs,
      prInfo: statSync(join(gitDir, PR_INFO_FILE)).mtimeMs,
    };
  } catch {
    return null;
  }
}

function sameMtimes(a: FileMtimes, b: FileMtimes): boolean {
  return (
    a.repoConfig === b.repoConfig &&
    a.metadataDb === b.metadataDb &&
    a.prInfo === b.prInfo
  );
}

// Graphite rewrites these files on every `gt` run, which panopticon cannot
// observe, so file mtimes are the only change signal.
export class GraphiteLocal {
  private readonly repoPath: string | null;
  private gitDirPromise: Promise<string | null> | null = null;
  private cache: {
    snapshot: GraphiteSnapshot | null;
    mtimes: FileMtimes;
  } | null = null;

  constructor(repoPath: string | null) {
    this.repoPath = repoPath;
  }

  async snapshot(): Promise<GraphiteSnapshot | null> {
    if (this.repoPath === null) return null;

    const gitDir = await this.resolveGitDir(this.repoPath);
    if (gitDir === null) return null;

    const mtimes = statMtimes(gitDir);
    if (mtimes === null) return null;
    if (this.cache && sameMtimes(this.cache.mtimes, mtimes)) {
      return this.cache.snapshot;
    }

    let snapshot: GraphiteSnapshot | null;
    try {
      snapshot = readGraphiteSnapshot(gitDir);
    } catch (error) {
      console.error(
        `panopticon graphiteLocal: could not read ${gitDir}`,
        error,
      );
      snapshot = null;
    }
    this.cache = { snapshot, mtimes };
    return snapshot;
  }

  private resolveGitDir(repoPath: string): Promise<string | null> {
    if (this.gitDirPromise) return this.gitDirPromise;
    this.gitDirPromise = this.doResolveGitDir(repoPath);
    return this.gitDirPromise;
  }

  private async doResolveGitDir(repoPath: string): Promise<string | null> {
    const proc = Bun.spawn(
      [
        'git',
        '-C',
        repoPath,
        'rev-parse',
        '--path-format=absolute',
        '--git-common-dir',
      ],
      { stdout: 'pipe', stderr: 'pipe' },
    );
    const [stdout, exitCode] = await Promise.all([
      new Response(proc.stdout).text(),
      proc.exited,
    ]);
    if (exitCode !== 0) {
      console.error(
        `panopticon graphiteLocal: could not resolve the git dir for ${repoPath}`,
      );
      return null;
    }
    return stdout.trim();
  }
}
