import type { Database } from 'bun:sqlite';
import { Hono } from 'hono';
import type { PrefsResponse, UpsertPrefsRequest } from '../../shared/api.ts';
import { ensureSchema } from '../db.ts';
import { PREFS_SCHEMA } from './schema.ts';

function isUpsertPrefsRequest(value: unknown): value is UpsertPrefsRequest {
  if (typeof value !== 'object' || value === null || Array.isArray(value))
    return false;
  return Object.values(value).every((v) => typeof v === 'string');
}

export function prefsRouter(db: Database): Hono {
  ensureSchema(db, PREFS_SCHEMA);

  const selectAll = db.query<{ key: string; value: string }, []>(
    'SELECT key, value FROM prefs',
  );
  const upsert = db.query<unknown, { $key: string; $value: string }>(
    'INSERT INTO prefs (key, value) VALUES ($key, $value) ON CONFLICT(key) DO UPDATE SET value = $value',
  );

  return new Hono()
    .get('/', (c) => {
      const prefs: Record<string, string> = {};
      for (const row of selectAll.all()) {
        prefs[row.key] = row.value;
      }
      const body: PrefsResponse = { prefs };
      return c.json(body);
    })
    .put('/', async (c) => {
      const body: unknown = await c.req.json();
      if (!isUpsertPrefsRequest(body)) {
        return c.json(
          { error: 'body must be an object of string values' },
          400,
        );
      }
      for (const [key, value] of Object.entries(body)) {
        upsert.run({ $key: key, $value: value });
      }
      return c.json({ ok: true });
    });
}
