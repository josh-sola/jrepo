import type { RepoConfig } from '../config.ts';
import { GraphiteLocal } from './graphiteLocal.ts';

// A repo without a referenceClone gets a GraphiteLocal that always resolves
// to no snapshot.
export class GraphiteLocals {
  private readonly repos: Record<string, RepoConfig>;
  private readonly locals = new Map<string, GraphiteLocal>();

  constructor(repos: Record<string, RepoConfig>) {
    this.repos = repos;
  }

  for(owner: string, repo: string): GraphiteLocal {
    const key = `${owner}/${repo}`;
    const existing = this.locals.get(key);
    if (existing) return existing;

    const local = new GraphiteLocal(this.repos[key]?.referenceClone ?? null);
    this.locals.set(key, local);
    return local;
  }
}
