import { describe, expect, test } from 'bun:test';
import type { WtRunResult, WtRunner } from './wt.ts';
import { treeNew, treePath, treeRm, treeStatus } from './wt.ts';

// Trimmed from a real `wt tree status <tree> --json` on an existing
// monorepo tree.
const REAL_STATUS_SAMPLE = `[
  {
    "branch": "josh/processing-audit-gemini",
    "elapsedSeconds": 1405037,
    "id": "01a05d70-34f6-7a51-8224-3af565dca3fa",
    "logPath": "/Users/joshbassin/repos/wt/monorepo/trees/01a05d70-34f6-7a51-8224-3af565dca3fa/.wt-provision.log",
    "name": "processing/audit-gemini",
    "path": "/Users/joshbassin/repos/wt/monorepo/trees/01a05d70-34f6-7a51-8224-3af565dca3fa",
    "repo": "monorepo",
    "stale": false,
    "state": "ready",
    "stepIndex": null,
    "stepLabel": null,
    "stepTotal": null
  }
]`;

// Trimmed from a real `wt tree ls --json`. `treeStatus` never parses this
// shape, but its fields overlap enough with the status sample above that
// the guard in `wt.ts` has to tell them apart correctly (`ls` has no
// `stale` or `logPath`).
const REAL_LS_SAMPLE = `[
  {
    "branch": "08-31-dspy-worker_add_json_lens_optimizer_and_eval_harness",
    "created": "2026-08-19T16:27:58.910567Z",
    "dirty": false,
    "id": "01a01ab2-22c3-7711-909d-3db0368fede2",
    "name": "processing/json-tool",
    "path": "/Users/joshbassin/repos/wt/monorepo/trees/01a01ab2-22c3-7711-909d-3db0368fede2",
    "repo": "monorepo",
    "spare": false,
    "startingBranch": "08-31-dspy-worker_add_json_lens_optimizer_and_eval_harness",
    "state": "ready"
  }
]`;

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

describe('treeNew', () => {
  test('builds argv with profiles and returns the printed path', async () => {
    const runner = fakeRunner(() => ({
      code: 0,
      stdout: '/Users/josh/repos/wt/monorepo/trees/abc\n',
      stderr: '',
    }));

    const path = await treeNew(runner, {
      repo: 'monorepo',
      name: 'panopticon-pr-42',
      branch: 'panopticon/pr-42',
      onto: 'deadbeef',
      profiles: ['node', 'python'],
    });

    expect(path).toBe('/Users/josh/repos/wt/monorepo/trees/abc');
    expect(runner.calls).toEqual([
      [
        'tree',
        'new',
        '--name',
        'panopticon-pr-42',
        '--branch',
        'panopticon/pr-42',
        '--onto',
        'deadbeef',
        '--profile',
        'node,python',
        'monorepo',
      ],
    ]);
  });

  test('omits --profile when no profiles are given', async () => {
    const runner = fakeRunner(() => ({
      code: 0,
      stdout: '/tmp/tree\n',
      stderr: '',
    }));

    await treeNew(runner, {
      repo: 'monorepo',
      name: 'panopticon-pr-1',
      branch: 'panopticon/pr-1',
      onto: 'deadbeef',
      profiles: [],
    });

    expect(runner.calls[0]).not.toContain('--profile');
  });

  test('throws with the CLI stderr on failure', async () => {
    const runner = fakeRunner(() => ({
      code: 1,
      stdout: '',
      stderr: 'onto ref not found',
    }));

    await expect(
      treeNew(runner, {
        repo: 'monorepo',
        name: 'panopticon-pr-1',
        branch: 'panopticon/pr-1',
        onto: 'missing-sha',
        profiles: [],
      }),
    ).rejects.toThrow(/onto ref not found/);
  });
});

describe('treeStatus', () => {
  test('parses a real recorded sample', async () => {
    const runner = fakeRunner(() => ({
      code: 0,
      stdout: REAL_STATUS_SAMPLE,
      stderr: '',
    }));

    const status = await treeStatus(runner, 'processing/audit-gemini');

    expect(status).toEqual({
      id: '01a05d70-34f6-7a51-8224-3af565dca3fa',
      name: 'processing/audit-gemini',
      branch: 'josh/processing-audit-gemini',
      repo: 'monorepo',
      path: '/Users/joshbassin/repos/wt/monorepo/trees/01a05d70-34f6-7a51-8224-3af565dca3fa',
      state: 'ready',
      stale: false,
      logPath:
        '/Users/joshbassin/repos/wt/monorepo/trees/01a05d70-34f6-7a51-8224-3af565dca3fa/.wt-provision.log',
    });
    expect(runner.calls).toEqual([
      ['tree', 'status', 'processing/audit-gemini', '--json'],
    ]);
  });

  test('returns null when wt reports no matching tree', async () => {
    const runner = fakeRunner(() => ({
      code: 1,
      stdout: '',
      stderr: "error: no tree matches selector 'nope'",
    }));

    expect(await treeStatus(runner, 'nope')).toBeNull();
  });

  test('returns null on an empty JSON array', async () => {
    const runner = fakeRunner(() => ({ code: 0, stdout: '[]', stderr: '' }));

    expect(await treeStatus(runner, 'some-tree')).toBeNull();
  });

  test('rejects a shape that does not match a status entry', async () => {
    const runner = fakeRunner(() => ({
      code: 0,
      stdout: REAL_LS_SAMPLE,
      stderr: '',
    }));

    await expect(treeStatus(runner, 'some-tree')).rejects.toThrow(
      /unexpected shape/,
    );
  });
});

describe('treePath', () => {
  test('returns the trimmed stdout path', async () => {
    const runner = fakeRunner(() => ({
      code: 0,
      stdout: '/Users/josh/repos/wt/monorepo/trees/abc\n',
      stderr: '',
    }));

    expect(await treePath(runner, 'abc')).toBe(
      '/Users/josh/repos/wt/monorepo/trees/abc',
    );
  });

  test('returns null when wt fails to resolve the tree', async () => {
    const runner = fakeRunner(() => ({
      code: 1,
      stdout: '',
      stderr: "error: no tree matches selector 'nope'",
    }));

    expect(await treePath(runner, 'nope')).toBeNull();
  });
});

describe('treeRm', () => {
  test('builds argv for force removal with branch deletion', async () => {
    const runner = fakeRunner(() => ({ code: 0, stdout: '', stderr: '' }));

    await treeRm(runner, 'panopticon-pr-42', {
      force: true,
      deleteBranch: true,
    });

    expect(runner.calls).toEqual([
      ['tree', 'rm', 'panopticon-pr-42', '--force', '--delete-branch'],
    ]);
  });

  test('throws on failure', async () => {
    const runner = fakeRunner(() => ({
      code: 1,
      stdout: '',
      stderr: 'tree is dirty',
    }));

    await expect(
      treeRm(runner, 'panopticon-pr-42', { force: false, deleteBranch: false }),
    ).rejects.toThrow(/tree is dirty/);
  });
});
