// GitHub only accepts a review comment on a line that's part of the diff: a
// changed line, or one within three lines of a change on that side. Anything
// else comes back as a 422, so the gutter only offers "+" where this holds.
import type { DiffPayload } from '../../../shared/diff.ts';
import type { CommentSide } from '../../../shared/github.ts';

export type UiSide = 'old' | 'new';

export function toUiSide(side: CommentSide): UiSide {
  return side === 'RIGHT' ? 'new' : 'old';
}

export function toApiSide(side: UiSide): CommentSide {
  return side === 'new' ? 'RIGHT' : 'LEFT';
}

export interface CommentableLines {
  old: ReadonlySet<number>;
  new: ReadonlySet<number>;
}

const CONTEXT_RADIUS = 3;

function isChangedPair(
  file: DiffPayload,
  l: number | null,
  r: number | null,
): boolean {
  if (l == null || r == null) return true;
  return Boolean(file.lhsSpans[String(l)]) || Boolean(file.rhsSpans[String(r)]);
}

export function computeCommentableLines(file: DiffPayload): CommentableLines {
  const pairs = file.aligned;
  const changed = pairs.map(([l, r]) => isChangedPair(file, l, r));

  const old = new Set<number>();
  const next = new Set<number>();
  for (let i = 0; i < pairs.length; i++) {
    const lo = Math.max(0, i - CONTEXT_RADIUS);
    const hi = Math.min(pairs.length - 1, i + CONTEXT_RADIUS);
    let near = false;
    for (let j = lo; j <= hi; j++) {
      if (changed[j]) {
        near = true;
        break;
      }
    }
    if (!near) continue;
    const pair = pairs[i];
    if (!pair) continue;
    const [l, r] = pair;
    if (l != null) old.add(l);
    if (r != null) next.add(r);
  }

  return { old, new: next };
}

export function isCommentable(
  lines: CommentableLines,
  side: UiSide,
  lineIndex: number,
): boolean {
  return (side === 'old' ? lines.old : lines.new).has(lineIndex);
}

// The new side is preferred; a deletion-only file has nothing there.
export function firstCommentableLine(
  lines: CommentableLines,
): { side: UiSide; line: number } | null {
  if (lines.new.size > 0) return { side: 'new', line: Math.min(...lines.new) };
  if (lines.old.size > 0) return { side: 'old', line: Math.min(...lines.old) };
  return null;
}
