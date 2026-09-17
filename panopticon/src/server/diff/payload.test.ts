import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { PrFile } from '../../shared/github.ts';
import { runDifft } from './difft.ts';
import { buildStructuralPayload, toDiffStatus } from './payload.ts';

const FIXTURES = join(import.meta.dir, '__fixtures__');

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

describe('toDiffStatus', () => {
  test('maps every PrFileStatus to a DiffPayload status', () => {
    expect(toDiffStatus('added')).toBe('created');
    expect(toDiffStatus('removed')).toBe('deleted');
    expect(toDiffStatus('modified')).toBe('changed');
    expect(toDiffStatus('renamed')).toBe('changed');
    expect(toDiffStatus('copied')).toBe('changed');
    expect(toDiffStatus('changed')).toBe('changed');
    expect(toDiffStatus('unchanged')).toBe('changed');
  });
});

describe('buildStructuralPayload', () => {
  test('carries difft spans and alignment through for a changed file', async () => {
    const oldText = readFileSync(
      join(FIXTURES, 'typescript', 'old.ts'),
      'utf-8',
    );
    const newText = readFileSync(
      join(FIXTURES, 'typescript', 'new.ts'),
      'utf-8',
    );
    const result = await runDifft(
      join(FIXTURES, 'typescript', 'old.ts'),
      join(FIXTURES, 'typescript', 'new.ts'),
    );

    const payload = buildStructuralPayload(prFile(), oldText, newText, result);

    expect(payload.status).toBe('changed');
    expect(payload.structural).toBe(true);
    expect(payload.fallbackReason).toBeNull();
    expect(payload.language).toBe('TypeScript');
    expect(payload.aligned).toEqual(result.aligned_lines ?? []);
    expect(Object.keys(payload.lhsSpans).length).toBeGreaterThan(0);
    expect(Object.keys(payload.rhsSpans).length).toBeGreaterThan(0);
    // The renamed line sits at 0-based index 4 in both files.
    expect(payload.lhsSpans['4']?.[0]?.content).toBe('oldName');
    expect(payload.rhsSpans['4']?.[0]?.content).toBe('newName');
    expect(payload.stat.added).toBeGreaterThan(0);
    expect(payload.stat.modified).toBeGreaterThan(0);
  });

  test('byte offsets for a multi-byte character land on the right boundary', async () => {
    const oldText = readFileSync(
      join(FIXTURES, 'multibyte', 'old.ts'),
      'utf-8',
    );
    const newText = readFileSync(
      join(FIXTURES, 'multibyte', 'new.ts'),
      'utf-8',
    );
    const result = await runDifft(
      join(FIXTURES, 'multibyte', 'old.ts'),
      join(FIXTURES, 'multibyte', 'new.ts'),
    );

    const payload = buildStructuralPayload(prFile(), oldText, newText, result);

    // "café" is 4 UTF-16 code units but 5 UTF-8 bytes (é is 2 bytes), so the
    // word after it must start 5 bytes past the opening quote, not 4.
    const lhsChangedWord = payload.lhsSpans['0']?.find(
      (span) => span.content === 'old',
    );
    const rhsChangedWord = payload.rhsSpans['0']?.find(
      (span) => span.content === 'new',
    );
    expect(lhsChangedWord).toBeDefined();
    expect(rhsChangedWord).toBeDefined();
    const encoder = new TextEncoder();
    const oldLine = oldText.split('\n')[0] ?? '';
    const expectedStart = encoder.encode(
      oldLine.slice(0, oldLine.indexOf('old')),
    ).length;
    expect(lhsChangedWord?.start).toBe(expectedStart);
  });

  test('created status synthesizes a full add alignment with no spans', () => {
    const newText = 'export const x = 1;\nexport const y = 2;\n';
    const payload = buildStructuralPayload(
      prFile({ status: 'added', oldOid: null }),
      '',
      newText,
      { path: 'file.ts', language: 'TypeScript', status: 'created' },
    );
    expect(payload.status).toBe('created');
    expect(payload.aligned).toEqual([
      [null, 0],
      [null, 1],
    ]);
    expect(payload.lhsLines).toEqual([]);
    expect(payload.rhsLines).toEqual([
      'export const x = 1;',
      'export const y = 2;',
    ]);
    expect(payload.stat).toEqual({ added: 2, removed: 0, modified: 0 });
    expect(payload.lhsSpans).toEqual({});
    expect(payload.rhsSpans).toEqual({});
  });

  test('deleted status synthesizes a full remove alignment', () => {
    const oldText = 'export const x = 1;\n';
    const payload = buildStructuralPayload(
      prFile({ status: 'removed', newOid: null }),
      oldText,
      '',
      { path: 'file.ts', language: 'TypeScript', status: 'deleted' },
    );
    expect(payload.status).toBe('deleted');
    expect(payload.aligned).toEqual([[0, null]]);
    expect(payload.stat).toEqual({ added: 0, removed: 1, modified: 0 });
  });

  test('unchanged status maps to a full 1:1 alignment with no spans', () => {
    const text = 'export const x = 1;\nexport const y = 2;\n';
    const payload = buildStructuralPayload(prFile(), text, text, {
      path: 'file.ts',
      language: 'TypeScript',
      status: 'unchanged',
    });
    expect(payload.status).toBe('changed');
    expect(payload.aligned).toEqual([
      [0, 0],
      [1, 1],
    ]);
    expect(payload.stat).toEqual({ added: 0, removed: 0, modified: 0 });
    expect(payload.lhsSpans).toEqual({});
    expect(payload.rhsSpans).toEqual({});
  });

  test("oldPath comes from the file's previousPath", () => {
    const payload = buildStructuralPayload(
      prFile({ path: 'new-name.ts', previousPath: 'old-name.ts' }),
      'a\n',
      'a\n',
      { path: 'file.ts', language: 'TypeScript', status: 'unchanged' },
    );
    expect(payload.path).toBe('new-name.ts');
    expect(payload.oldPath).toBe('old-name.ts');
  });
});
