import { describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import type { StackResponse } from '../../shared/stack.ts';
import { createStackCache } from '../stack/cache.ts';
import { GraphiteLocal } from '../stack/graphiteLocal.ts';
import { StackError } from '../stack/resolve.ts';
import type { StackSource } from '../stack/source.ts';
import type { StackRouterDeps } from './stack.ts';
import { stackRouter } from './stack.ts';

// No referenceClone, so snapshot() always resolves null without spawning
// git; every test here exercises the GitHub fallback path.
function noGraphiteLocal(): GraphiteLocal {
  return new GraphiteLocal(null);
}

function mountStackRouter(deps: StackRouterDeps): Hono {
  const app = new Hono();
  app.route('/api/pr/:owner/:repo/:number/stack', stackRouter(deps));
  return app;
}

function unusedSource(): StackSource {
  return {
    getPull: () => Promise.reject(new Error('not used in this test')),
    listOpenPulls: () => Promise.reject(new Error('not used in this test')),
    findPullByHead: () => Promise.reject(new Error('not used in this test')),
    listBaseRefChanges: () =>
      Promise.reject(new Error('not used in this test')),
  };
}

describe('stackRouter', () => {
  test('returns the stack from a cache hit without calling the source', async () => {
    const cached: StackResponse = { entries: [], truncatedBelow: false };
    const cache = createStackCache();
    cache.set('o/r#1', cached);
    const app = mountStackRouter({
      sourceFor: () => unusedSource(),
      trunkFor: () => 'master',
      graphiteFor: () => noGraphiteLocal(),
      cache,
    });

    const res = await app.request('/api/pr/o/r/1/stack');

    expect(res.status).toBe(200);
    expect(await res.json()).toEqual(cached);
  });

  test('maps a missing PR to 404', async () => {
    const source: StackSource = {
      ...unusedSource(),
      getPull: () => Promise.resolve(null),
      listOpenPulls: () => Promise.resolve([]),
    };
    const app = mountStackRouter({
      sourceFor: () => source,
      trunkFor: () => 'master',
      graphiteFor: () => noGraphiteLocal(),
      cache: createStackCache(),
    });

    const res = await app.request('/api/pr/o/r/404/stack');

    expect(res.status).toBe(404);
  });

  test('maps another source failure to 502 with the message', async () => {
    const source: StackSource = {
      ...unusedSource(),
      getPull: () => Promise.reject(new Error('github is down')),
    };
    const app = mountStackRouter({
      sourceFor: () => source,
      trunkFor: () => 'master',
      graphiteFor: () => noGraphiteLocal(),
      cache: createStackCache(),
    });

    const res = await app.request('/api/pr/o/r/1/stack');

    expect(res.status).toBe(502);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe('github is down');
  });

  test('a StackError with a non-404 status still maps to 502', async () => {
    const source: StackSource = {
      ...unusedSource(),
      getPull: () => Promise.reject(new StackError('bad upstream data', 500)),
    };
    const app = mountStackRouter({
      sourceFor: () => source,
      trunkFor: () => 'master',
      graphiteFor: () => noGraphiteLocal(),
      cache: createStackCache(),
    });

    const res = await app.request('/api/pr/o/r/1/stack');

    expect(res.status).toBe(502);
  });
});
