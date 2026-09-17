import { describe, expect, test } from 'bun:test';
import type { PrFile } from '../../shared/github.ts';
import { buildTextPayload } from './textdiff.ts';

function prFile(overrides: Partial<PrFile> = {}): PrFile {
  return {
    path: 'file.md',
    previousPath: null,
    status: 'modified',
    additions: 0,
    deletions: 0,
    oldOid: 'old-oid',
    newOid: 'new-oid',
    ...overrides,
  };
}

describe('buildTextPayload', () => {
  test('pairs a modified line and produces word-level spans', () => {
    const payload = buildTextPayload(
      prFile(),
      'line one\nline two\nline three\n',
      'line one\nline TWO\nline three\n',
      { ignoreWhitespace: false, reason: 'markdown' },
    );

    expect(payload.structural).toBe(false);
    expect(payload.fallbackReason).toBe('markdown');
    expect(payload.language).toBe('Text');
    expect(payload.aligned).toEqual([
      [0, 0],
      [1, 1],
      [2, 2],
    ]);
    expect(payload.lhsSpans['1']?.[0]?.content).toBe('two');
    expect(payload.rhsSpans['1']?.[0]?.content).toBe('TWO');
    expect(payload.stat).toEqual({ added: 0, removed: 0, modified: 1 });
  });

  test('byte offsets account for a multi-byte character before the change', () => {
    const oldText = 'export const label = "café old";\n';
    const newText = 'export const label = "café new";\n';
    const payload = buildTextPayload(prFile(), oldText, newText, {
      ignoreWhitespace: false,
      reason: 'Text (unparsed extension)',
    });

    const lhsSpan = payload.lhsSpans['0']?.find(
      (span) => span.content === 'old',
    );
    const rhsSpan = payload.rhsSpans['0']?.find(
      (span) => span.content === 'new',
    );
    expect(lhsSpan).toBeDefined();
    expect(rhsSpan).toBeDefined();
    const oldLine = oldText.split('\n')[0] ?? '';
    const expectedStart = new TextEncoder().encode(
      oldLine.slice(0, oldLine.indexOf('old')),
    ).length;
    expect(lhsSpan?.start).toBe(expectedStart);
  });

  test('pure additions get no spans and count toward added', () => {
    const payload = buildTextPayload(
      prFile({ status: 'added', oldOid: null }),
      '',
      'a\nb\n',
      {
        ignoreWhitespace: false,
        reason: 'markdown',
      },
    );
    expect(payload.status).toBe('created');
    expect(payload.aligned).toEqual([
      [null, 0],
      [null, 1],
    ]);
    expect(payload.lhsSpans).toEqual({});
    expect(payload.stat).toEqual({ added: 2, removed: 0, modified: 0 });
  });

  test('pure removals get no spans and count toward removed', () => {
    const payload = buildTextPayload(
      prFile({ status: 'removed', newOid: null }),
      'a\nb\n',
      '',
      { ignoreWhitespace: false, reason: 'markdown' },
    );
    expect(payload.status).toBe('deleted');
    expect(payload.aligned).toEqual([
      [0, null],
      [1, null],
    ]);
    expect(payload.stat).toEqual({ added: 0, removed: 2, modified: 0 });
  });

  test('ignoreWhitespace treats a leading/trailing whitespace change as unchanged context', () => {
    const oldText = '  value = 1\n';
    const newText = 'value = 1\n';
    const withIgnore = buildTextPayload(prFile(), oldText, newText, {
      ignoreWhitespace: true,
      reason: 'markdown',
    });
    expect(withIgnore.stat).toEqual({ added: 0, removed: 0, modified: 0 });
    expect(withIgnore.aligned).toEqual([[0, 0]]);

    const withoutIgnore = buildTextPayload(prFile(), oldText, newText, {
      ignoreWhitespace: false,
      reason: 'markdown',
    });
    expect(withoutIgnore.stat.modified).toBe(1);
  });
});
