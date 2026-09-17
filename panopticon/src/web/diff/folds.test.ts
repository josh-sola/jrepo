import { describe, expect, it } from 'bun:test';
import {
  applyFolds,
  computeFoldableRegions,
  EMPTY_FOLD_STATE,
  foldWindowsForSplit,
  foldWindowsForUnified,
  revealAll,
  revealFromBottom,
  revealFromTop,
  type FoldState,
} from './folds.ts';
import type { DiffRow, RowKind } from './rows.ts';

function rowsOf(kinds: RowKind[]): DiffRow[] {
  return kinds.map((kind, i) => ({ l: i, r: i, kind }));
}

describe('computeFoldableRegions', () => {
  it('trims `context` rows of visible padding off each side of an unchanged run bordered by changes', () => {
    // mod, mod, 11x unchanged, add, add — hidden = 11 - 3 - 3 = 5, at minHidden.
    const rows = rowsOf([
      'mod',
      'mod',
      ...Array(11).fill('unchanged'),
      'add',
      'add',
    ]);
    expect(computeFoldableRegions(rows)).toEqual([{ start: 5, end: 9 }]);
  });

  it('keeps context only on the side that borders a changed run at the start of the file', () => {
    // Unchanged run starts at index 0 (no left neighbor) and is followed by
    // an add run — only the right-side 3 rows of context are trimmed.
    const rows = rowsOf([...Array(8).fill('unchanged'), 'add', 'add']);
    expect(computeFoldableRegions(rows)).toEqual([{ start: 0, end: 4 }]);
  });

  it('keeps context only on the side that borders a changed run at the end of the file', () => {
    const rows = rowsOf(['del', 'del', ...Array(8).fill('unchanged')]);
    expect(computeFoldableRegions(rows)).toEqual([{ start: 5, end: 9 }]);
  });

  it('keeps no context at all for a file that is one giant unchanged run (a rename-ish diff with no changed lines)', () => {
    const rows = rowsOf(Array(12).fill('unchanged'));
    expect(computeFoldableRegions(rows)).toEqual([{ start: 0, end: 11 }]);
  });

  it('does not fold a run below minHidden even after context is trimmed', () => {
    // mod, 10x unchanged, add — hidden = 10 - 3 - 3 = 4, just under the
    // default minHidden of 5.
    const rows = rowsOf(['mod', ...Array(10).fill('unchanged'), 'add']);
    expect(computeFoldableRegions(rows)).toEqual([]);
  });

  it('does not fold a short unchanged run even when it borders nothing (whole file is short)', () => {
    const rows = rowsOf(['unchanged', 'unchanged']);
    expect(computeFoldableRegions(rows)).toEqual([]);
  });

  it('respects custom context/minHidden options', () => {
    const rows = rowsOf(['mod', ...Array(6).fill('unchanged'), 'add']);
    expect(computeFoldableRegions(rows, { context: 1, minHidden: 3 })).toEqual([
      { start: 2, end: 5 },
    ]);
    expect(computeFoldableRegions(rows, { context: 1, minHidden: 5 })).toEqual(
      [],
    );
  });

  it('finds multiple independent regions in one file', () => {
    // Each 11-row unchanged run, flanked on both sides, hides exactly 5
    // (11 - 3 - 3) — at minHidden.
    const rows = rowsOf([
      'mod',
      ...Array(11).fill('unchanged'),
      'add',
      'mod',
      ...Array(11).fill('unchanged'),
      'del',
    ]);
    expect(computeFoldableRegions(rows)).toEqual([
      { start: 4, end: 8 },
      { start: 17, end: 21 },
    ]);
  });
});

describe('reveal operations', () => {
  const region = { start: 5, end: 28 }; // 24 rows hidden
  const state: FoldState = [region];

  it('revealFromTop shrinks the range from its start', () => {
    const next = revealFromTop(state, region, 20);
    expect(next).toEqual([{ start: 25, end: 28 }]);
  });

  it('revealFromBottom shrinks the range from its end', () => {
    const next = revealFromBottom(state, region, 20);
    expect(next).toEqual([{ start: 5, end: 8 }]);
  });

  it('revealFromTop drops the range once fewer than one row would remain', () => {
    const small = { start: 5, end: 9 }; // 5 rows
    expect(revealFromTop([small], small, 20)).toEqual([]);
    // Exactly draining it (n === length) also drops it, not a 0-length range.
    expect(revealFromTop([small], small, 5)).toEqual([]);
  });

  it('revealFromBottom drops the range once fewer than one row would remain', () => {
    const small = { start: 5, end: 9 };
    expect(revealFromBottom([small], small, 20)).toEqual([]);
    expect(revealFromBottom([small], small, 5)).toEqual([]);
  });

  it('revealAll drops the range in one step regardless of size', () => {
    expect(revealAll(state, region)).toEqual([]);
  });

  it('stepwise reveal from both ends exhausts the range without ever going below one row', () => {
    let s: FoldState = [{ start: 0, end: 43 }]; // 44 rows
    s = revealFromTop(s, s[0]!, 20);
    expect(s).toEqual([{ start: 20, end: 43 }]);
    s = revealFromBottom(s, s[0]!, 20);
    expect(s).toEqual([{ start: 20, end: 23 }]); // 4 rows left
    s = revealAll(s, s[0]!);
    expect(s).toEqual([]);
  });

  it('leaves unrelated ranges in the state untouched', () => {
    const other = { start: 40, end: 50 };
    const both: FoldState = [region, other];
    expect(revealFromTop(both, region, 20)).toEqual([
      { start: 25, end: 28 },
      other,
    ]);
  });

  it('is a no-op if the target range is no longer in state (already revealed elsewhere)', () => {
    expect(revealFromTop([], region, 20)).toEqual([]);
  });
});

describe('applyFolds', () => {
  it('passes rows through unchanged when there is nothing to fold', () => {
    const rows = ['a', 'b', 'c'];
    expect(applyFolds(rows, [])).toEqual([
      { type: 'row', row: 'a', index: 0 },
      { type: 'row', row: 'b', index: 1 },
      { type: 'row', row: 'c', index: 2 },
    ]);
  });

  it('replaces a hidden range with one fold entry, keeping visible rows on either side', () => {
    const rows = ['a', 'b', 'c', 'd', 'e'];
    const window = {
      rowRange: { start: 1, end: 3 },
      alignedRange: { start: 1, end: 3 },
    };
    expect(applyFolds(rows, [window])).toEqual([
      { type: 'row', row: 'a', index: 0 },
      { type: 'fold', window },
      { type: 'row', row: 'e', index: 4 },
    ]);
  });

  it('handles multiple non-overlapping windows', () => {
    const rows = ['a', 'b', 'c', 'd', 'e', 'f', 'g'];
    const w1 = {
      rowRange: { start: 0, end: 1 },
      alignedRange: { start: 0, end: 1 },
    };
    const w2 = {
      rowRange: { start: 4, end: 5 },
      alignedRange: { start: 4, end: 5 },
    };
    expect(applyFolds(rows, [w1, w2])).toEqual([
      { type: 'fold', window: w1 },
      { type: 'row', row: 'c', index: 2 },
      { type: 'row', row: 'd', index: 3 },
      { type: 'fold', window: w2 },
      { type: 'row', row: 'g', index: 6 },
    ]);
  });
});

describe('foldWindowsForSplit', () => {
  it("maps a FoldState 1:1 (split mode's row-index space is aligned-row-index space)", () => {
    const state: FoldState = [{ start: 5, end: 9 }];
    expect(foldWindowsForSplit(state)).toEqual([
      { rowRange: { start: 5, end: 9 }, alignedRange: { start: 5, end: 9 } },
    ]);
  });

  it('returns [] for EMPTY_FOLD_STATE', () => {
    expect(foldWindowsForSplit(EMPTY_FOLD_STATE)).toEqual([]);
  });
});

describe('foldWindowsForUnified', () => {
  it('remaps an aligned-space range onto unified-row-space via the parallel alignedIndex array', () => {
    // Unified rows 0-1 came from a changed run (no aligned correspondence),
    // rows 2-6 are the unchanged pairs at aligned indices 1-5.
    const alignedIndex: (number | null)[] = [null, null, 1, 2, 3, 4, 5];
    const state: FoldState = [{ start: 2, end: 4 }];
    expect(foldWindowsForUnified(state, alignedIndex)).toEqual([
      { rowRange: { start: 3, end: 5 }, alignedRange: { start: 2, end: 4 } },
    ]);
  });

  it('drops a range that no longer has both endpoints represented in alignedIndex', () => {
    const alignedIndex: (number | null)[] = [null, 1, 2];
    const state: FoldState = [{ start: 5, end: 9 }];
    expect(foldWindowsForUnified(state, alignedIndex)).toEqual([]);
  });
});
