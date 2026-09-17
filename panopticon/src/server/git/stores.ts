import { join } from 'node:path';
import type { RepoConfig } from '../config.ts';
import { RepoStore } from './store.ts';

// Lazily builds and caches one RepoStore per GitHub repo. gh's own git
// remote protocol here is ssh, so the clone URL is always the ssh form.
export class RepoStores {
  private readonly dataDir: string;
  private readonly repos: Record<string, RepoConfig>;
  private readonly stores = new Map<string, RepoStore>();

  constructor(dataDir: string, repos: Record<string, RepoConfig>) {
    this.dataDir = dataDir;
    this.repos = repos;
  }

  for(owner: string, repo: string): RepoStore {
    const key = `${owner}/${repo}`;
    const existing = this.stores.get(key);
    if (existing) return existing;

    const store = new RepoStore({
      dir: join(this.dataDir, 'repos', owner, `${repo}.git`),
      remoteUrl: `git@github.com:${owner}/${repo}.git`,
      referenceClone: this.repos[key]?.referenceClone ?? null,
    });
    this.stores.set(key, store);
    return store;
  }
}
