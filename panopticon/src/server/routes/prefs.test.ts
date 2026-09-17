import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { PrefsResponse } from '../../shared/api.ts';
import { openDb } from '../db.ts';
import { prefsRouter } from './prefs.ts';

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

describe('prefsRouter', () => {
  test('round-trips a value through PUT then GET', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-prefs-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    const router = prefsRouter(db);

    const putRes = await router.request('/', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ theme: 'dark' }),
    });
    expect(putRes.status).toBe(200);

    const getRes = await router.request('/');
    expect(getRes.status).toBe(200);
    const body = (await getRes.json()) as PrefsResponse;
    expect(body.prefs.theme).toBe('dark');

    db.close();
  });

  test('rejects a non-object body', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-prefs-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    const router = prefsRouter(db);

    const res = await router.request('/', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(['not', 'an', 'object']),
    });
    expect(res.status).toBe(400);

    db.close();
  });
});
