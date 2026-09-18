import { afterEach, describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { PrState, PrSummary } from '../../shared/github.ts';
import { resetGraphiteBaseMemoForTests } from './graphiteBase.ts';
import type {
  GraphiteBranch,
  GraphitePr,
  GraphiteSnapshot,
} from './graphiteLocal.ts';
import { resolveStack, StackError } from './resolve.ts';
import type { BaseRefChange, StackSource } from './source.ts';

const FIXTURES_DIR = join(import.meta.dir, '__fixtures__');
const OWNER = 'Sola-Solutions';
const REPO = 'monorepo';
const TRUNK = 'master';

afterEach(() => {
  resetGraphiteBaseMemoForTests();
});

function readFixture(name: string): unknown {
  return JSON.parse(readFileSync(join(FIXTURES_DIR, name), 'utf-8'));
}

interface RawPull {
  number: number;
  title: string;
  body: string | null;
  state: string;
  draft: boolean;
  merge_commit_sha: string | null;
  additions: number;
  deletions: number;
  changed_files: number;
  created_at: string;
  updated_at: string;
  html_url: string;
  base: { ref: string; sha: string };
  head: { ref: string; sha: string };
  user: { login: string; avatar_url: string; type: string };
}

// The real REST-to-PrSummary mapper lives with the GitHub client; this is a
// trimmed stand-in just for turning recorded fixtures into test input.
function prSummaryFromFixture(name: string): PrSummary {
  const raw = readFixture(name) as RawPull;
  const state: PrState = raw.merge_commit_sha
    ? 'merged'
    : raw.state === 'open'
      ? 'open'
      : 'closed';
  return {
    owner: OWNER,
    repo: REPO,
    number: raw.number,
    title: raw.title,
    body: raw.body ?? '',
    state,
    draft: raw.draft,
    author: {
      login: raw.user.login,
      avatarUrl: raw.user.avatar_url,
      isBot: raw.user.type === 'Bot',
    },
    base: { ref: raw.base.ref, sha: raw.base.sha },
    head: { ref: raw.head.ref, sha: raw.head.sha },
    mergeCommitSha: raw.merge_commit_sha,
    additions: raw.additions,
    deletions: raw.deletions,
    changedFiles: raw.changed_files,
    createdAt: raw.created_at,
    updatedAt: raw.updated_at,
    url: raw.html_url,
  };
}

interface RawTimelineChange {
  previousRefName: string;
  currentRefName: string;
  createdAt: string;
  actor: { login: string } | null;
}

function baseRefChangesFromFixture(name: string): BaseRefChange[] {
  const parsed = readFixture(name) as {
    data: {
      repository: {
        pullRequest: {
          timelineItems: { nodes: RawTimelineChange[] };
        };
      };
    };
  };
  return parsed.data.repository.pullRequest.timelineItems.nodes.map((node) => ({
    previousRefName: node.previousRefName,
    currentRefName: node.currentRefName,
    createdAt: node.createdAt,
    actorLogin: node.actor?.login ?? null,
  }));
}

function makePr(overrides: Partial<PrSummary> & { number: number }): PrSummary {
  return {
    owner: OWNER,
    repo: REPO,
    title: `PR #${overrides.number}`,
    body: '',
    state: 'open',
    draft: false,
    author: { login: 'someone', avatarUrl: null, isBot: false },
    base: { ref: TRUNK, sha: 'base-sha' },
    head: { ref: `branch-${overrides.number}`, sha: 'head-sha' },
    mergeCommitSha: null,
    additions: 1,
    deletions: 1,
    changedFiles: 1,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    url: `https://github.com/${OWNER}/${REPO}/pull/${overrides.number}`,
    ...overrides,
  };
}

// GitHub's list endpoints never carry the diff counts; use this to mirror
// that when building a fake listOpenPulls response so the resolver's
// backfill (a direct getPull) is what supplies the real numbers.
function zeroCounts(pr: PrSummary): PrSummary {
  return { ...pr, additions: 0, deletions: 0, changedFiles: 0 };
}

class FakeStackSource implements StackSource {
  readonly getPullCalls: number[] = [];
  listOpenPullsCalls = 0;

  constructor(
    private readonly pulls: Map<number, PrSummary>,
    private readonly openPulls: PrSummary[] = [],
    private readonly baseRefChanges: Map<number, BaseRefChange[]> = new Map(),
    private readonly byHead: Map<string, PrSummary> = new Map(),
  ) {}

  getPull(number: number): Promise<PrSummary | null> {
    this.getPullCalls.push(number);
    return Promise.resolve(this.pulls.get(number) ?? null);
  }

  listOpenPulls(): Promise<PrSummary[]> {
    this.listOpenPullsCalls++;
    return Promise.resolve(this.openPulls);
  }

  findPullByHead(branch: string): Promise<PrSummary | null> {
    return Promise.resolve(this.byHead.get(branch) ?? null);
  }

  listBaseRefChanges(number: number): Promise<BaseRefChange[]> {
    return Promise.resolve(this.baseRefChanges.get(number) ?? []);
  }
}

describe('resolveStack', () => {
  test('resolves the #12110 chain to bottom #12105 -> #12110', async () => {
    const pr12110 = prSummaryFromFixture('pr-12110.json');
    const pr12105 = prSummaryFromFixture('pr-12105.json');
    const source = new FakeStackSource(
      new Map([
        [12110, pr12110],
        [12105, pr12105],
      ]),
      [],
      new Map([[12110, baseRefChangesFromFixture('pr-12110-timeline.json')]]),
      new Map([[pr12105.head.ref, pr12105]]),
    );

    const result = await resolveStack(source, 12110, TRUNK, null);

    expect(result.truncatedBelow).toBe(false);
    expect(result.entries.map((e) => e.number)).toEqual([12105, 12110]);
    expect(result.entries[0]?.isCurrent).toBe(false);
    expect(result.entries[0]?.parent).toBeNull();
    expect(result.entries[1]?.isCurrent).toBe(true);
    expect(result.entries[1]?.parent).toBe(12105);
    expect(result.entries[1]?.state).toBe('merged');
  });

  test('resolves an open stack bottom to top, walking both directions', async () => {
    const prs = [20816, 20817, 20818, 20819].map((n) =>
      prSummaryFromFixture(`pr-${n}.json`),
    );
    const byNumber = new Map(prs.map((pr) => [pr.number, pr]));
    // listOpenPulls mirrors the real API and omits diff counts; getPull
    // (backed by byNumber) is the only source of the real numbers.
    const source = new FakeStackSource(byNumber, prs.map(zeroCounts));

    const result = await resolveStack(source, 20818, TRUNK, null);

    expect(result.truncatedBelow).toBe(false);
    expect(result.entries.map((e) => e.number)).toEqual([
      20816, 20817, 20818, 20819,
    ]);
    const current = result.entries.find((e) => e.isCurrent);
    expect(current?.number).toBe(20818);
    expect(result.entries.find((e) => e.number === 20819)?.draft).toBe(true);

    const byNumberEntry = new Map(result.entries.map((e) => [e.number, e]));
    expect(byNumberEntry.get(20816)).toMatchObject({
      parent: null,
      additions: 1115,
      deletions: 24,
    });
    expect(byNumberEntry.get(20817)).toMatchObject({
      parent: 20816,
      additions: 733,
      deletions: 4,
    });
    expect(byNumberEntry.get(20818)).toMatchObject({
      parent: 20817,
      additions: 2861,
      deletions: 20,
    });
    expect(byNumberEntry.get(20819)).toMatchObject({
      parent: 20818,
      additions: 139,
      deletions: 40,
    });

    // Every zero-count entry (everything but the target) gets backfilled by
    // a direct getPull, and the target is only ever fetched once.
    expect(source.getPullCalls.filter((n) => n === 20818)).toEqual([20818]);
    expect(source.getPullCalls).toEqual(
      expect.arrayContaining([20816, 20817, 20819]),
    );
  });

  test('keeps every open fork, not just the newest sibling', async () => {
    const target = makePr({ number: 10, base: { ref: TRUNK, sha: 'b' } });
    const fork1 = makePr({
      number: 11,
      base: { ref: target.head.ref, sha: 'h' },
    });
    const fork2 = makePr({
      number: 12,
      base: { ref: target.head.ref, sha: 'h' },
    });
    const grandchild = makePr({
      number: 13,
      base: { ref: fork1.head.ref, sha: 'h' },
    });
    const source = new FakeStackSource(new Map([[10, target]]), [
      fork1,
      fork2,
      grandchild,
    ]);

    const result = await resolveStack(source, 10, TRUNK, null);

    expect(
      result.entries.map((e) => ({ number: e.number, parent: e.parent })),
    ).toEqual([
      { number: 10, parent: null },
      { number: 11, parent: 10 },
      { number: 12, parent: 10 },
      { number: 13, parent: 11 },
    ]);
    expect(result.entries[0]?.isCurrent).toBe(true);
  });

  test('returns no entries for a standalone PR', async () => {
    const standalone = makePr({ number: 1, base: { ref: TRUNK, sha: 's' } });
    const source = new FakeStackSource(new Map([[1, standalone]]), []);

    const result = await resolveStack(source, 1, TRUNK, null);

    expect(result.entries).toEqual([]);
    expect(result.truncatedBelow).toBe(false);
  });

  test('resolves a graphite-base/<own number> placeholder through history to an open PR', async () => {
    // Graphite names the placeholder after the PR itself, not the PR below
    // it, so the real parent only comes from the base-change history.
    const bottom = makePr({
      number: 100,
      head: { ref: 'branch-100', sha: 'h' },
      base: { ref: TRUNK, sha: 'b' },
    });
    const target = makePr({
      number: 101,
      base: { ref: 'graphite-base/101', sha: 'b' },
    });
    const source = new FakeStackSource(
      new Map([
        [100, bottom],
        [101, target],
      ]),
      [bottom],
      new Map([
        [
          101,
          [
            {
              previousRefName: 'branch-100',
              currentRefName: 'graphite-base/101',
              createdAt: '2026-01-01T00:00:00Z',
              actorLogin: 'graphite-app',
            },
          ],
        ],
      ]),
    );

    const result = await resolveStack(source, 101, TRUNK, null);

    expect(result.entries.map((e) => e.number)).toEqual([100, 101]);
    expect(result.entries[0]?.parent).toBeNull();
    expect(result.entries[1]?.parent).toBe(100);
    expect(result.truncatedBelow).toBe(false);
  });

  test('an open PR pinned to a placeholder resolves via history onto the target and shows up above it', async () => {
    const target = makePr({
      number: 110,
      head: { ref: 'branch-110', sha: 'h' },
      base: { ref: TRUNK, sha: 'b' },
    });
    const above = makePr({
      number: 111,
      base: { ref: 'graphite-base/111', sha: 'b' },
    });
    const source = new FakeStackSource(
      new Map([[110, target]]),
      [above],
      new Map([
        [
          111,
          [
            {
              previousRefName: 'branch-110',
              currentRefName: 'graphite-base/111',
              createdAt: '2026-01-01T00:00:00Z',
              actorLogin: 'graphite-app',
            },
          ],
        ],
      ]),
    );

    const result = await resolveStack(source, 110, TRUNK, null);

    expect(
      result.entries.map((e) => ({ number: e.number, parent: e.parent })),
    ).toEqual([
      { number: 110, parent: null },
      { number: 111, parent: 110 },
    ]);
  });

  test('marks truncatedBelow when a branch cannot be resolved to a PR', async () => {
    const target = makePr({
      number: 200,
      head: { ref: 'branch-200', sha: 'h' },
      base: { ref: 'someone/deleted-branch', sha: 'b' },
    });
    const above = makePr({
      number: 201,
      base: { ref: 'branch-200', sha: 'h' },
    });
    const source = new FakeStackSource(
      new Map([
        [200, target],
        [201, above],
      ]),
      [above],
    );

    const result = await resolveStack(source, 200, TRUNK, null);

    expect(result.truncatedBelow).toBe(true);
    expect(result.entries.map((e) => e.number)).toEqual([200, 201]);
    expect(result.entries[0]?.parent).toBeNull();
    expect(result.entries[1]?.parent).toBe(200);
  });

  test('terminates when the base chain cycles', async () => {
    const a = makePr({
      number: 300,
      head: { ref: 'branch-a', sha: 'h' },
      base: { ref: 'branch-b', sha: 'b' },
    });
    const b = makePr({
      number: 301,
      head: { ref: 'branch-b', sha: 'h' },
      base: { ref: 'branch-a', sha: 'b' },
    });
    const source = new FakeStackSource(
      new Map([
        [300, a],
        [301, b],
      ]),
      [],
      new Map(),
      new Map([
        ['branch-b', b],
        ['branch-a', a],
      ]),
    );

    const result = await resolveStack(source, 300, TRUNK, null);

    expect(result.entries.length).toBeLessThan(10);
  });

  test('throws a 404 StackError when the target PR is missing', async () => {
    const source = new FakeStackSource(new Map(), []);

    await expect(resolveStack(source, 999, TRUNK, null)).rejects.toThrow(
      StackError,
    );
  });
});

function graphitePr(
  overrides: Partial<GraphitePr> & { number: number },
): GraphitePr {
  return {
    headRef: `branch-${overrides.number}`,
    baseRef: TRUNK,
    state: 'OPEN',
    title: `PR #${overrides.number}`,
    draft: false,
    ...overrides,
  };
}

function graphiteBranch(
  name: string,
  parent: string | null,
  children: string[] = [],
): GraphiteBranch {
  return { name, parent, children };
}

function graphiteSnapshot(
  branches: GraphiteBranch[],
  prs: GraphitePr[],
): GraphiteSnapshot {
  return {
    trunk: TRUNK,
    branches: new Map(branches.map((b) => [b.name, b])),
    prsByHead: new Map(prs.map((p) => [p.headRef, p])),
    prsByNumber: new Map(prs.map((p) => [p.number, p])),
  };
}

describe('resolveStack (local Graphite metadata)', () => {
  test('resolves a merged #12110 -> #12115 -> #12119 chain below an open target', async () => {
    const bottom = graphitePr({ number: 12110, state: 'MERGED' });
    const middle = graphitePr({
      number: 12115,
      state: 'MERGED',
      baseRef: bottom.headRef,
    });
    const top = graphitePr({ number: 12119, baseRef: middle.headRef });
    const snapshot = graphiteSnapshot(
      [
        graphiteBranch(bottom.headRef, TRUNK, [middle.headRef]),
        graphiteBranch(middle.headRef, bottom.headRef, [top.headRef]),
        graphiteBranch(top.headRef, middle.headRef, []),
      ],
      [bottom, middle, top],
    );
    const source = new FakeStackSource(
      new Map([
        [
          12110,
          makePr({
            number: 12110,
            state: 'merged',
            head: { ref: bottom.headRef, sha: 'h' },
          }),
        ],
        [
          12115,
          makePr({
            number: 12115,
            state: 'merged',
            head: { ref: middle.headRef, sha: 'h' },
          }),
        ],
        [
          12119,
          makePr({ number: 12119, head: { ref: top.headRef, sha: 'h' } }),
        ],
      ]),
    );

    const result = await resolveStack(source, 12119, TRUNK, snapshot);

    expect(result.truncatedBelow).toBe(false);
    expect(result.entries.map((e) => e.number)).toEqual([12110, 12115, 12119]);
    expect(result.entries[0]?.parent).toBeNull();
    expect(result.entries[1]?.parent).toBe(12110);
    expect(result.entries[2]?.parent).toBe(12115);
    expect(result.entries[2]?.isCurrent).toBe(true);
    expect(result.entries[0]?.state).toBe('merged');
  });

  test('resolves an open stack with a fork above', async () => {
    const target = graphitePr({ number: 1 });
    const forkA = graphitePr({ number: 3, baseRef: target.headRef });
    const forkB = graphitePr({ number: 2, baseRef: target.headRef });
    const snapshot = graphiteSnapshot(
      [
        graphiteBranch(target.headRef, TRUNK, [forkA.headRef, forkB.headRef]),
        graphiteBranch(forkA.headRef, target.headRef, []),
        graphiteBranch(forkB.headRef, target.headRef, []),
      ],
      [target, forkA, forkB],
    );
    const source = new FakeStackSource(
      new Map([
        [1, makePr({ number: 1, head: { ref: target.headRef, sha: 'h' } })],
        [2, makePr({ number: 2, head: { ref: forkB.headRef, sha: 'h' } })],
        [3, makePr({ number: 3, head: { ref: forkA.headRef, sha: 'h' } })],
      ]),
    );

    const result = await resolveStack(source, 1, TRUNK, snapshot);

    expect(
      result.entries.map((e) => ({ number: e.number, parent: e.parent })),
    ).toEqual([
      { number: 1, parent: null },
      { number: 2, parent: 1 },
      { number: 3, parent: 1 },
    ]);
  });

  test('marks truncatedBelow when the parent branch has no PR', async () => {
    const target = graphitePr({ number: 1, baseRef: 'someone/deleted' });
    const above = graphitePr({ number: 2, baseRef: target.headRef });
    const snapshot = graphiteSnapshot(
      [
        graphiteBranch(target.headRef, 'someone/deleted', [above.headRef]),
        graphiteBranch(above.headRef, target.headRef, []),
      ],
      [target, above],
    );
    const source = new FakeStackSource(
      new Map([
        [1, makePr({ number: 1, head: { ref: target.headRef, sha: 'h' } })],
        [2, makePr({ number: 2, head: { ref: above.headRef, sha: 'h' } })],
      ]),
    );

    const result = await resolveStack(source, 1, TRUNK, snapshot);

    expect(result.truncatedBelow).toBe(true);
    expect(result.entries.map((e) => e.number)).toEqual([1, 2]);
  });

  test('marks truncatedBelow for an untracked head branch (null parent)', async () => {
    const target = graphitePr({ number: 1 });
    const above = graphitePr({ number: 2, baseRef: target.headRef });
    const snapshot = graphiteSnapshot(
      [
        graphiteBranch(target.headRef, null, [above.headRef]),
        graphiteBranch(above.headRef, target.headRef, []),
      ],
      [target, above],
    );
    const source = new FakeStackSource(
      new Map([
        [1, makePr({ number: 1, head: { ref: target.headRef, sha: 'h' } })],
        [2, makePr({ number: 2, head: { ref: above.headRef, sha: 'h' } })],
      ]),
    );

    const result = await resolveStack(source, 1, TRUNK, snapshot);

    expect(result.truncatedBelow).toBe(true);
    expect(result.entries.map((e) => e.number)).toEqual([1, 2]);
  });

  test('returns no entries for a standalone PR in the local walk', async () => {
    const target = graphitePr({ number: 1 });
    const snapshot = graphiteSnapshot(
      [graphiteBranch(target.headRef, TRUNK, [])],
      [target],
    );
    const source = new FakeStackSource(
      new Map([
        [1, makePr({ number: 1, head: { ref: target.headRef, sha: 'h' } })],
      ]),
    );

    const result = await resolveStack(source, 1, TRUNK, snapshot);

    expect(result.entries).toEqual([]);
    expect(result.truncatedBelow).toBe(false);
  });

  test('a PR unknown to the snapshot falls back to the GitHub walk', async () => {
    const target = makePr({ number: 500, base: { ref: TRUNK, sha: 'b' } });
    const snapshot = graphiteSnapshot([], []);
    const source = new FakeStackSource(new Map([[500, target]]), []);

    const result = await resolveStack(source, 500, TRUNK, snapshot);

    expect(result.entries).toEqual([]);
    expect(source.listOpenPullsCalls).toBeGreaterThan(0);
  });

  test('falls back to the GraphitePr fields when getPull returns null', async () => {
    const target = graphitePr({ number: 1, title: 'Graphite-only title' });
    const above = graphitePr({ number: 2, baseRef: target.headRef });
    const snapshot = graphiteSnapshot(
      [
        graphiteBranch(target.headRef, TRUNK, [above.headRef]),
        graphiteBranch(above.headRef, target.headRef, []),
      ],
      [target, above],
    );
    const source = new FakeStackSource(
      new Map([
        [2, makePr({ number: 2, head: { ref: above.headRef, sha: 'h' } })],
      ]),
    );

    const result = await resolveStack(source, 1, TRUNK, snapshot);

    const current = result.entries.find((e) => e.number === 1);
    expect(current).toMatchObject({
      title: 'Graphite-only title',
      state: 'open',
      draft: false,
      headRef: target.headRef,
      baseRef: TRUNK,
      additions: 0,
      deletions: 0,
    });
  });

  test('a parent cycle in local metadata terminates', async () => {
    const a = graphitePr({
      number: 300,
      headRef: 'branch-a',
      baseRef: 'branch-b',
    });
    const b = graphitePr({
      number: 301,
      headRef: 'branch-b',
      baseRef: 'branch-a',
    });
    const snapshot = graphiteSnapshot(
      [
        graphiteBranch('branch-a', 'branch-b', []),
        graphiteBranch('branch-b', 'branch-a', []),
      ],
      [a, b],
    );
    const source = new FakeStackSource(
      new Map([
        [300, makePr({ number: 300, head: { ref: 'branch-a', sha: 'h' } })],
        [301, makePr({ number: 301, head: { ref: 'branch-b', sha: 'h' } })],
      ]),
    );

    const result = await resolveStack(source, 300, TRUNK, snapshot);

    expect(result.entries.length).toBeLessThan(10);
  });
});
