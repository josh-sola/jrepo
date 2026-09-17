import { join } from 'node:path';
import { buildApp } from './app.ts';
import { loadConfig } from './config.ts';
import { openDb } from './db.ts';

const config = loadConfig();
const db = openDb(join(config.dataDir, 'panopticon.sqlite'));
const app = buildApp(db);

Bun.serve({
  port: config.port,
  fetch: app.fetch,
});

console.log(`panopticon listening on http://localhost:${config.port}`);
