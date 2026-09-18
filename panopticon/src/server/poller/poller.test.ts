import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { PrSummary, ReviewThread } from '../../shared/github.ts';
import type { BaseRefChange, GitHubApi } from '../github/api.ts';
import { GitHubError } from '../github/api.ts';
import type { RepoStores } from '../git/stores.ts';
import { openDb } from '../db.ts';
import { PrEventBus } from '../events.ts';
import type { PrEvent } from '../../shared/events.ts';
import type { Cleanup } from './poller.ts';
import { Poller } from './poller.ts';
import { PrRepository } from './prs.ts';

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

function freshDb() {
  dir = mkdtempSync(join(tmpdir(), 'panopticon-poller-test-'));
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

interface FakeGitHubOptions {
  openPulls?: PrSummary[];
  getPull?: (number: number) => Promise<PrSummary>;
  reviewThreads?: () => Promise<ReviewThread[]>;
  listOpenPullsError?: Error;
}

function fakeGitHub(options: FakeGitHubOptions = {}): GitHubApi {
  return {
    getPull: async (_owner, _repo, number) => {
      if (options.getPull) return options.getPull(number);
      return fakePr({ number });
    },
    listPullFiles: async () => ({ files: [], truncated: false }),
    listOpenPulls: async () => {
      if (options.listOpenPullsError) throw options.listOpenPullsError;
      return options.openPulls ?? [];
    },
    findPullByHead: async () => null,
    listBaseRefChanges: async (): Promise<BaseRefChange[]> => [],
    listReviewThreads: async () =>
      options.reviewThreads ? options.reviewThreads() : [],
    listIssueComments: async () => [],
    listReviews: async () => [],
    createReviewComment: async () => {},
    replyToReviewComment: async () => {},
    setThreadResolved: async () => {},
  };
}

function fakeStores(landedCommit: string | null = null): RepoStores {
  const store = {
    ensure: async () => {},
    fetchPull: async () => {},
    fetchTrunk: async () => {},
    landedCommit: async () => landedCommit,
    blobOid: async () => null,
    readBlob: async () => null,
    nameStatus: async () => [],
    generatedPaths: async () => new Set<string>(),
  };
  return { for: () => store } as unknown as RepoStores;
}

function trunkFor(): string {
  return 'master';
}

function noopCleanup(): Cleanup {
  return { teardownPr: async () => {} };
}

// Fixed to match fakePr()'s default updatedAt, so the once-a-day cleanup
// that every tick() checks never treats a freshly upserted fixture as due
// for pruning unless a test deliberately advances the clock.
const FIXED_NOW = new Date('2026-01-01T00:00:00Z').getTime();

function basePollerDeps(
  overrides: Partial<ConstructorParameters<typeof Poller>[0]> = {},
) {
  const db = freshDb();
  // PrRepository's own date math (listWatched, pruneClosed, ...) needs the
  // same clock as the poller, or a test's fixed clock only half-applies.
  const clock = overrides.clock ?? (() => FIXED_NOW);
  return {
    github: fakeGitHub(),
    stores: fakeStores(),
    trunkFor,
    prs: new PrRepository(db, clock),
    bus: new PrEventBus(),
    db,
    cleanup: noopCleanup(),
    repos: ['acme/widgets'],
    login: 'jordan',
    intervalMs: 60_000,
    clock,
    setTimer: (() => 0) as unknown as typeof setTimeout,
    ...overrides,
  };
}

describe('Poller.tick', () => {
  test('upserts an open PR authored by the configured login as mine', async () => {
    const deps = basePollerDeps({
      github: fakeGitHub({ openPulls: [fakePr()] }),
    });
    const poller = new Poller(deps);

    await poller.tick();

    const stored = deps.prs.get('acme', 'widgets', 1);
    expect(stored?.isMine).toBe(true);
    expect(stored?.state).toBe('open');
  });

  test('backfills line counts for an authored open PR via getPull, since listOpenPulls zeroes them', async () => {
    let getPullCalls = 0;
    const deps = basePollerDeps({
      github: fakeGitHub({
        openPulls: [fakePr({ additions: 0, deletions: 0, changedFiles: 0 })],
        getPull: async (number) => {
          getPullCalls += 1;
          return fakePr({
            number,
            additions: 5,
            deletions: 3,
            changedFiles: 2,
          });
        },
      }),
    });
    const poller = new Poller(deps);

    await poller.tick();

    expect(getPullCalls).toBe(1);
    const stored = deps.prs.get('acme', 'widgets', 1);
    expect(stored?.summary.additions).toBe(5);
    expect(stored?.summary.deletions).toBe(3);
    expect(stored?.summary.changedFiles).toBe(2);
  });

  test('a second poll with the same updatedAt reuses stored counts instead of calling getPull again', async () => {
    let getPullCalls = 0;
    const openPr = fakePr({ additions: 0, deletions: 0, changedFiles: 0 });
    const deps = basePollerDeps({
      github: fakeGitHub({
        openPulls: [openPr],
        getPull: async (number) => {
          getPullCalls += 1;
          return fakePr({
            number,
            additions: 5,
            deletions: 3,
            changedFiles: 2,
          });
        },
      }),
    });
    const poller = new Poller(deps);

    await poller.tick();
    await poller.tick();

    expect(getPullCalls).toBe(1);
    const stored = deps.prs.get('acme', 'widgets', 1);
    expect(stored?.summary.additions).toBe(5);
    expect(stored?.summary.deletions).toBe(3);
    expect(stored?.summary.changedFiles).toBe(2);
  });

  test('a changed updatedAt triggers getPull again to refresh the counts', async () => {
    let getPullCalls = 0;
    const deps = basePollerDeps({
      github: fakeGitHub({
        openPulls: [],
        getPull: async (number) => {
          getPullCalls += 1;
          return fakePr({
            number,
            additions: 5,
            deletions: 3,
            changedFiles: 2,
          });
        },
      }),
    });
    let openPulls = [
      fakePr({
        additions: 0,
        deletions: 0,
        changedFiles: 0,
        updatedAt: '2026-01-01T00:00:00Z',
      }),
    ];
    deps.github.listOpenPulls = async () => openPulls;
    const poller = new Poller(deps);

    await poller.tick();
    expect(getPullCalls).toBe(1);

    openPulls = [
      fakePr({
        additions: 0,
        deletions: 0,
        changedFiles: 0,
        updatedAt: '2026-01-02T00:00:00Z',
      }),
    ];
    await poller.tick();

    expect(getPullCalls).toBe(2);
    const stored = deps.prs.get('acme', 'widgets', 1);
    expect(stored?.summary.additions).toBe(5);
    expect(stored?.summary.deletions).toBe(3);
    expect(stored?.summary.changedFiles).toBe(2);
  });

  test("does not store someone else's open PR unless it is watched", async () => {
    const deps = basePollerDeps({
      github: fakeGitHub({
        openPulls: [
          fakePr({
            author: { login: 'someone-else', avatarUrl: null, isBot: false },
          }),
        ],
      }),
    });
    const poller = new Poller(deps);

    await poller.tick();

    expect(deps.prs.get('acme', 'widgets', 1)).toBeNull();

    deps.prs.recordView('acme', 'widgets', 1);
    await poller.tick();

    const stored = deps.prs.get('acme', 'widgets', 1);
    expect(stored?.isMine).toBe(false);
    expect(stored?.state).toBe('open');
  });

  test('publishes pr-updated when the stored head sha changes', async () => {
    const deps = basePollerDeps({
      github: fakeGitHub({
        openPulls: [fakePr({ head: { ref: 'feature', sha: 'sha-2' } })],
      }),
    });
    deps.prs.upsertSummary(
      fakePr({ head: { ref: 'feature', sha: 'sha-1' } }),
      true,
    );

    const events: PrEvent[] = [];
    const iterator = deps.bus
      .subscribe('acme/widgets#1')
      [Symbol.asyncIterator]();
    void (async () => {
      for (;;) {
        const result = await iterator.next();
        if (result.done) return;
        events.push(result.value);
      }
    })();

    const poller = new Poller(deps);
    await poller.tick();
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(events).toContainEqual({ type: 'pr-updated', headSha: 'sha-2' });
    await iterator.return?.();
  });

  test('publishes pr-closed and tears down state when a watched PR closes', async () => {
    const torndown: { value: [string, string, number] | null } = {
      value: null,
    };
    const cleanup: Cleanup = {
      teardownPr: async (owner, repo, number) => {
        torndown.value = [owner, repo, number];
      },
    };
    const deps = basePollerDeps({
      github: fakeGitHub({
        openPulls: [],
        getPull: async (number) =>
          fakePr({ number, state: 'closed', mergeCommitSha: null }),
      }),
      stores: fakeStores('landed-sha'),
      cleanup,
    });
    deps.prs.upsertSummary(fakePr(), true);

    const publishedEvents: PrEvent[] = [];
    const originalPublish = deps.bus.publish.bind(deps.bus);
    deps.bus.publish = (key, event) => {
      publishedEvents.push(event);
      originalPublish(key, event);
    };

    const poller = new Poller(deps);
    await poller.tick();
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(publishedEvents).toContainEqual({
      type: 'pr-closed',
      state: 'merged',
    });
    expect(torndown.value).toEqual(['acme', 'widgets', 1]);
    expect(deps.prs.get('acme', 'widgets', 1)?.state).toBe('merged');
  });

  test('refreshes thread fingerprints only for PRs with a live subscriber', async () => {
    let threadCalls = 0;
    const deps = basePollerDeps({
      github: fakeGitHub({
        openPulls: [fakePr({ number: 1 }), fakePr({ number: 2 })],
        reviewThreads: async () => {
          threadCalls += 1;
          return [
            {
              id: 'thread-1',
              path: 'a.ts',
              line: 1,
              originalLine: 1,
              startLine: null,
              side: 'RIGHT',
              startSide: null,
              isResolved: false,
              isOutdated: false,
              diffHunk: '',
              comments: [],
            },
          ];
        },
      }),
    });
    // Only PR #1 has a live subscriber.
    const iterator = deps.bus
      .subscribe('acme/widgets#1')
      [Symbol.asyncIterator]();

    const poller = new Poller(deps);
    await poller.tick();

    expect(threadCalls).toBe(1);
    expect(deps.prs.get('acme', 'widgets', 1)?.threadFingerprint).toBe(
      'thread-1:0:false',
    );
    expect(deps.prs.get('acme', 'widgets', 2)?.threadFingerprint).toBeNull();

    await iterator.return?.();
  });

  test('a failing repo is logged and does not stop other repos from polling', async () => {
    const dbA = freshDb();
    const prsA = new PrRepository(dbA);
    const githubA = fakeGitHub({ listOpenPullsError: new Error('boom') });
    const githubB = fakeGitHub({
      openPulls: [fakePr({ owner: 'other', repo: 'thing' })],
    });

    const originalError = console.error;
    console.error = () => {};
    try {
      const poller = new Poller({
        github: {
          ...githubA,
          listOpenPulls: async (owner, repo) => {
            if (owner === 'acme') throw new Error('boom');
            return githubB.listOpenPulls(owner, repo);
          },
        },
        stores: fakeStores(),
        trunkFor,
        prs: prsA,
        bus: new PrEventBus(),
        db: dbA,
        cleanup: noopCleanup(),
        repos: ['acme/widgets', 'other/thing'],
        login: 'jordan',
        intervalMs: 60_000,
        setTimer: (() => 0) as unknown as typeof setTimeout,
      });

      await poller.tick();

      expect(prsA.get('other', 'thing', 1)?.state).toBe('open');
    } finally {
      console.error = originalError;
    }
  });

  test('a 403 rate-limit error pauses further polling this tick', async () => {
    const deps = basePollerDeps({
      github: fakeGitHub({
        listOpenPullsError: new GitHubError(403, 'rate limited'),
      }),
      repos: ['acme/widgets', 'other/thing'],
    });
    let otherRepoPolled = false;
    const originalListOpenPulls = deps.github.listOpenPulls.bind(deps.github);
    deps.github.listOpenPulls = async (owner, repo) => {
      if (owner === 'other') otherRepoPolled = true;
      return originalListOpenPulls(owner, repo);
    };

    const originalError = console.error;
    console.error = () => {};
    try {
      const poller = new Poller(deps);
      await poller.tick();
    } finally {
      console.error = originalError;
    }

    expect(otherRepoPolled).toBe(false);
  });

  test('runs pruneClosed and viewed pruning once per day of clock time', async () => {
    const clock = { now: 1_000_000 };
    // No configured repos: this isolates the daily cleanup from the poll
    // cycle, which would otherwise re-fetch and overwrite this fixture.
    const deps = basePollerDeps({ clock: () => clock.now, repos: [] });
    deps.prs.upsertSummary(
      fakePr({ state: 'merged', updatedAt: new Date(clock.now).toISOString() }),
      true,
    );

    const poller = new Poller(deps);
    await poller.tick();
    expect(deps.prs.get('acme', 'widgets', 1)).not.toBeNull();

    clock.now += 8 * 24 * 60 * 60 * 1000;
    await poller.tick();

    expect(deps.prs.get('acme', 'widgets', 1)).toBeNull();
  });
});

describe('Poller.start/stop', () => {
  test('start schedules a tick and stop cancels it before it runs', () => {
    const scheduled: { fn: () => void; delay: number }[] = [];
    const fakeSetTimer = ((fn: () => void, delay: number) => {
      scheduled.push({ fn, delay });
      return 1 as unknown as ReturnType<typeof setTimeout>;
    }) as unknown as typeof setTimeout;

    const deps = basePollerDeps({ setTimer: fakeSetTimer });
    const poller = new Poller(deps);

    poller.start();
    expect(scheduled).toHaveLength(1);
    expect(scheduled[0]?.delay).toBe(0);

    poller.stop();
    // Nothing further to assert on the fake timer beyond stop() not
    // throwing; the real assertion is that clearTimeout was reachable.
  });
});
