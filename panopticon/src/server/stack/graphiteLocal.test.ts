import { Database } from 'bun:sqlite';
import { afterEach, describe, expect, test } from 'bun:test';
import {
  mkdtempSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { GraphiteLocal, readGraphiteSnapshot } from './graphiteLocal.ts';

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

function freshDir(): string {
  dir = mkdtempSync(join(tmpdir(), 'panopticon-graphite-test-'));
  return dir;
}

function writeRepoConfig(gitDir: string, trunk = 'master'): void {
  writeFileSync(
    join(gitDir, '.graphite_repo_config'),
    JSON.stringify({ trunk, trunks: [{ name: trunk }] }),
  );
}

interface BranchRow {
  branch_name: string;
  parent_branch_name: string | null;
  children: string | null;
}

function writeMetadataDb(gitDir: string, rows: BranchRow[]): void {
  const db = new Database(join(gitDir, '.graphite_metadata.db'));
  db.run(
    'CREATE TABLE "branch_metadata" ("branch_name" text not null primary key, "parent_branch_name" text, "parent_branch_revision" text, "last_submitted_version" text, "state" text, "children" text, "branch_revision" text, "validation_result" text, "parent_head_revision" text)',
  );
  const insert = db.prepare(
    'INSERT INTO branch_metadata (branch_name, parent_branch_name, children) VALUES ($name, $parent, $children)',
  );
  for (const row of rows) {
    insert.run({
      $name: row.branch_name,
      $parent: row.parent_branch_name,
      $children: row.children,
    });
  }
  db.close();
}

function writePrInfo(gitDir: string, prInfos: unknown[]): void {
  writeFileSync(
    join(gitDir, '.graphite_pr_info'),
    JSON.stringify({ prInfos, mergeabilityStatuses: [] }),
  );
}

function prInfo(
  overrides: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    prNumber: 1,
    headRefName: 'branch-1',
    baseRefName: 'master',
    state: 'OPEN',
    title: 'A pull request',
    isDraft: false,
    ...overrides,
  };
}

describe('readGraphiteSnapshot', () => {
  test('reads trunk, branches, and prs from the three local files', () => {
    const gitDir = freshDir();
    writeRepoConfig(gitDir, 'master');
    writeMetadataDb(gitDir, [
      {
        branch_name: 'branch-1',
        parent_branch_name: 'master',
        children: '["branch-2"]',
      },
      {
        branch_name: 'branch-2',
        parent_branch_name: 'branch-1',
        children: '[]',
      },
    ]);
    writePrInfo(gitDir, [
      prInfo({ prNumber: 1, headRefName: 'branch-1' }),
      prInfo({ prNumber: 2, headRefName: 'branch-2', baseRefName: 'branch-1' }),
    ]);

    const snapshot = readGraphiteSnapshot(gitDir);

    expect(snapshot?.trunk).toBe('master');
    expect(snapshot?.branches.get('branch-1')).toEqual({
      name: 'branch-1',
      parent: 'master',
      children: ['branch-2'],
    });
    expect(snapshot?.prsByHead.get('branch-2')?.number).toBe(2);
    expect(snapshot?.prsByNumber.get(1)?.headRef).toBe('branch-1');
  });

  test('returns null when a file is missing', () => {
    const gitDir = freshDir();
    writeRepoConfig(gitDir);
    writeMetadataDb(gitDir, []);
    // .graphite_pr_info is never written.

    expect(readGraphiteSnapshot(gitDir)).toBeNull();
  });

  test('treats invalid or missing children JSON as an empty array', () => {
    const gitDir = freshDir();
    writeRepoConfig(gitDir);
    writeMetadataDb(gitDir, [
      {
        branch_name: 'branch-a',
        parent_branch_name: null,
        children: 'not json',
      },
      { branch_name: 'branch-b', parent_branch_name: null, children: null },
    ]);
    writePrInfo(gitDir, []);

    const snapshot = readGraphiteSnapshot(gitDir);

    expect(snapshot?.branches.get('branch-a')?.children).toEqual([]);
    expect(snapshot?.branches.get('branch-b')?.children).toEqual([]);
  });

  test('skips a prInfo entry with a bad shape', () => {
    const gitDir = freshDir();
    writeRepoConfig(gitDir);
    writeMetadataDb(gitDir, []);
    writePrInfo(gitDir, [
      prInfo({ prNumber: 1 }),
      { prNumber: 2, headRefName: 'branch-2' },
      prInfo({ prNumber: 3, state: 'UNKNOWN' }),
    ]);

    const snapshot = readGraphiteSnapshot(gitDir);

    expect([...(snapshot?.prsByNumber.keys() ?? [])]).toEqual([1]);
  });
});

describe('GraphiteLocal', () => {
  test('resolves to null without spawning when the repo has no reference clone', async () => {
    const local = new GraphiteLocal(null);
    expect(await local.snapshot()).toBeNull();
  });

  test('resolves a real clone through git rev-parse and memoizes on mtime', async () => {
    const repoDir = freshDir();
    await Bun.spawn(['git', 'init', '-q', repoDir], {
      stdout: 'pipe',
      stderr: 'pipe',
    }).exited;
    const gitDir = join(repoDir, '.git');
    writeRepoConfig(gitDir, 'master');
    writeMetadataDb(gitDir, [
      { branch_name: 'branch-1', parent_branch_name: 'master', children: '[]' },
    ]);
    writePrInfo(gitDir, [prInfo({ prNumber: 1, headRefName: 'branch-1' })]);

    const local = new GraphiteLocal(repoDir);
    const first = await local.snapshot();
    expect(first?.trunk).toBe('master');
    expect(first?.prsByNumber.has(1)).toBe(true);

    const second = await local.snapshot();
    expect(second).toBe(first);

    const prInfoPath = join(gitDir, '.graphite_pr_info');
    const beforeMtime = statSync(prInfoPath).mtimeMs;
    writePrInfo(gitDir, [prInfo({ prNumber: 2, headRefName: 'branch-1' })]);
    if (statSync(prInfoPath).mtimeMs === beforeMtime) {
      const bumped = new Date(beforeMtime + 1000);
      utimesSync(prInfoPath, bumped, bumped);
    }

    const third = await local.snapshot();
    expect(third).not.toBe(first);
    expect(third?.prsByNumber.has(2)).toBe(true);
  });

  test('a malformed metadata db resolves null and is not re-read while mtimes are unchanged', async () => {
    const repoDir = freshDir();
    await Bun.spawn(['git', 'init', '-q', repoDir], {
      stdout: 'pipe',
      stderr: 'pipe',
    }).exited;
    const gitDir = join(repoDir, '.git');
    const dbPath = join(gitDir, '.graphite_metadata.db');
    // utimesSync rounds to whole milliseconds, so pin the mtime to one.
    const fixedMtime = new Date('2024-01-01T00:00:00.000Z');
    writeRepoConfig(gitDir, 'master');
    writeFileSync(dbPath, 'not a sqlite file');
    utimesSync(dbPath, fixedMtime, fixedMtime);
    writePrInfo(gitDir, []);

    const local = new GraphiteLocal(repoDir);
    expect(await local.snapshot()).toBeNull();

    // Valid content under the same mtime must still read as cached null.
    rmSync(dbPath);
    writeMetadataDb(gitDir, [
      { branch_name: 'branch-1', parent_branch_name: 'master', children: '[]' },
    ]);
    utimesSync(dbPath, fixedMtime, fixedMtime);

    expect(await local.snapshot()).toBeNull();
  });
});
