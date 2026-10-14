import type { RepoConfig } from '../config.ts';
import type { Git } from '../git/git.ts';
import { GraphiteLocal } from './graphiteLocal.ts';

// A repo without a referenceClone gets a GraphiteLocal that always resolves
// to no snapshot.
export class GraphiteLocals {
  private readonly repos: Record<string, RepoConfig>;
  private readonly git: Git;
  private readonly locals = new Map<string, GraphiteLocal>();

  constructor(repos: Record<string, RepoConfig>, git: Git) {
    this.repos = repos;
    this.git = git;
  }

  for(owner: string, repo: string): GraphiteLocal {
    const key = `${owner}/${repo}`;
    const existing = this.locals.get(key);
    if (existing) return existing;

    const local = new GraphiteLocal(
      this.repos[key]?.referenceClone ?? null,
      this.git,
    );
    this.locals.set(key, local);
    return local;
  }
}
