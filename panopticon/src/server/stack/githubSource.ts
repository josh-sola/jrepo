import { GitHubError, type GitHubApi } from '../github/api.ts';
import { withLandedState } from '../github/landed.ts';
import type { RepoStore } from '../git/store.ts';
import type { StackSource } from './source.ts';

export function githubStackSource(
  github: GitHubApi,
  store: RepoStore,
  trunk: string,
  owner: string,
  repo: string,
): StackSource {
  return {
    async getPull(number) {
      try {
        const pr = await github.getPull(owner, repo, number);
        return await withLandedState(pr, store, trunk);
      } catch (error) {
        if (error instanceof GitHubError && error.status === 404) return null;
        throw error;
      }
    },
    listOpenPulls: () => github.listOpenPulls(owner, repo),
    async findPullByHead(branch) {
      const pr = await github.findPullByHead(owner, repo, branch);
      return pr === null ? null : withLandedState(pr, store, trunk);
    },
    listBaseRefChanges: (number) =>
      github.listBaseRefChanges(owner, repo, number),
  };
}
