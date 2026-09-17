import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { buildApp } from './app.ts';
import { loadConfig } from './config.ts';
import { openDb } from './db.ts';
import { assertDifftVersion } from './diff/difft.ts';
import { RepoStores } from './git/stores.ts';
import { createGitHubApi, readGhToken } from './github/client.ts';

// The diff engine's golden tests were recorded against this difft release.
const DIFFT_VERSION = '0.69.0';

const config = loadConfig();
await assertDifftVersion(DIFFT_VERSION);

const tmpDir = join(config.dataDir, 'tmp');
mkdirSync(tmpDir, { recursive: true });

const db = openDb(join(config.dataDir, 'panopticon.sqlite'));
const github = createGitHubApi(await readGhToken());
const stores = new RepoStores(config.dataDir, config.repos);
const app = buildApp({ db, config, github, stores, tmpDir });

Bun.serve({
  port: config.port,
  fetch: app.fetch,
});

console.log(`panopticon listening on http://localhost:${config.port}`);
