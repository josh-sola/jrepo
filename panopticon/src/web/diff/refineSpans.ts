// Refines difftastic's per-line change spans (DiffPayload.lhsSpans/rhsSpans)
// before they reach the diff table's highlight painting, for the case
// difftastic's own word-matching gives up on: a prose-heavy hunk (e.g. a
// reflowed docstring) where it tiles change spans across every word of every
// line on both sides. Rendered faithfully that's a solid red/green wall with
// no signal, even though the true edit might be a handful of words.
//
// One pass, three parts:
//   1. Saturation — a mod row (both aligned sides present) is "saturated"
//      when its spans already cover almost the whole line on both sides;
//      that's the signature of difftastic giving up, not a real near-total
//      rewrite (guard 2 below tells those apart after refinement).
//   2. Run grouping — a maximal run of consecutive changed rows anchored by
//      at least one saturated mod row. add/del rows inside the run
//      contribute their one-sided text so a run straddling an inserted or
//      deleted line still LCS-matches the surrounding mod lines correctly.
//   3. Word-level LCS over the run's two texts (lines joined with "\n",
//      tokenized on /\S+/), replacing the saturated tiling with spans over
//      just the tokens outside the longest common subsequence.
//
// Two guards keep this from producing worse output than the original
// tiling: an LCS token-count product cap (the DP is O(n*m)), and a
// post-refinement re-check — if the refined result is itself still
// near-total on both sides, the run really is mostly-new text and gets no
// spans at all, leaving the plain modified-row tint as the signal.
import type { DiffChangeSpan, DiffPayload } from '../../shared/diff.ts';
import {
  charSpansToByteSpans,
  utf8Length,
  type CharSpan,
} from './changeSpans.ts';
import { buildRows } from './rows.ts';

// A mod row's spans covering at least this fraction of a line's
// non-whitespace bytes, on both sides, is difftastic tiling the whole line
// rather than reporting a real word-level match.
const SATURATION_THRESHOLD = 0.85;

// Above this lhsTokens.length * rhsTokens.length product, the O(n*m) LCS DP
// table gets too large to build per render; skip refinement for the run
// rather than block the UI.
const LCS_TOKEN_PRODUCT_LIMIT = 1_000_000;

export interface EffectiveSpans {
  lhs: Record<string, DiffChangeSpan[]>;
  rhs: Record<string, DiffChangeSpan[]>;
}

// Per-byte-offset "is this byte part of a non-whitespace codepoint",
// built once per line so coverage checks (saturation, and the
// post-refinement guard) don't re-derive byte/char alignment per span.
function nonWhitespaceByteMask(line: string): boolean[] {
  const mask: boolean[] = [];
  for (let i = 0; i < line.length;) {
    const codePoint = line.codePointAt(i)!;
    const isWs = /\s/.test(String.fromCodePoint(codePoint));
    const byteLen = utf8Length(codePoint);
    for (let b = 0; b < byteLen; b++) mask.push(!isWs);
    i += codePoint > 0xffff ? 2 : 1;
  }
  return mask;
}

// Fraction of non-whitespace bytes across `indices` (line indices into
// `lines`) covered by `spansFor(idx)`, aggregated (not averaged) across
// lines — used both for single-line saturation (indices.length === 1) and
// the whole-run post-refinement guard.
function nonWhitespaceCoverageRatio(
  lines: string[],
  indices: number[],
  spansFor: (idx: number) => readonly { start: number; end: number }[],
): number {
  let total = 0;
  let covered = 0;
  for (const idx of indices) {
    const line = lines[idx] ?? '';
    const mask = nonWhitespaceByteMask(line);
    const coveredMask: boolean[] = Array.from(
      { length: mask.length },
      () => false,
    );
    for (const span of spansFor(idx)) {
      const start = Math.max(0, span.start);
      const end = Math.min(mask.length, span.end);
      for (let i = start; i < end; i++) coveredMask[i] = true;
    }
    for (let i = 0; i < mask.length; i++) {
      if (!mask[i]) continue;
      total++;
      if (coveredMask[i]) covered++;
    }
  }
  return total === 0 ? 0 : covered / total;
}

function isSaturatedMod(file: DiffPayload, l: number, r: number): boolean {
  if (file.lhsLines[l] == null || file.rhsLines[r] == null) return false;
  const lhsRatio = nonWhitespaceCoverageRatio(
    file.lhsLines,
    [l],
    (idx) => file.lhsSpans[String(idx)] ?? [],
  );
  const rhsRatio = nonWhitespaceCoverageRatio(
    file.rhsLines,
    [r],
    (idx) => file.rhsSpans[String(idx)] ?? [],
  );
  return lhsRatio >= SATURATION_THRESHOLD && rhsRatio >= SATURATION_THRESHOLD;
}

interface Token {
  token: string;
  start: number;
  end: number;
}

function tokenize(text: string): Token[] {
  const tokens: Token[] = [];
  const re = /\S+/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)))
    tokens.push({ token: m[0], start: m.index, end: m.index + m[0].length });
  return tokens;
}

interface LineRange {
  lineIndex: number;
  start: number;
  end: number;
}

// Joins the run's lines (in row order) with "\n", recording each line's char
// range in the joined text so tokens can be mapped back to (line, local
// offset) afterward.
function buildRunText(
  allLines: string[],
  lineIndices: number[],
): { text: string; lineRanges: LineRange[] } {
  const parts: string[] = [];
  const lineRanges: LineRange[] = [];
  let offset = 0;
  for (const idx of lineIndices) {
    const text = allLines[idx] ?? '';
    lineRanges.push({
      lineIndex: idx,
      start: offset,
      end: offset + text.length,
    });
    parts.push(text);
    offset += text.length + 1; // +1 for the "\n" joiner, never itself tokenized
  }
  return { text: parts.join('\n'), lineRanges };
}

// Classic O(n*m) LCS over token *values* (not positions), so repeated words
// match by identity rather than position — matched[i] is whether token i
// took part in the longest common subsequence.
function lcsMatched(
  a: string[],
  b: string[],
): { aMatched: boolean[]; bMatched: boolean[] } {
  const n = a.length;
  const m = b.length;
  const dp: Uint32Array[] = Array.from({ length: n + 1 });
  for (let i = 0; i <= n; i++) dp[i] = new Uint32Array(m + 1);
  for (let i = 1; i <= n; i++) {
    for (let j = 1; j <= m; j++) {
      dp[i]![j] =
        a[i - 1] === b[j - 1]
          ? dp[i - 1]![j - 1]! + 1
          : Math.max(dp[i - 1]![j]!, dp[i]![j - 1]!);
    }
  }
  const aMatched: boolean[] = Array.from({ length: n }, () => false);
  const bMatched: boolean[] = Array.from({ length: m }, () => false);
  let i = n;
  let j = m;
  while (i > 0 && j > 0) {
    if (a[i - 1] === b[j - 1]) {
      aMatched[i - 1] = true;
      bMatched[j - 1] = true;
      i--;
      j--;
    } else if (dp[i - 1]![j]! >= dp[i]![j - 1]!) {
      i--;
    } else {
      j--;
    }
  }
  return { aMatched, bMatched };
}

// Walks `tokens` in order, grouping consecutive changed tokens that fall on
// the same line into one span (they're separated only by whitespace, since
// tokens are exactly the /\S+/ runs) — an intervening unchanged token or a
// line boundary always closes the current group.
function mergeChangedTokenSpans(
  tokens: Token[],
  changed: boolean[],
  lineRanges: LineRange[],
): Map<number, CharSpan[]> {
  const result = new Map<number, CharSpan[]>();
  let rangeIdx = 0;
  let groupLineIndex = -1;
  let groupStart = -1;
  let groupEnd = -1;
  let prevChangedLineIndex: number | null = null;

  const closeGroup = () => {
    if (groupLineIndex === -1) return;
    const arr = result.get(groupLineIndex) ?? [];
    arr.push({ start: groupStart, end: groupEnd });
    result.set(groupLineIndex, arr);
    groupLineIndex = -1;
  };

  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i]!;
    while (
      rangeIdx < lineRanges.length - 1 &&
      token.start >= lineRanges[rangeIdx]!.end
    )
      rangeIdx++;
    const range = lineRanges[rangeIdx]!;
    if (!changed[i]) {
      closeGroup();
      prevChangedLineIndex = null;
      continue;
    }
    const localStart = token.start - range.start;
    const localEnd = token.end - range.start;
    if (
      prevChangedLineIndex === range.lineIndex &&
      groupLineIndex === range.lineIndex
    ) {
      groupEnd = localEnd;
    } else {
      closeGroup();
      groupLineIndex = range.lineIndex;
      groupStart = localStart;
      groupEnd = localEnd;
    }
    prevChangedLineIndex = range.lineIndex;
  }
  closeGroup();
  return result;
}

// Converts merged char spans per line into byte spans, keyed by line index
// (present for every index in `lineIndices`, empty array when that line had
// no changed tokens after LCS).
function toByteSpansByLine(
  lines: string[],
  lineIndices: number[],
  merged: Map<number, CharSpan[]>,
): Map<number, DiffChangeSpan[]> {
  const result = new Map<number, DiffChangeSpan[]>();
  for (const idx of lineIndices) {
    const line = lines[idx] ?? '';
    result.set(idx, charSpansToByteSpans(line, merged.get(idx) ?? []));
  }
  return result;
}

function assign(
  target: Record<string, DiffChangeSpan[]>,
  byLine: Map<number, DiffChangeSpan[]>,
): void {
  for (const [idx, spans] of byLine) target[String(idx)] = spans;
}

function emptySpansByLine(
  lineIndices: number[],
): Map<number, DiffChangeSpan[]> {
  const result = new Map<number, DiffChangeSpan[]>();
  for (const idx of lineIndices) result.set(idx, []);
  return result;
}

export function refineFileSpans(file: DiffPayload): EffectiveSpans {
  const lhs: Record<string, DiffChangeSpan[]> = { ...file.lhsSpans };
  const rhs: Record<string, DiffChangeSpan[]> = { ...file.rhsSpans };

  const rows = buildRows(file);
  let i = 0;
  while (i < rows.length) {
    const row = rows[i]!;
    const isCandidate =
      row.kind === 'add' ||
      row.kind === 'del' ||
      (row.kind === 'mod' &&
        row.l != null &&
        row.r != null &&
        isSaturatedMod(file, row.l, row.r));
    if (!isCandidate) {
      i++;
      continue;
    }
    const runStart = i;
    while (i < rows.length) {
      const r = rows[i]!;
      const candidate =
        r.kind === 'add' ||
        r.kind === 'del' ||
        (r.kind === 'mod' &&
          r.l != null &&
          r.r != null &&
          isSaturatedMod(file, r.l, r.r));
      if (!candidate) break;
      i++;
    }
    const run = rows.slice(runStart, i);
    const hasSaturatedMod = run.some(
      (r) =>
        r.kind === 'mod' &&
        r.l != null &&
        r.r != null &&
        isSaturatedMod(file, r.l, r.r),
    );
    if (!hasSaturatedMod) continue;

    const lhsIndices = run.filter((r) => r.l != null).map((r) => r.l as number);
    const rhsIndices = run.filter((r) => r.r != null).map((r) => r.r as number);

    const { text: lhsText, lineRanges: lhsLineRanges } = buildRunText(
      file.lhsLines,
      lhsIndices,
    );
    const { text: rhsText, lineRanges: rhsLineRanges } = buildRunText(
      file.rhsLines,
      rhsIndices,
    );
    const lhsTokens = tokenize(lhsText);
    const rhsTokens = tokenize(rhsText);

    if (lhsTokens.length * rhsTokens.length > LCS_TOKEN_PRODUCT_LIMIT) {
      assign(lhs, emptySpansByLine(lhsIndices));
      assign(rhs, emptySpansByLine(rhsIndices));
      continue;
    }

    const { aMatched, bMatched } = lcsMatched(
      lhsTokens.map((t) => t.token),
      rhsTokens.map((t) => t.token),
    );
    const lhsChanged = aMatched.map((matched) => !matched);
    const rhsChanged = bMatched.map((matched) => !matched);

    const lhsMerged = mergeChangedTokenSpans(
      lhsTokens,
      lhsChanged,
      lhsLineRanges,
    );
    const rhsMerged = mergeChangedTokenSpans(
      rhsTokens,
      rhsChanged,
      rhsLineRanges,
    );

    const lhsByLine = toByteSpansByLine(file.lhsLines, lhsIndices, lhsMerged);
    const rhsByLine = toByteSpansByLine(file.rhsLines, rhsIndices, rhsMerged);

    const lhsRatio = nonWhitespaceCoverageRatio(
      file.lhsLines,
      lhsIndices,
      (idx) => lhsByLine.get(idx) ?? [],
    );
    const rhsRatio = nonWhitespaceCoverageRatio(
      file.rhsLines,
      rhsIndices,
      (idx) => rhsByLine.get(idx) ?? [],
    );
    if (lhsRatio >= SATURATION_THRESHOLD && rhsRatio >= SATURATION_THRESHOLD) {
      assign(lhs, emptySpansByLine(lhsIndices));
      assign(rhs, emptySpansByLine(rhsIndices));
      continue;
    }

    assign(lhs, lhsByLine);
    assign(rhs, rhsByLine);
  }

  return { lhs, rhs };
}
