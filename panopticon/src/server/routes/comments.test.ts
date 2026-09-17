import { describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import type { ThreadsResponse } from '../../shared/api.ts';
import type { ReviewThread } from '../../shared/github.ts';
import type { GitHubApi } from '../github/api.ts';
import { GitHubError } from '../github/api.ts';
import { commentsRouter } from './comments.ts';

function fakeThread(overrides: Partial<ReviewThread> = {}): ReviewThread {
  return {
    id: 'thread-1',
    path: 'src/a.ts',
    line: 3,
    originalLine: 3,
    startLine: null,
    side: 'RIGHT',
    startSide: null,
    isResolved: false,
    isOutdated: false,
    diffHunk: '@@ -1,3 +1,3 @@',
    comments: [
      {
        id: 1,
        nodeId: 'PRRC_1',
        author: { login: 'reviewer', avatarUrl: null, isBot: false },
        body: 'looks good',
        createdAt: '2026-01-01T00:00:00Z',
        updatedAt: '2026-01-01T00:00:00Z',
        url: 'https://github.com/x',
        inReplyToId: null,
      },
    ],
    ...overrides,
  };
}

interface FakeGitHubOptions {
  threads?: ReviewThread[];
  createReviewCommentError?: Error;
  replyError?: Error;
  resolveError?: Error;
  onCreate?: (input: unknown) => void;
  onReply?: (commentId: number, body: string) => void;
  onResolve?: (threadId: string, resolved: boolean) => void;
}

function fakeGitHub(options: FakeGitHubOptions = {}): GitHubApi {
  return {
    getPull: () => {
      throw new Error('not used');
    },
    listPullFiles: () => {
      throw new Error('not used');
    },
    listOpenPulls: () => {
      throw new Error('not used');
    },
    findPullByHead: () => {
      throw new Error('not used');
    },
    listBaseRefChanges: () => {
      throw new Error('not used');
    },
    listReviewThreads: async () => options.threads ?? [fakeThread()],
    listIssueComments: async () => [],
    listReviews: async () => [],
    createReviewComment: async (_owner, _repo, _number, input) => {
      options.onCreate?.(input);
      if (options.createReviewCommentError)
        throw options.createReviewCommentError;
    },
    replyToReviewComment: async (_owner, _repo, _number, commentId, body) => {
      options.onReply?.(commentId, body);
      if (options.replyError) throw options.replyError;
    },
    setThreadResolved: async (threadId, resolved) => {
      options.onResolve?.(threadId, resolved);
      if (options.resolveError) throw options.resolveError;
    },
  };
}

// Mirrors how app.ts mounts this router under the shared pr param prefix.
function mount(github: GitHubApi): Hono {
  const app = new Hono();
  app.route('/api/pr/:owner/:repo/:number', commentsRouter({ github }));
  return app;
}

const BASE = '/api/pr/Sola-Solutions/monorepo/1';

describe('GET /threads', () => {
  test('returns the current threads', async () => {
    const threads = [fakeThread({ id: 'thread-2' })];
    const app = mount(fakeGitHub({ threads }));
    const res = await app.request(`${BASE}/threads`);
    expect(res.status).toBe(200);
    const body = (await res.json()) as ThreadsResponse;
    expect(body.threads[0]?.id).toBe('thread-2');
  });

  test('maps a GitHubError to its status', async () => {
    const github: GitHubApi = {
      ...fakeGitHub(),
      listReviewThreads: async () => {
        throw new GitHubError(403, 'Resource not accessible by integration');
      },
    };
    const app = mount(github);
    const res = await app.request(`${BASE}/threads`);
    expect(res.status).toBe(403);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe('Resource not accessible by integration');
  });
});

describe('POST /comments', () => {
  const validBody = {
    body: 'nice change',
    path: 'src/a.ts',
    line: 5,
    side: 'RIGHT',
    startLine: null,
    startSide: null,
    commitId: 'sha123',
  };

  test('creates the comment then returns the refreshed threads', async () => {
    let captured: unknown;
    const app = mount(
      fakeGitHub({
        threads: [fakeThread({ id: 'thread-after-create' })],
        onCreate: (input) => {
          captured = input;
        },
      }),
    );
    const res = await app.request(`${BASE}/comments`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(validBody),
    });
    expect(res.status).toBe(200);
    const body = (await res.json()) as ThreadsResponse;
    expect(body.threads[0]?.id).toBe('thread-after-create');
    expect(captured).toEqual(validBody);
  });

  test('rejects a body missing required fields with 400', async () => {
    const app = mount(fakeGitHub());
    const res = await app.request(`${BASE}/comments`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ body: 'x' }),
    });
    expect(res.status).toBe(400);
  });

  test('rejects an invalid side with 400', async () => {
    const app = mount(fakeGitHub());
    const res = await app.request(`${BASE}/comments`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ...validBody, side: 'MIDDLE' }),
    });
    expect(res.status).toBe(400);
  });

  test('passes a 422 from GitHub straight through with its message', async () => {
    const app = mount(
      fakeGitHub({
        createReviewCommentError: new GitHubError(
          422,
          'line 5 is not part of the diff',
        ),
      }),
    );
    const res = await app.request(`${BASE}/comments`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(validBody),
    });
    expect(res.status).toBe(422);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe('line 5 is not part of the diff');
  });
});

describe('POST /comments/:commentId/replies', () => {
  test('replies then returns the refreshed threads', async () => {
    let captured: unknown;
    const app = mount(
      fakeGitHub({
        threads: [fakeThread({ id: 'thread-after-reply' })],
        onReply: (commentId, body) => {
          captured = { commentId, body };
        },
      }),
    );
    const res = await app.request(`${BASE}/comments/42/replies`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ body: 'thanks' }),
    });
    expect(res.status).toBe(200);
    const body = (await res.json()) as ThreadsResponse;
    expect(body.threads[0]?.id).toBe('thread-after-reply');
    expect(captured).toEqual({ commentId: 42, body: 'thanks' });
  });

  test('rejects a non-numeric comment id with 400', async () => {
    const app = mount(fakeGitHub());
    const res = await app.request(`${BASE}/comments/not-a-number/replies`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ body: 'thanks' }),
    });
    expect(res.status).toBe(400);
  });

  test('rejects an empty reply body with 400', async () => {
    const app = mount(fakeGitHub());
    const res = await app.request(`${BASE}/comments/42/replies`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ body: '   ' }),
    });
    expect(res.status).toBe(400);
  });

  test('passes a 422 from GitHub straight through', async () => {
    const app = mount(
      fakeGitHub({ replyError: new GitHubError(422, 'thread is locked') }),
    );
    const res = await app.request(`${BASE}/comments/42/replies`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ body: 'thanks' }),
    });
    expect(res.status).toBe(422);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe('thread is locked');
  });
});

describe('POST /threads/:threadId/resolve', () => {
  test('resolves then returns the refreshed threads', async () => {
    let captured: unknown;
    const app = mount(
      fakeGitHub({
        threads: [fakeThread({ id: 'thread-1', isResolved: true })],
        onResolve: (threadId, resolved) => {
          captured = { threadId, resolved };
        },
      }),
    );
    const res = await app.request(`${BASE}/threads/PRRT_1/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ resolved: true }),
    });
    expect(res.status).toBe(200);
    const body = (await res.json()) as ThreadsResponse;
    expect(body.threads[0]?.isResolved).toBe(true);
    expect(captured).toEqual({ threadId: 'PRRT_1', resolved: true });
  });

  test('rejects a body without a boolean resolved field', async () => {
    const app = mount(fakeGitHub());
    const res = await app.request(`${BASE}/threads/PRRT_1/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ resolved: 'yes' }),
    });
    expect(res.status).toBe(400);
  });

  test('passes a GitHub failure through with its status', async () => {
    const app = mount(
      fakeGitHub({ resolveError: new GitHubError(404, 'thread not found') }),
    );
    const res = await app.request(`${BASE}/threads/PRRT_1/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ resolved: false }),
    });
    expect(res.status).toBe(404);
  });
});
