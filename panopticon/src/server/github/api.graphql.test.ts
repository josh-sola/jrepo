import { describe, expect, test } from 'bun:test';
import type { GraphqlClient, RestClient } from './api.ts';
import { createOctokitGitHubApi } from './api.ts';
import baseRefChanges from './__fixtures__/pull-12110-base-ref-changes.json' with { type: 'json' };
import reviewThreads from './__fixtures__/pull-20731-review-threads.json' with { type: 'json' };

const noRest = new Proxy(
  {},
  {
    get() {
      throw new Error('unexpected REST call');
    },
  },
) as RestClient;

function threadsResponse(page: { pageInfo: unknown; nodes: unknown[] }) {
  return { repository: { pullRequest: { reviewThreads: page } } };
}

function timelineResponse(page: { pageInfo: unknown; nodes: unknown[] }) {
  return { repository: { pullRequest: { timelineItems: page } } };
}

describe('listReviewThreads', () => {
  test('maps every recorded thread, including an outdated one with a null line', async () => {
    const graphqlClient: GraphqlClient = async () =>
      threadsResponse(reviewThreads);
    const api = createOctokitGitHubApi(noRest, graphqlClient);
    const threads = await api.listReviewThreads(
      'Sola-Solutions',
      'monorepo',
      20731,
    );

    expect(threads.length).toBe(4);
    const outdated = threads.find((t) => t.isOutdated);
    expect(outdated?.line).toBeNull();
    expect(outdated?.originalLine).toBe(145);
    expect(outdated?.isResolved).toBe(true);
  });

  test('maps side, inReplyToId, and the first comment diffHunk onto the thread', async () => {
    const graphqlClient: GraphqlClient = async () =>
      threadsResponse(reviewThreads);
    const api = createOctokitGitHubApi(noRest, graphqlClient);
    const threads = await api.listReviewThreads('o', 'r', 1);

    const first = threads[0];
    expect(first?.side).toBe('RIGHT');
    expect(first?.comments[0]?.inReplyToId).toBeNull();
    expect(first?.comments[1]?.inReplyToId).toBe(4032260092);
    const rawFirstComment = (
      reviewThreads as {
        nodes: { comments: { nodes: { diffHunk: string }[] } }[];
      }
    ).nodes[0]!.comments.nodes[0]!;
    expect(first?.diffHunk).toBe(rawFirstComment.diffHunk);
    expect(first?.comments[0]?.id).toBe(4032260092);
    expect(first?.comments[0]?.nodeId).toBe('PRRC_kwDOKs9ddM7wV2f8');
  });

  test('paginates across two pages of threads', async () => {
    const nodes = (reviewThreads as { nodes: unknown[] }).nodes;
    const pageOne = {
      pageInfo: { hasNextPage: true, endCursor: 'c1' },
      nodes: nodes.slice(0, 2),
    };
    const pageTwo = {
      pageInfo: { hasNextPage: false, endCursor: null },
      nodes: nodes.slice(2),
    };
    let call = 0;
    const graphqlClient: GraphqlClient = async (_query, variables) => {
      call += 1;
      if (call === 1) {
        expect(variables.cursor).toBeNull();
        return threadsResponse(pageOne);
      }
      expect(variables.cursor).toBe('c1');
      return threadsResponse(pageTwo);
    };
    const api = createOctokitGitHubApi(noRest, graphqlClient);
    const threads = await api.listReviewThreads('o', 'r', 1);
    expect(call).toBe(2);
    expect(threads.length).toBe(4);
  });

  test('follows per-thread comment pagination through the node() query', async () => {
    const firstThread = (
      reviewThreads as {
        nodes: { id: string; comments: { nodes: unknown[] } }[];
      }
    ).nodes[0]!;
    const firstComment = firstThread.comments.nodes[0];
    const secondComment = firstThread.comments.nodes[1];
    const truncatedThread = {
      ...firstThread,
      comments: {
        pageInfo: { hasNextPage: true, endCursor: 'cc1' },
        nodes: [firstComment],
      },
    };

    let nodeCalls = 0;
    const graphqlClient: GraphqlClient = async (query, variables) => {
      if (query.includes('reviewThreads')) {
        return threadsResponse({
          pageInfo: { hasNextPage: false, endCursor: null },
          nodes: [truncatedThread],
        });
      }
      nodeCalls += 1;
      expect(variables.id).toBe(firstThread.id);
      expect(variables.cursor).toBe('cc1');
      return {
        node: {
          comments: {
            pageInfo: { hasNextPage: false, endCursor: null },
            nodes: [secondComment],
          },
        },
      };
    };
    const api = createOctokitGitHubApi(noRest, graphqlClient);
    const threads = await api.listReviewThreads('o', 'r', 1);
    expect(nodeCalls).toBe(1);
    expect(threads[0]?.comments.length).toBe(2);
  });
});

describe('listBaseRefChanges', () => {
  test('maps the recorded BaseRefChangedEvent chain for #12110', async () => {
    const graphqlClient: GraphqlClient = async () =>
      timelineResponse(baseRefChanges);
    const api = createOctokitGitHubApi(noRest, graphqlClient);
    const changes = await api.listBaseRefChanges(
      'Sola-Solutions',
      'monorepo',
      12110,
    );

    expect(changes).toEqual([
      {
        previousRefName: 'josh/be-1709-migrate-v2-api-env-shared',
        currentRefName: 'graphite-base/12110',
        createdAt: '2026-07-09T06:25:25Z',
        actorLogin: 'graphite-app',
      },
      {
        previousRefName: 'graphite-base/12110',
        currentRefName: 'master',
        createdAt: '2026-08-05T15:57:15Z',
        actorLogin: 'anwar-sola',
      },
    ]);
  });

  test('maps an AutomaticBaseChangeSucceededEvent onto the same shape', async () => {
    const graphqlClient: GraphqlClient = async () =>
      timelineResponse({
        pageInfo: { hasNextPage: false, endCursor: null },
        nodes: [
          {
            __typename: 'AutomaticBaseChangeSucceededEvent',
            oldBase: 'graphite-base/9999',
            newBase: 'master',
            createdAt: '2026-05-01T00:00:00Z',
            actor: { login: 'graphite-app' },
          },
        ],
      });
    const api = createOctokitGitHubApi(noRest, graphqlClient);
    const changes = await api.listBaseRefChanges('o', 'r', 1);
    expect(changes).toEqual([
      {
        previousRefName: 'graphite-base/9999',
        currentRefName: 'master',
        createdAt: '2026-05-01T00:00:00Z',
        actorLogin: 'graphite-app',
      },
    ]);
  });

  test('paginates across two pages of timeline items', async () => {
    const nodes = (baseRefChanges as { nodes: unknown[] }).nodes;
    const pageOne = {
      pageInfo: { hasNextPage: true, endCursor: 'tc1' },
      nodes: nodes.slice(0, 1),
    };
    const pageTwo = {
      pageInfo: { hasNextPage: false, endCursor: null },
      nodes: nodes.slice(1),
    };
    let call = 0;
    const graphqlClient: GraphqlClient = async (_query, variables) => {
      call += 1;
      if (call === 1) {
        expect(variables.cursor).toBeNull();
        return timelineResponse(pageOne);
      }
      expect(variables.cursor).toBe('tc1');
      return timelineResponse(pageTwo);
    };
    const api = createOctokitGitHubApi(noRest, graphqlClient);
    const changes = await api.listBaseRefChanges('o', 'r', 1);
    expect(call).toBe(2);
    expect(changes.length).toBe(2);
  });
});
