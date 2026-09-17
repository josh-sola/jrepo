import type { DiffChangeSpan, DiffPayload } from '../../shared/diff.ts';
import type { PrFile, PrFileStatus } from '../../shared/github.ts';
import type { DifftChange, DifftFile } from './difft.ts';
import { splitLines, synthesizeAligned } from './lines.ts';
import { computeStat } from './stat.ts';

export function toDiffStatus(status: PrFileStatus): DiffPayload['status'] {
  switch (status) {
    case 'added':
      return 'created';
    case 'removed':
      return 'deleted';
    case 'modified':
    case 'renamed':
    case 'copied':
    case 'changed':
    case 'unchanged':
      return 'changed';
  }
}

function mapChanges(changes: DifftChange[]): DiffChangeSpan[] {
  return changes.map((change) => ({
    start: change.start,
    end: change.end,
    ...(change.content !== undefined ? { content: change.content } : {}),
    ...(change.highlight !== undefined ? { highlight: change.highlight } : {}),
  }));
}

export function buildStructuralPayload(
  file: PrFile,
  oldText: string,
  newText: string,
  result: DifftFile,
): DiffPayload {
  const lhsLines = splitLines(oldText);
  const rhsLines = splitLines(newText);
  const lhsSpans: Record<string, DiffChangeSpan[]> = {};
  const rhsSpans: Record<string, DiffChangeSpan[]> = {};

  let aligned: [number | null, number | null][];
  if (result.status === 'created') {
    aligned = rhsLines.map((_, i): [number | null, number | null] => [null, i]);
  } else if (result.status === 'deleted') {
    aligned = lhsLines.map((_, i): [number | null, number | null] => [i, null]);
  } else if (result.status === 'unchanged') {
    aligned = lhsLines.map((_, i): [number | null, number | null] => [i, i]);
  } else {
    aligned = result.aligned_lines ?? synthesizeAligned(lhsLines, rhsLines);
    for (const chunk of result.chunks ?? []) {
      for (const item of chunk) {
        if (item.lhs !== undefined) {
          lhsSpans[String(item.lhs.line_number)] = mapChanges(item.lhs.changes);
        }
        if (item.rhs !== undefined) {
          rhsSpans[String(item.rhs.line_number)] = mapChanges(item.rhs.changes);
        }
      }
    }
  }

  return {
    path: file.path,
    oldPath: file.previousPath,
    status: result.status === 'unchanged' ? 'changed' : result.status,
    language: result.language,
    aligned,
    lhsLines,
    rhsLines,
    lhsSpans,
    rhsSpans,
    stat: computeStat(aligned, lhsSpans, rhsSpans),
    structural: true,
    fallbackReason: null,
    binary: false,
    collapseReason: null,
  };
}
