import { afterEach, describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { InboxResponse } from '../../shared/inbox.ts';
import type { PrSummary } from '../../shared/github.ts';
import { openDb } from '../db.ts';
import { PrRepository } from '../poller/prs.ts';
import type { GraphiteSnapshot } from '../stack/graphiteLocal.ts';
import { GraphiteLocal } from '../stack/graphiteLocal.ts';
import type { InboxDeps } from './inbox.ts';
import { groupIntoStacks, inboxRouter } from './inbox.ts';

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

function freshDb() {
  dir = mkdtempSync(join(tmpdir(), 'panopticon-inbox-test-'));
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

function parentOfFromSnapshot(
  snapshot: GraphiteSnapshot | null,
): (pr: PrSummary) => string {
  return (pr) => snapshot?.branches.get(pr.head.ref)?.parent ?? pr.base.ref;
}

describe('groupIntoStacks', () => {
  test('chains a three-PR stack bottom to top', () => {
    const bottom = fakePr({
      number: 1,
      base: { ref: 'master', sha: 's' },
      head: { ref: 'branch-1', sha: 's' },
    });
    const middle = fakePr({
      number: 2,
      base: { ref: 'branch-1', sha: 's' },
      head: { ref: 'branch-2', sha: 's' },
    });
    const top = fakePr({
      number: 3,
      base: { ref: 'branch-2', sha: 's' },
      head: { ref: 'branch-3', sha: 's' },
    });

    const stacks = groupIntoStacks(
      [top, bottom, middle],
      parentOfFromSnapshot(null),
    );

    expect(stacks).toHaveLength(1);
    expect(stacks[0]?.owner).toBe('acme');
    expect(stacks[0]?.repo).toBe('widgets');
    expect(stacks[0]?.entries.map((e) => [e.number, e.parent])).toEqual([
      [1, null],
      [2, 1],
      [3, 2],
    ]);
  });

  test('a PR based on trunk with nothing above it is a standalone stack', () => {
    const standalone = fakePr({
      number: 5,
      base: { ref: 'master', sha: 's' },
      head: { ref: 'branch-5', sha: 's' },
    });

    const stacks = groupIntoStacks([standalone], parentOfFromSnapshot(null));

    expect(stacks).toHaveLength(1);
    expect(stacks[0]?.entries.map((e) => [e.number, e.parent])).toEqual([
      [5, null],
    ]);
  });

  test('a chain plus a standalone PR produce two separate stacks', () => {
    const bottom = fakePr({
      number: 1,
      base: { ref: 'master', sha: 's' },
      head: { ref: 'branch-1', sha: 's' },
    });
    const top = fakePr({
      number: 2,
      base: { ref: 'branch-1', sha: 's' },
      head: { ref: 'branch-2', sha: 's' },
    });
    const standalone = fakePr({
      number: 9,
      base: { ref: 'master', sha: 's' },
      head: { ref: 'branch-9', sha: 's' },
    });

    const stacks = groupIntoStacks(
      [bottom, top, standalone],
      parentOfFromSnapshot(null),
    );

    expect(stacks).toHaveLength(2);
    expect(
      stacks.map((stack) => stack.entries.map((e) => e.number)).sort(),
    ).toEqual([[1, 2], [9]].sort());
  });

  test('a fork keeps every child instead of only the newest', () => {
    const bottom = fakePr({
      number: 1,
      base: { ref: 'master', sha: 's' },
      head: { ref: 'branch-a', sha: 's' },
    });
    const forkOne = fakePr({
      number: 2,
      base: { ref: 'branch-a', sha: 's' },
      head: { ref: 'branch-b', sha: 's' },
    });
    const forkTwo = fakePr({
      number: 3,
      base: { ref: 'branch-a', sha: 's' },
      head: { ref: 'branch-c', sha: 's' },
    });
    const aboveForkOne = fakePr({
      number: 4,
      base: { ref: 'branch-b', sha: 's' },
      head: { ref: 'branch-d', sha: 's' },
    });

    const stacks = groupIntoStacks(
      [bottom, forkOne, forkTwo, aboveForkOne],
      parentOfFromSnapshot(null),
    );

    expect(stacks).toHaveLength(1);
    expect(stacks[0]?.entries.map((e) => [e.number, e.parent])).toEqual([
      [1, null],
      [2, 1],
      [3, 1],
      [4, 2],
    ]);
  });

  test('a PR pinned to a graphite-base placeholder joins its local metadata parent', () => {
    const bottom = fakePr({
      number: 1,
      base: { ref: 'master', sha: 's' },
      head: { ref: 'branch-1', sha: 's' },
    });
    // Graphite pinned the top PR's base to a placeholder named after
    // itself; the local branch metadata still carries the real parent.
    const top = fakePr({
      number: 2,
      base: { ref: 'graphite-base/2', sha: 's' },
      head: { ref: 'branch-2', sha: 's' },
    });
    const snapshot: GraphiteSnapshot = {
      trunk: 'master',
      branches: new Map([
        [
          'branch-1',
          { name: 'branch-1', parent: 'master', children: ['branch-2'] },
        ],
        ['branch-2', { name: 'branch-2', parent: 'branch-1', children: [] }],
      ]),
      prsByHead: new Map(),
      prsByNumber: new Map(),
    };

    const stacks = groupIntoStacks(
      [bottom, top],
      parentOfFromSnapshot(snapshot),
    );

    expect(stacks).toHaveLength(1);
    expect(stacks[0]?.entries.map((e) => [e.number, e.parent])).toEqual([
      [1, null],
      [2, 1],
    ]);
    expect(stacks[0]?.entries.find((e) => e.number === 2)?.baseRef).toBe(
      'branch-1',
    );
  });
});

function mount(
  prs: PrRepository,
  repos: string[],
  graphiteFor: InboxDeps['graphiteFor'] = () => new GraphiteLocal(null),
): Hono {
  return new Hono().route(
    '/api/inbox',
    inboxRouter({ prs, repos, graphiteFor }),
  );
}

describe('inboxRouter GET /', () => {
  test('groups mine open PRs into stacks and lists recent PRs separately', async () => {
    const prs = new PrRepository(freshDb());
    const bottom = fakePr({
      number: 1,
      base: { ref: 'master', sha: 's' },
      head: { ref: 'branch-1', sha: 's' },
    });
    const top = fakePr({
      number: 2,
      base: { ref: 'branch-1', sha: 's' },
      head: { ref: 'branch-2', sha: 's' },
    });
    const viewedOnly = fakePr({
      number: 42,
      author: { login: 'someone-else', avatarUrl: null, isBot: false },
    });
    prs.upsertSummary(bottom, true);
    prs.upsertSummary(top, true);
    prs.upsertSummary(viewedOnly, false);
    prs.recordView('acme', 'widgets', 42);

    const app = mount(prs, ['acme/widgets']);
    const res = await app.request('/api/inbox');

    expect(res.status).toBe(200);
    const body = (await res.json()) as InboxResponse;
    expect(body.stacks).toHaveLength(1);
    expect(body.stacks[0]?.entries.map((e) => e.number)).toEqual([1, 2]);
    expect(body.recent.map((pr) => pr.number)).toEqual([42]);
    expect(typeof body.fetchedAt).toBe('string');
  });

  test('orders stacks by their most recently updated PR', async () => {
    const prs = new PrRepository(freshDb());
    // Stack A: an old bottom with a freshly updated PR on top.
    const aBottom = fakePr({
      number: 1,
      updatedAt: '2026-01-01T00:00:00Z',
      base: { ref: 'master', sha: 's' },
      head: { ref: 'a-1', sha: 's' },
    });
    const aTop = fakePr({
      number: 2,
      updatedAt: '2026-03-01T00:00:00Z',
      base: { ref: 'a-1', sha: 's' },
      head: { ref: 'a-2', sha: 's' },
    });
    // Stack B: a single PR updated between the two.
    const b = fakePr({
      number: 3,
      updatedAt: '2026-02-01T00:00:00Z',
      base: { ref: 'master', sha: 's' },
      head: { ref: 'b-1', sha: 's' },
    });
    prs.upsertSummary(b, true);
    prs.upsertSummary(aBottom, true);
    prs.upsertSummary(aTop, true);

    const app = mount(prs, ['acme/widgets']);
    const body = (await (
      await app.request('/api/inbox')
    ).json()) as InboxResponse;

    expect(body.stacks.map((stack) => stack.entries[0]?.number)).toEqual([
      1, 3,
    ]);
  });

  test('recent excludes PRs already shown in a stack', async () => {
    const prs = new PrRepository(freshDb());
    const mine = fakePr({ number: 1 });
    prs.upsertSummary(mine, true);
    prs.recordView('acme', 'widgets', 1);

    const app = mount(prs, ['acme/widgets']);
    const res = await app.request('/api/inbox');
    const body = (await res.json()) as InboxResponse;

    expect(body.stacks[0]?.entries.map((e) => e.number)).toEqual([1]);
    expect(body.recent).toEqual([]);
  });

  test('a view on a PR the poller has not fetched yet is hidden until hydrated', async () => {
    const prs = new PrRepository(freshDb());
    prs.recordView('acme', 'widgets', 99);

    const app = mount(prs, ['acme/widgets']);
    const res = await app.request('/api/inbox');
    const body = (await res.json()) as InboxResponse;

    expect(body.recent).toEqual([]);
  });

  test('closed mine PRs do not appear in the My open PRs stacks', async () => {
    const prs = new PrRepository(freshDb());
    prs.upsertSummary(fakePr({ number: 1, state: 'closed' }), true);

    const app = mount(prs, ['acme/widgets']);
    const res = await app.request('/api/inbox');
    const body = (await res.json()) as InboxResponse;

    expect(body.stacks).toEqual([]);
  });
});
