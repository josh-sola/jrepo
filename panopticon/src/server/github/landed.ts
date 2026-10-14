import type { PrSummary } from '../../shared/github.ts';
import type { GtApiClient } from '../gtapi/client.ts';

// Graphite's merge queue closes a PR instead of merging it through GitHub,
// so a closed PR only counts as merged once gt-api finds its squash commit
// on trunk. Open PRs are returned unchanged; nothing here can tell an open PR
// apart from one that will never land. If gt-api fails, the PR stays as it is.
export async function withLandedState(
  pr: PrSummary,
  gtApi: GtApiClient,
): Promise<PrSummary> {
  if (pr.state !== 'closed') return pr;

  let sha: string | null;
  try {
    sha = await gtApi.landed(pr.owner, pr.repo, pr.number, pr.createdAt);
  } catch (error) {
    console.warn(
      `panopticon: landed check for ${pr.owner}/${pr.repo}#${pr.number} failed`,
      error,
    );
    return pr;
  }
  if (sha === null) return pr;

  return { ...pr, state: 'merged', mergeCommitSha: sha };
}
