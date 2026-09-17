import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { assertDifftVersion, parseDifftOutput, runDifft } from './difft.ts';

const FIXTURES = join(import.meta.dir, '__fixtures__');

let dir: string | undefined;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

describe('assertDifftVersion', () => {
  test('resolves for the pinned difft build', async () => {
    await assertDifftVersion('0.69.0');
  });

  test('throws with both versions on mismatch', async () => {
    await expect(assertDifftVersion('9.9.9')).rejects.toThrow(/9\.9\.9/);
  });
});

describe('runDifft', () => {
  test('parses a changed TypeScript pair', async () => {
    const result = await runDifft(
      join(FIXTURES, 'typescript', 'old.ts'),
      join(FIXTURES, 'typescript', 'new.ts'),
    );
    expect(result.status).toBe('changed');
    expect(result.language).toBe('TypeScript');
    expect(result.aligned_lines).toBeDefined();
    expect(result.chunks).toBeDefined();
  });

  test('reports created and deleted files with no chunks', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-difft-test-'));
    const emptyPath = join(dir, 'empty.ts');
    const contentPath = join(dir, 'content.ts');
    writeFileSync(emptyPath, '');
    writeFileSync(contentPath, 'export const x = 1;\n');

    const created = await runDifft(emptyPath, contentPath);
    expect(created.status).toBe('created');
    expect(created.chunks).toBeUndefined();
    expect(created.aligned_lines).toBeUndefined();

    const deleted = await runDifft(contentPath, emptyPath);
    expect(deleted.status).toBe('deleted');
  });

  test('flags a parse-error pair with a Text language fallback', async () => {
    // Stored with a non-.ts extension so lint/format tooling doesn't try to
    // parse deliberately broken TypeScript; difft still needs the real
    // extension on the file it reads, so the test copies it into a temp
    // dir first.
    dir = mkdtempSync(join(tmpdir(), 'panopticon-difft-parse-error-test-'));
    const oldPath = join(dir, 'old.ts');
    const newPath = join(dir, 'new.ts');
    writeFileSync(
      oldPath,
      readFileSync(join(FIXTURES, 'parse-error', 'old.ts.broken'), 'utf-8'),
    );
    writeFileSync(
      newPath,
      readFileSync(join(FIXTURES, 'parse-error', 'new.ts.broken'), 'utf-8'),
    );

    const result = await runDifft(oldPath, newPath);
    expect(result.language.startsWith('Text')).toBe(true);
  });

  test('marks whitespace-only edits unchanged for a parsed language', async () => {
    const result = await runDifft(
      join(FIXTURES, 'whitespace', 'old.ts'),
      join(FIXTURES, 'whitespace', 'new.ts'),
    );
    expect(result.status).toBe('unchanged');
  });

  test('markdown has no grammar and reports as Text', async () => {
    const result = await runDifft(
      join(FIXTURES, 'markdown', 'old.md'),
      join(FIXTURES, 'markdown', 'new.md'),
    );
    expect(result.language.startsWith('Text')).toBe(true);
  });

  test('parses a changed TSX pair', async () => {
    // Stored with a non-.tsx extension so the server tsconfig (no JSX
    // support) doesn't try to typecheck it; difft still needs the real
    // extension, so the test copies it into a temp dir first.
    dir = mkdtempSync(join(tmpdir(), 'panopticon-difft-tsx-test-'));
    const oldPath = join(dir, 'old.tsx');
    const newPath = join(dir, 'new.tsx');
    writeFileSync(
      oldPath,
      readFileSync(join(FIXTURES, 'tsx', 'old.tsx.fixture'), 'utf-8'),
    );
    writeFileSync(
      newPath,
      readFileSync(join(FIXTURES, 'tsx', 'new.tsx.fixture'), 'utf-8'),
    );

    const result = await runDifft(oldPath, newPath);
    expect(result.status).toBe('changed');
    expect(result.language).toBe('TypeScript TSX');
  });

  test('a file past the byte limit falls back to Text', async () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-difft-bytelimit-test-'));
    const oldPath = join(dir, 'big.ts');
    const newPath2 = join(dir, 'big2.ts');
    writeFileSync(oldPath, 'x'.repeat(1_100_000));
    writeFileSync(newPath2, `${'x'.repeat(1_100_000)}y`);

    const result = await runDifft(oldPath, newPath2);
    expect(result.language.startsWith('Text')).toBe(true);
  });
});

describe('parseDifftOutput', () => {
  test('parses JSONL with multiple objects', () => {
    const raw = [
      '{"path":"a.ts","language":"TypeScript","status":"unchanged"}',
      '{"path":"b.ts","language":"TypeScript","status":"created"}',
    ].join('\n');
    const files = parseDifftOutput(raw);
    expect(files.length).toBe(2);
    expect(files[0]?.path).toBe('a.ts');
    expect(files[1]?.status).toBe('created');
  });

  test('falls back to brace-matching concatenated objects with no separator', () => {
    const raw =
      '{"path":"a.ts","language":"TypeScript","status":"unchanged"}{"path":"b.ts","language":"TypeScript","status":"created"}';
    const files = parseDifftOutput(raw);
    expect(files.length).toBe(2);
    expect(files[0]?.path).toBe('a.ts');
    expect(files[1]?.path).toBe('b.ts');
  });

  test('ignores objects that do not look like a DifftFile', () => {
    const raw =
      '{"foo":"bar"}{"path":"b.ts","language":"Text","status":"created"}';
    const files = parseDifftOutput(raw);
    expect(files.length).toBe(1);
    expect(files[0]?.path).toBe('b.ts');
  });

  test('returns an empty array for blank output', () => {
    expect(parseDifftOutput('')).toEqual([]);
    expect(parseDifftOutput('   \n  ')).toEqual([]);
  });
});
