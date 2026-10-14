import { describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import type { RepoConfig } from '../config.ts';
import { testGit } from '../git/__fixtures__/testGit.ts';
import { createStackCache } from '../stack/cache.ts';
import { GraphiteLocal } from '../stack/graphiteLocal.ts';
import { StackError } from '../stack/resolve.ts';
import type { StackSource } from '../stack/source.ts';
import type { ReposRouterDeps } from './repos.ts';
import { reposRouter } from './repos.ts';

const REPO: RepoConfig = { owner: 'o', repo: 'r', trunk: 'master' };

function unusedSource(): StackSource {
  return {
    getPull: () => Promise.reject(new Error('not used in this test')),
    listOpenPulls: () => Promise.reject(new Error('not used in this test')),
    findPullByHead: () => Promise.reject(new Error('not used in this test')),
    listBaseRefChanges: () =>
      Promise.reject(new Error('not used in this test')),
  };
}

function mount(overrides: Partial<ReposRouterDeps> = {}): Hono {
  const app = new Hono();
  app.route(
    '/repos/:owner/:repo',
    reposRouter({
      repoFor: (owner, repo) =>
        owner === REPO.owner && repo === REPO.repo ? REPO : null,
      sourceFor: () => unusedSource(),
      // No checkout, so snapshot() resolves null without spawning git.
      graphiteFor: () => new GraphiteLocal(null, testGit),
      landedCommit: () => Promise.reject(new Error('not used in this test')),
      cache: createStackCache(),
      ...overrides,
    }),
  );
  return app;
}

describe('reposRouter', () => {
  test('answers 404 unknown-repo for a repo outside the config', async () => {
    const res = await mount().request('/repos/x/y/snapshot');

    expect(res.status).toBe(404);
    expect(await res.json()).toEqual({ error: 'unknown-repo' });
  });

  test('answers 404 no-graphite-state when the repo has no checkout', async () => {
    const res = await mount().request('/repos/o/r/snapshot');

    expect(res.status).toBe(404);
    expect(await res.json()).toEqual({ error: 'no-graphite-state' });
  });

  test('returns the stack from a cache hit without calling the source', async () => {
    const cached = { entries: [], truncatedBelow: false };
    const cache = createStackCache();
    cache.set('o/r#1', cached);

    const res = await mount({ cache }).request('/repos/o/r/prs/1/stack');

    expect(res.status).toBe(200);
    expect(await res.json()).toEqual(cached);
  });

  test('maps a missing PR to 404', async () => {
    const source: StackSource = {
      ...unusedSource(),
      getPull: () => Promise.resolve(null),
      listOpenPulls: () => Promise.resolve([]),
    };

    const res = await mount({ sourceFor: () => source }).request(
      '/repos/o/r/prs/404/stack',
    );

    expect(res.status).toBe(404);
  });

  test('maps another source failure to 502 with the message', async () => {
    const source: StackSource = {
      ...unusedSource(),
      getPull: () => Promise.reject(new Error('github is down')),
    };

    const res = await mount({ sourceFor: () => source }).request(
      '/repos/o/r/prs/1/stack',
    );

    expect(res.status).toBe(502);
    expect(await res.json()).toEqual({ error: 'github is down' });
  });

  test('a StackError with a non-404 status still maps to 502', async () => {
    const source: StackSource = {
      ...unusedSource(),
      getPull: () => Promise.reject(new StackError('bad upstream data', 500)),
    };

    const res = await mount({ sourceFor: () => source }).request(
      '/repos/o/r/prs/1/stack',
    );

    expect(res.status).toBe(502);
  });

  test('landed returns the commit the search finds', async () => {
    const calls: unknown[][] = [];
    const app = mount({
      landedCommit: async (repo, number, createdAt) => {
        calls.push([repo, number, createdAt]);
        return 'cafef00d';
      },
    });

    const res = await app.request(
      '/repos/o/r/prs/7/landed?createdAt=2026-01-01T00:00:00Z',
    );

    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ mergeCommitSha: 'cafef00d' });
    expect(calls).toEqual([[REPO, 7, '2026-01-01T00:00:00Z']]);
  });

  test('landed answers 400 when createdAt is missing or invalid', async () => {
    const app = mount();

    expect((await app.request('/repos/o/r/prs/7/landed')).status).toBe(400);
    expect(
      (await app.request('/repos/o/r/prs/7/landed?createdAt=yesterday')).status,
    ).toBe(400);
  });
});
