import { describe, expect, test } from 'bun:test';
import type { PrSummary } from '../../shared/github.ts';
import type { GtApiClient } from '../gtapi/client.ts';
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

interface LandedCall {
  owner: string;
  repo: string;
  number: number;
  createdAt: string;
}

function fakeGtApi(
  landed: () => Promise<string | null>,
  calls: LandedCall[] = [],
): GtApiClient {
  return {
    snapshot: async () => null,
    stack: async () => ({ status: 200, body: null }),
    landed: async (owner, repo, number, createdAt) => {
      calls.push({ owner, repo, number, createdAt });
      return landed();
    },
  };
}

describe('withLandedState', () => {
  test('returns an open PR unchanged without calling gt-api', async () => {
    const calls: LandedCall[] = [];
    const gtApi = fakeGtApi(async () => 'deadbeef', calls);
    const pr = fakePr({ state: 'open' });

    const result = await withLandedState(pr, gtApi);

    expect(result).toEqual(pr);
    expect(calls).toEqual([]);
  });

  test('promotes a closed PR to merged when gt-api finds its squash commit', async () => {
    const calls: LandedCall[] = [];
    const gtApi = fakeGtApi(async () => 'cafef00d', calls);
    const pr = fakePr({ state: 'closed', number: 20744 });

    const result = await withLandedState(pr, gtApi);

    expect(result.state).toBe('merged');
    expect(result.mergeCommitSha).toBe('cafef00d');
    expect(calls).toEqual([
      {
        owner: 'Sola-Solutions',
        repo: 'monorepo',
        number: 20744,
        createdAt: pr.createdAt,
      },
    ]);
  });

  test('leaves a closed PR closed when gt-api finds no squash commit', async () => {
    const gtApi = fakeGtApi(async () => null);
    const pr = fakePr({ state: 'closed' });

    expect(await withLandedState(pr, gtApi)).toEqual(pr);
  });

  test('leaves a closed PR untouched when gt-api fails', async () => {
    const gtApi = fakeGtApi(async () => {
      throw new Error('gt-api is down');
    });
    const pr = fakePr({ state: 'closed' });
    const warn = console.warn;
    console.warn = () => {};
    try {
      expect(await withLandedState(pr, gtApi)).toEqual(pr);
    } finally {
      console.warn = warn;
    }
  });
});
