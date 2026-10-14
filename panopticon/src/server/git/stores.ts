import { join } from 'node:path';
import type { Git } from './git.ts';
import { RepoStore } from './store.ts';

// Lazily builds and caches one RepoStore per GitHub repo. Clones use HTTPS;
// `git` supplies the credentials.
export class RepoStores {
  private readonly dataDir: string;
  private readonly git: Git;
  private readonly stores = new Map<string, RepoStore>();

  constructor(dataDir: string, git: Git) {
    this.dataDir = dataDir;
    this.git = git;
  }

  for(owner: string, repo: string): RepoStore {
    const key = `${owner}/${repo}`;
    const existing = this.stores.get(key);
    if (existing) return existing;

    const store = new RepoStore({
      dir: join(this.dataDir, 'repos', owner, `${repo}.git`),
      remoteUrl: `https://github.com/${owner}/${repo}.git`,
      git: this.git,
    });
    this.stores.set(key, store);
    return store;
  }
}
