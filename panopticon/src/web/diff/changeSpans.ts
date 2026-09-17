// Turns difftastic's per-line character spans (DiffFile.lhsSpans/rhsSpans —
// see types.ts's DiffChangeSpan doc) into DOM Ranges over a rendered code
// cell, for painting via the CSS Custom Highlight API (DiffView.tsx). Two
// steps, kept separate because they convert between two different offset
// spaces:
//   1. byteSpansToCharSpans (and its inverse, charSpansToByteSpans — used by
//      diff/refineSpans.ts, which computes token spans in char space but
//      must emit byte spans to match difftastic's own convention):
//      difftastic's start/end are UTF-8 *byte* offsets into the raw line —
//      translate to/from UTF-16 code-unit offsets (the space JS
//      strings/DOM Ranges use) against that same raw line text.
//   2. rangesForSpans: walk a cell's text nodes (TreeWalker, same approach as
//      highlight.ts's rangeFromOffsets) to turn UTF-16 offsets into Ranges,
//      clamping out-of-bounds offsets instead of rejecting them — a stale
//      span (line text edited since the payload was generated, or a tokenizer
//      quirk) should degrade to "highlight less" rather than "highlight
//      nothing on this line".

export interface CharSpan {
  start: number;
  end: number;
}

export function utf8Length(codePoint: number): number {
  if (codePoint <= 0x7f) return 1;
  if (codePoint <= 0x7ff) return 2;
  if (codePoint <= 0xffff) return 3;
  return 4;
}

// Every requested UTF-8 byte offset -> the UTF-16 code-unit offset in `line`
// at that same byte position, in one pass over `line`. Byte offsets that
// don't land on a codepoint boundary in `line` (a stale span against edited
// content) resolve to the next boundary at or after that byte; offsets past
// the line's own byte length clamp to `line.length`; negative offsets clamp
// to 0.
function byteOffsetsToCharOffsets(
  line: string,
  byteOffsets: number[],
): Map<number, number> {
  const map = new Map<number, number>();
  const targets = new Set(byteOffsets);
  for (const target of targets) if (target <= 0) map.set(target, 0);
  let byte = 0;
  let i = 0;
  while (i < line.length) {
    for (const target of targets)
      if (!map.has(target) && target <= byte) map.set(target, i);
    const codePoint = line.codePointAt(i)!;
    byte += utf8Length(codePoint);
    i += codePoint > 0xffff ? 2 : 1;
  }
  for (const target of targets)
    if (!map.has(target)) map.set(target, line.length);
  return map;
}

// Convert one line's difftastic change spans (byte offsets) into char spans
// (UTF-16 code-unit offsets) against that same line's raw text.
export function byteSpansToCharSpans(
  line: string,
  spans: readonly { start: number; end: number }[],
): CharSpan[] {
  if (!spans.length) return [];
  const offsets: number[] = [];
  for (const s of spans) offsets.push(s.start, s.end);
  const charOffset = byteOffsetsToCharOffsets(line, offsets);
  return spans.map((s) => ({
    start: charOffset.get(s.start)!,
    end: charOffset.get(s.end)!,
  }));
}

// The inverse of byteOffsetsToCharOffsets: every requested UTF-16 code-unit
// offset -> the UTF-8 byte offset in `line` at that same position. Same
// clamping rules (out-of-range offsets clamp to the nearest end).
function charOffsetsToByteOffsets(
  line: string,
  charOffsets: number[],
): Map<number, number> {
  const map = new Map<number, number>();
  const targets = new Set(charOffsets);
  for (const target of targets) if (target <= 0) map.set(target, 0);
  let byte = 0;
  let i = 0;
  while (i < line.length) {
    for (const target of targets)
      if (!map.has(target) && target <= i) map.set(target, byte);
    const codePoint = line.codePointAt(i)!;
    byte += utf8Length(codePoint);
    i += codePoint > 0xffff ? 2 : 1;
  }
  for (const target of targets) if (!map.has(target)) map.set(target, byte);
  return map;
}

// Convert one line's char spans (UTF-16 code-unit offsets, e.g. token spans
// from a word-level LCS) into difftastic-style change spans (UTF-8 byte
// offsets), the inverse of byteSpansToCharSpans.
export function charSpansToByteSpans(
  line: string,
  spans: readonly CharSpan[],
): { start: number; end: number }[] {
  if (!spans.length) return [];
  const offsets: number[] = [];
  for (const s of spans) offsets.push(s.start, s.end);
  const byteOffset = charOffsetsToByteOffsets(line, offsets);
  return spans.map((s) => ({
    start: byteOffset.get(s.start)!,
    end: byteOffset.get(s.end)!,
  }));
}

interface TextNodeSpan {
  node: Text;
  start: number;
  end: number;
}

function textNodeSpans(root: Node): TextNodeSpan[] {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const spans: TextNodeSpan[] = [];
  let acc = 0;
  let node = walker.nextNode() as Text | null;
  while (node) {
    const len = node.textContent?.length ?? 0;
    spans.push({ node, start: acc, end: acc + len });
    acc += len;
    node = walker.nextNode() as Text | null;
  }
  return spans;
}

// The text-node + in-node offset at char offset `offset` into root's
// textContent (`offset` already clamped to [0, totalLength] by the caller).
function positionAt(
  nodeSpans: TextNodeSpan[],
  offset: number,
): { node: Text; offset: number } | null {
  for (const span of nodeSpans) {
    if (offset <= span.end)
      return { node: span.node, offset: offset - span.start };
  }
  return null;
}

// Build one Range per char span (UTF-16 offsets into `root.textContent`),
// walking root's text nodes so a span crossing a syntax `<span>` boundary
// still yields a single correct Range. Offsets outside [0, textContent
// length] clamp rather than drop the span; a span that collapses to nothing
// after clamping (or falls on a node boundary DOMException can't span) is
// skipped.
export function rangesForSpans(
  root: Element,
  spans: readonly CharSpan[],
): Range[] {
  if (!spans.length) return [];
  const totalLength = root.textContent?.length ?? 0;
  if (!totalLength) return [];
  const nodeSpans = textNodeSpans(root);
  if (!nodeSpans.length) return [];

  const ranges: Range[] = [];
  for (const span of spans) {
    const start = Math.max(0, Math.min(span.start, totalLength));
    const end = Math.max(0, Math.min(span.end, totalLength));
    if (end <= start) continue;
    const startPos = positionAt(nodeSpans, start);
    const endPos = positionAt(nodeSpans, end);
    if (!startPos || !endPos) continue;
    const range = document.createRange();
    try {
      range.setStart(startPos.node, startPos.offset);
      range.setEnd(endPos.node, endPos.offset);
    } catch {
      continue;
    }
    if (!range.collapsed) ranges.push(range);
  }
  return ranges;
}
