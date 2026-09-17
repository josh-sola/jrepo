// Serves fixture JSON for the review page's routes so `VITE_MOCK=1` renders
// without the Bun server. Comment POSTs mutate an in-memory copy of the PR
// fixture, so the composer flow has something to write to and read back.
import prFixture from './fixtures/pr.json';
import diffFixture from './fixtures/diff.json';
import viewedFixture from './fixtures/viewed.json';
import prefsFixture from './fixtures/prefs.json';
import stackFixture from './fixtures/stack.json';
import hoverFixture from './fixtures/hover.json';
import hoverStatusFixture from './fixtures/hover-status.json';
import type {
  CreateReviewCommentRequest,
  PrResponse,
  ReplyRequest,
  ResolveThreadRequest,
  ThreadsResponse,
} from '../../shared/api.ts';
import type { ReviewComment, ReviewThread } from '../../shared/github.ts';

type Fixture = unknown;

const STATIC_ROUTES: { pattern: RegExp; fixture: Fixture }[] = [
  {
    pattern: /^\/api\/pr\/[^/]+\/[^/]+\/\d+\/diff(\?.*)?$/,
    fixture: diffFixture,
  },
  { pattern: /^\/api\/pr\/[^/]+\/[^/]+\/\d+\/viewed$/, fixture: viewedFixture },
  { pattern: /^\/api\/pr\/[^/]+\/[^/]+\/\d+\/stack$/, fixture: stackFixture },
  {
    pattern: /^\/api\/pr\/[^/]+\/[^/]+\/\d+\/hover\/status$/,
    fixture: hoverStatusFixture,
  },
  {
    pattern: /^\/api\/pr\/[^/]+\/[^/]+\/\d+\/hover(\?.*)?$/,
    fixture: hoverFixture,
  },
  { pattern: /^\/api\/prefs$/, fixture: prefsFixture },
];

const PR_PATTERN = /^\/api\/pr\/[^/]+\/[^/]+\/\d+$/;
const CREATE_COMMENT_PATTERN = /^\/api\/pr\/[^/]+\/[^/]+\/\d+\/comments$/;
const REPLY_PATTERN =
  /^\/api\/pr\/[^/]+\/[^/]+\/\d+\/comments\/(\d+)\/replies$/;
const RESOLVE_PATTERN =
  /^\/api\/pr\/[^/]+\/[^/]+\/\d+\/threads\/([^/]+)\/resolve$/;

// Deep-cloned once so repeated mock POSTs mutate a page-local copy, never
// the imported fixture module itself.
let prState: PrResponse = structuredClone(prFixture) as PrResponse;
let nextCommentId = 100000;

function threadsResponse(): ThreadsResponse {
  return { threads: prState.threads };
}

function appendNewThread(input: CreateReviewCommentRequest): void {
  const id = nextCommentId;
  nextCommentId += 1;
  const comment: ReviewComment = {
    id,
    nodeId: `PRRC_mock_${id}`,
    author: {
      login: 'josh-bassin',
      avatarUrl: 'https://avatars.githubusercontent.com/u/1?v=4',
      isBot: false,
    },
    body: input.body,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    url: '#',
    inReplyToId: null,
  };
  const thread: ReviewThread = {
    id: `thread-mock-${id}`,
    path: input.path,
    line: input.line,
    originalLine: input.line,
    startLine: input.startLine,
    side: input.side,
    startSide: input.startSide,
    isResolved: false,
    isOutdated: false,
    diffHunk: '',
    comments: [comment],
  };
  prState.threads = [...prState.threads, thread];
}

function appendReply(commentId: number, input: ReplyRequest): void {
  const id = nextCommentId;
  nextCommentId += 1;
  const comment: ReviewComment = {
    id,
    nodeId: `PRRC_mock_${id}`,
    author: {
      login: 'josh-bassin',
      avatarUrl: 'https://avatars.githubusercontent.com/u/1?v=4',
      isBot: false,
    },
    body: input.body,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    url: '#',
    inReplyToId: commentId,
  };
  prState.threads = prState.threads.map((thread) =>
    thread.comments.some((c) => c.id === commentId)
      ? { ...thread, comments: [...thread.comments, comment] }
      : thread,
  );
}

function setResolved(threadId: string, input: ResolveThreadRequest): void {
  prState.threads = prState.threads.map((thread) =>
    thread.id === threadId ? { ...thread, isResolved: input.resolved } : thread,
  );
}

function jsonResponse(body: Fixture): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

async function jsonBody<T>(init: RequestInit | undefined): Promise<T> {
  return JSON.parse(String(init?.body ?? '{}')) as T;
}

export function installMockFetch(): void {
  const realFetch = window.fetch.bind(window);
  const shimFetch = async (
    input: RequestInfo | URL,
    init?: RequestInit,
  ): Promise<Response> => {
    const url =
      typeof input === 'string'
        ? input
        : input instanceof URL
          ? input.href
          : input.url;
    const path = url.startsWith('http')
      ? new URL(url).pathname + new URL(url).search
      : url;
    const method = init?.method ?? 'GET';

    if (method === 'POST' && CREATE_COMMENT_PATTERN.test(path)) {
      appendNewThread(await jsonBody<CreateReviewCommentRequest>(init));
      return jsonResponse(threadsResponse());
    }
    const replyMatch = method === 'POST' ? REPLY_PATTERN.exec(path) : null;
    if (replyMatch?.[1]) {
      appendReply(Number(replyMatch[1]), await jsonBody<ReplyRequest>(init));
      return jsonResponse(threadsResponse());
    }
    const resolveMatch = method === 'POST' ? RESOLVE_PATTERN.exec(path) : null;
    if (resolveMatch?.[1]) {
      setResolved(resolveMatch[1], await jsonBody<ResolveThreadRequest>(init));
      return jsonResponse(threadsResponse());
    }
    if (method === 'GET' && PR_PATTERN.test(path)) {
      return jsonResponse(prState);
    }

    const route = STATIC_ROUTES.find((r) => r.pattern.test(path));
    if (!route) return realFetch(input, init);
    return jsonResponse(route.fixture);
  };
  window.fetch = shimFetch as typeof fetch;
}
