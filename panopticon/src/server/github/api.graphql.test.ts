import { describe, expect, test } from 'bun:test';
import type { GraphqlClient, RestClient } from './api.ts';
import { createOctokitGitHubApi } from './api.ts';
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
