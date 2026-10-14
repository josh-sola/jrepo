import { buildApp } from './app.ts';
import { loadConfig } from './config.ts';
import { createGit } from './git/git.ts';
import { RepoStores } from './git/stores.ts';
import { createGitHubApi, readGhToken } from './github/client.ts';
import { GraphiteLocals } from './stack/graphiteLocals.ts';

const config = loadConfig();

const token = await readGhToken();
const git = createGit({
  token,
  safeDirectories: config.repos.flatMap((repo) =>
    repo.checkout === undefined ? [] : [repo.checkout],
  ),
});

const app = buildApp({
  config,
  github: createGitHubApi(token),
  stores: new RepoStores(config.dataDir, git),
  graphiteLocals: new GraphiteLocals(git),
});

Bun.serve({
  port: config.port,
  fetch: app.fetch,
});

console.log(`gt-api listening on http://localhost:${config.port}`);
