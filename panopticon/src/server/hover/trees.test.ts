import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { RepoConfig } from '../config.ts';
import { openDb } from '../db.ts';
import type { GitOps } from './trees.ts';
import { TreeManager } from './trees.ts';
import type { WtRunResult, WtRunner } from './wt.ts';

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

function fakeRunner(
  handler: (args: string[]) => WtRunResult,
): WtRunner & { calls: string[][] } {
  const calls: string[][] = [];
  return {
    calls,
    run: (args) => {
      calls.push(args);
      return Promise.resolve(handler(args));
    },
  };
}

function fakeGit(): GitOps & {
  calls: { op: string; cwd: string; ref: string }[];
} {
  const calls: { op: string; cwd: string; ref: string }[] = [];
  return {
    calls,
    fetch: (cwd, ref) => {
      calls.push({ op: 'fetch', cwd, ref });
      return Promise.resolve();
    },
    checkoutDetached: (cwd, sha) => {
      calls.push({ op: 'checkout', cwd, ref: sha });
      return Promise.resolve();
    },
  };
}

const REPOS: Record<string, RepoConfig> = {
  'acme/widgets': {
    wtRepo: 'widgets',
    referenceClone: '/repos/widgets',
    trunk: 'main',
  },
  'acme/unsupported': { trunk: 'main' },
};

function openTestDb() {
  dir = mkdtempSync(join(tmpdir(), 'panopticon-trees-test-'));
  return openDb(join(dir, 'test.sqlite'));
}

function statusResponse(
  state: 'provisioning' | 'ready' | 'failed',
  overrides: Partial<{ path: string; logPath: string }> = {},
): string {
  return JSON.stringify([
    {
      id: 'tree-id',
      name: 'panopticon-pr-1',
      branch: 'panopticon/pr-1',
      repo: 'widgets',
      path: overrides.path ?? '/repos/wt/widgets/trees/panopticon-pr-1',
      state,
      stale: false,
      logPath:
        overrides.logPath ??
        '/repos/wt/widgets/trees/panopticon-pr-1/.wt-provision.log',
    },
  ]);
}

describe('TreeManager.ensureTree', () => {
  test('reports unsupported and does nothing for a repo without wtRepo', async () => {
    const db = openTestDb();
    const runner = fakeRunner(() => ({ code: 0, stdout: '[]', stderr: '' }));
    const stopServers = () => Promise.resolve();
    const manager = new TreeManager({ db, runner, repos: REPOS, stopServers });

    await manager.ensureTree({
      owner: 'acme',
      repo: 'unsupported',
      number: 1,
      headSha: 'sha1',
      languages: ['typescript'],
      changedPaths: [],
    });

    expect(runner.calls).toEqual([]);
    expect(await manager.status('acme', 'unsupported', 1)).toEqual({
      state: 'unsupported',
    });
    db.close();
  });

  test('fetches the base checkout and runs wt tree new for a fresh PR', async () => {
    const db = openTestDb();
    const runner = fakeRunner((args) => {
      if (args[0] === 'tree' && args[1] === 'new') {
        return {
          code: 0,
          stdout: '/repos/wt/widgets/trees/panopticon-pr-1\n',
          stderr: '',
        };
      }
      return { code: 0, stdout: '[]', stderr: '' };
    });
    const git = fakeGit();
    const stopServers = () => Promise.resolve();
    const manager = new TreeManager({
      db,
      runner,
      repos: REPOS,
      stopServers,
      git,
    });

    await manager.ensureTree({
      owner: 'acme',
      repo: 'widgets',
      number: 7,
      headSha: 'headsha',
      languages: ['typescript', 'python'],
      changedPaths: ['src/a.ts'],
    });

    expect(git.calls).toEqual([
      { op: 'fetch', cwd: '/repos/widgets', ref: 'headsha' },
    ]);
    expect(runner.calls).toEqual([
      [
        'tree',
        'new',
        '--name',
        'panopticon-pr-7',
        '--branch',
        'panopticon/pr-7',
        '--onto',
        'headsha',
        '--profile',
        'node,python',
        'widgets',
      ],
    ]);

    const row = db
      .query<{ tree_name: string; head_sha: string; profiles: string }, []>(
        'SELECT tree_name, head_sha, profiles FROM hover_trees',
      )
      .get();
    expect(row).toEqual({
      tree_name: 'panopticon-pr-7',
      head_sha: 'headsha',
      profiles: 'node,python',
    });
    db.close();
  });

  test('does nothing when the recorded head SHA already matches', async () => {
    const db = openTestDb();
    const runner = fakeRunner(() => ({ code: 0, stdout: '[]', stderr: '' }));
    const git = fakeGit();
    const stopServers = () => Promise.resolve();
    const manager = new TreeManager({
      db,
      runner,
      repos: REPOS,
      stopServers,
      git,
    });

    await manager.ensureTree({
      owner: 'acme',
      repo: 'widgets',
      number: 7,
      headSha: 'sha-a',
      languages: [],
      changedPaths: [],
    });
    runner.calls.length = 0;
    git.calls.length = 0;

    // Re-inserted with the same head SHA below, then ensureTree again with
    // the identical SHA should be a no-op besides touching last_used_at.
    await manager.ensureTree({
      owner: 'acme',
      repo: 'widgets',
      number: 7,
      headSha: 'sha-a',
      languages: [],
      changedPaths: [],
    });

    expect(runner.calls).toEqual([]);
    expect(git.calls).toEqual([]);
    db.close();
  });

  test('checks out the new head in place when no lockfile changed', async () => {
    const db = openTestDb();
    const runner = fakeRunner((args) => {
      if (args[0] === 'tree' && args[1] === 'new') {
        return {
          code: 0,
          stdout: '/repos/wt/widgets/trees/panopticon-pr-7\n',
          stderr: '',
        };
      }
      if (args[0] === 'tree' && args[1] === 'path') {
        return {
          code: 0,
          stdout: '/repos/wt/widgets/trees/panopticon-pr-7\n',
          stderr: '',
        };
      }
      return { code: 0, stdout: '[]', stderr: '' };
    });
    const git = fakeGit();
    const stopServers = () => Promise.resolve();
    const manager = new TreeManager({
      db,
      runner,
      repos: REPOS,
      stopServers,
      git,
    });

    await manager.ensureTree({
      owner: 'acme',
      repo: 'widgets',
      number: 7,
      headSha: 'sha-a',
      languages: ['typescript'],
      changedPaths: ['src/a.ts'],
    });
    git.calls.length = 0;
    runner.calls.length = 0;

    await manager.ensureTree({
      owner: 'acme',
      repo: 'widgets',
      number: 7,
      headSha: 'sha-b',
      languages: ['typescript'],
      changedPaths: ['src/a.ts'],
    });

    expect(runner.calls).toEqual([['tree', 'path', 'panopticon-pr-7']]);
    expect(git.calls).toEqual([
      {
        op: 'fetch',
        cwd: '/repos/wt/widgets/trees/panopticon-pr-7',
        ref: 'sha-b',
      },
      {
        op: 'checkout',
        cwd: '/repos/wt/widgets/trees/panopticon-pr-7',
        ref: 'sha-b',
      },
    ]);

    const row = db
      .query<{ head_sha: string }, []>('SELECT head_sha FROM hover_trees')
      .get();
    expect(row).toEqual({ head_sha: 'sha-b' });
    db.close();
  });

  test('tears down and reprovisions when the diff touches a lockfile', async () => {
    const db = openTestDb();
    let treeRmCalled = false;
    const runner = fakeRunner((args) => {
      if (args[0] === 'tree' && args[1] === 'new') {
        return {
          code: 0,
          stdout: '/repos/wt/widgets/trees/panopticon-pr-7\n',
          stderr: '',
        };
      }
      if (args[0] === 'tree' && args[1] === 'status') {
        return { code: 0, stdout: statusResponse('ready'), stderr: '' };
      }
      if (args[0] === 'tree' && args[1] === 'rm') {
        treeRmCalled = true;
        return { code: 0, stdout: '', stderr: '' };
      }
      return { code: 0, stdout: '[]', stderr: '' };
    });
    const git = fakeGit();
    let stopped = false;
    const stopServers = () => {
      stopped = true;
      return Promise.resolve();
    };
    const manager = new TreeManager({
      db,
      runner,
      repos: REPOS,
      stopServers,
      git,
    });

    await manager.ensureTree({
      owner: 'acme',
      repo: 'widgets',
      number: 7,
      headSha: 'sha-a',
      languages: ['typescript'],
      changedPaths: ['src/a.ts'],
    });
    runner.calls.length = 0;
    git.calls.length = 0;

    await manager.ensureTree({
      owner: 'acme',
      repo: 'widgets',
      number: 7,
      headSha: 'sha-b',
      languages: ['typescript'],
      changedPaths: ['pnpm-lock.yaml'],
    });

    expect(treeRmCalled).toBe(true);
    expect(stopped).toBe(true);
    // reprovision fetches the new head into the base checkout again, then
    // runs wt tree new with the new SHA.
    expect(git.calls).toEqual([
      { op: 'fetch', cwd: '/repos/widgets', ref: 'sha-b' },
    ]);
    expect(
      runner.calls.some(
        (call) =>
          call[0] === 'tree' && call[1] === 'new' && call.includes('sha-b'),
      ),
    ).toBe(true);
    db.close();
  });
});

describe('TreeManager.status', () => {
  test('maps wt status to the hover contract', async () => {
    const db = openTestDb();
    const runner = fakeRunner((args) => {
      if (args[0] === 'tree' && args[1] === 'new') {
        return {
          code: 0,
          stdout: '/repos/wt/widgets/trees/panopticon-pr-1\n',
          stderr: '',
        };
      }
      if (args[0] === 'tree' && args[1] === 'status') {
        return { code: 0, stdout: statusResponse('ready'), stderr: '' };
      }
      return { code: 0, stdout: '[]', stderr: '' };
    });
    const stopServers = () => Promise.resolve();
    const manager = new TreeManager({
      db,
      runner,
      repos: REPOS,
      stopServers,
      git: fakeGit(),
    });

    await manager.ensureTree({
      owner: 'acme',
      repo: 'widgets',
      number: 1,
      headSha: 'sha1',
      languages: [],
      changedPaths: [],
    });

    expect(await manager.status('acme', 'widgets', 1)).toEqual({
      state: 'ready',
      path: '/repos/wt/widgets/trees/panopticon-pr-1',
    });
    db.close();
  });

  test('reports none and drops the row when wt no longer knows the tree', async () => {
    const db = openTestDb();
    const runner = fakeRunner((args) => {
      if (args[0] === 'tree' && args[1] === 'new') {
        return {
          code: 0,
          stdout: '/repos/wt/widgets/trees/panopticon-pr-1\n',
          stderr: '',
        };
      }
      if (args[0] === 'tree' && args[1] === 'status') {
        return {
          code: 1,
          stdout: '',
          stderr: 'error: no tree matches selector',
        };
      }
      return { code: 0, stdout: '[]', stderr: '' };
    });
    const stopServers = () => Promise.resolve();
    const manager = new TreeManager({
      db,
      runner,
      repos: REPOS,
      stopServers,
      git: fakeGit(),
    });

    await manager.ensureTree({
      owner: 'acme',
      repo: 'widgets',
      number: 1,
      headSha: 'sha1',
      languages: [],
      changedPaths: [],
    });

    expect(await manager.status('acme', 'widgets', 1)).toEqual({
      state: 'none',
    });
    const row = db.query('SELECT * FROM hover_trees').get();
    expect(row).toBeNull();
    db.close();
  });
});

describe('TreeManager.teardown', () => {
  test('removes the wt tree, stops servers, and deletes the row', async () => {
    const db = openTestDb();
    const removeCalls: string[][] = [];
    const runner = fakeRunner((args) => {
      if (args[0] === 'tree' && args[1] === 'new') {
        return {
          code: 0,
          stdout: '/repos/wt/widgets/trees/panopticon-pr-1\n',
          stderr: '',
        };
      }
      if (args[0] === 'tree' && args[1] === 'status') {
        return { code: 0, stdout: statusResponse('ready'), stderr: '' };
      }
      if (args[0] === 'tree' && args[1] === 'rm') {
        removeCalls.push(args);
        return { code: 0, stdout: '', stderr: '' };
      }
      return { code: 0, stdout: '[]', stderr: '' };
    });
    const stoppedFor: string[] = [];
    const stopServers = (
      owner: string,
      repo: string,
      number: number,
    ): Promise<void> => {
      stoppedFor.push(`${owner}/${repo}#${number}`);
      return Promise.resolve();
    };
    const manager = new TreeManager({
      db,
      runner,
      repos: REPOS,
      stopServers,
      git: fakeGit(),
    });

    await manager.ensureTree({
      owner: 'acme',
      repo: 'widgets',
      number: 9,
      headSha: 'sha1',
      languages: [],
      changedPaths: [],
    });

    await manager.teardown('acme', 'widgets', 9);

    expect(removeCalls).toEqual([
      ['tree', 'rm', 'panopticon-pr-9', '--force', '--delete-branch'],
    ]);
    expect(stoppedFor).toEqual(['acme/widgets#9']);
    const row = db.query('SELECT * FROM hover_trees').get();
    expect(row).toBeNull();
    db.close();
  });

  test('is a no-op besides stopping servers when there is no row', async () => {
    const db = openTestDb();
    const runner = fakeRunner(() => ({ code: 0, stdout: '[]', stderr: '' }));
    let stopCount = 0;
    const stopServers = () => {
      stopCount += 1;
      return Promise.resolve();
    };
    const manager = new TreeManager({ db, runner, repos: REPOS, stopServers });

    await manager.teardown('acme', 'widgets', 999);

    expect(runner.calls).toEqual([]);
    expect(stopCount).toBe(1);
    db.close();
  });
});

describe('TreeManager.reapIdle', () => {
  test('tears down every tree whose last_used_at is older than the cutoff', async () => {
    const db = openTestDb();
    const runner = fakeRunner((args) => {
      if (args[0] === 'tree' && args[1] === 'new') {
        return {
          code: 0,
          stdout: '/repos/wt/widgets/trees/panopticon-pr-1\n',
          stderr: '',
        };
      }
      if (args[0] === 'tree' && args[1] === 'status') {
        return { code: 0, stdout: statusResponse('ready'), stderr: '' };
      }
      return { code: 0, stdout: '[]', stderr: '' };
    });
    const stopServers = () => Promise.resolve();

    let now = 0;
    const clock = () => now;
    const manager = new TreeManager({
      db,
      runner,
      repos: REPOS,
      stopServers,
      git: fakeGit(),
      clock,
    });

    now = 1_000;
    await manager.ensureTree({
      owner: 'acme',
      repo: 'widgets',
      number: 1,
      headSha: 'sha1',
      languages: [],
      changedPaths: [],
    });

    now = 1_000 + 20 * 60 * 1000;
    const reaped = await manager.reapIdle(15 * 60 * 1000);

    expect(reaped).toEqual([{ owner: 'acme', repo: 'widgets', number: 1 }]);
    const row = db.query('SELECT * FROM hover_trees').get();
    expect(row).toBeNull();
    db.close();
  });
});
