// GitHub-style folding of unchanged runs, operating purely on `buildRows`
// output indices (rows.ts) — the "aligned-row index space" shared by both
// split mode (rows render 1:1 against that space) and unified mode (which
// maps into it via `buildUnifiedAlignedIndex`, see below). Keeping fold
// state in that one shared space is what lets a fold survive a split/unified
// toggle: DiffView holds one `FoldState` per file regardless of which mode
// is currently rendering it.
import type { DiffRow } from './rows.ts';

// Inclusive aligned-row-index bounds.
export interface Range {
  start: number;
  end: number;
}

// The set of currently-hidden ranges for one file, sorted ascending and
// non-overlapping (each foldable region starts out as its own entry; reveal
// operations shrink or drop entries but never merge or split them, since the
// only reveals are from an end or in full).
export type FoldState = Range[];

// Shared by every file with nothing (yet) hidden — avoids handing back a
// fresh `[]` on every lookup, which would defeat the `memo()` on
// FileSection/UnifiedFileSection (a new array reference reads as a changed
// prop even when semantically empty both times).
export const EMPTY_FOLD_STATE: FoldState = [];

export const DEFAULT_CONTEXT = 3;
export const DEFAULT_MIN_HIDDEN = 5;
// Rows revealed per click of the top/bottom expand affordances.
export const REVEAL_STEP = 20;

export interface FoldOptions {
  context?: number;
  minHidden?: number;
}

// Runs of consecutive `unchanged` rows, minus `context` rows of visible
// padding kept on each side that borders a changed run (a run touching the
// start/end of the file with nothing beyond it gets no padding on that side
// — there's no neighboring change for it to give context to). Only regions
// that would actually hide at least `minHidden` rows survive; a short
// unchanged run just renders in full, uncollapsible.
export function computeFoldableRegions(
  rows: Pick<DiffRow, 'kind'>[],
  {
    context = DEFAULT_CONTEXT,
    minHidden = DEFAULT_MIN_HIDDEN,
  }: FoldOptions = {},
): Range[] {
  const regions: Range[] = [];
  let i = 0;
  while (i < rows.length) {
    if (rows[i]!.kind !== 'unchanged') {
      i++;
      continue;
    }
    const start = i;
    while (i < rows.length && rows[i]!.kind === 'unchanged') i++;
    const end = i - 1; // inclusive
    const hasLeft = start > 0;
    const hasRight = end < rows.length - 1;
    const hiddenStart = start + (hasLeft ? context : 0);
    const hiddenEnd = end - (hasRight ? context : 0);
    if (hiddenEnd - hiddenStart + 1 >= minHidden) {
      regions.push({ start: hiddenStart, end: hiddenEnd });
    }
  }
  return regions;
}

function replaceRange(
  state: FoldState,
  target: Range,
  next: Range | null,
): FoldState {
  const idx = state.findIndex(
    (r) => r.start === target.start && r.end === target.end,
  );
  if (idx === -1) return state;
  if (next == null) return [...state.slice(0, idx), ...state.slice(idx + 1)];
  return [...state.slice(0, idx), next, ...state.slice(idx + 1)];
}

// Unfold `n` rows off the top of `range` (the "expand down" affordance —
// pulls the hunk above down into the gap). Shrinks `range` in place; once
// fewer than one row of the range would remain, it's dropped entirely rather
// than left hiding zero rows.
export function revealFromTop(
  state: FoldState,
  range: Range,
  n: number = REVEAL_STEP,
): FoldState {
  const newStart = range.start + n;
  return replaceRange(
    state,
    range,
    newStart > range.end ? null : { start: newStart, end: range.end },
  );
}

// Unfold `n` rows off the bottom of `range` (the "expand up" affordance —
// pulls the hunk below up into the gap).
export function revealFromBottom(
  state: FoldState,
  range: Range,
  n: number = REVEAL_STEP,
): FoldState {
  const newEnd = range.end - n;
  return replaceRange(
    state,
    range,
    newEnd < range.start ? null : { start: range.start, end: newEnd },
  );
}

// Unfold all of `range` at once.
export function revealAll(state: FoldState, range: Range): FoldState {
  return replaceRange(state, range, null);
}

// One rendered row, or one fold bar standing in for a hidden run — the
// shape FileSection/UnifiedFileSection map over instead of the raw row
// array once folding is in play. `window.alignedRange` is always in
// aligned-row-index space (what reveal callbacks operate on); `window.rowRange`
// is in the space of the `rows` array actually being rendered (identical to
// `alignedRange` in split mode, remapped in unified mode — see
// `foldWindowsForUnified`).
export interface FoldWindow {
  rowRange: Range;
  alignedRange: Range;
}

export type FoldedItem<T> =
  | { type: 'row'; row: T; index: number }
  | { type: 'fold'; window: FoldWindow };

// Filters `rows` down to what's actually rendered: hidden rows disappear,
// replaced by one `FoldedItem` fold entry per window. `windows` must be
// sorted ascending by `rowRange.start` and non-overlapping (both hold for
// anything built from a `FoldState`, the only producer).
export function applyFolds<T>(
  rows: T[],
  windows: FoldWindow[],
): FoldedItem<T>[] {
  const result: FoldedItem<T>[] = [];
  let i = 0;
  let w = 0;
  while (i < rows.length) {
    const window = windows[w];
    if (window && i === window.rowRange.start) {
      result.push({ type: 'fold', window });
      i = window.rowRange.end + 1;
      w++;
      continue;
    }
    result.push({ type: 'row', row: rows[i]!, index: i });
    i++;
  }
  return result;
}

// Split mode renders `buildRows` output directly, so aligned-row-index space
// *is* that array's index space — no remapping needed.
export function foldWindowsForSplit(state: FoldState): FoldWindow[] {
  return state.map((range) => ({ rowRange: range, alignedRange: range }));
}

// Unified mode renders `buildUnifiedRows` output, whose indices don't line
// up 1:1 with aligned-row-index space (a changed run's one aligned pair can
// become two unified rows). `alignedIndex` is `buildUnifiedAlignedIndex`'s
// parallel array: the source aligned index for each unified row that came
// from an unchanged pair (null for rows from a changed run). Since a fold
// region only ever spans unchanged rows, every aligned index in `state`
// resolves to exactly one unified-row index this way.
export function foldWindowsForUnified(
  state: FoldState,
  alignedIndex: ReadonlyArray<number | null>,
): FoldWindow[] {
  const rowIndexForAligned = new Map<number, number>();
  alignedIndex.forEach((aligned, rowIdx) => {
    if (aligned != null) rowIndexForAligned.set(aligned, rowIdx);
  });
  const windows: FoldWindow[] = [];
  for (const range of state) {
    const start = rowIndexForAligned.get(range.start);
    const end = rowIndexForAligned.get(range.end);
    if (start != null && end != null)
      windows.push({ rowRange: { start, end }, alignedRange: range });
  }
  return windows;
}
