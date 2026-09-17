import type { Database } from 'bun:sqlite';
import { Hono } from 'hono';
import type { PrDiffResponse } from '../../shared/api.ts';
import type { PrFile, PrSummary } from '../../shared/github.ts';
import type { CollapseRule } from '../config.ts';
import { ensureSchema } from '../db.ts';
import { DIFF_CACHE_SCHEMA } from '../diff/cache.ts';
import { buildPrDiff } from '../diff/index.ts';

export interface DiffRouterDeps {
  loadPr(
    owner: string,
    repo: string,
    number: number,
  ): Promise<{ pr: PrSummary; files: PrFile[] }>;
  storeFor(
    owner: string,
    repo: string,
  ): Promise<{
    readBlob(oid: string): Promise<Uint8Array | null>;
    generatedPaths(headSha: string, paths: string[]): Promise<Set<string>>;
  }>;
  db: Database;
  tmpDir: string;
  rules: CollapseRule[];
}

// Mounted under `/api/pr/:owner/:repo/:number/diff`, so the params exist at
// runtime but this sub-router's types do not declare them.
function requireParam(value: string | undefined, name: string): string {
  if (value === undefined) {
    throw new Error(`diffRouter: missing route param "${name}"`);
  }
  return value;
}

export function diffRouter(deps: DiffRouterDeps): Hono {
  ensureSchema(deps.db, DIFF_CACHE_SCHEMA);

  return new Hono().get('/', async (c) => {
    const owner = requireParam(c.req.param('owner'), 'owner');
    const repo = requireParam(c.req.param('repo'), 'repo');
    const number = Number(requireParam(c.req.param('number'), 'number'));
    const ignoreWhitespace = c.req.query('ws') === 'ignore';

    const { pr, files } = await deps.loadPr(owner, repo, number);
    const store = await deps.storeFor(owner, repo);

    const diff = await buildPrDiff(
      {
        readBlob: (oid) => store.readBlob(oid),
        generatedPaths: (headSha, paths) =>
          store.generatedPaths(headSha, paths),
        db: deps.db,
        tmpDir: deps.tmpDir,
        rules: deps.rules,
      },
      {
        baseSha: pr.base.sha,
        headSha: pr.head.sha,
        files,
        ignoreWhitespace,
      },
    );

    const body: PrDiffResponse = diff;
    return c.json(body);
  });
}
