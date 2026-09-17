import { describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import type { HoverResponse, HoverStatusResponse } from '../../shared/hover.ts';
import type { PrFile, PrSummary } from '../../shared/github.ts';
import type { HoverServerKey, HoverServers } from '../hover/servers.ts';
import type { HoverTreeState, TreeManager } from '../hover/trees.ts';
import type { HoverRouterDeps } from './hover.ts';
import { hoverRouter } from './hover.ts';

function mountHoverRouter(deps: HoverRouterDeps): Hono {
  const app = new Hono();
  app.route('/api/pr/:owner/:repo/:number/hover', hoverRouter(deps));
  return app;
}

const MOUNT = '/api/pr/acme/widgets/1/hover';

function fakeTrees(overrides: Partial<TreeManager> = {}): TreeManager {
  const base = {
    status: () => Promise.reject(new Error('status not stubbed')),
    ensureTree: () => Promise.resolve(),
    touch: () => {},
    path: () => Promise.reject(new Error('path not stubbed')),
    teardown: () => Promise.reject(new Error('teardown not stubbed')),
    reapIdle: () => Promise.reject(new Error('reapIdle not stubbed')),
  };
  const stub = { ...base, ...overrides };
  return stub as TreeManager;
}

function fakeServers(overrides: Partial<HoverServers> = {}): HoverServers {
  const base = {
    status: () => ({ typescript: 'stopped', python: 'stopped' }) as const,
    ensureStarted: () => Promise.resolve(),
    hover: () => Promise.reject(new Error('hover not stubbed')),
    stopAll: () => Promise.resolve(),
  };
  const stub = { ...base, ...overrides };
  return stub as HoverServers;
}

function fakePr(files: PrFile[] = []): { pr: PrSummary; files: PrFile[] } {
  return {
    pr: {
      owner: 'acme',
      repo: 'widgets',
      number: 1,
      title: 'A PR',
      body: '',
      state: 'open',
      draft: false,
      author: { login: 'jordan', avatarUrl: null, isBot: false },
      base: { ref: 'main', sha: 'base-sha' },
      head: { ref: 'feature', sha: 'head-sha' },
      mergeCommitSha: null,
      additions: 1,
      deletions: 1,
      changedFiles: files.length,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z',
      url: 'https://github.com/acme/widgets/pull/1',
    },
    files,
  };
}

describe('hoverRouter GET /', () => {
  test('rejects a request missing query params', async () => {
    const app = mountHoverRouter({
      trees: fakeTrees(),
      servers: fakeServers(),
      loadPr: () => Promise.reject(new Error('not used')),
    });

    const res = await app.request(`${MOUNT}?path=src/a.ts`);

    expect(res.status).toBe(400);
  });

  test('reports unsupported for a file extension with no language server', async () => {
    const app = mountHoverRouter({
      trees: fakeTrees(),
      servers: fakeServers(),
      loadPr: () => Promise.reject(new Error('not used')),
    });

    const res = await app.request(`${MOUNT}?path=README.md&line=0&character=0`);

    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({
      status: 'unsupported',
    } satisfies HoverResponse);
  });

  test('starts provisioning and answers preparing when there is no tree yet', async () => {
    let ensureTreeCalled: unknown = null;
    const trees = fakeTrees({
      status: () => Promise.resolve<HoverTreeState>({ state: 'none' }),
      ensureTree: (params) => {
        ensureTreeCalled = params;
        return Promise.resolve();
      },
    });
    const app = mountHoverRouter({
      trees,
      servers: fakeServers(),
      loadPr: () =>
        Promise.resolve(
          fakePr([
            {
              path: 'src/a.ts',
              previousPath: null,
              status: 'modified',
              additions: 1,
              deletions: 0,
              oldOid: 'a',
              newOid: 'b',
            },
          ]),
        ),
    });

    const res = await app.request(`${MOUNT}?path=src/a.ts&line=0&character=0`);

    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({
      status: 'preparing',
      tree: 'provisioning',
      server: 'stopped',
    } satisfies HoverResponse);
    // Give the fire-and-forget ensureTree call a turn to run.
    await Promise.resolve();
    expect(ensureTreeCalled).toMatchObject({
      owner: 'acme',
      repo: 'widgets',
      number: 1,
      headSha: 'head-sha',
      languages: ['typescript'],
    });
  });

  test('answers preparing while the tree is still provisioning', async () => {
    const trees = fakeTrees({
      status: () => Promise.resolve<HoverTreeState>({ state: 'provisioning' }),
    });
    const app = mountHoverRouter({
      trees,
      servers: fakeServers(),
      loadPr: () => Promise.reject(new Error('not used')),
    });

    const res = await app.request(`${MOUNT}?path=src/a.ts&line=0&character=0`);

    expect(await res.json()).toEqual({
      status: 'preparing',
      tree: 'provisioning',
      server: 'stopped',
    } satisfies HoverResponse);
  });

  test('starts the server and answers preparing when the tree is ready but the server is not', async () => {
    let touched = false;
    let startedWith: unknown = null;
    const trees = fakeTrees({
      status: () =>
        Promise.resolve<HoverTreeState>({ state: 'ready', path: '/tree' }),
      touch: () => {
        touched = true;
      },
    });
    const servers = fakeServers({
      status: () => ({ typescript: 'stopped', python: 'stopped' }) as const,
      ensureStarted: (pr: HoverServerKey, language, treePath, filePath) => {
        startedWith = { pr, language, treePath, filePath };
        return Promise.resolve();
      },
    });
    const app = mountHoverRouter({
      trees,
      servers,
      loadPr: () => Promise.reject(new Error('not used')),
    });

    const res = await app.request(`${MOUNT}?path=src/a.ts&line=0&character=0`);

    expect(touched).toBe(true);
    expect(await res.json()).toEqual({
      status: 'preparing',
      tree: 'ready',
      server: 'stopped',
    } satisfies HoverResponse);
    await Promise.resolve();
    expect(startedWith).toEqual({
      pr: { owner: 'acme', repo: 'widgets', number: 1 },
      language: 'typescript',
      treePath: '/tree',
      filePath: 'src/a.ts',
    });
  });

  test('returns the hover markdown when the tree and server are both ready', async () => {
    const trees = fakeTrees({
      status: () =>
        Promise.resolve<HoverTreeState>({ state: 'ready', path: '/tree' }),
    });
    const servers = fakeServers({
      status: () => ({ typescript: 'ready', python: 'stopped' }) as const,
      hover: () => Promise.resolve('**hello**'),
    });
    const app = mountHoverRouter({
      trees,
      servers,
      loadPr: () => Promise.reject(new Error('not used')),
    });

    const res = await app.request(`${MOUNT}?path=src/a.ts&line=3&character=7`);

    expect(await res.json()).toEqual({
      status: 'ready',
      contents: '**hello**',
    } satisfies HoverResponse);
  });
});

describe('hoverRouter GET /status', () => {
  test('reports failed with a message for an unsupported repo', async () => {
    const trees = fakeTrees({
      status: () => Promise.resolve<HoverTreeState>({ state: 'unsupported' }),
    });
    const app = mountHoverRouter({
      trees,
      servers: fakeServers(),
      loadPr: () => Promise.reject(new Error('not used')),
    });

    const res = await app.request(`${MOUNT}/status`);

    expect(await res.json()).toEqual({
      tree: 'failed',
      treeError: 'this repo has no wt tree configured for hover',
      servers: { typescript: 'stopped', python: 'stopped' },
    } satisfies HoverStatusResponse);
  });

  test('passes through a ready tree and server state', async () => {
    const trees = fakeTrees({
      status: () =>
        Promise.resolve<HoverTreeState>({ state: 'ready', path: '/tree' }),
    });
    const servers = fakeServers({
      status: () => ({ typescript: 'ready', python: 'stopped' }) as const,
    });
    const app = mountHoverRouter({
      trees,
      servers,
      loadPr: () => Promise.reject(new Error('not used')),
    });

    const res = await app.request(`${MOUNT}/status`);

    expect(await res.json()).toEqual({
      tree: 'ready',
      treeError: null,
      servers: { typescript: 'ready', python: 'stopped' },
    } satisfies HoverStatusResponse);
  });
});
