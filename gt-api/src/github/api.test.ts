import { describe, expect, test } from 'bun:test';
import pull20731 from './__fixtures__/pull-20731.json' with { type: 'json' };
import pullsOpen from './__fixtures__/pulls-open.json' with { type: 'json' };
import { createOctokitGitHubApi, GitHubError } from './api.ts';
import type { OctokitResponseLike, RestClient } from './api.ts';
import { ConditionalFetcher, InMemoryEtagStore } from './conditional.ts';

function ok(data: unknown, etag: string | null = null): OctokitResponseLike {
  return { data, headers: etag ? { etag } : {}, status: 200 };
}

function notModified(): never {
  throw { status: 304, message: 'Not Modified' };
}

interface RestStubOverrides {
  pulls?: Partial<RestClient['pulls']>;
}

// A REST client that never sees a GraphQL call and only ever answers the one
// REST endpoint under test; every other method throws if it is reached.
function stubRest(overrides: RestStubOverrides): RestClient {
  const unexpected =
    (name: string): (() => Promise<never>) =>
    async () => {
      throw new Error(`unexpected call to ${name}`);
    };
  return {
    pulls: {
      get: unexpected('pulls.get'),
      list: unexpected('pulls.list'),
      ...overrides.pulls,
    },
  };
}

const noGraphql = () => {
  throw new Error('unexpected GraphQL call');
};

describe('getPull', () => {
  test('a closed PR with merge_commit_sha but no merged_at (Graphite queue) is not yet merged at this layer', async () => {
    const rest = stubRest({
      pulls: { get: async () => ok(pull20731) },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const pr = await api.getPull('Sola-Solutions', 'monorepo', 20731);
    expect(pr.state).toBe('closed');
    expect(pr.mergeCommitSha).toBeNull();
    expect(pr.author).toEqual({
      login: 'jordan-sola',
      avatarUrl: 'https://avatars.githubusercontent.com/u/304500111?v=4',
      isBot: false,
    });
  });

  test('derives merged from a non-null merged_at', async () => {
    const rest = stubRest({
      pulls: {
        get: async () =>
          ok({ ...pull20731, merged_at: '2026-09-17T19:14:39Z' }),
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const pr = await api.getPull('Sola-Solutions', 'monorepo', 20731);
    expect(pr.state).toBe('merged');
    expect(pr.mergeCommitSha).toBe('533d98f2e844d15ce902c92eb216482dc615c48d');
  });

  test('keeps an open PR open even though GitHub reports a test merge sha', async () => {
    const rest = stubRest({
      pulls: { get: async () => ok({ ...pull20731, state: 'open' }) },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const pr = await api.getPull('Sola-Solutions', 'monorepo', 20731);
    expect(pr.state).toBe('open');
    expect(pr.mergeCommitSha).toBeNull();
  });

  test('reports open and closed-without-merge states from their raw fields', async () => {
    const base = { ...pull20731, merge_commit_sha: null };
    let call = 0;
    const rest = stubRest({
      pulls: {
        get: async () => {
          call += 1;
          return ok({ ...base, state: call === 1 ? 'open' : 'closed' });
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    expect((await api.getPull('o', 'r', 1)).state).toBe('open');
    expect((await api.getPull('o', 'r', 1)).state).toBe('closed');
  });

  test('flags a Bot user type and a [bot]-suffixed login as a bot', async () => {
    const rest = stubRest({
      pulls: {
        get: async () =>
          ok({
            ...pull20731,
            user: { login: 'vercel[bot]', type: 'Bot', avatar_url: null },
          }),
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const pr = await api.getPull('o', 'r', 1);
    expect(pr.author.isBot).toBe(true);
  });

  test('caches the etag and returns the cached body on a 304', async () => {
    const store = new InMemoryEtagStore();
    const fetcher = new ConditionalFetcher(store);
    let calls = 0;
    const rest = stubRest({
      pulls: {
        get: async ({ headers }) => {
          calls += 1;
          if (headers?.['if-none-match'] === 'W/"abc"') notModified();
          return ok(pull20731, 'W/"abc"');
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql, fetcher);

    const first = await api.getPull('Sola-Solutions', 'monorepo', 20731);
    const second = await api.getPull('Sola-Solutions', 'monorepo', 20731);

    expect(calls).toBe(2);
    expect(second).toEqual(first);
  });

  test('wraps a GitHub failure in a GitHubError carrying its status and message', async () => {
    const rest = stubRest({
      pulls: {
        get: async () => {
          throw { status: 404, response: { data: { message: 'Not Found' } } };
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    await expect(api.getPull('o', 'r', 1)).rejects.toThrow(GitHubError);
    try {
      await api.getPull('o', 'r', 1);
      throw new Error('expected getPull to reject');
    } catch (error) {
      expect(error).toBeInstanceOf(GitHubError);
      expect((error as GitHubError).status).toBe(404);
      expect((error as GitHubError).message).toBe('Not Found');
    }
  });
});

describe('listOpenPulls', () => {
  test('sorts the result by PR number', async () => {
    const rest = stubRest({
      pulls: {
        list: async () =>
          ok([
            { ...pull20731, number: 30 },
            { ...pull20731, number: 10 },
            { ...pull20731, number: 20 },
          ]),
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const pulls = await api.listOpenPulls('o', 'r');
    expect(pulls.map((p) => p.number)).toEqual([10, 20, 30]);
  });

  test('maps real list-endpoint items, which omit additions/deletions/changed_files, defaulting the counts to 0', async () => {
    const rest = stubRest({
      pulls: { list: async () => ok(pullsOpen) },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const pulls = await api.listOpenPulls('Sola-Solutions', 'monorepo');

    expect(pulls.length).toBe((pullsOpen as unknown[]).length);
    for (const pr of pulls) {
      expect(pr.additions).toBe(0);
      expect(pr.deletions).toBe(0);
      expect(pr.changedFiles).toBe(0);
    }
    const bot = pulls.find((p) => p.number === 20924);
    expect(bot?.author.isBot).toBe(true);
  });
});

describe('findPullByHead', () => {
  test('returns null when no PR matches the head branch', async () => {
    const rest = stubRest({
      pulls: { list: async () => ok([]) },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    expect(await api.findPullByHead('o', 'r', 'no-such-branch')).toBeNull();
  });

  test('returns the newest match when several PRs share a head', async () => {
    const rest = stubRest({
      pulls: {
        list: async () =>
          ok([
            { ...pull20731, number: 1, created_at: '2026-01-01T00:00:00Z' },
            { ...pull20731, number: 2, created_at: '2026-06-01T00:00:00Z' },
          ]),
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const found = await api.findPullByHead('o', 'r', 'some-branch');
    expect(found?.number).toBe(2);
  });

  test('maps a real head-filtered list item, which also omits the counts', async () => {
    const [singleMatch] = pullsOpen as unknown[];
    const rest = stubRest({
      pulls: { list: async () => ok([singleMatch]) },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const found = await api.findPullByHead(
      'Sola-Solutions',
      'monorepo',
      'helmsync/helm-08e8a63',
    );
    expect(found?.number).toBe(20924);
    expect(found?.additions).toBe(0);
    expect(found?.deletions).toBe(0);
    expect(found?.changedFiles).toBe(0);
  });
});
