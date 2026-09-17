import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { PrFile } from '../../shared/github.ts';
import { buildBinaryPayload, isBinary } from './binary.ts';

const FIXTURES = join(import.meta.dir, '__fixtures__');

describe('isBinary', () => {
  test('detects a NUL byte in the fixture binary file', () => {
    const bytes = readFileSync(join(FIXTURES, 'binary', 'old.bin'));
    expect(isBinary(bytes)).toBe(true);
  });

  test('treats plain text as non-binary', () => {
    const bytes = readFileSync(join(FIXTURES, 'typescript', 'old.ts'));
    expect(isBinary(bytes)).toBe(false);
  });

  test('only scans the first 8000 bytes', () => {
    const bytes = new Uint8Array(9000).fill(65); // 'A'
    bytes[8500] = 0;
    expect(isBinary(bytes)).toBe(false);
  });
});

describe('buildBinaryPayload', () => {
  test('produces a stub card with empty lines and no spans', () => {
    const file: PrFile = {
      path: 'image.png',
      previousPath: null,
      status: 'modified',
      additions: 0,
      deletions: 0,
      oldOid: 'a',
      newOid: 'b',
    };
    const payload = buildBinaryPayload(file);
    expect(payload.binary).toBe(true);
    expect(payload.structural).toBe(false);
    expect(payload.fallbackReason).toBe('binary');
    expect(payload.lhsLines).toEqual([]);
    expect(payload.rhsLines).toEqual([]);
    expect(payload.aligned).toEqual([]);
    expect(payload.stat).toEqual({ added: 0, removed: 0, modified: 0 });
  });
});
