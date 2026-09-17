import { Database } from 'bun:sqlite';

export function openDb(path: string): Database {
  const db = new Database(path, { create: true });
  db.exec('PRAGMA journal_mode = WAL;');
  db.exec('PRAGMA foreign_keys = ON;');
  return db;
}

// Runs a schema's `CREATE TABLE IF NOT EXISTS` statements as one script, so
// each route module can own its own tables without a central migration list.
export function ensureSchema(db: Database, sql: string): void {
  db.exec(sql);
}
