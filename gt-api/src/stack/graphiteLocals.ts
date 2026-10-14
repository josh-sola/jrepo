import type { RepoConfig } from '../config.ts';
import { repoKey } from '../config.ts';
import type { Git } from '../git/git.ts';
import { GraphiteLocal } from './graphiteLocal.ts';

// A repo without a checkout gets a GraphiteLocal that always resolves to no
// snapshot.
export class GraphiteLocals {
  private readonly git: Git;
  private readonly locals = new Map<string, GraphiteLocal>();

  constructor(git: Git) {
    this.git = git;
  }

  for(repo: RepoConfig): GraphiteLocal {
    const key = repoKey(repo.owner, repo.repo);
    const existing = this.locals.get(key);
    if (existing) return existing;

    const local = new GraphiteLocal(repo.checkout ?? null, this.git);
    this.locals.set(key, local);
    return local;
  }
}
