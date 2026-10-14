import { existsSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { buildApp } from './app.ts';
import { loadConfig } from './config.ts';
import { openDb } from './db.ts';
import { PrEventBus } from './events.ts';
import { Poller } from './poller/poller.ts';
import { PrRepository } from './poller/prs.ts';
import { assertDifftVersion } from './diff/difft.ts';
import { createGit } from './git/git.ts';
import { RepoStores } from './git/stores.ts';
import { createGitHubApi, readGhToken } from './github/client.ts';
import { createGtApiClient } from './gtapi/client.ts';
import { createHoverServers } from './hover/servers.ts';
import { TreeManager } from './hover/trees.ts';

// The diff engine's golden tests were recorded against this difft release.
const DIFFT_VERSION = '0.69.0';

const REAP_INTERVAL_MS = 15 * 60 * 1000;
const TREE_IDLE_MS = 24 * 60 * 60 * 1000;

const config = loadConfig();
await assertDifftVersion(DIFFT_VERSION);

const tmpDir = join(config.dataDir, 'tmp');
mkdirSync(tmpDir, { recursive: true });

const db = openDb(join(config.dataDir, 'panopticon.sqlite'));
const token = await readGhToken();
const github = createGitHubApi(token);
const git = createGit({ token });
const stores = new RepoStores(config.dataDir, git);
const gtApi = createGtApiClient({ baseUrl: config.gtApiUrl });
const servers = createHoverServers();
const trees = new TreeManager({
  db,
  dataDir: config.dataDir,
  repos: config.repos,
  maxTrees: config.hover.maxTrees,
  git,
  bareClone: async (owner, repo) => {
    const store = stores.for(owner, repo);
    await store.ensure();
    return store.dir;
  },
  existingBareClone: (owner, repo) => {
    const { dir } = stores.for(owner, repo);
    return existsSync(dir) ? dir : null;
  },
  stopServers: (owner, repo, number) =>
    servers.stopAll({ owner, repo, number }),
});
const recovered = await trees.recover();
console.log(
  `panopticon: removed ${recovered.removedBroken} failed or interrupted hover trees and ${recovered.removedOrphans} orphaned directories`,
);
const bus = new PrEventBus();
const prs = new PrRepository(db);
const poller = new Poller({
  github,
  gtApi,
  prs,
  bus,
  db,
  cleanup: {
    teardownPr: (owner, repo, number) => trees.teardown(owner, repo, number),
  },
  repos: Object.keys(config.repos),
  login: config.githubLogin,
  intervalMs: config.pollIntervalSeconds * 1000,
});
const app = buildApp({
  db,
  config,
  github,
  stores,
  gtApi,
  tmpDir,
  trees,
  servers,
  prs,
  bus,
});
poller.start();
setInterval(() => {
  trees.reapIdle(TREE_IDLE_MS).catch((error: unknown) => {
    console.error('panopticon: reaping idle hover trees failed', error);
  });
}, REAP_INTERVAL_MS);

Bun.serve({
  port: config.port,
  fetch: app.fetch,
});

console.log(`panopticon listening on http://localhost:${config.port}`);
