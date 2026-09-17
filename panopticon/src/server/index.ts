import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { buildApp } from './app.ts';
import { loadConfig } from './config.ts';
import { openDb } from './db.ts';
import { PrEventBus } from './events.ts';
import { Poller } from './poller/poller.ts';
import { PrRepository } from './poller/prs.ts';
import { assertDifftVersion } from './diff/difft.ts';
import { RepoStores } from './git/stores.ts';
import { createGitHubApi, readGhToken } from './github/client.ts';
import { createHoverServers } from './hover/servers.ts';
import { TreeManager } from './hover/trees.ts';
import { BunWtRunner } from './hover/wt.ts';

// The diff engine's golden tests were recorded against this difft release.
const DIFFT_VERSION = '0.69.0';

const config = loadConfig();
await assertDifftVersion(DIFFT_VERSION);

const tmpDir = join(config.dataDir, 'tmp');
mkdirSync(tmpDir, { recursive: true });

const db = openDb(join(config.dataDir, 'panopticon.sqlite'));
const github = createGitHubApi(await readGhToken());
const stores = new RepoStores(config.dataDir, config.repos);
const servers = createHoverServers();
const trees = new TreeManager({
  db,
  runner: new BunWtRunner(),
  repos: config.repos,
  stopServers: (owner, repo, number) =>
    servers.stopAll({ owner, repo, number }),
});
const bus = new PrEventBus();
const prs = new PrRepository(db);
const trunkFor = (owner: string, repo: string): string =>
  config.repos[`${owner}/${repo}`]?.trunk ?? 'main';
const poller = new Poller({
  github,
  stores,
  trunkFor,
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
  tmpDir,
  trees,
  servers,
  prs,
  bus,
});
poller.start();

Bun.serve({
  port: config.port,
  fetch: app.fetch,
});

console.log(`panopticon listening on http://localhost:${config.port}`);
