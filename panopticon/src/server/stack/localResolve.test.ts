import { describe, expect, test } from 'bun:test';
import type {
  GraphiteBranch,
  GraphitePr,
  GraphiteSnapshot,
} from './graphiteLocal.ts';
import { walkLocalStack } from './localResolve.ts';

function pr(overrides: Partial<GraphitePr> & { number: number }): GraphitePr {
  return {
    headRef: `branch-${overrides.number}`,
    baseRef: 'master',
    state: 'OPEN',
    title: `PR #${overrides.number}`,
    draft: false,
    ...overrides,
  };
}

function branch(
  name: string,
  parent: string | null,
  children: string[] = [],
): GraphiteBranch {
  return { name, parent, children };
}

function makeSnapshot(
  trunk: string,
  branches: GraphiteBranch[],
  prs: GraphitePr[],
): GraphiteSnapshot {
  return {
    trunk,
    branches: new Map(branches.map((b) => [b.name, b])),
    prsByHead: new Map(prs.map((p) => [p.headRef, p])),
    prsByNumber: new Map(prs.map((p) => [p.number, p])),
  };
}

describe('walkLocalStack', () => {
  test('returns null when the snapshot has never seen the PR number', () => {
    const snapshot = makeSnapshot('master', [], []);

    expect(walkLocalStack(snapshot, 1)).toBeNull();
  });

  test('returns null when the head branch is missing from branch_metadata', () => {
    const target = pr({ number: 1 });
    const snapshot = makeSnapshot('master', [], [target]);

    expect(walkLocalStack(snapshot, 1)).toBeNull();
  });

  test('walks a merged chain below an open PR to trunk', () => {
    const bottom = pr({ number: 12110, state: 'MERGED' });
    const middle = pr({
      number: 12115,
      state: 'MERGED',
      baseRef: bottom.headRef,
    });
    const top = pr({ number: 12119, baseRef: middle.headRef });
    const snapshot = makeSnapshot(
      'master',
      [
        branch(bottom.headRef, 'master', [middle.headRef]),
        branch(middle.headRef, bottom.headRef, [top.headRef]),
        branch(top.headRef, middle.headRef, []),
      ],
      [bottom, middle, top],
    );

    const walk = walkLocalStack(snapshot, 12119);

    expect(walk?.current.number).toBe(12119);
    expect(walk?.below.map((p) => p.number)).toEqual([12115, 12110]);
    expect(walk?.truncatedBelow).toBe(false);
    expect(walk?.above).toEqual([]);
  });

  test('walks up through a fork, keeping every open child and dropping merged ones', () => {
    const target = pr({ number: 1 });
    const openForkA = pr({ number: 3, baseRef: target.headRef });
    const openForkB = pr({ number: 2, baseRef: target.headRef });
    const mergedFork = pr({
      number: 4,
      baseRef: target.headRef,
      state: 'MERGED',
    });
    const grandchild = pr({ number: 5, baseRef: openForkB.headRef });
    const snapshot = makeSnapshot(
      'master',
      [
        branch(target.headRef, 'master', [
          openForkA.headRef,
          openForkB.headRef,
          mergedFork.headRef,
        ]),
        branch(openForkA.headRef, target.headRef, []),
        branch(openForkB.headRef, target.headRef, [grandchild.headRef]),
        branch(mergedFork.headRef, target.headRef, []),
        branch(grandchild.headRef, openForkB.headRef, []),
      ],
      [target, openForkA, openForkB, mergedFork, grandchild],
    );

    const walk = walkLocalStack(snapshot, 1);

    expect(
      walk?.above.map((a) => ({ number: a.pr.number, parent: a.parent })),
    ).toEqual([
      { number: 2, parent: 1 },
      { number: 3, parent: 1 },
      { number: 5, parent: 2 },
    ]);
  });

  test('truncates below when the parent branch has no PR', () => {
    const target = pr({ number: 1 });
    const snapshot = makeSnapshot(
      'master',
      [branch(target.headRef, 'someone/local-only-branch', [])],
      [target],
    );

    const walk = walkLocalStack(snapshot, 1);

    expect(walk?.below).toEqual([]);
    expect(walk?.truncatedBelow).toBe(true);
  });

  test('truncates below when the branch is untracked (null parent)', () => {
    const target = pr({ number: 1 });
    const snapshot = makeSnapshot(
      'master',
      [branch(target.headRef, null, [])],
      [target],
    );

    const walk = walkLocalStack(snapshot, 1);

    expect(walk?.below).toEqual([]);
    expect(walk?.truncatedBelow).toBe(true);
  });

  test('a standalone PR based on trunk has no below and no above', () => {
    const target = pr({ number: 1 });
    const snapshot = makeSnapshot(
      'master',
      [branch(target.headRef, 'master', [])],
      [target],
    );

    const walk = walkLocalStack(snapshot, 1);

    expect(walk?.below).toEqual([]);
    expect(walk?.above).toEqual([]);
    expect(walk?.truncatedBelow).toBe(false);
  });

  test('a parent cycle terminates instead of looping forever', () => {
    const a = pr({ number: 1, baseRef: 'branch-2' });
    const b = pr({ number: 2, baseRef: 'branch-1' });
    const snapshot = makeSnapshot(
      'master',
      [branch('branch-1', 'branch-2', []), branch('branch-2', 'branch-1', [])],
      [a, b],
    );

    const walk = walkLocalStack(snapshot, 1);

    expect(walk?.below.length).toBeLessThan(5);
    expect(walk?.truncatedBelow).toBe(false);
  });
});
