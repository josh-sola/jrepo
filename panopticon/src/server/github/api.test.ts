import { describe, expect, test } from 'bun:test';
import pull20731Files from './__fixtures__/pull-20731-files.json' with { type: 'json' };
import pull20731IssueComments from './__fixtures__/pull-20731-issue-comments.json' with { type: 'json' };
import pull20731Reviews from './__fixtures__/pull-20731-reviews.json' with { type: 'json' };
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
  issues?: Partial<RestClient['issues']>;
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
      listFiles: unexpected('pulls.listFiles'),
      listReviews: unexpected('pulls.listReviews'),
      createReviewComment: unexpected('pulls.createReviewComment'),
      createReplyForReviewComment: unexpected(
        'pulls.createReplyForReviewComment',
      ),
      ...overrides.pulls,
    },
    issues: {
      listComments: unexpected('issues.listComments'),
      ...overrides.issues,
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

describe('listPullFiles', () => {
  test('fills PrFile from the file list with null oids', async () => {
    const rest = stubRest({
      pulls: {
        listFiles: async () => ok(pull20731Files),
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const { files, truncated } = await api.listPullFiles(
      'Sola-Solutions',
      'monorepo',
      20731,
    );
    expect(files.length).toBe((pull20731Files as unknown[]).length);
    expect(truncated).toBe(false);
    expect(files[0]?.oldOid).toBeNull();
    expect(files[0]?.newOid).toBeNull();
  });

  test('follows pagination across two pages', async () => {
    const firstPage = Array.from({ length: 100 }, (_, i) => ({
      filename: `file-${i}.ts`,
      previous_filename: null,
      status: 'modified',
      additions: 1,
      deletions: 1,
    }));
    const secondPage = Array.from({ length: 50 }, (_, i) => ({
      filename: `file-${100 + i}.ts`,
      previous_filename: null,
      status: 'modified',
      additions: 1,
      deletions: 1,
    }));
    let page = 0;
    const rest = stubRest({
      pulls: {
        listFiles: async (params) => {
          page += 1;
          expect(params.page).toBe(page);
          return ok(page === 1 ? firstPage : secondPage);
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const { files } = await api.listPullFiles('o', 'r', 1);
    expect(page).toBe(2);
    expect(files.length).toBe(150);
    expect(files[149]?.path).toBe('file-149.ts');
  });

  test('reports truncated once the file-list cap is hit', async () => {
    const fullPage = (start: number) =>
      Array.from({ length: 100 }, (_, i) => ({
        filename: `file-${start + i}.ts`,
        previous_filename: null,
        status: 'modified',
        additions: 1,
        deletions: 1,
      }));
    let page = 0;
    const rest = stubRest({
      pulls: {
        listFiles: async () => {
          page += 1;
          // GitHub caps the file list at 3,000 entries (30 full pages of
          // 100) and returns nothing past it, which is what ends the loop.
          return ok(page <= 30 ? fullPage((page - 1) * 100) : []);
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const { files, truncated } = await api.listPullFiles('o', 'r', 1);
    expect(page).toBe(31);
    expect(files.length).toBe(3000);
    expect(truncated).toBe(true);
  });

  function rawFile(i: number) {
    return {
      filename: `file-${i}.ts`,
      previous_filename: null,
      status: 'modified',
      additions: 1,
      deletions: 1,
    };
  }

  test('re-fetches a later page whose content changed even though an earlier page 304s', async () => {
    const fetcher = new ConditionalFetcher(new InMemoryEtagStore());
    const page1 = Array.from({ length: 100 }, (_, i) => rawFile(i));
    const page2v1 = Array.from({ length: 50 }, (_, i) => rawFile(100 + i));
    const page2v2 = Array.from({ length: 50 }, (_, i) => rawFile(200 + i));
    const callsByPage = new Map<number, number>();
    const rest = stubRest({
      pulls: {
        listFiles: async ({ page, headers }) => {
          const call = (callsByPage.get(page) ?? 0) + 1;
          callsByPage.set(page, call);
          if (page === 1) {
            if (call === 2 && headers?.['if-none-match'] === 'etag-p1')
              notModified();
            return ok(page1, 'etag-p1');
          }
          // Page 2's content changed on GitHub's side, so a real request
          // always answers with a fresh 200, never a 304.
          return ok(call === 1 ? page2v1 : page2v2, `etag-p2-v${call}`);
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql, fetcher);

    const first = await api.listPullFiles('o', 'r', 1);
    expect(first.files.length).toBe(150);

    const second = await api.listPullFiles('o', 'r', 1);
    expect(second.files.length).toBe(150);
    expect(second.files.slice(100).map((f) => f.path)).toEqual(
      page2v2.map((f) => f.filename),
    );
    expect(callsByPage.get(1)).toBe(2);
    expect(callsByPage.get(2)).toBe(2);
  });

  test('fetches a newly appeared page even though it previously stopped pagination', async () => {
    const fetcher = new ConditionalFetcher(new InMemoryEtagStore());
    const page1 = Array.from({ length: 100 }, (_, i) => rawFile(i));
    const newPage2 = [rawFile(100)];
    const callsByPage = new Map<number, number>();
    const rest = stubRest({
      pulls: {
        listFiles: async ({ page, headers }) => {
          const call = (callsByPage.get(page) ?? 0) + 1;
          callsByPage.set(page, call);
          if (page === 1) {
            if (call === 2 && headers?.['if-none-match'] === 'etag-p1')
              notModified();
            return ok(page1, 'etag-p1');
          }
          // The list used to end at page 1 (page 2 was empty); it later
          // grew, so page 2 now has real content.
          return call === 1
            ? ok([], 'etag-p2-empty')
            : ok(newPage2, 'etag-p2-new');
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql, fetcher);

    const first = await api.listPullFiles('o', 'r', 1);
    expect(first.files.length).toBe(100);

    const second = await api.listPullFiles('o', 'r', 1);
    expect(second.files.length).toBe(101);
    expect(second.files[100]?.path).toBe(newPage2[0]?.filename);
    expect(callsByPage.get(2)).toBe(2);
  });

  test('returns the same merged list when every page 304s', async () => {
    const fetcher = new ConditionalFetcher(new InMemoryEtagStore());
    const page1 = Array.from({ length: 100 }, (_, i) => rawFile(i));
    const page2 = Array.from({ length: 50 }, (_, i) => rawFile(100 + i));
    const callsByPage = new Map<number, number>();
    const rest = stubRest({
      pulls: {
        listFiles: async ({ page, headers }) => {
          const call = (callsByPage.get(page) ?? 0) + 1;
          callsByPage.set(page, call);
          const items = page === 1 ? page1 : page2;
          const etag = page === 1 ? 'etag-p1' : 'etag-p2';
          if (call === 2 && headers?.['if-none-match'] === etag) notModified();
          return ok(items, etag);
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql, fetcher);

    const first = await api.listPullFiles('o', 'r', 1);
    const second = await api.listPullFiles('o', 'r', 1);

    expect(second.files.map((f) => f.path)).toEqual(
      first.files.map((f) => f.path),
    );
    expect(callsByPage.get(1)).toBe(2);
    expect(callsByPage.get(2)).toBe(2);
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

describe('listIssueComments and listReviews', () => {
  test('marks vercel[bot] and greptile-apps[bot] issue comment authors as bots', async () => {
    const rest = stubRest({
      issues: {
        listComments: async () => ok(pull20731IssueComments),
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const comments = await api.listIssueComments(
      'Sola-Solutions',
      'monorepo',
      20731,
    );
    const byLogin = new Map(
      comments.map((c) => [c.author.login, c.author.isBot]),
    );
    expect(byLogin.get('vercel[bot]')).toBe(true);
    expect(byLogin.get('greptile-apps[bot]')).toBe(true);
    expect(byLogin.get('jordan-sola')).toBe(false);
  });

  test('marks a Bot-typed review author as a bot and keeps the review state', async () => {
    const rest = stubRest({
      pulls: {
        listReviews: async () => ok(pull20731Reviews),
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    const reviews = await api.listReviews('Sola-Solutions', 'monorepo', 20731);
    const approved = reviews.find((r) => r.state === 'APPROVED');
    expect(approved?.author.login).toBe('jared-sola');
    const bot = reviews.find((r) => r.author.login === 'greptile-apps[bot]');
    expect(bot?.author.isBot).toBe(true);
  });
});

describe('createReviewComment', () => {
  test('posts body, commit_id, path, line, and side, omitting start_line for a single-line comment', async () => {
    let captured: unknown;
    const rest = stubRest({
      pulls: {
        createReviewComment: async (params) => {
          captured = params;
          return ok({});
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    await api.createReviewComment('o', 'r', 1, {
      body: 'looks good',
      path: 'src/greet.ts',
      line: 12,
      side: 'RIGHT',
      startLine: null,
      startSide: null,
      commitId: 'abc123',
    });
    expect(captured).toEqual({
      owner: 'o',
      repo: 'r',
      pull_number: 1,
      body: 'looks good',
      commit_id: 'abc123',
      path: 'src/greet.ts',
      line: 12,
      side: 'RIGHT',
    });
  });

  test('adds start_line and start_side for a multi-line comment', async () => {
    let captured: unknown;
    const rest = stubRest({
      pulls: {
        createReviewComment: async (params) => {
          captured = params;
          return ok({});
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    await api.createReviewComment('o', 'r', 1, {
      body: 'range comment',
      path: 'src/greet.ts',
      line: 15,
      side: 'RIGHT',
      startLine: 12,
      startSide: 'RIGHT',
      commitId: 'abc123',
    });
    expect(captured).toEqual({
      owner: 'o',
      repo: 'r',
      pull_number: 1,
      body: 'range comment',
      commit_id: 'abc123',
      path: 'src/greet.ts',
      line: 15,
      side: 'RIGHT',
      start_line: 12,
      start_side: 'RIGHT',
    });
  });

  test('wraps a 422 in a GitHubError carrying the message GitHub gives for the refused line', async () => {
    const rest = stubRest({
      pulls: {
        createReviewComment: async () => {
          throw {
            status: 422,
            response: { data: { message: 'line must be part of the diff' } },
          };
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    try {
      await api.createReviewComment('o', 'r', 1, {
        body: 'x',
        path: 'p',
        line: 1,
        side: 'RIGHT',
        startLine: null,
        startSide: null,
        commitId: 'abc',
      });
      throw new Error('expected createReviewComment to reject');
    } catch (error) {
      expect(error).toBeInstanceOf(GitHubError);
      expect((error as GitHubError).status).toBe(422);
      expect((error as GitHubError).message).toBe(
        'line must be part of the diff',
      );
    }
  });
});

describe('replyToReviewComment', () => {
  test('posts to the replies endpoint with comment_id and body', async () => {
    let captured: unknown;
    const rest = stubRest({
      pulls: {
        createReplyForReviewComment: async (params) => {
          captured = params;
          return ok({});
        },
      },
    });
    const api = createOctokitGitHubApi(rest, noGraphql);
    await api.replyToReviewComment('o', 'r', 1, 42, 'thanks');
    expect(captured).toEqual({
      owner: 'o',
      repo: 'r',
      pull_number: 1,
      comment_id: 42,
      body: 'thanks',
    });
  });
});

describe('setThreadResolved', () => {
  test('calls resolveReviewThread with the thread id', async () => {
    let capturedQuery = '';
    let capturedVars: Record<string, unknown> = {};
    const graphqlClient = async (
      query: string,
      variables: Record<string, unknown>,
    ) => {
      capturedQuery = query;
      capturedVars = variables;
      return { resolveReviewThread: { thread: { id: 'T1' } } };
    };
    const api = createOctokitGitHubApi(stubRest({}), graphqlClient);
    await api.setThreadResolved('T1', true);
    expect(capturedQuery).toContain('resolveReviewThread');
    expect(capturedVars).toEqual({ threadId: 'T1' });
  });

  test('calls unresolveReviewThread when resolved is false', async () => {
    let capturedQuery = '';
    const graphqlClient = async (query: string) => {
      capturedQuery = query;
      return { unresolveReviewThread: { thread: { id: 'T1' } } };
    };
    const api = createOctokitGitHubApi(stubRest({}), graphqlClient);
    await api.setThreadResolved('T1', false);
    expect(capturedQuery).toContain('unresolveReviewThread');
  });

  test('wraps a GraphQL failure in a GitHubError', async () => {
    const graphqlClient = async () => {
      throw { status: 403, message: 'Resource not accessible' };
    };
    const api = createOctokitGitHubApi(stubRest({}), graphqlClient);
    await expect(api.setThreadResolved('T1', true)).rejects.toThrow(
      GitHubError,
    );
  });
});
