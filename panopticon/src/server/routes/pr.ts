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

async function fillBlobOids(
  store: RepoStore,
  pr: PrSummary,
  files: PrFile[],
): Promise<PrFile[]> {
  const oldPaths = files.map((file) => file.previousPath ?? file.path);
  const newPaths = files.map((file) => file.path);
  const [oldOids, newOids] = await Promise.all([
    store.blobOids(pr.base.sha, oldPaths),
    store.blobOids(pr.head.sha, newPaths),
  ]);
  return files.map((file, index) => ({
    ...file,
    oldOid: oldOids[index] ?? null,
    newOid: newOids[index] ?? null,
  }));
}

// Concurrent callers loading the same PR (the pr, diff, and hover routes
// all fire on page load) share one in-flight load instead of each running
// its own GitHub calls and git fetches. Entries are removed once they
// settle, success or failure, so a later call always sees fresh state —
// the poller and SSE invalidation rely on that when the head changes.
const inFlightLoads = new Map<
  string,
  Promise<{ pr: PrSummary; files: PrFile[] }>
>();

export function resetLoadPrForTests(): void {
  inFlightLoads.clear();
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
  const key = `${owner}/${repo}#${number}`;
  const existing = inFlightLoads.get(key);
  if (existing) return existing;
  const promise = loadPrUncached(deps, owner, repo, number).finally(() => {
    inFlightLoads.delete(key);
  });
  inFlightLoads.set(key, promise);
  return promise;
}

async function loadPrUncached(
  deps: PrDeps,
  owner: string,
  repo: string,
  number: number,
): Promise<{ pr: PrSummary; files: PrFile[] }> {
  // The file list does not depend on the PR summary, so both GitHub calls
  // go out at once; only the git fetch has to wait for the base and head shas.
  const fileListPromise = deps.github.listPullFiles(owner, repo, number);
  const store = deps.stores.for(owner, repo);
  const ensurePromise = store.ensure();
  const rawPr = await deps.github.getPull(owner, repo, number);
  const pr = await withLandedState(rawPr, store, deps.trunkFor(owner, repo));

  const [fileList] = await Promise.all([
    fileListPromise,
    ensurePromise.then(() => store.fetchPull(number, pr.base.sha, pr.head.sha)),
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
