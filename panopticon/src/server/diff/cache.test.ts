import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { DiffPayload } from '../../shared/diff.ts';
import { ensureSchema, openDb } from '../db.ts';
import { DIFF_CACHE_SCHEMA, getCachedDiff, setCachedDiff } from './cache.ts';

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

function samplePayload(): DiffPayload {
  return {
    path: 'file.ts',
    oldPath: null,
    status: 'changed',
    language: 'TypeScript',
    aligned: [[0, 0]],
    lhsLines: ['a'],
    rhsLines: ['a'],
    lhsSpans: {},
    rhsSpans: {},
    stat: { added: 0, removed: 0, modified: 0 },
    structural: true,
    fallbackReason: null,
    binary: false,
    collapseReason: null,
  };
}

describe('diff cache', () => {
  test('round-trips a payload for a given key', () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-diff-cache-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    ensureSchema(db, DIFF_CACHE_SCHEMA);

    const key = {
      oldOid: 'old-oid',
      newOid: 'new-oid',
      path: 'file.ts',
      whitespace: 'keep' as const,
    };
    expect(getCachedDiff(db, key)).toBeNull();

    setCachedDiff(db, key, samplePayload());
    const cached = getCachedDiff(db, key);
    expect(cached).toEqual(samplePayload());

    db.close();
  });

  test('stores null oids as the literal dash and keeps whitespace modes separate', () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-diff-cache-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    ensureSchema(db, DIFF_CACHE_SCHEMA);

    const keepKey = {
      oldOid: null,
      newOid: 'new-oid',
      path: 'file.ts',
      whitespace: 'keep' as const,
    };
    const ignoreKey = { ...keepKey, whitespace: 'ignore' as const };

    setCachedDiff(db, keepKey, {
      ...samplePayload(),
      language: 'keep-variant',
    });
    expect(getCachedDiff(db, ignoreKey)).toBeNull();
    expect(getCachedDiff(db, keepKey)?.language).toBe('keep-variant');

    const row = db
      .query<{ old_oid: string }, []>(
        "SELECT old_oid FROM diff_cache WHERE path = 'file.ts'",
      )
      .get();
    expect(row?.old_oid).toBe('-');

    db.close();
  });

  test('a second set overwrites the cached payload for the same key', () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-diff-cache-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    ensureSchema(db, DIFF_CACHE_SCHEMA);

    const key = {
      oldOid: 'old-oid',
      newOid: 'new-oid',
      path: 'file.ts',
      whitespace: 'keep' as const,
    };
    setCachedDiff(db, key, samplePayload());
    setCachedDiff(db, key, { ...samplePayload(), language: 'updated' });
    expect(getCachedDiff(db, key)?.language).toBe('updated');

    db.close();
  });
});
