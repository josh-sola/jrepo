import { beforeEach, describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import type { PrResponse } from '../../shared/api.ts';
import type {
  IssueComment,
  PrFile,
  PrSummary,
  ReviewSummary,
  ReviewThread,
} from '../../shared/github.ts';
import type { BaseRefChange, GitHubApi } from '../github/api.ts';
import { GitHubError } from '../github/api.ts';
import type { RepoStores } from '../git/stores.ts';
import { loadPr, prRouter, resetLoadPrForTests } from './pr.ts';

function fakePr(overrides: Partial<PrSummary> = {}): PrSummary {
  return {
    owner: 'Sola-Solutions',
    repo: 'monorepo',
    number: 1,
    title: 'A pull request',
    body: '',
    state: 'open',
    draft: false,
    author: { login: 'jordan-sola', avatarUrl: null, isBot: false },
    base: { ref: 'master', sha: 'base-sha' },
    head: { ref: 'feature', sha: 'head-sha' },
    mergeCommitSha: null,
    additions: 1,
    deletions: 1,
    changedFiles: 1,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    url: 'https://github.com/Sola-Solutions/monorepo/pull/1',
    ...overrides,
  };
}

function fakeFile(overrides: Partial<PrFile> = {}): PrFile {
  return {
    path: 'src/a.ts',
    previousPath: null,
    status: 'modified',
    additions: 1,
    deletions: 1,
    oldOid: null,
    newOid: null,
    ...overrides,
  };
}

interface FakeGitHubOptions {
  pr?: PrSummary;
  files?: { files: PrFile[]; truncated: boolean };
  threads?: ReviewThread[];
  issueComments?: IssueComment[];
  reviews?: ReviewSummary[];
  getPullError?: Error;
  onGetPull?: () => void;
}

function fakeGitHub(options: FakeGitHubOptions): GitHubApi {
  return {
    getPull: async () => {
      options.onGetPull?.();
      if (options.getPullError) throw options.getPullError;
      return options.pr ?? fakePr();
    },
    listPullFiles: async () =>
      options.files ?? { files: [fakeFile()], truncated: false },
    listOpenPulls: async () => [],
    findPullByHead: async () => null,
    listBaseRefChanges: async (): Promise<BaseRefChange[]> => [],
    listReviewThreads: async () => options.threads ?? [],
    listIssueComments: async () => options.issueComments ?? [],
    listReviews: async () => options.reviews ?? [],
    createReviewComment: async () => {},
    replyToReviewComment: async () => {},
    setThreadResolved: async () => {},
    searchReviewInbox: async () => [],
  };
}

interface FakeStoreOptions {
  blobOid?: (treeSha: string, path: string) => Promise<string | null>;
  hasCommit?: (sha: string) => Promise<boolean>;
  nameStatus?: PrFile[];
  landedCommit?: string | null;
}

function fakeStores(options: FakeStoreOptions = {}): RepoStores {
  const blobOid =
    options.blobOid ?? (async (treeSha: string) => `${treeSha}-blob`);
  const store = {
    ensure: async () => {},
    fetchPull: async () => {},
    fetchTrunk: async () => {},
    hasCommit: options.hasCommit ?? (async () => true),
    landedCommit: async () => options.landedCommit ?? null,
    blobOid,
    blobOids: async (treeSha: string, paths: string[]) =>
      Promise.all(paths.map((path) => blobOid(treeSha, path))),
    readBlob: async () => null,
    nameStatus: async () => options.nameStatus ?? [],
    generatedPaths: async () => new Set<string>(),
  };
  return { for: () => store } as unknown as RepoStores;
}

function trunkFor(): string {
  return 'master';
}

describe('loadPr', () => {
  test('fills each file oldOid and newOid from the base and head trees', async () => {
    const pr = fakePr();
    const github = fakeGitHub({
      pr,
      files: { files: [fakeFile({ path: 'src/a.ts' })], truncated: false },
    });
    const stores = fakeStores({
      blobOid: async (treeSha, path) => `${treeSha}:${path}`,
    });

    const result = await loadPr(
      { github, stores, trunkFor },
      'Sola-Solutions',
      'monorepo',
      1,
    );

    expect(result.files[0]?.oldOid).toBe('base-sha:src/a.ts');
    expect(result.files[0]?.newOid).toBe('head-sha:src/a.ts');
  });

  test('resolves a renamed file oldOid against its previous path', async () => {
    const pr = fakePr();
    const github = fakeGitHub({
      pr,
      files: {
        files: [
          fakeFile({
            path: 'src/new.ts',
            previousPath: 'src/old.ts',
            status: 'renamed',
          }),
        ],
        truncated: false,
      },
    });
    const stores = fakeStores({
      blobOid: async (treeSha, path) => `${treeSha}:${path}`,
    });

    const result = await loadPr({ github, stores, trunkFor }, 'o', 'r', 1);

    expect(result.files[0]?.oldOid).toBe('base-sha:src/old.ts');
    expect(result.files[0]?.newOid).toBe('head-sha:src/new.ts');
  });

  test('falls back to git name-status when the file list is truncated', async () => {
    const pr = fakePr();
    const nameStatusFiles = [fakeFile({ path: 'from-git-diff.ts' })];
    const github = fakeGitHub({
      pr,
      files: { files: [fakeFile({ path: 'from-rest.ts' })], truncated: true },
    });
    const stores = fakeStores({ nameStatus: nameStatusFiles });

    const result = await loadPr({ github, stores, trunkFor }, 'o', 'r', 1);

    expect(result.files.map((f) => f.path)).toEqual(['from-git-diff.ts']);
  });

  test('promotes a closed PR to merged when its squash commit landed on trunk', async () => {
    const pr = fakePr({ state: 'closed', number: 20744, mergeCommitSha: null });
    const github = fakeGitHub({ pr });
    const stores = fakeStores({ landedCommit: 'cafef00d' });

    const result = await loadPr({ github, stores, trunkFor }, 'o', 'r', 20744);

    expect(result.pr.state).toBe('merged');
    expect(result.pr.mergeCommitSha).toBe('cafef00d');
  });

  test('leaves a closed PR closed when no squash commit landed', async () => {
    const pr = fakePr({ state: 'closed', mergeCommitSha: null });
    const github = fakeGitHub({ pr });
    const stores = fakeStores({ landedCommit: null });

    const result = await loadPr({ github, stores, trunkFor }, 'o', 'r', 1);

    expect(result.pr.state).toBe('closed');
    expect(result.pr.mergeCommitSha).toBeNull();
  });
});

describe('loadPr concurrency', () => {
  beforeEach(() => {
    resetLoadPrForTests();
  });

  test('two concurrent loads for the same PR call github.getPull once', async () => {
    // Mirrors how the pr, diff, and hover routes each call loadPr on the
    // same page load: fired together, before either has settled.
    let getPullCalls = 0;
    const github = fakeGitHub({
      onGetPull: () => {
        getPullCalls += 1;
      },
    });
    const stores = fakeStores();

    const [firstResult, secondResult] = await Promise.all([
      loadPr({ github, stores, trunkFor }, 'o', 'r', 1),
      loadPr({ github, stores, trunkFor }, 'o', 'r', 1),
    ]);

    expect(getPullCalls).toBe(1);
    expect(firstResult).toEqual(secondResult);
  });

  test('a later load for the same PR runs fresh, after the first settles', async () => {
    let getPullCalls = 0;
    const github = fakeGitHub({
      onGetPull: () => {
        getPullCalls += 1;
      },
    });
    const stores = fakeStores();

    await loadPr({ github, stores, trunkFor }, 'o', 'r', 1);
    await loadPr({ github, stores, trunkFor }, 'o', 'r', 1);

    expect(getPullCalls).toBe(2);
  });
});

// Mirrors how app.ts mounts this router, since :owner/:repo/:number only
// exist as route params once something mounts prRouter under that pattern.
function mount(deps: Parameters<typeof prRouter>[0]): Hono {
  const app = new Hono();
  app.route('/api/pr/:owner/:repo/:number', prRouter(deps));
  return app;
}

describe('prRouter GET /', () => {
  test('returns a PrResponse assembled from the PR, files, threads, comments, and reviews', async () => {
    const pr = fakePr();
    const github = fakeGitHub({
      pr,
      files: { files: [fakeFile()], truncated: false },
      issueComments: [
        {
          id: 1,
          author: { login: 'jordan-sola', avatarUrl: null, isBot: false },
          body: 'hi',
          createdAt: '2026-01-01T00:00:00Z',
          url: 'https://github.com/x',
        },
      ],
      reviews: [
        {
          id: 2,
          author: { login: 'jared-sola', avatarUrl: null, isBot: false },
          state: 'APPROVED',
          body: '',
          submittedAt: '2026-01-01T00:00:00Z',
          url: 'https://github.com/y',
        },
      ],
    });
    const app = mount({ github, stores: fakeStores(), trunkFor });

    const res = await app.request('/api/pr/Sola-Solutions/monorepo/1');

    expect(res.status).toBe(200);
    const body = (await res.json()) as PrResponse;
    expect(body.pr.number).toBe(1);
    expect(body.files.length).toBe(1);
    expect(body.issueComments[0]?.body).toBe('hi');
    expect(body.reviews[0]?.state).toBe('APPROVED');
    expect(typeof body.fetchedAt).toBe('string');
  });

  test('rejects a non-numeric PR number with 400', async () => {
    const app = mount({
      github: fakeGitHub({}),
      stores: fakeStores(),
      trunkFor,
    });
    const res = await app.request('/api/pr/o/r/not-a-number');
    expect(res.status).toBe(400);
  });

  test('returns 404 when GitHub reports the PR does not exist', async () => {
    const github = fakeGitHub({
      getPullError: new GitHubError(404, 'Not Found'),
    });
    const app = mount({ github, stores: fakeStores(), trunkFor });
    const res = await app.request('/api/pr/o/r/99999');
    expect(res.status).toBe(404);
  });

  test('returns 502 with the GitHub message for a non-404 GitHub failure', async () => {
    const github = fakeGitHub({
      getPullError: new GitHubError(500, 'Internal Server Error'),
    });
    const app = mount({ github, stores: fakeStores(), trunkFor });
    const res = await app.request('/api/pr/o/r/1');
    expect(res.status).toBe(502);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe('Internal Server Error');
  });
});
