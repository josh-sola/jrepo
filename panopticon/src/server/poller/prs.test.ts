import { afterEach, describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { PrSummary } from '../../shared/github.ts';
import { openDb } from '../db.ts';
import { PrRepository, viewRecorder } from './prs.ts';

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

function freshDb() {
  dir = mkdtempSync(join(tmpdir(), 'panopticon-prs-test-'));
  return openDb(join(dir, 'test.sqlite'));
}

function fakePr(overrides: Partial<PrSummary> = {}): PrSummary {
  return {
    owner: 'acme',
    repo: 'widgets',
    number: 1,
    title: 'A pull request',
    body: '',
    state: 'open',
    draft: false,
    author: { login: 'jordan', avatarUrl: null, isBot: false },
    base: { ref: 'master', sha: 'base-sha' },
    head: { ref: 'feature', sha: 'head-sha' },
    mergeCommitSha: null,
    additions: 1,
    deletions: 1,
    changedFiles: 1,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    url: 'https://github.com/acme/widgets/pull/1',
    ...overrides,
  };
}

describe('PrRepository', () => {
  test('upsertSummary then get round-trips the summary', () => {
    const repo = new PrRepository(freshDb());
    const pr = fakePr();

    repo.upsertSummary(pr, true);
    const stored = repo.get('acme', 'widgets', 1);

    expect(stored?.headSha).toBe('head-sha');
    expect(stored?.isMine).toBe(true);
    expect(stored?.summary).toEqual(pr);
  });

  test('re-upserting updates the stored fields but keeps last_viewed_at', () => {
    const clock = { now: 1_000_000 };
    const repo = new PrRepository(freshDb(), () => clock.now);

    repo.upsertSummary(fakePr(), true);
    repo.recordView('acme', 'widgets', 1);
    clock.now += 60_000;
    repo.upsertSummary(
      fakePr({ head: { ref: 'feature', sha: 'new-sha' } }),
      true,
    );

    const stored = repo.get('acme', 'widgets', 1);
    expect(stored?.headSha).toBe('new-sha');
    expect(stored?.lastViewedAt).toBe(new Date(1_000_000).toISOString());
  });

  test('recordView on an unknown PR creates a stub row listWatched can find', () => {
    const repo = new PrRepository(freshDb());

    repo.recordView('acme', 'widgets', 42);

    const stored = repo.get('acme', 'widgets', 42);
    expect(stored).not.toBeNull();
    expect(stored?.lastViewedAt).not.toBeNull();

    const watched = repo.listWatched(7);
    expect(watched.map((p) => p.number)).toEqual([42]);
  });

  test('recordView on a known PR only updates last_viewed_at', () => {
    const repo = new PrRepository(freshDb());
    repo.upsertSummary(fakePr(), false);

    repo.recordView('acme', 'widgets', 1);

    const stored = repo.get('acme', 'widgets', 1);
    expect(stored?.title).toBe('A pull request');
    expect(stored?.lastViewedAt).not.toBeNull();
  });

  test('updateThreadFingerprint sets only that column', () => {
    const repo = new PrRepository(freshDb());
    repo.upsertSummary(fakePr(), true);

    repo.updateThreadFingerprint('acme', 'widgets', 1, 'abc:1:false');

    const stored = repo.get('acme', 'widgets', 1);
    expect(stored?.threadFingerprint).toBe('abc:1:false');
    expect(stored?.headSha).toBe('head-sha');
  });

  test('listMine scopes to owner, repo, and the is_mine flag', () => {
    const repo = new PrRepository(freshDb());
    repo.upsertSummary(fakePr({ number: 1 }), true);
    repo.upsertSummary(fakePr({ number: 2 }), false);
    repo.upsertSummary(fakePr({ number: 3, owner: 'other' }), true);

    expect(repo.listMine('acme', 'widgets').map((p) => p.number)).toEqual([1]);
  });

  test('listRecentlyViewed returns rows viewed within the window, newest first', () => {
    const clock = { now: 1_000_000 };
    const repo = new PrRepository(freshDb(), () => clock.now);
    repo.upsertSummary(fakePr({ number: 1 }), false);
    repo.upsertSummary(fakePr({ number: 2 }), false);

    repo.recordView('acme', 'widgets', 1);
    clock.now += 1000;
    repo.recordView('acme', 'widgets', 2);

    expect(repo.listRecentlyViewed(7).map((p) => p.number)).toEqual([2, 1]);
  });

  test('listRecentlyViewed excludes views older than the window', () => {
    const clock = { now: 1_000_000 };
    const repo = new PrRepository(freshDb(), () => clock.now);
    repo.upsertSummary(fakePr({ number: 1 }), false);
    repo.recordView('acme', 'widgets', 1);

    clock.now += 8 * 24 * 60 * 60 * 1000;
    expect(repo.listRecentlyViewed(7)).toEqual([]);
  });

  test('listWatched is the union of mine and recently viewed', () => {
    const repo = new PrRepository(freshDb());
    repo.upsertSummary(fakePr({ number: 1 }), true);
    repo.upsertSummary(fakePr({ number: 2 }), false);
    repo.upsertSummary(fakePr({ number: 3 }), false);
    repo.recordView('acme', 'widgets', 2);

    const numbers = repo
      .listWatched(7)
      .map((p) => p.number)
      .sort();
    expect(numbers).toEqual([1, 2]);
  });

  test('pruneClosed removes only closed or merged rows past the cutoff', () => {
    const clock = { now: 1_000_000 };
    const repo = new PrRepository(freshDb(), () => clock.now);
    repo.upsertSummary(fakePr({ number: 1, state: 'open' }), true);
    repo.upsertSummary(
      fakePr({
        number: 2,
        state: 'merged',
        updatedAt: new Date(clock.now).toISOString(),
      }),
      true,
    );

    clock.now += 8 * 24 * 60 * 60 * 1000;
    repo.pruneClosed(7);

    expect(repo.get('acme', 'widgets', 1)).not.toBeNull();
    expect(repo.get('acme', 'widgets', 2)).toBeNull();
  });

  test('pruneClosed leaves a recently closed PR alone', () => {
    const clock = { now: 1_000_000 };
    const repo = new PrRepository(freshDb(), () => clock.now);
    repo.upsertSummary(
      fakePr({
        number: 1,
        state: 'closed',
        updatedAt: new Date(clock.now).toISOString(),
      }),
      true,
    );

    repo.pruneClosed(7);

    expect(repo.get('acme', 'widgets', 1)).not.toBeNull();
  });
});

function mount(prs: PrRepository): Hono {
  const app = new Hono();
  app.use('/api/pr/:owner/:repo/:number', viewRecorder(prs));
  app.get('/api/pr/:owner/:repo/:number', (c) => c.json({ ok: true }));
  return app;
}

describe('viewRecorder', () => {
  test('records a view on GET of the PR route', async () => {
    const repo = new PrRepository(freshDb());
    const app = mount(repo);

    const res = await app.request('/api/pr/acme/widgets/7');

    expect(res.status).toBe(200);
    expect(repo.get('acme', 'widgets', 7)?.lastViewedAt).not.toBeNull();
  });
});
