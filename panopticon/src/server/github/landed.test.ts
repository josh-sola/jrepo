import { describe, expect, test } from 'bun:test';
import type { PrSummary } from '../../shared/github.ts';
import type { RepoStore } from '../git/store.ts';
import { withLandedState } from './landed.ts';

function fakePr(overrides: Partial<PrSummary> = {}): PrSummary {
  return {
    owner: 'Sola-Solutions',
    repo: 'monorepo',
    number: 1,
    title: 'A pull request',
    body: '',
    state: 'closed',
    draft: false,
    author: { login: 'jordan-sola', avatarUrl: null, isBot: false },
    base: { ref: 'master', sha: 'base-sha' },
    head: { ref: 'feature', sha: 'head-sha' },
    mergeCommitSha: null,
    additions: 0,
    deletions: 0,
    changedFiles: 0,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    url: 'https://github.com/Sola-Solutions/monorepo/pull/1',
    ...overrides,
  };
}

interface FakeStoreCalls {
  fetchTrunk: number;
  landedCommitArgs: { trunk: string; number: number; since: string }[];
}

function fakeStore(landedSha: string | null, calls: FakeStoreCalls): RepoStore {
  const store = {
    fetchTrunk: async () => {
      calls.fetchTrunk += 1;
    },
    landedCommit: async (trunk: string, number: number, since: string) => {
      calls.landedCommitArgs.push({ trunk, number, since });
      return landedSha;
    },
  };
  return store as unknown as RepoStore;
}

describe('withLandedState', () => {
  test('returns an open PR unchanged without touching git', async () => {
    const calls: FakeStoreCalls = { fetchTrunk: 0, landedCommitArgs: [] };
    const store = fakeStore('deadbeef', calls);
    const pr = fakePr({ state: 'open' });

    const result = await withLandedState(pr, store, 'master');

    expect(result).toEqual(pr);
    expect(calls.fetchTrunk).toBe(0);
    expect(calls.landedCommitArgs.length).toBe(0);
  });

  test('promotes a closed PR to merged when its squash commit is on trunk', async () => {
    const calls: FakeStoreCalls = { fetchTrunk: 0, landedCommitArgs: [] };
    const store = fakeStore('cafef00d', calls);
    const pr = fakePr({ state: 'closed', number: 20744 });

    const result = await withLandedState(pr, store, 'master');

    expect(result.state).toBe('merged');
    expect(result.mergeCommitSha).toBe('cafef00d');
    expect(calls.landedCommitArgs).toEqual([
      { trunk: 'master', number: 20744, since: pr.createdAt },
    ]);
  });

  test('leaves a closed PR closed when no squash commit lands', async () => {
    const calls: FakeStoreCalls = { fetchTrunk: 0, landedCommitArgs: [] };
    const store = fakeStore(null, calls);
    const pr = fakePr({ state: 'closed' });

    const result = await withLandedState(pr, store, 'master');

    expect(result).toEqual(pr);
  });

  test('fetches trunk once per store even across several closed-PR checks', async () => {
    const calls: FakeStoreCalls = { fetchTrunk: 0, landedCommitArgs: [] };
    const store = fakeStore('cafef00d', calls);

    await Promise.all([
      withLandedState(fakePr({ number: 1 }), store, 'master'),
      withLandedState(fakePr({ number: 2 }), store, 'master'),
      withLandedState(fakePr({ number: 3 }), store, 'master'),
    ]);

    expect(calls.fetchTrunk).toBe(1);
    expect(calls.landedCommitArgs.length).toBe(3);
  });
});
