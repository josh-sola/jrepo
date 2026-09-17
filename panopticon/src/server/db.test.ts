import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ensureSchema, openDb } from './db.ts';

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

describe('openDb', () => {
  test('enables WAL mode and foreign keys', () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-db-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    const journalMode = db
      .query<{ journal_mode: string }, []>('PRAGMA journal_mode')
      .get();
    const foreignKeys = db
      .query<{ foreign_keys: number }, []>('PRAGMA foreign_keys')
      .get();
    expect(journalMode?.journal_mode).toBe('wal');
    expect(foreignKeys?.foreign_keys).toBe(1);
    db.close();
  });
});

describe('ensureSchema', () => {
  test('creates tables idempotently', () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-db-test-'));
    const db = openDb(join(dir, 'test.sqlite'));
    const schema =
      'CREATE TABLE IF NOT EXISTS widgets (id INTEGER PRIMARY KEY);';
    ensureSchema(db, schema);
    ensureSchema(db, schema);
    db.query('INSERT INTO widgets DEFAULT VALUES').run();
    const rows = db.query<{ id: number }, []>('SELECT id FROM widgets').all();
    expect(rows.length).toBe(1);
    db.close();
  });
});
