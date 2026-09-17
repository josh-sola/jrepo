import { afterEach, describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ViewedResponse } from '../../shared/api.ts';
import { openDb } from '../db.ts';
import { viewedRouter } from './viewed.ts';

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

// viewedRouter reads owner/repo/number from its parent mount's path params,
// so tests mount it the same way app.ts does instead of hitting it bare.
function mountedRouter(db: Parameters<typeof viewedRouter>[0]): Hono {
  return new Hono().route(
    '/api/pr/:owner/:repo/:number/viewed',
    viewedRouter(db),
  );
}

const MOUNT = '/api/pr/acme/widgets/42/viewed';

describe('viewedRouter', () => {
  test('marking a file viewed shows up in the GET listing', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-viewed-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    const router = mountedRouter(db);

    const putRes = await router.request(MOUNT, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: 'src/a.ts', oid: 'blob-1', viewed: true }),
    });
    expect(putRes.status).toBe(200);

    const getRes = await router.request(MOUNT);
    expect(getRes.status).toBe(200);
    const body = (await getRes.json()) as ViewedResponse;
    expect(body.viewed['src/a.ts']).toBe('blob-1');

    db.close();
  });

  test('unmarking a file removes it from the listing', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-viewed-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    const router = mountedRouter(db);

    await router.request(MOUNT, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: 'src/a.ts', oid: 'blob-1', viewed: true }),
    });
    const putRes = await router.request(MOUNT, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: 'src/a.ts', oid: 'blob-1', viewed: false }),
    });
    expect(putRes.status).toBe(200);

    const getRes = await router.request(MOUNT);
    const body = (await getRes.json()) as ViewedResponse;
    expect(body.viewed['src/a.ts']).toBeUndefined();

    db.close();
  });

  test('re-marking viewed with a new oid updates the stored oid', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-viewed-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    const router = mountedRouter(db);

    await router.request(MOUNT, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: 'src/a.ts', oid: 'blob-1', viewed: true }),
    });
    await router.request(MOUNT, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: 'src/a.ts', oid: 'blob-2', viewed: true }),
    });

    const getRes = await router.request(MOUNT);
    const body = (await getRes.json()) as ViewedResponse;
    expect(body.viewed['src/a.ts']).toBe('blob-2');

    db.close();
  });

  test('scopes viewed state to owner, repo, and number', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-viewed-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    const router = mountedRouter(db);

    await router.request(MOUNT, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: 'src/a.ts', oid: 'blob-1', viewed: true }),
    });

    const otherRes = await router.request('/api/pr/acme/widgets/99/viewed');
    const otherBody = (await otherRes.json()) as ViewedResponse;
    expect(otherBody.viewed).toEqual({});

    db.close();
  });

  test('rejects a body missing required fields', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-viewed-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    const router = mountedRouter(db);

    const res = await router.request(MOUNT, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: 'src/a.ts' }),
    });
    expect(res.status).toBe(400);

    db.close();
  });
});
