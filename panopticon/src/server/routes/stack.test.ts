import { describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import type { StackResponse } from '../../shared/stack.ts';
import type { GtApiClient, GtApiPassthrough } from '../gtapi/client.ts';
import { stackRouter } from './stack.ts';

function mountStackRouter(
  stack: (
    owner: string,
    repo: string,
    number: number,
  ) => Promise<GtApiPassthrough>,
): Hono {
  const gtApi: GtApiClient = {
    snapshot: async () => null,
    stack,
    landed: async () => null,
  };
  const app = new Hono();
  app.route('/api/pr/:owner/:repo/:number/stack', stackRouter({ gtApi }));
  return app;
}

describe('stackRouter', () => {
  test('passes a gt-api stack through with its status', async () => {
    const stack: StackResponse = { entries: [], truncatedBelow: false };
    const calls: [string, string, number][] = [];
    const app = mountStackRouter(async (owner, repo, number) => {
      calls.push([owner, repo, number]);
      return { status: 200, body: stack };
    });

    const res = await app.request('/api/pr/o/r/1/stack');

    expect(res.status).toBe(200);
    expect(await res.json()).toEqual(stack);
    expect(calls).toEqual([['o', 'r', 1]]);
  });

  test('passes a gt-api error status and body through', async () => {
    const app = mountStackRouter(async () => ({
      status: 404,
      body: { error: 'pull request not found' },
    }));

    const res = await app.request('/api/pr/o/r/404/stack');

    expect(res.status).toBe(404);
    expect(await res.json()).toEqual({ error: 'pull request not found' });
  });

  test('maps an unreachable gt-api to 502 with the message', async () => {
    const app = mountStackRouter(() =>
      Promise.reject(new Error('gt-api down')),
    );

    const res = await app.request('/api/pr/o/r/1/stack');

    expect(res.status).toBe(502);
    expect(await res.json()).toEqual({ error: 'gt-api down' });
  });

  test('rejects a non-numeric PR number', async () => {
    const app = mountStackRouter(() => Promise.reject(new Error('not used')));

    const res = await app.request('/api/pr/o/r/abc/stack');

    expect(res.status).toBe(400);
  });
});
