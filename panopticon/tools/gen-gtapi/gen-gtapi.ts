// Generates src/server/gtapi/schema.d.ts from gt-api's committed OpenAPI
// spec. With --check, fails when the generated file is stale. Without the
// spec (the Docker build sees only this directory), --check passes quietly.
// This tool has its own package because openapi-typescript needs TypeScript 5,
// which panopticon's TypeScript 7 no longer supports.

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import openapiTS, { astToString } from 'openapi-typescript';

const SPEC = new URL('../../../gt-api/openapi.json', import.meta.url);
const OUTPUT = new URL('../../src/server/gtapi/schema.d.ts', import.meta.url);

const check = process.argv.includes('--check');

if (!existsSync(SPEC)) {
  if (check) {
    console.log('gt-api/openapi.json not found; skipping gtapi schema check.');
    process.exit(0);
  }
  console.error('gt-api/openapi.json not found.');
  process.exit(1);
}

const generated = astToString(await openapiTS(SPEC));

if (check) {
  const current = existsSync(OUTPUT) ? readFileSync(OUTPUT, 'utf8') : '';
  if (current !== generated) {
    console.error('schema.d.ts is stale. Run `bun run gen:gtapi`.');
    process.exit(1);
  }
} else {
  writeFileSync(OUTPUT, generated);
}
