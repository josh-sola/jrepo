import { Hono } from 'hono';
import type { PrResponse } from '../../shared/api.ts';
import type { PrFile, PrSummary } from '../../shared/github.ts';
import { GitHubError } from '../github/api.ts';
import type { GitHubApi } from '../github/api.ts';
import { withLandedState } from '../github/landed.ts';
import type { RepoStore } from '../git/store.ts';
import type { RepoStores } from '../git/stores.ts';

export interface PrDeps {
  github: GitHubApi;
  stores: RepoStores;
  trunkFor(owner: string, repo: string): string;
}

const OID_FILL_CONCURRENCY = 16;

async function mapWithConcurrency<T, R>(
  items: T[],
  limit: number,
  fn: (item: T) => Promise<R>,
): Promise<R[]> {
  const results: R[] = Array.from({ length: items.length });
  let next = 0;

  async function worker(): Promise<void> {
    for (;;) {
      const index = next;
      next += 1;
      if (index >= items.length) return;
      const item = items[index];
      if (item === undefined) return;
      results[index] = await fn(item);
    }
  }

  await Promise.all(
    Array.from({ length: Math.min(limit, items.length) }, () => worker()),
  );
  return results;
}

async function fillBlobOids(
  store: RepoStore,
  pr: PrSummary,
  files: PrFile[],
): Promise<PrFile[]> {
  return mapWithConcurrency(files, OID_FILL_CONCURRENCY, async (file) => {
    const oldPath = file.previousPath ?? file.path;
    const [oldOid, newOid] = await Promise.all([
      store.blobOid(pr.base.sha, oldPath),
      store.blobOid(pr.head.sha, file.path),
    ]);
    return { ...file, oldOid, newOid };
  });
}

// Fetches a PR's summary and file list, fetches its base and head into the
// repo's bare clone, and fills every file's blob oids from git. Other
// routes that only need the PR and its files, not comments or reviews,
// call this directly instead of going through the HTTP layer.
export async function loadPr(
  deps: PrDeps,
  owner: string,
  repo: string,
  number: number,
): Promise<{ pr: PrSummary; files: PrFile[] }> {
  const rawPr = await deps.github.getPull(owner, repo, number);
  const store = deps.stores.for(owner, repo);
  const pr = await withLandedState(rawPr, store, deps.trunkFor(owner, repo));

  const [fileList] = await Promise.all([
    deps.github.listPullFiles(owner, repo, number),
    store.ensure().then(() => store.fetchPull(number, pr.base.sha)),
  ]);

  const files = fileList.truncated
    ? await store.nameStatus(pr.base.sha, pr.head.sha)
    : fileList.files;

  return { pr, files: await fillBlobOids(store, pr, files) };
}

function parsePrNumber(raw: string): number | null {
  if (!/^\d+$/.test(raw)) return null;
  return Number(raw);
}

export function prRouter(deps: PrDeps): Hono {
  return new Hono().get('/', async (c) => {
    const owner = c.req.param('owner');
    const repo = c.req.param('repo');
    const numberParam = c.req.param('number');
    const number =
      numberParam === undefined ? null : parsePrNumber(numberParam);
    if (owner === undefined || repo === undefined || number === null) {
      return c.json({ error: 'expected /:owner/:repo/:number' }, 400);
    }

    try {
      const [{ pr, files }, threads, issueComments, reviews] =
        await Promise.all([
          loadPr(deps, owner, repo, number),
          deps.github.listReviewThreads(owner, repo, number),
          deps.github.listIssueComments(owner, repo, number),
          deps.github.listReviews(owner, repo, number),
        ]);
      const body: PrResponse = {
        pr,
        files,
        threads,
        issueComments,
        reviews,
        fetchedAt: new Date().toISOString(),
      };
      return c.json(body);
    } catch (error) {
      if (error instanceof GitHubError) {
        if (error.status === 404) {
          return c.json({ error: error.message }, 404);
        }
        return c.json({ error: error.message }, 502);
      }
      throw error;
    }
  });
}
