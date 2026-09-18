import { afterEach, describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { PrDiffResponse } from '../../shared/api.ts';
import type { PrFile, PrSummary } from '../../shared/github.ts';
import { openDb } from '../db.ts';
import { GitHubError } from '../github/api.ts';
import { diffRouter, type DiffRouterDeps } from './diff.ts';

const FIXTURES = join(import.meta.dir, '..', 'diff', '__fixtures__');

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

function fixturePr(): { pr: PrSummary; files: PrFile[] } {
  const pr: PrSummary = {
    owner: 'acme',
    repo: 'widgets',
    number: 42,
    title: 'Rename oldName',
    body: '',
    state: 'open',
    draft: false,
    author: { login: 'josh', avatarUrl: null, isBot: false },
    base: { ref: 'main', sha: 'base-sha' },
    head: { ref: 'josh/rename', sha: 'head-sha' },
    mergeCommitSha: null,
    additions: 4,
    deletions: 0,
    changedFiles: 1,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    url: 'https://github.com/acme/widgets/pull/42',
  };
  const files: PrFile[] = [
    {
      path: 'src/math.ts',
      previousPath: null,
      status: 'modified',
      additions: 4,
      deletions: 0,
      oldOid: 'old-oid',
      newOid: 'new-oid',
    },
  ];
  return { pr, files };
}

function mountedRouter(deps: DiffRouterDeps): Hono {
  return new Hono().route(
    '/api/pr/:owner/:repo/:number/diff',
    diffRouter(deps),
  );
}

describe('diffRouter', () => {
  test('resolves a PR through loadPr and storeFor and returns its diff', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-diff-route-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    const { pr, files } = fixturePr();

    const deps: DiffRouterDeps = {
      loadPr: async () => ({ pr, files }),
      storeFor: async () => ({
        readBlob: async (oid) => {
          if (oid === 'old-oid')
            return new Uint8Array(
              readFileSync(join(FIXTURES, 'typescript', 'old.ts')),
            );
          if (oid === 'new-oid')
            return new Uint8Array(
              readFileSync(join(FIXTURES, 'typescript', 'new.ts')),
            );
          return null;
        },
        generatedPaths: async () => new Set(),
      }),
      db,
      tmpDir: dir,
      rules: [],
    };

    const router = mountedRouter(deps);
    const res = await router.request('/api/pr/acme/widgets/42/diff');
    expect(res.status).toBe(200);
    const body = (await res.json()) as PrDiffResponse;
    expect(body.headSha).toBe('head-sha');
    expect(body.baseSha).toBe('base-sha');
    expect(body.files.length).toBe(1);
    expect(body.files[0]?.structural).toBe(true);

    db.close();
  });

  test('defaults to keeping whitespace and honors ?ws=ignore', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-diff-route-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    // Markdown always goes through the text fallback, where ignoreWhitespace
    // actually changes the result, unlike the structural path.
    const pr = fixturePr().pr;
    const files: PrFile[] = [
      {
        path: 'README.md',
        previousPath: null,
        status: 'modified',
        additions: 1,
        deletions: 1,
        oldOid: 'old-oid',
        newOid: 'new-oid',
      },
    ];

    const deps: DiffRouterDeps = {
      loadPr: async () => ({ pr, files }),
      storeFor: async () => ({
        readBlob: async (oid) => {
          if (oid === 'old-oid')
            return new TextEncoder().encode('  same line\n');
          if (oid === 'new-oid') return new TextEncoder().encode('same line\n');
          return null;
        },
        generatedPaths: async () => new Set(),
      }),
      db,
      tmpDir: dir,
      rules: [],
    };

    const router = mountedRouter(deps);
    const keepRes = await router.request('/api/pr/acme/widgets/42/diff');
    const keepBody = (await keepRes.json()) as PrDiffResponse;
    expect(keepBody.files[0]?.stat.modified).toBe(1);

    const ignoreRes = await router.request(
      '/api/pr/acme/widgets/42/diff?ws=ignore',
    );
    const ignoreBody = (await ignoreRes.json()) as PrDiffResponse;
    expect(ignoreBody.files[0]?.stat.modified).toBe(0);

    db.close();
  });

  test('passes owner, repo, and number from the mounted path to loadPr', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-diff-route-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    const { pr, files } = fixturePr();
    const calls: [string, string, number][] = [];

    const deps: DiffRouterDeps = {
      loadPr: async (owner, repo, number) => {
        calls.push([owner, repo, number]);
        return { pr, files };
      },
      storeFor: async () => ({
        readBlob: async () => new TextEncoder().encode('a\n'),
        generatedPaths: async () => new Set(),
      }),
      db,
      tmpDir: dir,
      rules: [],
    };

    const router = mountedRouter(deps);
    await router.request('/api/pr/acme/widgets/42/diff');
    expect(calls).toEqual([['acme', 'widgets', 42]]);

    db.close();
  });

  test('a throwing loadPr yields a 500 with a JSON error body', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-diff-route-test-'));
    const db = openDb(join(dir, 'test.sqlite'));

    const deps: DiffRouterDeps = {
      loadPr: async () => {
        throw new Error('boom');
      },
      storeFor: async () => ({
        readBlob: async () => null,
        generatedPaths: async () => new Set(),
      }),
      db,
      tmpDir: dir,
      rules: [],
    };

    const router = mountedRouter(deps);
    const res = await router.request('/api/pr/acme/widgets/42/diff');
    expect(res.status).toBe(500);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe('boom');

    db.close();
  });

  test('a GitHubError 404 from loadPr yields a 404 with the GitHub message', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-diff-route-test-'));
    const db = openDb(join(dir, 'test.sqlite'));

    const deps: DiffRouterDeps = {
      loadPr: async () => {
        throw new GitHubError(404, 'Not Found');
      },
      storeFor: async () => ({
        readBlob: async () => null,
        generatedPaths: async () => new Set(),
      }),
      db,
      tmpDir: dir,
      rules: [],
    };

    const router = mountedRouter(deps);
    const res = await router.request('/api/pr/acme/widgets/42/diff');
    expect(res.status).toBe(404);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe('Not Found');

    db.close();
  });
});
