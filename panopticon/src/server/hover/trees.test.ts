import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { RepoConfig } from '../config.ts';
import { openDb } from '../db.ts';
import type { Git } from '../git/git.ts';
import type { EnsureTreeParams, ProvisionStep, StepRunner } from './trees.ts';
import { PROVISION_LOG_FILE, TreeManager } from './trees.ts';

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

const REPOS: Record<string, RepoConfig> = {
  'acme/widgets': {
    trunk: 'main',
    hover: {
      steps: [
        { profile: 'node', run: 'pnpm install' },
        { profile: 'python', cwd: 'py', run: 'uv sync' },
      ],
    },
  },
  'acme/unsupported': { trunk: 'main' },
};

const BARE = '/bare/acme/widgets.git';

interface GitCall {
  args: string[];
  cwd?: string;
}

function harness(
  options: {
    maxTrees?: number;
    runStep?: StepRunner;
    db?: ReturnType<typeof openDb>;
    runOrThrow?: Git['runOrThrow'];
  } = {},
) {
  dir ??= mkdtempSync(join(tmpdir(), 'panopticon-trees-test-'));
  const db = options.db ?? openDb(join(dir, 'test.sqlite'));
  const gitCalls: GitCall[] = [];
  const stopped: number[] = [];
  const steps: ProvisionStep[] = [];
  const clock = { now: Date.parse('2026-01-01T00:00:00Z') };

  const git: Git = {
    run: () => Promise.reject(new Error('run not used')),
    runOrThrow:
      options.runOrThrow ??
      ((args, runOptions) => {
        gitCalls.push({ args, cwd: runOptions?.cwd });
        return Promise.resolve();
      }),
  };
  const runStep: StepRunner =
    options.runStep ??
    ((step) => {
      steps.push(step);
      return Promise.resolve(0);
    });

  const manager = new TreeManager({
    db,
    dataDir: dir,
    repos: REPOS,
    maxTrees: options.maxTrees ?? 3,
    git,
    bareClone: () => Promise.resolve(BARE),
    stopServers: (_owner, _repo, number) => {
      stopped.push(number);
      return Promise.resolve();
    },
    runStep,
    clock: () => clock.now,
  });
  return { manager, db, gitCalls, stopped, steps, clock, dataDir: dir };
}

function params(overrides: Partial<EnsureTreeParams> = {}): EnsureTreeParams {
  return {
    owner: 'acme',
    repo: 'widgets',
    number: 1,
    headSha: 'sha1',
    languages: ['typescript'],
    changedPaths: ['src/a.ts'],
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

describe('TreeManager.ensureTree', () => {
  test('does nothing for a repo without hover config', async () => {
    const { manager, gitCalls } = harness();

    await manager.ensureTree(params({ repo: 'unsupported' }));

    expect(gitCalls).toEqual([]);
    expect(manager.status('acme', 'unsupported', 1)).toEqual({
      state: 'unsupported',
    });
  });

  test('fetches the head, adds a worktree, and runs only the steps for the needed profiles', async () => {
    const { manager, gitCalls, steps, dataDir } = harness();

    await manager.ensureTree(params());

    const path = join(dataDir, 'trees', 'acme', 'widgets', 'pr-1');
    expect(gitCalls).toEqual([
      { args: ['fetch', 'origin', 'sha1'], cwd: BARE },
      { args: ['worktree', 'prune'], cwd: BARE },
      { args: ['worktree', 'add', '--detach', path, 'sha1'], cwd: BARE },
    ]);
    expect(steps).toEqual([
      {
        run: 'pnpm install',
        cwd: path,
        logPath: join(path, PROVISION_LOG_FILE),
      },
    ]);
    expect(manager.status('acme', 'widgets', 1)).toEqual({
      state: 'ready',
      headSha: 'sha1',
      path,
    });
  });

  test('runs steps for every profile in order, from their own cwd', async () => {
    const { manager, steps, dataDir } = harness();

    await manager.ensureTree(params({ languages: ['python', 'typescript'] }));

    const path = join(dataDir, 'trees', 'acme', 'widgets', 'pr-1');
    expect(steps.map((step) => [step.run, step.cwd])).toEqual([
      ['pnpm install', path],
      ['uv sync', join(path, 'py')],
    ]);
  });

  test('reports provisioning until the steps finish', async () => {
    const gate = deferred<number>();
    const { manager } = harness({ runStep: () => gate.promise });

    const attempt = manager.ensureTree(params());
    await Promise.resolve();
    await Promise.resolve();
    expect(manager.status('acme', 'widgets', 1)).toMatchObject({
      state: 'provisioning',
      headSha: 'sha1',
    });

    gate.resolve(0);
    await attempt;
    expect(manager.status('acme', 'widgets', 1).state).toBe('ready');
  });

  test('marks the tree failed with the step and exit code when a step fails', async () => {
    const { manager } = harness({ runStep: () => Promise.resolve(2) });

    await manager.ensureTree(params());

    const state = manager.status('acme', 'widgets', 1);
    expect(state).toMatchObject({ state: 'failed', headSha: 'sha1' });
    expect(state.state === 'failed' && state.error).toContain('pnpm install');
    expect(state.state === 'failed' && state.error).toContain('code 2');
  });

  test('marks the tree failed when a git command fails', async () => {
    const { manager } = harness({
      runOrThrow: () => Promise.reject(new Error('git fetch failed: nope')),
    });

    await manager.ensureTree(params());

    expect(manager.status('acme', 'widgets', 1)).toEqual({
      state: 'failed',
      headSha: 'sha1',
      error: 'git fetch failed: nope',
    });
  });

  test('a failed tree stays failed for the same head and retries for a new one', async () => {
    let exitCode = 1;
    const calls: string[] = [];
    const { manager } = harness({
      runStep: (step) => {
        calls.push(step.run);
        return Promise.resolve(exitCode);
      },
    });

    await manager.ensureTree(params());
    await manager.ensureTree(params());
    await manager.ensureTree(params());
    expect(calls).toHaveLength(1);

    exitCode = 0;
    await manager.ensureTree(params({ headSha: 'sha2' }));
    expect(calls).toHaveLength(2);
    expect(manager.status('acme', 'widgets', 1)).toMatchObject({
      state: 'ready',
      headSha: 'sha2',
    });
  });

  test('reuses a ready tree for the same head and bumps last use', async () => {
    const { manager, gitCalls, steps, clock, db } = harness();
    await manager.ensureTree(params());
    const calls = gitCalls.length;
    const stepCount = steps.length;

    clock.now += 60_000;
    await manager.ensureTree(params());

    expect(gitCalls).toHaveLength(calls);
    expect(steps).toHaveLength(stepCount);
    const row = db
      .query<{ last_used_at: string }, []>(
        'SELECT last_used_at FROM hover_trees',
      )
      .get();
    expect(row?.last_used_at).toBe(new Date(clock.now).toISOString());
  });

  test('checks out a moved head in place when no lockfile changed', async () => {
    const { manager, gitCalls, steps, dataDir } = harness();
    await manager.ensureTree(params());
    gitCalls.length = 0;
    const stepCount = steps.length;

    await manager.ensureTree(params({ headSha: 'sha2' }));

    const path = join(dataDir, 'trees', 'acme', 'widgets', 'pr-1');
    expect(gitCalls).toEqual([
      { args: ['fetch', 'origin', 'sha2'], cwd: path },
      { args: ['checkout', '--detach', 'sha2'], cwd: path },
    ]);
    expect(steps).toHaveLength(stepCount);
    expect(manager.status('acme', 'widgets', 1)).toEqual({
      state: 'ready',
      headSha: 'sha2',
      path,
    });
  });

  test('rebuilds the tree when the PR touches a lockfile', async () => {
    const { manager, gitCalls, steps, stopped, dataDir } = harness();
    await manager.ensureTree(params());
    gitCalls.length = 0;
    const stepCount = steps.length;

    await manager.ensureTree(
      params({ headSha: 'sha2', changedPaths: ['pnpm-lock.yaml'] }),
    );

    const path = join(dataDir, 'trees', 'acme', 'widgets', 'pr-1');
    expect(stopped).toEqual([1]);
    expect(gitCalls.map((call) => call.args)).toContainEqual([
      'worktree',
      'add',
      '--detach',
      path,
      'sha2',
    ]);
    expect(steps).toHaveLength(stepCount + 1);
    expect(manager.status('acme', 'widgets', 1)).toMatchObject({
      state: 'ready',
      headSha: 'sha2',
    });
  });

  test('runs one provision at a time, in arrival order', async () => {
    const gates = [deferred<number>(), deferred<number>()];
    const started: number[] = [];
    let active = 0;
    let maxActive = 0;
    const { manager } = harness({
      runStep: async () => {
        const index = started.length;
        started.push(index);
        active += 1;
        maxActive = Math.max(maxActive, active);
        const code = await gates[index]!.promise;
        active -= 1;
        return code;
      },
    });

    const first = manager.ensureTree(params({ number: 1 }));
    const second = manager.ensureTree(params({ number: 2 }));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(started).toEqual([0]);
    expect(manager.status('acme', 'widgets', 2).state).toBe('provisioning');

    gates[0]!.resolve(0);
    await first;
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(started).toEqual([0, 1]);

    gates[1]!.resolve(0);
    await second;
    expect(maxActive).toBe(1);
  });

  test('shares one attempt between concurrent calls for the same PR', async () => {
    const { manager, steps } = harness();

    await Promise.all([
      manager.ensureTree(params()),
      manager.ensureTree(params()),
    ]);

    expect(steps).toHaveLength(1);
  });

  test('evicts the least recently used tree when over the cap', async () => {
    const { manager, stopped, clock } = harness({ maxTrees: 2 });
    await manager.ensureTree(params({ number: 1 }));
    clock.now += 1000;
    await manager.ensureTree(params({ number: 2 }));
    clock.now += 1000;
    manager.touch('acme', 'widgets', 1);
    clock.now += 1000;

    await manager.ensureTree(params({ number: 3 }));

    expect(stopped).toEqual([2]);
    expect(manager.status('acme', 'widgets', 2).state).toBe('none');
    expect(manager.status('acme', 'widgets', 1).state).toBe('ready');
    expect(manager.status('acme', 'widgets', 3).state).toBe('ready');
  });

  test('fails trees left provisioning by a previous process on boot', async () => {
    const gate = deferred<number>();
    const { manager, db } = harness({ runStep: () => gate.promise });
    void manager.ensureTree(params());
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(manager.status('acme', 'widgets', 1).state).toBe('provisioning');

    const rebooted = harness({ db }).manager;

    expect(rebooted.status('acme', 'widgets', 1)).toMatchObject({
      state: 'failed',
      error: expect.stringContaining('restart'),
    });
    gate.resolve(0);
  });

  test('replaces a table from the previous schema', () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-trees-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    db.exec(
      'CREATE TABLE hover_trees (owner TEXT, repo TEXT, number INTEGER, tree_name TEXT, head_sha TEXT)',
    );

    const { manager } = harness({ db });

    expect(manager.status('acme', 'widgets', 1)).toEqual({ state: 'none' });
  });
});

describe('TreeManager teardown and reaping', () => {
  test('teardown stops servers, prunes worktrees, and drops the row', async () => {
    const { manager, gitCalls, stopped } = harness();
    await manager.ensureTree(params());
    gitCalls.length = 0;

    await manager.teardown('acme', 'widgets', 1);

    expect(stopped).toEqual([1]);
    expect(gitCalls).toEqual([{ args: ['worktree', 'prune'], cwd: BARE }]);
    expect(manager.status('acme', 'widgets', 1)).toEqual({ state: 'none' });
  });

  test('teardown of an unknown tree does nothing', async () => {
    const { manager, stopped } = harness();

    await manager.teardown('acme', 'widgets', 9);

    expect(stopped).toEqual([]);
  });

  test('reapIdle removes only trees idle past the cutoff', async () => {
    const { manager, clock } = harness();
    await manager.ensureTree(params({ number: 1 }));
    clock.now += 2 * 60 * 60 * 1000;
    await manager.ensureTree(params({ number: 2 }));

    const reaped = await manager.reapIdle(60 * 60 * 1000);

    expect(reaped).toEqual([{ owner: 'acme', repo: 'widgets', number: 1 }]);
    expect(manager.status('acme', 'widgets', 1).state).toBe('none');
    expect(manager.status('acme', 'widgets', 2).state).toBe('ready');
  });
});
