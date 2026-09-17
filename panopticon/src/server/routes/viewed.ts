import type { Database } from 'bun:sqlite';
import { Hono } from 'hono';
import type { SetViewedRequest, ViewedResponse } from '../../shared/api.ts';
import { ensureSchema } from '../db.ts';

export const VIEWED_SCHEMA = `
CREATE TABLE IF NOT EXISTS viewed (
  owner TEXT NOT NULL,
  repo TEXT NOT NULL,
  number INTEGER NOT NULL,
  path TEXT NOT NULL,
  oid TEXT NOT NULL,
  viewed_at TEXT NOT NULL,
  PRIMARY KEY (owner, repo, number, path)
);
`;

function isSetViewedRequest(value: unknown): value is SetViewedRequest {
  if (typeof value !== 'object' || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.path === 'string' &&
    typeof v.oid === 'string' &&
    typeof v.viewed === 'boolean'
  );
}

// Mounted under `/api/pr/:owner/:repo/:number/viewed`, so the params exist at
// runtime but this sub-router's types do not declare them.
function requireParam(value: string | undefined, name: string): string {
  if (value === undefined) {
    throw new Error(`viewedRouter: missing route param "${name}"`);
  }
  return value;
}

export function viewedRouter(db: Database): Hono {
  ensureSchema(db, VIEWED_SCHEMA);

  const selectAll = db.query<
    { path: string; oid: string },
    { $owner: string; $repo: string; $number: number }
  >(
    'SELECT path, oid FROM viewed WHERE owner = $owner AND repo = $repo AND number = $number',
  );
  const upsert = db.query<
    unknown,
    {
      $owner: string;
      $repo: string;
      $number: number;
      $path: string;
      $oid: string;
      $viewedAt: string;
    }
  >(
    `INSERT INTO viewed (owner, repo, number, path, oid, viewed_at)
     VALUES ($owner, $repo, $number, $path, $oid, $viewedAt)
     ON CONFLICT(owner, repo, number, path) DO UPDATE SET oid = $oid, viewed_at = $viewedAt`,
  );
  const del = db.query<
    unknown,
    { $owner: string; $repo: string; $number: number; $path: string }
  >(
    'DELETE FROM viewed WHERE owner = $owner AND repo = $repo AND number = $number AND path = $path',
  );

  return new Hono()
    .get('/', (c) => {
      const owner = requireParam(c.req.param('owner'), 'owner');
      const repo = requireParam(c.req.param('repo'), 'repo');
      const number = Number(requireParam(c.req.param('number'), 'number'));
      const viewed: Record<string, string> = {};
      for (const row of selectAll.all({
        $owner: owner,
        $repo: repo,
        $number: number,
      })) {
        viewed[row.path] = row.oid;
      }
      const body: ViewedResponse = { viewed };
      return c.json(body);
    })
    .put('/', async (c) => {
      const owner = requireParam(c.req.param('owner'), 'owner');
      const repo = requireParam(c.req.param('repo'), 'repo');
      const number = Number(requireParam(c.req.param('number'), 'number'));
      const body: unknown = await c.req.json();
      if (!isSetViewedRequest(body)) {
        return c.json({ error: 'body must be { path, oid, viewed }' }, 400);
      }
      if (body.viewed) {
        upsert.run({
          $owner: owner,
          $repo: repo,
          $number: number,
          $path: body.path,
          $oid: body.oid,
          $viewedAt: new Date().toISOString(),
        });
      } else {
        del.run({
          $owner: owner,
          $repo: repo,
          $number: number,
          $path: body.path,
        });
      }
      return c.json({ ok: true });
    });
}
