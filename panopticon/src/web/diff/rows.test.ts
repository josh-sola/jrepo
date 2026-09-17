import { describe, expect, it } from 'bun:test';
import {
  buildRows,
  buildUnifiedAlignedIndex,
  buildUnifiedRows,
  rowKind,
} from './rows.ts';
import { diffPayload } from './__fixtures__/diffPayload.ts';
import type { DiffPayload } from '../../shared/diff.ts';

const [changed, created, deleted, renamed] = diffPayload.files as [
  DiffPayload,
  DiffPayload,
  DiffPayload,
  DiffPayload,
];

// Minimal synthetic DiffPayload builder for buildUnifiedRows cases the fixture
// files don't exercise (a mixed del+add run with no word-level span, i.e. two
// wholesale-replaced lines rather than a single "mod" pair).
function makeFile(overrides: Partial<DiffPayload>): DiffPayload {
  return {
    path: 'synthetic.txt',
    oldPath: null,
    status: 'changed',
    language: 'Text',
    aligned: [],
    lhsLines: [],
    rhsLines: [],
    lhsSpans: {},
    rhsSpans: {},
    stat: { added: 0, removed: 0, modified: 0 },
    structural: true,
    fallbackReason: null,
    binary: false,
    collapseReason: null,
    ...overrides,
  };
}

describe('rowKind', () => {
  it('classifies add/del/mod/unchanged per review.js semantics', () => {
    expect(rowKind(changed, 0, 0)).toBe('mod'); // in lhsSpans/rhsSpans
    expect(rowKind(changed, 2, 2)).toBe('unchanged'); // blank line, no span
    expect(rowKind(created, null, 0)).toBe('add');
    expect(rowKind(deleted, 0, null)).toBe('del');
  });
});

describe('buildRows', () => {
  it('derives one row per aligned pair, kind matching rowKind', () => {
    const rows = buildRows(changed);
    expect(rows).toEqual([
      { l: 0, r: 0, kind: 'mod' },
      { l: 1, r: 1, kind: 'mod' },
      { l: 2, r: 2, kind: 'unchanged' },
      { l: 3, r: 3, kind: 'unchanged' },
    ]);
  });

  it('derives pure-add rows for a created file', () => {
    const rows = buildRows(created);
    expect(rows).toEqual([
      { l: null, r: 0, kind: 'add' },
      { l: null, r: 1, kind: 'add' },
      { l: null, r: 2, kind: 'add' },
    ]);
  });

  it('derives pure-del rows for a deleted file', () => {
    const rows = buildRows(deleted);
    expect(rows).toEqual([
      { l: 0, r: null, kind: 'del' },
      { l: 1, r: null, kind: 'del' },
    ]);
  });

  it('mixes unchanged and add rows for a rename with only additions', () => {
    const rows = buildRows(renamed);
    expect(rows).toEqual([
      { l: 0, r: 0, kind: 'unchanged' },
      { l: 1, r: 1, kind: 'unchanged' },
      { l: null, r: 2, kind: 'add' },
      { l: null, r: 3, kind: 'add' },
      { l: null, r: 4, kind: 'add' },
    ]);
  });
});

describe('buildUnifiedRows', () => {
  it('emits one new-side row per pure-add pair, in order', () => {
    expect(buildUnifiedRows(created)).toEqual([
      { kind: 'add', side: 'new', l: null, r: 0 },
      { kind: 'add', side: 'new', l: null, r: 1 },
      { kind: 'add', side: 'new', l: null, r: 2 },
    ]);
  });

  it('emits one old-side row per pure-del pair, in order', () => {
    expect(buildUnifiedRows(deleted)).toEqual([
      { kind: 'del', side: 'old', l: 0, r: null },
      { kind: 'del', side: 'old', l: 1, r: null },
    ]);
  });

  it('splits a mod run into its old lines then its new lines, then leaves unchanged pairs single-row', () => {
    // changed: two contiguous mod pairs, then two unchanged pairs.
    expect(buildUnifiedRows(changed)).toEqual([
      { kind: 'mod', side: 'old', l: 0, r: null },
      { kind: 'mod', side: 'old', l: 1, r: null },
      { kind: 'mod', side: 'new', l: null, r: 0 },
      { kind: 'mod', side: 'new', l: null, r: 1 },
      { kind: 'unchanged', side: 'new', l: 2, r: 2 },
      { kind: 'unchanged', side: 'new', l: 3, r: 3 },
    ]);
  });

  it('interleaves unchanged rows (both line numbers, new-side anchor) with a following add run', () => {
    // renamed: two unchanged pairs, then a contiguous run of three adds.
    expect(buildUnifiedRows(renamed)).toEqual([
      { kind: 'unchanged', side: 'new', l: 0, r: 0 },
      { kind: 'unchanged', side: 'new', l: 1, r: 1 },
      { kind: 'add', side: 'new', l: null, r: 2 },
      { kind: 'add', side: 'new', l: null, r: 3 },
      { kind: 'add', side: 'new', l: null, r: 4 },
    ]);
  });

  it('groups a mixed del+add run old-line-first (GitHub-style) even with no word-level span', () => {
    // Two wholesale-replaced lines (no lhsSpans/rhsSpans entry, so the pair
    // never merges into a single "mod") followed by an unchanged tail line.
    const file = makeFile({
      aligned: [
        [0, null],
        [null, 0],
        [1, 1],
      ],
      lhsLines: ['old line 0', 'same'],
      rhsLines: ['new line 0', 'same'],
    });
    expect(buildUnifiedRows(file)).toEqual([
      { kind: 'del', side: 'old', l: 0, r: null },
      { kind: 'add', side: 'new', l: null, r: 0 },
      { kind: 'unchanged', side: 'new', l: 1, r: 1 },
    ]);
  });

  it('keeps old-lines-then-new-lines ordering and correct line numbers across a mod+del+add run', () => {
    const file = makeFile({
      aligned: [
        [0, 0],
        [1, null],
        [null, 1],
        [2, 2],
      ],
      lhsLines: ['line0-old', 'line1-old', 'line2-same'],
      rhsLines: ['line0-new', 'line1-new-add', 'line2-same'],
      lhsSpans: { '0': [{ start: 0, end: 4 }] },
      rhsSpans: { '0': [{ start: 0, end: 4 }] },
    });
    expect(buildUnifiedRows(file)).toEqual([
      { kind: 'mod', side: 'old', l: 0, r: null },
      { kind: 'del', side: 'old', l: 1, r: null },
      { kind: 'mod', side: 'new', l: null, r: 0 },
      { kind: 'add', side: 'new', l: null, r: 1 },
      { kind: 'unchanged', side: 'new', l: 2, r: 2 },
    ]);
  });
});

describe('buildUnifiedAlignedIndex', () => {
  // diff/folds.ts maps a fold range (aligned-row-index space) onto unified
  // rows via this parallel array — every unchanged unified row should carry
  // its source `buildRows` index; every row born from a changed run (which
  // can't be inside a fold region by construction) should carry null.
  it('is null for every row of a changed run and the aligned index for unchanged rows', () => {
    expect(buildUnifiedAlignedIndex(changed)).toEqual([
      null,
      null,
      null,
      null,
      2,
      3,
    ]);
  });

  it('is null for every row of a pure-add file (every pair is a changed "add")', () => {
    expect(buildUnifiedAlignedIndex(created)).toEqual([null, null, null]);
  });

  it('mixes aligned indices (unchanged prefix) with nulls (the add run) for a rename', () => {
    expect(buildUnifiedAlignedIndex(renamed)).toEqual([0, 1, null, null, null]);
  });

  it("has the same length as buildUnifiedRows' output for every fixture file", () => {
    for (const file of [changed, created, deleted, renamed]) {
      expect(buildUnifiedAlignedIndex(file)).toHaveLength(
        buildUnifiedRows(file).length,
      );
    }
  });
});
