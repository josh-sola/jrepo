import type { DiffChangeSpan, DiffStat } from '../../shared/diff.ts';

// A line counts as modified when its side of the alignment carries at least
// one intra-line change span; a line present on only one side is added or
// removed instead.
export function computeStat(
  aligned: [number | null, number | null][],
  lhsSpans: Record<string, DiffChangeSpan[]>,
  rhsSpans: Record<string, DiffChangeSpan[]>,
): DiffStat {
  let added = 0;
  let removed = 0;
  let modified = 0;
  for (const [oldIdx, newIdx] of aligned) {
    if (oldIdx === null && newIdx !== null) {
      added += 1;
    } else if (newIdx === null && oldIdx !== null) {
      removed += 1;
    } else if (
      (oldIdx !== null && (lhsSpans[String(oldIdx)]?.length ?? 0) > 0) ||
      (newIdx !== null && (rhsSpans[String(newIdx)]?.length ?? 0) > 0)
    ) {
      modified += 1;
    }
  }
  return { added, removed, modified };
}
