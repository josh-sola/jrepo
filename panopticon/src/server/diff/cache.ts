import type { Database } from 'bun:sqlite';
import type { DiffPayload } from '../../shared/diff.ts';

export const DIFF_CACHE_SCHEMA = `
CREATE TABLE IF NOT EXISTS diff_cache (
  old_oid TEXT NOT NULL,
  new_oid TEXT NOT NULL,
  path TEXT NOT NULL,
  whitespace TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (old_oid, new_oid, path, whitespace)
);
`;

// A force-push only changes the oids of the files it touches, so caching by
// (old_oid, new_oid, path) skips difft entirely for everything else.
const NULL_OID = '-';

export type WhitespaceMode = 'ignore' | 'keep';

export interface DiffCacheKey {
  oldOid: string | null;
  newOid: string | null;
  path: string;
  whitespace: WhitespaceMode;
}

function keyParams(key: DiffCacheKey): {
  $oldOid: string;
  $newOid: string;
  $path: string;
  $whitespace: string;
} {
  return {
    $oldOid: key.oldOid ?? NULL_OID,
    $newOid: key.newOid ?? NULL_OID,
    $path: key.path,
    $whitespace: key.whitespace,
  };
}

export function getCachedDiff(
  db: Database,
  key: DiffCacheKey,
): DiffPayload | null {
  const row = db
    .query<
      { payload: string },
      { $oldOid: string; $newOid: string; $path: string; $whitespace: string }
    >(
      'SELECT payload FROM diff_cache WHERE old_oid = $oldOid AND new_oid = $newOid AND path = $path AND whitespace = $whitespace',
    )
    .get(keyParams(key));
  if (row === null) return null;
  return JSON.parse(row.payload) as DiffPayload;
}

export function setCachedDiff(
  db: Database,
  key: DiffCacheKey,
  payload: DiffPayload,
): void {
  db.query<
    unknown,
    {
      $oldOid: string;
      $newOid: string;
      $path: string;
      $whitespace: string;
      $payload: string;
      $createdAt: string;
    }
  >(
    `INSERT INTO diff_cache (old_oid, new_oid, path, whitespace, payload, created_at)
     VALUES ($oldOid, $newOid, $path, $whitespace, $payload, $createdAt)
     ON CONFLICT(old_oid, new_oid, path, whitespace)
     DO UPDATE SET payload = $payload, created_at = $createdAt`,
  ).run({
    ...keyParams(key),
    $payload: JSON.stringify(payload),
    $createdAt: new Date().toISOString(),
  });
}
