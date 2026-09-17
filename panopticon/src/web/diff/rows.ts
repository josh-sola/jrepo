// Row model for the side-by-side diff table. Kept behavior-compatible with
// planhub's original row/fold semantics, since this module's ported tests
// must keep passing unchanged.
import type { DiffChangeSpan, DiffPayload } from '../../shared/diff.ts';

export type RowKind = 'add' | 'del' | 'mod' | 'unchanged';

export interface DiffRow {
  l: number | null;
  r: number | null;
  kind: RowKind;
}

function hasSpan(
  spans: Record<string, DiffChangeSpan[]>,
  idx: number | null,
): boolean {
  return idx != null && Boolean(spans[String(idx)]);
}

export function rowKind(
  file: DiffPayload,
  l: number | null,
  r: number | null,
): RowKind {
  if (l == null && r != null) return 'add';
  if (r == null && l != null) return 'del';
  if (hasSpan(file.lhsSpans, l) || hasSpan(file.rhsSpans, r)) return 'mod';
  return 'unchanged';
}

export function buildRows(file: DiffPayload): DiffRow[] {
  return file.aligned
    .map(([l, r]) => ({ l, r, kind: rowKind(file, l, r) }))
    .filter(
      ({ l, r }) =>
        !(
          (l == null || l >= file.lhsLines.length) &&
          (r == null || r >= file.rhsLines.length)
        ),
    );
}

export interface UnifiedRow {
  kind: RowKind;
  side: 'old' | 'new';
  l: number | null;
  r: number | null;
}

// Unified-mode rows for a rich-payload file, derived from the same `aligned`
// pairs as buildRows above. An unchanged pair carries both line numbers on
// one row, anchored (by convention, matching the payload-less fallback in
// parseUnified.ts) to the new side. A contiguous run of changed pairs
// (add/del/mod) renders GitHub-style: every old-side line in the run first,
// then every new-side line — so a single "mod" pair (one line changed to
// another, sharing one row in split view) becomes two rows here, one per
// side.
//
// `alignedIndex` is a parallel array (same length as `rows`): for a row that
// came from an unchanged pair it's that pair's index in `buildRows(file)`
// (aligned-row-index space); for a row that came from a changed run it's
// `null` — there's no 1:1 correspondence once one aligned pair fans out
// into two unified rows. diff/folds.ts uses this to map a fold range (which
// lives in aligned-row-index space, shared with split mode) onto the
// unified rows it should hide.
function buildUnifiedRowsWithAlignedIndex(file: DiffPayload): {
  rows: UnifiedRow[];
  alignedIndex: (number | null)[];
} {
  const aligned = buildRows(file);
  const unified: UnifiedRow[] = [];
  const alignedIndex: (number | null)[] = [];
  let i = 0;
  while (i < aligned.length) {
    const current = aligned[i]!;
    if (current.kind === 'unchanged') {
      const { l, r, kind } = current;
      unified.push({ kind, side: 'new', l, r });
      alignedIndex.push(i);
      i++;
      continue;
    }
    const runStart = i;
    while (i < aligned.length && aligned[i]!.kind !== 'unchanged') i++;
    const run = aligned.slice(runStart, i);
    for (const row of run) {
      if (row.l != null) {
        unified.push({ kind: row.kind, side: 'old', l: row.l, r: null });
        alignedIndex.push(null);
      }
    }
    for (const row of run) {
      if (row.r != null) {
        unified.push({ kind: row.kind, side: 'new', l: null, r: row.r });
        alignedIndex.push(null);
      }
    }
  }
  return { rows: unified, alignedIndex };
}

export function buildUnifiedRows(file: DiffPayload): UnifiedRow[] {
  return buildUnifiedRowsWithAlignedIndex(file).rows;
}

export function buildUnifiedAlignedIndex(file: DiffPayload): (number | null)[] {
  return buildUnifiedRowsWithAlignedIndex(file).alignedIndex;
}
