import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { DiffPayload } from '../../shared/diff.ts';
import type { PrFile } from '../../shared/github.ts';
import type { CollapseRule } from '../config.ts';
import { ensureSchema, openDb } from '../db.ts';
import { DIFF_CACHE_SCHEMA } from './cache.ts';
import { buildPrDiff, type DiffDeps } from './index.ts';

const FIXTURES = join(import.meta.dir, '__fixtures__');
const NO_RULES: CollapseRule[] = [];

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

function newDeps(overrides: Partial<DiffDeps> = {}): {
  deps: DiffDeps;
  cleanup: () => void;
} {
  const base = mkdtempSync(join(tmpdir(), 'panopticon-diff-index-test-'));
  const db = openDb(join(base, 'test.sqlite'));
  ensureSchema(db, DIFF_CACHE_SCHEMA);
  const deps: DiffDeps = {
    readBlob: async () => null,
    generatedPaths: async () => new Set(),
    db,
    tmpDir: base,
    rules: NO_RULES,
    ...overrides,
  };
  return { deps, cleanup: () => db.close() };
}

function prFile(overrides: Partial<PrFile> = {}): PrFile {
  return {
    path: 'file.ts',
    previousPath: null,
    status: 'modified',
    additions: 0,
    deletions: 0,
    oldOid: 'old-oid',
    newOid: 'new-oid',
    ...overrides,
  };
}

function blobFromFixture(path: string): Uint8Array {
  return new Uint8Array(readFileSync(path));
}

describe('buildPrDiff', () => {
  test('builds a structural payload for a changed TypeScript file', async () => {
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) => {
        if (oid === 'old-oid')
          return blobFromFixture(join(FIXTURES, 'typescript', 'old.ts'));
        if (oid === 'new-oid')
          return blobFromFixture(join(FIXTURES, 'typescript', 'new.ts'));
        return null;
      },
    });
    dir = deps.tmpDir;

    const diff = await buildPrDiff(deps, {
      baseSha: 'base',
      headSha: 'head',
      files: [prFile()],
      ignoreWhitespace: false,
    });

    expect(diff.files.length).toBe(1);
    const file = diff.files[0];
    expect(file?.structural).toBe(true);
    expect(file?.status).toBe('changed');
    expect(file?.collapseReason).toBeNull();
    cleanup();
  });

  test('routes markdown straight to the text fallback', async () => {
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) => {
        if (oid === 'old-oid')
          return blobFromFixture(join(FIXTURES, 'markdown', 'old.md'));
        if (oid === 'new-oid')
          return blobFromFixture(join(FIXTURES, 'markdown', 'new.md'));
        return null;
      },
    });
    dir = deps.tmpDir;

    const diff = await buildPrDiff(deps, {
      baseSha: 'base',
      headSha: 'head',
      files: [prFile({ path: 'README.md' })],
      ignoreWhitespace: false,
    });

    const file = diff.files[0];
    expect(file?.structural).toBe(false);
    expect(file?.fallbackReason).toBe('markdown');
    cleanup();
  });

  test('falls back to text diff when difft reports a parse-error Text language', async () => {
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) => {
        if (oid === 'old-oid')
          return blobFromFixture(
            join(FIXTURES, 'parse-error', 'old.ts.broken'),
          );
        if (oid === 'new-oid')
          return blobFromFixture(
            join(FIXTURES, 'parse-error', 'new.ts.broken'),
          );
        return null;
      },
    });
    dir = deps.tmpDir;

    const diff = await buildPrDiff(deps, {
      baseSha: 'base',
      headSha: 'head',
      files: [prFile({ path: 'broken.ts' })],
      ignoreWhitespace: false,
    });

    const file = diff.files[0];
    expect(file?.structural).toBe(false);
    expect(file?.fallbackReason?.startsWith('Text')).toBe(true);
    cleanup();
  });

  test('builds a binary stub without invoking difft', async () => {
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) => {
        if (oid === 'old-oid')
          return blobFromFixture(join(FIXTURES, 'binary', 'old.bin'));
        if (oid === 'new-oid')
          return blobFromFixture(join(FIXTURES, 'binary', 'new.bin'));
        return null;
      },
    });
    dir = deps.tmpDir;

    const diff = await buildPrDiff(deps, {
      baseSha: 'base',
      headSha: 'head',
      files: [prFile({ path: 'image.bin' })],
      ignoreWhitespace: false,
    });

    const file = diff.files[0];
    expect(file?.binary).toBe(true);
    expect(file?.fallbackReason).toBe('binary');
    cleanup();
  });

  test('falls back to text diff when a file exceeds the byte limit', async () => {
    const big = 'x'.repeat(1_100_000);
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) => {
        if (oid === 'old-oid') return new TextEncoder().encode(big);
        if (oid === 'new-oid') return new TextEncoder().encode(`${big}y`);
        return null;
      },
    });
    dir = deps.tmpDir;

    const diff = await buildPrDiff(deps, {
      baseSha: 'base',
      headSha: 'head',
      files: [prFile({ path: 'huge.ts' })],
      ignoreWhitespace: false,
    });

    const file = diff.files[0];
    expect(file?.structural).toBe(false);
    expect(file?.fallbackReason?.startsWith('Text')).toBe(true);
    cleanup();
  }, 15000);

  test('a created file gets an all-added alignment', async () => {
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) =>
        oid === 'new-oid'
          ? new TextEncoder().encode('export const x = 1;\n')
          : null,
    });
    dir = deps.tmpDir;

    const diff = await buildPrDiff(deps, {
      baseSha: 'base',
      headSha: 'head',
      files: [prFile({ path: 'new.ts', status: 'added', oldOid: null })],
      ignoreWhitespace: false,
    });

    const file = diff.files[0];
    expect(file?.status).toBe('created');
    expect(file?.aligned).toEqual([[null, 0]]);
    cleanup();
  });

  test('a deleted file gets an all-removed alignment', async () => {
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) =>
        oid === 'old-oid'
          ? new TextEncoder().encode('export const x = 1;\n')
          : null,
    });
    dir = deps.tmpDir;

    const diff = await buildPrDiff(deps, {
      baseSha: 'base',
      headSha: 'head',
      files: [prFile({ path: 'gone.ts', status: 'removed', newOid: null })],
      ignoreWhitespace: false,
    });

    const file = diff.files[0];
    expect(file?.status).toBe('deleted');
    expect(file?.aligned).toEqual([[0, null]]);
    cleanup();
  });

  test('applies collapse rules by glob', async () => {
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) => {
        if (oid === 'old-oid')
          return blobFromFixture(join(FIXTURES, 'typescript', 'old.ts'));
        if (oid === 'new-oid')
          return blobFromFixture(join(FIXTURES, 'typescript', 'new.ts'));
        return null;
      },
      rules: [{ glob: '**/*.ts', reason: 'test' }],
    });
    dir = deps.tmpDir;

    const diff = await buildPrDiff(deps, {
      baseSha: 'base',
      headSha: 'head',
      files: [prFile({ path: 'src/thing.test.ts' })],
      ignoreWhitespace: false,
    });

    expect(diff.files[0]?.collapseReason).toBe('test');
    cleanup();
  });

  test('flags generated files ahead of any glob rule', async () => {
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) => {
        if (oid === 'old-oid')
          return blobFromFixture(join(FIXTURES, 'typescript', 'old.ts'));
        if (oid === 'new-oid')
          return blobFromFixture(join(FIXTURES, 'typescript', 'new.ts'));
        return null;
      },
      generatedPaths: async () => new Set(['src/thing.ts']),
      rules: [{ glob: '**/*.ts', reason: 'test' }],
    });
    dir = deps.tmpDir;

    const diff = await buildPrDiff(deps, {
      baseSha: 'base',
      headSha: 'head',
      files: [prFile({ path: 'src/thing.ts' })],
      ignoreWhitespace: false,
    });

    expect(diff.files[0]?.collapseReason).toBe('generated');
    cleanup();
  });

  test('a cache hit skips difft entirely', async () => {
    let readBlobCalls = 0;
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) => {
        readBlobCalls += 1;
        if (readBlobCalls > 2) {
          throw new Error(
            'readBlob should not be called after the cache is warm',
          );
        }
        if (oid === 'old-oid')
          return blobFromFixture(join(FIXTURES, 'typescript', 'old.ts'));
        if (oid === 'new-oid')
          return blobFromFixture(join(FIXTURES, 'typescript', 'new.ts'));
        return null;
      },
    });
    dir = deps.tmpDir;

    const input = {
      baseSha: 'base',
      headSha: 'head',
      files: [prFile()],
      ignoreWhitespace: false,
    };

    const first = await buildPrDiff(deps, input);
    const second = await buildPrDiff(deps, input);

    expect(readBlobCalls).toBe(2);
    expect(second.files[0]).toEqual(first.files[0]);
    cleanup();
  });

  test('preserves input file order across concurrent difft runs', async () => {
    const paths = Array.from({ length: 8 }, (_, i) => `file-${i}.ts`);
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) => {
        if (oid.startsWith('old-'))
          return new TextEncoder().encode(`old ${oid}\n`);
        if (oid.startsWith('new-'))
          return new TextEncoder().encode(`new ${oid}\n`);
        return null;
      },
    });
    dir = deps.tmpDir;

    const files = paths.map((path, i) =>
      prFile({ path, oldOid: `old-${i}`, newOid: `new-${i}` }),
    );
    const diff = await buildPrDiff(deps, {
      baseSha: 'base',
      headSha: 'head',
      files,
      ignoreWhitespace: false,
    });

    expect(diff.files.map((f) => f.path)).toEqual(paths);
    cleanup();
  });

  test('golden: the TypeScript pair matches the saved DiffPayload exactly', async () => {
    const { deps, cleanup } = newDeps({
      readBlob: async (oid) => {
        if (oid === 'old-oid')
          return blobFromFixture(join(FIXTURES, 'typescript', 'old.ts'));
        if (oid === 'new-oid')
          return blobFromFixture(join(FIXTURES, 'typescript', 'new.ts'));
        return null;
      },
    });
    dir = deps.tmpDir;

    const golden = JSON.parse(
      readFileSync(join(FIXTURES, 'typescript', 'golden.json'), 'utf-8'),
    ) as DiffPayload;

    const diff = await buildPrDiff(deps, {
      baseSha: 'base',
      headSha: 'head',
      files: [
        prFile({
          path: golden.path,
          oldOid: 'old-oid',
          newOid: 'new-oid',
        }),
      ],
      ignoreWhitespace: false,
    });

    expect(diff.files[0]).toEqual(golden);
    cleanup();
  });
});
