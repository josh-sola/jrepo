// Normalizes both diff-render paths (the rich DiffPayload and the
// dependency-free unified-diff fallback) into one per-file summary shape —
// the file sidebar and the default-collapse heuristic key off this instead
// of branching on which parse path produced the file list.
import type { DiffPayload, DiffStat, PrDiff } from '../../shared/diff.ts';
import type { UnifiedFile } from './parseUnified.ts';

export interface RailFile {
  path: string;
  oldPath: string | null;
  status: string;
  stat: DiffStat;
}

// A diff this size shouldn't eagerly mount thousands of table rows (DOM
// layout/paint cost) or run highlighting content most users won't scroll to
// — default it to collapsed instead, GitHub-style. Picked to comfortably
// cover normal source files while catching generated/bundled output.
const MAX_AUTO_EXPAND_ROWS = 5_000; // aligned pairs (or unified lines) before default-collapse
const MAX_AUTO_EXPAND_BYTES = 500_000; // combined lhs+rhs chars before default-collapse

function combinedChars(...lineSets: string[][]): number {
  let total = 0;
  for (const lines of lineSets) for (const line of lines) total += line.length;
  return total;
}

export function isLargeDiffFile(file: DiffPayload): boolean {
  return (
    file.aligned.length > MAX_AUTO_EXPAND_ROWS ||
    combinedChars(file.lhsLines, file.rhsLines) > MAX_AUTO_EXPAND_BYTES
  );
}

export function isLargeUnifiedFile(file: UnifiedFile): boolean {
  return (
    file.lines.length > MAX_AUTO_EXPAND_ROWS ||
    combinedChars(file.lines.map((line) => line.text)) > MAX_AUTO_EXPAND_BYTES
  );
}

// The unified fallback has no difftastic-computed stat block, so approximate
// one from the line kinds alone — there's no span data to distinguish a
// "modified" pair from an unrelated add+del, so modified is always 0 here.
function unifiedFileStat(file: UnifiedFile): DiffStat {
  let added = 0;
  let removed = 0;
  for (const line of file.lines) {
    if (line.kind === 'add') added++;
    else if (line.kind === 'del') removed++;
  }
  return { added, removed, modified: 0 };
}

export function railFilesFromPayload(payload: PrDiff): RailFile[] {
  return payload.files.map((file) => ({
    path: file.path,
    oldPath: file.oldPath,
    status: file.status,
    stat: file.stat,
  }));
}

export function railFilesFromUnified(files: UnifiedFile[]): RailFile[] {
  return files.map((file) => ({
    path: file.path,
    oldPath: file.oldPath,
    status: file.status,
    stat: unifiedFileStat(file),
  }));
}
