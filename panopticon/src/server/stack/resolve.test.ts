import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { PrState, PrSummary } from '../../shared/github.ts';
import { resolveStack, StackError } from './resolve.ts';
import type { BaseRefChange, StackSource } from './source.ts';

const FIXTURES_DIR = join(import.meta.dir, '__fixtures__');
const OWNER = 'Sola-Solutions';
const REPO = 'monorepo';
const TRUNK = 'master';

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

class FakeStackSource implements StackSource {
  constructor(
    private readonly pulls: Map<number, PrSummary>,
    private readonly openPulls: PrSummary[] = [],
    private readonly baseRefChanges: Map<number, BaseRefChange[]> = new Map(),
    private readonly byHead: Map<string, PrSummary> = new Map(),
  ) {}

  getPull(number: number): Promise<PrSummary | null> {
    return Promise.resolve(this.pulls.get(number) ?? null);
  }

  listOpenPulls(): Promise<PrSummary[]> {
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

    const result = await resolveStack(source, 12110, TRUNK);

    expect(result.truncatedBelow).toBe(false);
    expect(result.entries.map((e) => e.number)).toEqual([12105, 12110]);
    expect(result.entries[0]?.isCurrent).toBe(false);
    expect(result.entries[1]?.isCurrent).toBe(true);
    expect(result.entries[1]?.state).toBe('merged');
  });

  test('resolves an open stack bottom to top, walking both directions', async () => {
    const prs = [20816, 20817, 20818, 20819].map((n) =>
      prSummaryFromFixture(`pr-${n}.json`),
    );
    const byNumber = new Map(prs.map((pr) => [pr.number, pr]));
    const source = new FakeStackSource(byNumber, prs);

    const result = await resolveStack(source, 20818, TRUNK);

    expect(result.truncatedBelow).toBe(false);
    expect(result.entries.map((e) => e.number)).toEqual([
      20816, 20817, 20818, 20819,
    ]);
    const current = result.entries.find((e) => e.isCurrent);
    expect(current?.number).toBe(20818);
    expect(result.entries.find((e) => e.number === 20819)?.draft).toBe(true);
  });

  test('returns no entries for a standalone PR', async () => {
    const standalone = makePr({ number: 1, base: { ref: TRUNK, sha: 's' } });
    const source = new FakeStackSource(new Map([[1, standalone]]), []);

    const result = await resolveStack(source, 1, TRUNK);

    expect(result.entries).toEqual([]);
    expect(result.truncatedBelow).toBe(false);
  });

  test('resolves a graphite-base/<n> base through getPull', async () => {
    const bottom = makePr({
      number: 100,
      head: { ref: 'branch-100', sha: 'h' },
      base: { ref: TRUNK, sha: 'b' },
    });
    const target = makePr({
      number: 101,
      base: { ref: 'graphite-base/100', sha: 'b' },
    });
    const source = new FakeStackSource(
      new Map([
        [100, bottom],
        [101, target],
      ]),
      [],
    );

    const result = await resolveStack(source, 101, TRUNK);

    expect(result.entries.map((e) => e.number)).toEqual([100, 101]);
    expect(result.truncatedBelow).toBe(false);
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

    const result = await resolveStack(source, 200, TRUNK);

    expect(result.truncatedBelow).toBe(true);
    expect(result.entries.map((e) => e.number)).toEqual([200, 201]);
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

    const result = await resolveStack(source, 300, TRUNK);

    expect(result.entries.length).toBeLessThan(10);
  });

  test('throws a 404 StackError when the target PR is missing', async () => {
    const source = new FakeStackSource(new Map(), []);

    await expect(resolveStack(source, 999, TRUNK)).rejects.toThrow(StackError);
  });
});
