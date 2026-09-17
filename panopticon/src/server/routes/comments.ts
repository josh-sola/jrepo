import { Hono } from 'hono';
import type {
  CreateReviewCommentRequest,
  ReplyRequest,
  ResolveThreadRequest,
  ThreadsResponse,
  ApiError,
} from '../../shared/api.ts';
import type { CommentSide } from '../../shared/github.ts';
import { GitHubError } from '../github/api.ts';
import type { GitHubApi } from '../github/api.ts';

export interface CommentsRouterDeps {
  github: GitHubApi;
}

// Mounted under `/api/pr/:owner/:repo/:number`, so the params exist at
// runtime but this sub-router's types do not declare them.
function requireParam(value: string | undefined, name: string): string {
  if (value === undefined) {
    throw new Error(`commentsRouter: missing route param "${name}"`);
  }
  return value;
}

function parseId(raw: string): number | null {
  if (!/^\d+$/.test(raw)) return null;
  return Number(raw);
}

function isCommentSide(value: unknown): value is CommentSide {
  return value === 'LEFT' || value === 'RIGHT';
}

function isCreateReviewCommentRequest(
  value: unknown,
): value is CreateReviewCommentRequest {
  if (typeof value !== 'object' || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.body === 'string' &&
    v.body.trim().length > 0 &&
    typeof v.path === 'string' &&
    v.path.length > 0 &&
    typeof v.line === 'number' &&
    isCommentSide(v.side) &&
    (v.startLine === null || typeof v.startLine === 'number') &&
    (v.startSide === null || isCommentSide(v.startSide)) &&
    typeof v.commitId === 'string' &&
    v.commitId.length > 0
  );
}

function isReplyRequest(value: unknown): value is ReplyRequest {
  if (typeof value !== 'object' || value === null) return false;
  const v = value as Record<string, unknown>;
  return typeof v.body === 'string' && v.body.trim().length > 0;
}

function isResolveThreadRequest(value: unknown): value is ResolveThreadRequest {
  if (typeof value !== 'object' || value === null) return false;
  const v = value as Record<string, unknown>;
  return typeof v.resolved === 'boolean';
}

function githubErrorResponse(error: GitHubError): Response {
  const status = error.status >= 400 && error.status < 600 ? error.status : 502;
  const body: ApiError = { error: error.message };
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

export function commentsRouter(deps: CommentsRouterDeps): Hono {
  async function currentThreads(
    owner: string,
    repo: string,
    number: number,
  ): Promise<ThreadsResponse> {
    return {
      threads: await deps.github.listReviewThreads(owner, repo, number),
    };
  }

  return new Hono()
    .get('/threads', async (c) => {
      const owner = requireParam(c.req.param('owner'), 'owner');
      const repo = requireParam(c.req.param('repo'), 'repo');
      const number = Number(requireParam(c.req.param('number'), 'number'));
      try {
        return c.json(await currentThreads(owner, repo, number));
      } catch (error) {
        if (error instanceof GitHubError) {
          return githubErrorResponse(error);
        }
        throw error;
      }
    })
    .post('/comments', async (c) => {
      const owner = requireParam(c.req.param('owner'), 'owner');
      const repo = requireParam(c.req.param('repo'), 'repo');
      const number = Number(requireParam(c.req.param('number'), 'number'));
      const body: unknown = await c.req.json();
      if (!isCreateReviewCommentRequest(body)) {
        return c.json({ error: 'invalid review comment request' }, 400);
      }
      try {
        await deps.github.createReviewComment(owner, repo, number, body);
        return c.json(await currentThreads(owner, repo, number));
      } catch (error) {
        if (error instanceof GitHubError) {
          return githubErrorResponse(error);
        }
        throw error;
      }
    })
    .post('/comments/:commentId/replies', async (c) => {
      const owner = requireParam(c.req.param('owner'), 'owner');
      const repo = requireParam(c.req.param('repo'), 'repo');
      const number = Number(requireParam(c.req.param('number'), 'number'));
      const commentId = parseId(
        requireParam(c.req.param('commentId'), 'commentId'),
      );
      if (commentId === null) {
        return c.json({ error: 'invalid comment id' }, 400);
      }
      const body: unknown = await c.req.json();
      if (!isReplyRequest(body)) {
        return c.json({ error: 'invalid reply request' }, 400);
      }
      try {
        await deps.github.replyToReviewComment(
          owner,
          repo,
          number,
          commentId,
          body.body,
        );
        return c.json(await currentThreads(owner, repo, number));
      } catch (error) {
        if (error instanceof GitHubError) {
          return githubErrorResponse(error);
        }
        throw error;
      }
    })
    .post('/threads/:threadId/resolve', async (c) => {
      const owner = requireParam(c.req.param('owner'), 'owner');
      const repo = requireParam(c.req.param('repo'), 'repo');
      const number = Number(requireParam(c.req.param('number'), 'number'));
      const threadId = requireParam(c.req.param('threadId'), 'threadId');
      const body: unknown = await c.req.json();
      if (!isResolveThreadRequest(body)) {
        return c.json({ error: 'invalid resolve request' }, 400);
      }
      try {
        await deps.github.setThreadResolved(threadId, body.resolved);
        return c.json(await currentThreads(owner, repo, number));
      } catch (error) {
        if (error instanceof GitHubError) {
          return githubErrorResponse(error);
        }
        throw error;
      }
    });
}
