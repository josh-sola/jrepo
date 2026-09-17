// Comment-anchor capture for the diff view, ported from review.js's
// onSelection/climbToCode (see ~/.claude/skills/changes/assets/review.js).
// Code cells carry data-file/data-side/data-line (data-line is the 0-indexed
// offset into that side's full line array — for the side-by-side table this
// is the `l`/`r` value from `aligned`; for the unified fallback it's
// (real file line number - 1) — so `+1` always recovers the real,
// 1-indexed file line number).

export interface DiffAnchor {
  file: string;
  side: 'old' | 'new';
  start_line: number;
  end_line: number;
  quote: string;
}

export function climbToCode(node: Node | null): HTMLElement | null {
  let n: Node | null = node;
  while (n && n.nodeType !== 1) n = n.parentNode;
  let el = n as HTMLElement | null;
  while (el && !el.classList.contains('diff-code')) el = el.parentElement;
  return el && el.dataset.file != null ? el : null;
}

export function captureSelectionAnchor(
  sel: Selection | null = window.getSelection(),
): DiffAnchor | null {
  if (!sel || sel.isCollapsed) return null;
  const text = sel.toString();
  if (!text.trim()) return null;
  const anchorCell = climbToCode(sel.anchorNode);
  const focusCell = climbToCode(sel.focusNode);
  if (!anchorCell || !focusCell) return null;
  if (anchorCell.dataset.file !== focusCell.dataset.file) return null;
  const side = anchorCell.dataset.side;
  if (side !== 'old' && side !== 'new') return null;
  const lines = [anchorCell, focusCell].map(
    (c) => parseInt(c.dataset.line ?? '0', 10) + 1,
  );
  return {
    file: anchorCell.dataset.file!,
    side,
    start_line: Math.min(...lines),
    end_line: Math.max(...lines),
    quote: text,
  };
}
