import '../test-setup.ts';
import { afterEach, describe, expect, it } from 'bun:test';
import {
  fingerprintsForPayload,
  fingerprintsForUnified,
  loadViewed,
  saveViewed,
} from './viewedStorage.ts';
import type { DiffPayload, PrDiff } from '../../shared/diff.ts';
import type { UnifiedFile } from './parseUnified.ts';

afterEach(() => localStorage.clear());

function diffFile(overrides: Partial<DiffPayload> = {}): DiffPayload {
  return {
    path: 'src/a.ts',
    oldPath: null,
    status: 'changed',
    language: 'TypeScript',
    aligned: [[0, 0]],
    lhsLines: ['one'],
    rhsLines: ['ONE'],
    lhsSpans: {},
    rhsSpans: {},
    stat: { added: 1, removed: 1, modified: 0 },
    structural: true,
    fallbackReason: null,
    binary: false,
    collapseReason: null,
    ...overrides,
  };
}

function payloadOf(files: DiffPayload[]): PrDiff {
  return { headSha: 'test-head', baseSha: 'test-base', files };
}

describe('fingerprintsForPayload', () => {
  it('is stable for identical content', () => {
    const files = [diffFile()];
    expect(fingerprintsForPayload(payloadOf(files))).toEqual(
      fingerprintsForPayload(payloadOf(files)),
    );
  });

  it("changes when a file's lines change even if stat is unchanged", () => {
    const a = diffFile({ lhsLines: ['one'], rhsLines: ['ONE'] });
    const b = diffFile({ lhsLines: ['one'], rhsLines: ['one!'] }); // same stat, different content
    const fpA = fingerprintsForPayload(payloadOf([a]))['src/a.ts'];
    const fpB = fingerprintsForPayload(payloadOf([b]))['src/a.ts'];
    expect(fpA).not.toBe(fpB);
  });

  it("keeps other files' fingerprints unchanged when one file's content changes", () => {
    const a = diffFile({ path: 'src/a.ts' });
    const b = diffFile({ path: 'src/b.ts', rhsLines: ['two'] });
    const before = fingerprintsForPayload(payloadOf([a, b]));

    const changedA = diffFile({ path: 'src/a.ts', rhsLines: ['ONE!!'] });
    const after = fingerprintsForPayload(payloadOf([changedA, b]));

    expect(after['src/a.ts']).not.toBe(before['src/a.ts']);
    expect(after['src/b.ts']).toBe(before['src/b.ts']);
  });
});

describe('fingerprintsForUnified', () => {
  function unifiedFile(overrides: Partial<UnifiedFile> = {}): UnifiedFile {
    return {
      path: 'src/a.ts',
      oldPath: null,
      status: 'changed',
      lines: [
        {
          kind: 'del',
          oldLine: 1,
          newLine: null,
          text: 'one',
          anchorSide: 'old',
          anchorLine: 0,
        },
        {
          kind: 'add',
          oldLine: null,
          newLine: 1,
          text: 'ONE',
          anchorSide: 'new',
          anchorLine: 0,
        },
      ],
      ...overrides,
    };
  }

  it('changes when hunk content changes', () => {
    const before = fingerprintsForUnified([unifiedFile()]);
    const after = fingerprintsForUnified([
      unifiedFile({
        lines: [
          {
            kind: 'del',
            oldLine: 1,
            newLine: null,
            text: 'one',
            anchorSide: 'old',
            anchorLine: 0,
          },
          {
            kind: 'add',
            oldLine: null,
            newLine: 1,
            text: 'ONE!!',
            anchorSide: 'new',
            anchorLine: 0,
          },
        ],
      }),
    ]);
    expect(after['src/a.ts']).not.toBe(before['src/a.ts']);
  });
});

describe('loadViewed / saveViewed', () => {
  it('keeps a mark when the file’s fingerprint is unchanged (a re-push with identical content)', () => {
    const fingerprints = { 'src/a.ts': 'fp-a', 'src/b.ts': 'fp-b' };
    saveViewed('my-slug', fingerprints, { 'src/a.ts': true });
    expect(loadViewed('my-slug', fingerprints)).toEqual({ 'src/a.ts': true });
  });

  it("clears only the changed file's mark, keeping a sibling's", () => {
    saveViewed(
      'my-slug',
      { 'src/a.ts': 'fp-a1', 'src/b.ts': 'fp-b' },
      {
        'src/a.ts': true,
        'src/b.ts': true,
      },
    );
    // src/a.ts's diff changed (new fingerprint); src/b.ts's didn't.
    expect(
      loadViewed('my-slug', { 'src/a.ts': 'fp-a2', 'src/b.ts': 'fp-b' }),
    ).toEqual({
      'src/b.ts': true,
    });
  });

  it('clears a mark when the fingerprint changes even though the caller-supplied stat looks the same', () => {
    // An edit that rewrites a line without moving the added/removed/modified
    // counters — the stats alone can't tell these two diffs apart.
    const a = diffFile({ lhsLines: ['one'], rhsLines: ['ONE'] });
    const changedA = diffFile({ lhsLines: ['one'], rhsLines: ['ONE!!'] });
    const before = fingerprintsForPayload(payloadOf([a]));
    saveViewed('my-slug', before, { 'src/a.ts': true });

    const after = fingerprintsForPayload(payloadOf([changedA]));
    expect(loadViewed('my-slug', after)).toEqual({});
  });

  it('returns empty when nothing is stored yet', () => {
    expect(loadViewed('brand-new-slug', { 'src/a.ts': 'fp-a' })).toEqual({});
  });

  it('keeps state isolated per slug', () => {
    const fingerprints = { 'src/a.ts': 'fp-a' };
    saveViewed('slug-one', fingerprints, { 'src/a.ts': true });
    expect(loadViewed('slug-two', fingerprints)).toEqual({});
  });

  it('loads empty for the old v1 {contentKey, paths} shape (one-time reset, no migration)', () => {
    localStorage.setItem(
      'panopticon_diff_viewed:legacy-slug',
      JSON.stringify({ contentKey: 'abc', paths: ['src/a.ts'] }),
    );
    expect(loadViewed('legacy-slug', { 'src/a.ts': 'fp-a' })).toEqual({});
  });

  it('tolerates corrupt localStorage content', () => {
    localStorage.setItem('panopticon_diff_viewed:broken-slug', 'not json');
    expect(loadViewed('broken-slug', { 'src/a.ts': 'fp-a' })).toEqual({});
  });
});
