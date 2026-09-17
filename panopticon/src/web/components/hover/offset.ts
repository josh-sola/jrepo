// Pure DOM math for turning a hit-tested point inside a `.diff-code` cell
// into a language-server position. Kept free of `caretPositionFromPoint`
// (happy-dom does not implement it) so it can be unit tested directly.
import { rangesForSpans } from '../../diff/changeSpans.ts';

const IDENTIFIER_CHAR = /[A-Za-z0-9_$]/;

// Sums the UTF-16 length of every text node before the hit node in document
// order, then adds the in-node offset. Shiki wraps each token in its own
// span, so a line's text is split across many nodes and the offset has to be
// walked, not read off a single node. `hitNode` is usually the text node
// under the pointer; when it is an element (the pointer landed between
// nodes), the offset is treated as a child index into it.
export function characterOffsetInCell(
  cell: Element,
  hitNode: Node,
  hitOffset: number,
): number | null {
  let target = hitNode;
  let targetOffset = hitOffset;
  if (target.nodeType !== Node.TEXT_NODE) {
    const child = target.childNodes[targetOffset];
    if (!child) return null;
    target = child;
    targetOffset = 0;
  }
  if (target.nodeType !== Node.TEXT_NODE) return null;

  const walker = document.createTreeWalker(cell, NodeFilter.SHOW_TEXT);
  let offset = 0;
  let node = walker.nextNode();
  while (node) {
    if (node === target) return offset + targetOffset;
    offset += node.textContent?.length ?? 0;
    node = walker.nextNode();
  }
  return null;
}

export interface WordRange {
  start: number;
  end: number;
}

// Expands a character offset to the identifier it sits inside. Returns null
// for whitespace, punctuation, or an offset off the end of the line, since
// there is nothing worth a hover request there.
export function identifierWordAt(
  text: string,
  offset: number,
): WordRange | null {
  if (offset < 0 || offset >= text.length) return null;
  const char = text[offset];
  if (char === undefined || !IDENTIFIER_CHAR.test(char)) return null;

  let start = offset;
  while (start > 0 && IDENTIFIER_CHAR.test(text[start - 1] ?? '')) start--;
  let end = offset + 1;
  while (end < text.length && IDENTIFIER_CHAR.test(text[end] ?? '')) end++;
  return { start, end };
}

export interface HoverTarget {
  character: number;
  word: WordRange;
}

// Combines the two steps above: locate the character under the pointer,
// then expand it to the enclosing identifier. Null whenever there is no
// identifier to hover, so callers can skip the lookup entirely.
export function resolveHoverTarget(
  cell: Element,
  hitNode: Node,
  hitOffset: number,
): HoverTarget | null {
  const character = characterOffsetInCell(cell, hitNode, hitOffset);
  if (character === null) return null;
  const word = identifierWordAt(cell.textContent ?? '', character);
  if (!word) return null;
  return { character, word };
}

// Rebuilds a DOM Range spanning a cell's [start, end) text offsets, so the
// popover can anchor to the identifier's rendered position rather than the
// raw pointer coordinates. Reuses the diff view's own span-to-Range walk.
export function rangeForOffsets(
  cell: Element,
  start: number,
  end: number,
): Range | null {
  const [range] = rangesForSpans(cell, [{ start, end }]);
  return range ?? null;
}
