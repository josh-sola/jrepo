// Writes openapi.json from the app without starting a server. With --check,
// fails when the committed file is stale.

import { readFileSync, writeFileSync } from 'node:fs';
import { buildApp } from '../src/app.ts';
import { createGit } from '../src/git/git.ts';
import { RepoStores } from '../src/git/stores.ts';
import { createGitHubApi } from '../src/github/client.ts';
import { OPENAPI_INFO } from '../src/openapi.ts';
import { GraphiteLocals } from '../src/stack/graphiteLocals.ts';

const SPEC_PATH = new URL('../openapi.json', import.meta.url);

// The routes are never called, so these deps stay inert.
const git = createGit({ token: '', safeDirectories: [] });
const app = buildApp({
  config: { port: 0, dataDir: '', repos: [] },
  github: createGitHubApi(''),
  stores: new RepoStores('', git),
  graphiteLocals: new GraphiteLocals(git),
});

const spec = `${JSON.stringify(
  app.getOpenAPI31Document({ openapi: '3.1.0', info: OPENAPI_INFO }),
  null,
  2,
)}\n`;

if (process.argv.includes('--check')) {
  let current = '';
  try {
    current = readFileSync(SPEC_PATH, 'utf8');
  } catch {
    // A missing file counts as stale.
  }
  if (current !== spec) {
    console.error('openapi.json is stale. Run `bun run openapi`.');
    process.exit(1);
  }
} else {
  writeFileSync(SPEC_PATH, spec);
}
