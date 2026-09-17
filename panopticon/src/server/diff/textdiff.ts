import { diffLines, diffWordsWithSpace } from 'diff';
import type { DiffChangeSpan, DiffPayload } from '../../shared/diff.ts';
import type { PrFile } from '../../shared/github.ts';
import { splitLines } from './lines.ts';
import { toDiffStatus } from './payload.ts';
import { computeStat } from './stat.ts';

export interface TextFallbackOptions {
  ignoreWhitespace: boolean;
  reason: string;
}

function byteOffset(line: string, utf16Index: number): number {
  return Buffer.byteLength(line.slice(0, utf16Index), 'utf8');
}

// Word-level spans for one paired line, in the same shape difft's
// `changes[]` uses: only the changed runs, byte offsets into the UTF-8 line.
function wordSpans(
  oldLine: string,
  newLine: string,
): { lhs: DiffChangeSpan[]; rhs: DiffChangeSpan[] } {
  const parts = diffWordsWithSpace(oldLine, newLine);
  const lhs: DiffChangeSpan[] = [];
  const rhs: DiffChangeSpan[] = [];
  let oldPos = 0;
  let newPos = 0;
  for (const part of parts) {
    const length = part.value.length;
    if (part.removed) {
      lhs.push({
        start: byteOffset(oldLine, oldPos),
        end: byteOffset(oldLine, oldPos + length),
        content: part.value,
      });
      oldPos += length;
    } else if (part.added) {
      rhs.push({
        start: byteOffset(newLine, newPos),
        end: byteOffset(newLine, newPos + length),
        content: part.value,
      });
      newPos += length;
    } else {
      oldPos += length;
      newPos += length;
    }
  }
  return { lhs, rhs };
}

export function buildTextPayload(
  file: PrFile,
  oldText: string,
  newText: string,
  options: TextFallbackOptions,
): DiffPayload {
  const changes = diffLines(oldText, newText, {
    ignoreWhitespace: options.ignoreWhitespace,
  });

  const lhsLines: string[] = [];
  const rhsLines: string[] = [];
  const aligned: [number | null, number | null][] = [];
  const lhsSpans: Record<string, DiffChangeSpan[]> = {};
  const rhsSpans: Record<string, DiffChangeSpan[]> = {};
  let oldIdx = 0;
  let newIdx = 0;

  const pushContext = (lines: string[]): void => {
    for (const line of lines) {
      lhsLines.push(line);
      rhsLines.push(line);
      aligned.push([oldIdx, newIdx]);
      oldIdx += 1;
      newIdx += 1;
    }
  };
  const pushRemoved = (lines: string[]): void => {
    for (const line of lines) {
      lhsLines.push(line);
      aligned.push([oldIdx, null]);
      oldIdx += 1;
    }
  };
  const pushAdded = (lines: string[]): void => {
    for (const line of lines) {
      rhsLines.push(line);
      aligned.push([null, newIdx]);
      newIdx += 1;
    }
  };
  // A removed run immediately followed by an added run is a set of modified
  // lines, not an unrelated delete-then-add; pair them up so the UI can show
  // word-level spans instead of whole-line replacement.
  const pushModified = (oldLine: string, newLine: string): void => {
    lhsLines.push(oldLine);
    rhsLines.push(newLine);
    aligned.push([oldIdx, newIdx]);
    const spans = wordSpans(oldLine, newLine);
    if (spans.lhs.length > 0) lhsSpans[String(oldIdx)] = spans.lhs;
    if (spans.rhs.length > 0) rhsSpans[String(newIdx)] = spans.rhs;
    oldIdx += 1;
    newIdx += 1;
  };

  let i = 0;
  while (i < changes.length) {
    const change = changes[i];
    if (change === undefined) break;
    if (change.removed) {
      const removedLines = splitLines(change.value);
      const next = changes[i + 1];
      if (next?.added) {
        const addedLines = splitLines(next.value);
        const pairCount = Math.min(removedLines.length, addedLines.length);
        for (let k = 0; k < pairCount; k += 1) {
          pushModified(removedLines[k] ?? '', addedLines[k] ?? '');
        }
        pushRemoved(removedLines.slice(pairCount));
        pushAdded(addedLines.slice(pairCount));
        i += 2;
        continue;
      }
      pushRemoved(removedLines);
      i += 1;
      continue;
    }
    if (change.added) {
      pushAdded(splitLines(change.value));
      i += 1;
      continue;
    }
    pushContext(splitLines(change.value));
    i += 1;
  }

  return {
    path: file.path,
    oldPath: file.previousPath,
    status: toDiffStatus(file.status),
    language: 'Text',
    aligned,
    lhsLines,
    rhsLines,
    lhsSpans,
    rhsSpans,
    stat: computeStat(aligned, lhsSpans, rhsSpans),
    structural: false,
    fallbackReason: options.reason,
    binary: false,
    collapseReason: null,
  };
}
