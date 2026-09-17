// Splits file text into lines the same way difftastic counts them: a
// trailing newline does not produce an extra empty final line.
export function splitLines(text: string): string[] {
  const lines = text.split('\n');
  if (lines.length > 0 && lines[lines.length - 1] === '') {
    lines.pop();
  }
  return lines;
}

// Builds the [oldIndex, newIndex] alignment for a file difftastic reports as
// a whole-file add or delete, since its JSON omits `aligned_lines` there.
export function synthesizeAligned(
  lhsLines: string[],
  rhsLines: string[],
): [number | null, number | null][] {
  if (rhsLines.length > 0 && lhsLines.length === 0) {
    return rhsLines.map((_, i): [number | null, number | null] => [null, i]);
  }
  if (lhsLines.length > 0 && rhsLines.length === 0) {
    return lhsLines.map((_, i): [number | null, number | null] => [i, null]);
  }
  const n = Math.max(lhsLines.length, rhsLines.length);
  const pairs: [number | null, number | null][] = [];
  for (let i = 0; i < n; i += 1) {
    pairs.push([
      i < lhsLines.length ? i : null,
      i < rhsLines.length ? i : null,
    ]);
  }
  return pairs;
}
