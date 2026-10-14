import { describe, expect, test } from 'bun:test';
import type { GraphqlClient, RestClient } from './api.ts';
import { createOctokitGitHubApi } from './api.ts';
import baseRefChanges from './__fixtures__/pull-12110-base-ref-changes.json' with { type: 'json' };

const noRest = new Proxy(
  {},
  {
    get() {
      throw new Error('unexpected REST call');
    },
  },
) as RestClient;

function timelineResponse(page: { pageInfo: unknown; nodes: unknown[] }) {
  return { repository: { pullRequest: { timelineItems: page } } };
}

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
