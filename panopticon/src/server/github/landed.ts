import type { PrSummary } from '../../shared/github.ts';
import type { RepoStore } from '../git/store.ts';

const trunkFetches = new WeakMap<RepoStore, Promise<void>>();

// Fetching trunk is one round trip per store, not per PR, so concurrent
// callers checking different closed PRs against the same store share it
// instead of each fetching trunk on their own.
function ensureTrunkFetched(store: RepoStore, trunk: string): Promise<void> {
  const existing = trunkFetches.get(store);
  if (existing) return existing;
  const fetch = store.fetchTrunk(trunk);
  trunkFetches.set(store, fetch);
  return fetch;
}

// Graphite's merge queue closes a PR instead of merging it through GitHub,
// so a closed PR only counts as merged once its squash commit is found on
// trunk. Open PRs are returned unchanged; nothing here can tell an open PR
// apart from one that will never land.
export async function withLandedState(
  pr: PrSummary,
  store: RepoStore,
  trunk: string,
): Promise<PrSummary> {
  if (pr.state !== 'closed') return pr;

  await ensureTrunkFetched(store, trunk);
  const sha = await store.landedCommit(trunk, pr.number, pr.createdAt);
  if (sha === null) return pr;

  return { ...pr, state: 'merged', mergeCommitSha: sha };
}
